use std::collections::{HashSet, VecDeque};

use rbs_environment::{ClassKind, Location, MethodType, ResolvedMethod, Substitution, Type};
use ruby_prism::{CallNode, Node};

use super::arguments::{
    ArgumentPair, ArityMismatch, CallArguments, match_arguments, select_overload,
};
use super::blocks::BlockSignature;
use super::{Checker, Context, Locals};
use crate::file_index::FileMethod;
use crate::source::{Span, constant_text};
use crate::steep_relation;
use crate::type_utilities::{
    can_be_nil, contains_uncertain_part, contains_variable_in, hash_of, instance_type_of_class,
    is_bottom, literal_class_name, members_of, remove_nil, self_instance_type,
};
use crate::unification::{Bindings, class_substitution};
use crate::{HoverKind, Severity, TypeHintKind};

const MAXIMUM_LOOKUP_DEPTH: usize = 8;

#[derive(Debug, Clone)]
pub(crate) struct FileTarget {
    pub(crate) owner: String,
    pub(crate) singleton: bool,
    pub(crate) method: FileMethod,
}

#[derive(Debug, Clone)]
pub(crate) struct FoundMethod {
    pub(crate) owner: String,
    pub(crate) singleton: bool,
    pub(crate) overloads: Vec<MethodType>,
    pub(crate) comment: Option<String>,
    pub(crate) location: Option<Location>,
    pub(crate) diagnostics_allowed: bool,
    pub(crate) is_private: bool,
    pub(crate) is_pure: bool,
    /// Found only in Ruby code although the class has a signature: typed from the code, reported like Steep does
    pub(crate) missing_from_signature: bool,
    pub(crate) file_target: Option<FileTarget>,
    pub(crate) fixed_return_type: Option<Type>,
}

pub(crate) enum Lookup {
    Found(Box<FoundMethod>),
    Missing { reportable: bool },
    Unknown,
    Unreachable,
}

pub(crate) enum CallBlock<'pr> {
    Absent,
    Literal(Node<'pr>),
    Argument {
        argument_type: Type,
        symbol: Option<String>,
        span: Span,
    },
}

impl CallBlock<'_> {
    fn span(&self) -> Option<Span> {
        match self {
            CallBlock::Absent => None,
            CallBlock::Literal(node) => Some(Span::of_node(node)),
            CallBlock::Argument { span, .. } => Some(*span),
        }
    }
}

/// Why one overload cannot take the call, in the categories Steep's `type_method_call` distinguishes
#[derive(Debug, Clone)]
enum OverloadError {
    RequiredBlockMissing,
    UnexpectedBlockGiven,
    Arity(ArityMismatch),
    TypeMismatch(ArgumentPair),
}

impl OverloadError {
    fn is_arity(&self) -> bool {
        !matches!(self, OverloadError::TypeMismatch(_))
    }
}

pub(crate) struct CallSite<'pr> {
    pub(crate) method_name: String,
    pub(crate) name_span: Span,
    pub(crate) block: CallBlock<'pr>,
    pub(crate) is_self_call: bool,
    pub(crate) reports_diagnostics: bool,
    pub(crate) deferred_lambdas: Vec<(usize, Node<'pr>)>,
    /// Where Steep puts diagnostics that cover the whole call (`UnresolvedOverloading`), the call from the method
    /// name to its last argument (`Insufficient*Arguments`), and `NoMethod` (the `=` of an attribute write)
    pub(crate) expression_span: Span,
    pub(crate) send_span: Span,
    pub(crate) no_method_span: Span,
}

struct PreparedCall {
    overload: Option<MethodType>,
    bindings: Bindings,
}

impl PreparedCall {
    fn new(
        environment: &rbs_environment::Environment,
        found: &FoundMethod,
        arguments: &CallArguments,
        has_block: bool,
    ) -> PreparedCall {
        if found.overloads.is_empty() {
            return PreparedCall {
                overload: None,
                bindings: Bindings::new(Vec::new()),
            };
        }
        let index = select_overload(environment, &found.overloads, arguments, has_block);
        let overload = found.overloads[index].clone();
        let mut bindings = Bindings::new(
            overload
                .type_parameters
                .iter()
                .map(|type_parameter| type_parameter.name.clone())
                .collect(),
        );
        if let Ok(pairs) = match_arguments(&overload.function, arguments) {
            for pair in &pairs {
                bindings.unify(environment, &pair.parameter_type, &pair.argument_type);
            }
        }
        PreparedCall {
            overload: Some(overload),
            bindings,
        }
    }

    fn block_signature(&self) -> BlockSignature {
        let Some(block) = self
            .overload
            .as_ref()
            .and_then(|overload| overload.block.as_ref())
        else {
            return BlockSignature::untyped();
        };
        let partial = self.bindings.substitution(false);
        let closing = close_variables(self.bindings.variable_names());
        BlockSignature {
            break_type: self
                .overload
                .as_ref()
                .map(|overload| overload.function.return_type.substitute(&partial)),
            function: Some(block.function.substitute(&partial).substitute(&closing)),
            self_type: block
                .self_type
                .as_ref()
                .map(|self_type| self_type.substitute(&partial).substitute(&closing)),
        }
    }

    /// The proc type the parameter receiving the positional argument at `value_index` expects
    fn parameter_proc_signature(
        &self,
        environment: &rbs_environment::Environment,
        arguments: &CallArguments,
        value_index: usize,
    ) -> Option<BlockSignature> {
        let overload = self.overload.as_ref()?;
        let function = &overload.function;
        let parameter_type = match &arguments.values.get(value_index)?.kind {
            super::arguments::ArgumentKind::Keyword { name, .. } => function
                .required_keywords
                .iter()
                .chain(function.optional_keywords.iter())
                .find(|(keyword, _)| keyword == name)
                .map(|(_, parameter)| parameter)
                .or(function.rest_keywords.as_ref())?
                .parameter_type
                .clone(),
            _ => {
                let pairs = match_arguments(function, arguments).ok()?;
                let positional_index = arguments.values[..value_index]
                    .iter()
                    .filter(|value| {
                        matches!(value.kind, super::arguments::ArgumentKind::Positional)
                    })
                    .count();
                pairs.get(positional_index)?.parameter_type.clone()
            }
        };
        let parameter_type = &parameter_type;
        let partial = self.bindings.substitution(false);
        let closing = close_variables(self.bindings.variable_names());
        let parameter_type = expand_aliases(
            environment,
            &parameter_type.substitute(&partial).substitute(&closing),
            0,
        );
        let proc_type =
            members_of(&parameter_type)
                .into_iter()
                .find_map(|member| match member {
                    Type::Proc(proc_type) => Some(proc_type),
                    _ => None,
                })?;
        Some(BlockSignature {
            function: Some(proc_type.function),
            self_type: proc_type.self_type,
            break_type: None,
        })
    }

    fn block_proc_type(&self) -> Option<Type> {
        let block = self.overload.as_ref()?.block.as_ref()?;
        let partial = self.bindings.substitution(false);
        Some(Type::Proc(Box::new(rbs_environment::ProcType {
            function: block.function.substitute(&partial),
            block: None,
            self_type: block.self_type.clone(),
        })))
    }

    fn block_first_parameter_type(&self) -> Option<Type> {
        let signature = self.block_signature();
        let function = signature.function?;
        function
            .required_positionals
            .first()
            .map(|parameter| parameter.parameter_type.clone())
    }

    fn finish(
        &mut self,
        environment: &rbs_environment::Environment,
        block_result: Option<&Type>,
    ) -> Type {
        let Some(overload) = &self.overload else {
            return Type::Untyped;
        };
        if let (Some(block), Some(block_result)) = (&overload.block, block_result) {
            self.bindings
                .unify(environment, &block.function.return_type, block_result);
        }
        overload
            .function
            .return_type
            .substitute(&self.bindings.substitution(true))
    }
}

fn close_variables(names: &[String]) -> Substitution {
    let mut substitution = Substitution::default();
    for name in names {
        substitution.variables.insert(name.clone(), Type::Untyped);
    }
    substitution
}

fn substitute_overload(overload: &MethodType, substitution: &Substitution) -> MethodType {
    let shadows = overload
        .type_parameters
        .iter()
        .any(|type_parameter| substitution.variables.contains_key(&type_parameter.name));
    if !shadows {
        return overload.substitute(substitution);
    }
    let mut narrowed = substitution.clone();
    for type_parameter in &overload.type_parameters {
        narrowed.variables.remove(&type_parameter.name);
    }
    overload.substitute(&narrowed)
}

impl<'environment, 'pr> Checker<'environment, 'pr> {
    pub(crate) fn infer_call(
        &mut self,
        call: &CallNode<'pr>,
        context: &Context,
        locals: &mut Locals,
    ) -> Type {
        let method_name = constant_text(&call.name());
        let receiver = call.receiver();
        let block_node = call.block();
        let typed_like_lambda =
            method_name == "lambda" || (method_name == "proc" && !self.options.steep_compatible);
        if receiver.is_none()
            && typed_like_lambda
            && let Some(block) = block_node
                .as_ref()
                .filter(|block| matches!(block, Node::BlockNode { .. }))
        {
            self.infer_arguments(call.arguments(), context, locals);
            return self.infer_lambda(block, context, locals);
        }
        // The parser gem Steep uses reads `/regexp/ =~ value` as `match_with_lvasgn`, which Steep types as `untyped`
        // without looking the method up
        if self.options.steep_compatible
            && method_name == "=~"
            && let Some(receiver) = receiver.as_ref().filter(|receiver| matches!(receiver, Node::RegularExpressionNode { .. }))
        {
            self.infer(receiver, context, locals);
            self.infer_arguments(call.arguments(), context, locals);
            return Type::Untyped;
        }
        let receiver_type = match &receiver {
            Some(receiver) => {
                let receiver_type = self.infer(receiver, context, locals);
                self.record_receiver(Span::of_node(receiver), &receiver_type);
                receiver_type
            }
            None => context.self_type.clone(),
        };
        let looked_up_receiver = if call.is_safe_navigation() {
            remove_nil(&receiver_type)
        } else {
            receiver_type.clone()
        };
        if let Some(error) = self.definition_error_of(&looked_up_receiver) {
            self.record_unexpected_error(Span::of_location(&call.location()), error);
            return Type::Untyped;
        }
        let mut deferred_lambdas = Vec::new();
        let arguments = self.infer_arguments_deferring_lambdas(
            call.arguments(),
            context,
            locals,
            Some(&mut deferred_lambdas),
        );
        let block = self.call_block(block_node, context, locals);
        let call_span = Span::of_location(&call.location());
        let name_span = call
            .message_loc()
            .map(|location| Span::of_location(&location))
            .unwrap_or(call_span);
        let send_end = call
            .closing_loc()
            .map(|location| location.end_offset())
            .or_else(|| {
                call.arguments()
                    .map(|arguments| arguments.location().end_offset())
            })
            .unwrap_or(name_span.end);
        let send_span = Span {
            start: name_span.start,
            end: send_end.max(name_span.end),
        };
        let no_method_span = match call.equal_loc() {
            Some(equal) if call.is_attribute_write() => Span::of_location(&equal),
            _ => name_span,
        };
        let is_safe_navigation = call.is_safe_navigation();
        let dispatch_receiver = if is_safe_navigation {
            remove_nil(&receiver_type)
        } else {
            receiver_type.clone()
        };
        let site = CallSite {
            method_name: method_name.clone(),
            name_span,
            block,
            is_self_call: receiver
                .as_ref()
                .is_none_or(|receiver| matches!(receiver, Node::SelfNode { .. })),
            reports_diagnostics: true,
            deferred_lambdas,
            expression_span: call_span,
            send_span,
            no_method_span,
        };
        if receiver_type.is_untyped()
            && let Some(arguments_node) = call.arguments()
        {
            // Steep synthesizes the arguments of a call on `untyped` one by one, and a lone splat is unsupported there
            for argument in arguments_node.arguments().iter() {
                if matches!(argument, Node::SplatNode { .. }) {
                    self.record_diagnostic(
                        Span::of_node(&argument),
                        Severity::Hint,
                        "Ruby::UnsupportedSyntax",
                        "Unsupported splat node occurrence".to_string(),
                    );
                }
            }
        }
        let mut result = self.dispatch_call(
            &dispatch_receiver,
            &receiver_type,
            &site,
            &arguments,
            context,
            locals,
        );
        if let Some((_, key)) = super::narrowing::pure_call_key(call)
            && let Some(refined) = locals.get(&key)
        {
            result = refined.clone();
        }
        let structural_receiver = match &dispatch_receiver {
            Type::Alias { name, arguments } => self
                .environment
                .expand_alias(name, arguments)
                .unwrap_or(dispatch_receiver.clone()),
            other => other.clone(),
        };
        if let Type::Record(fields) = &structural_receiver
            && let Some(field_type) = record_field(fields, &method_name, call)
        {
            result = field_type;
        }
        if let Some(compacted) = compact_result(&method_name, &arguments, call, &result) {
            result = compacted;
        }
        if let Type::Tuple(elements) = &structural_receiver
            && let Some(element) = tuple_element(elements, &method_name, call)
        {
            result = element;
        }
        if method_name == "class"
            && arguments.values.is_empty()
            && matches!(&result, Type::ClassInstance { name, .. } if name == "Class")
        {
            result = class_of(&dispatch_receiver).unwrap_or(result);
        }
        if is_safe_navigation && can_be_nil(&receiver_type) {
            result = Type::optional(result);
        }
        if receiver.is_none()
            && context.method.is_none()
            && matches!(
                method_name.as_str(),
                "attr_reader" | "attr_writer" | "attr_accessor"
            )
        {
            self.record_attribute_hints(call, &method_name, context);
        }
        result
    }

    /// Why Steep cannot build the definition it looks the method up in, when the receiver is an instance or the
    /// singleton of a class whose RBS definition fails to build
    pub(crate) fn definition_error_of(
        &self,
        receiver_type: &Type,
    ) -> Option<&'environment rbs_environment::DefinitionError> {
        if !self.options.steep_compatible {
            return None;
        }
        members_of(receiver_type)
            .iter()
            .flat_map(|member| members_of(&self.expand_alias_deeply(member, 0)))
            .find_map(|member| match member {
                Type::ClassInstance { name, .. } => {
                    self.environment.instance_definition_error(&name)
                }
                Type::ClassSingleton { name } => self.environment.singleton_definition_error(&name),
                _ => None,
            })
    }

    /// Steep's `synthesize` rescues the RBS error, reports it on the node and types the node and everything in it
    /// as `untyped` without checking them
    pub(crate) fn record_unexpected_error(
        &mut self,
        span: Span,
        error: &rbs_environment::DefinitionError,
    ) {
        self.record_diagnostic(
            span,
            Severity::Hint,
            "Ruby::UnexpectedError",
            format!("UnexpectedError: {}", error.message()),
        );
    }

    /// `it` is a method call to Steep, whose parser follows Ruby 3.3
    pub(crate) fn infer_it_as_method_call(
        &mut self,
        node: &Node<'pr>,
        context: &Context,
        locals: &mut Locals,
    ) -> Type {
        let span = Span::of_node(node);
        if let Some(error) = self.definition_error_of(&context.self_type) {
            self.record_unexpected_error(span, error);
            return Type::Untyped;
        }
        let site = CallSite {
            method_name: "it".to_string(),
            name_span: span,
            block: CallBlock::Absent,
            is_self_call: true,
            reports_diagnostics: true,
            deferred_lambdas: Vec::new(),
            expression_span: span,
            send_span: span,
            no_method_span: span,
        };
        let self_type = context.self_type.clone();
        self.dispatch_call(
            &self_type,
            &self_type,
            &site,
            &CallArguments { values: Vec::new() },
            context,
            locals,
        )
    }

    fn call_block(
        &mut self,
        block_node: Option<Node<'pr>>,
        context: &Context,
        locals: &mut Locals,
    ) -> CallBlock<'pr> {
        match block_node {
            Some(block) if matches!(block, Node::BlockNode { .. }) => CallBlock::Literal(block),
            Some(block) => {
                let Some(block_argument) = block.as_block_argument_node() else {
                    return CallBlock::Absent;
                };
                let Some(expression) = block_argument.expression() else {
                    return CallBlock::Argument {
                        argument_type: Type::Untyped,
                        symbol: None,
                        span: Span::of_node(&block),
                    };
                };
                let symbol = expression
                    .as_symbol_node()
                    .map(|symbol| String::from_utf8_lossy(symbol.unescaped()).into_owned());
                let argument_type = self.infer(&expression, context, locals);
                CallBlock::Argument {
                    argument_type,
                    symbol,
                    span: Span::of_node(&block),
                }
            }
            None => CallBlock::Absent,
        }
    }

    pub(crate) fn dispatch_call(
        &mut self,
        receiver_type: &Type,
        displayed_receiver_type: &Type,
        site: &CallSite<'pr>,
        arguments: &CallArguments,
        context: &Context,
        locals: &mut Locals,
    ) -> Type {
        let members: Vec<Type> = members_of(receiver_type)
            .iter()
            .flat_map(|member| members_of(&self.expand_alias_deeply(member, 0)))
            .collect();
        let mut lookups: Vec<Lookup> = Vec::with_capacity(members.len());
        for member in &members {
            let lookup = match self.lookup_member(member, &site.method_name, context, 0) {
                Lookup::Found(found) if found.is_private && !site.is_self_call => Lookup::Missing {
                    reportable: found.diagnostics_allowed,
                },
                other => other,
            };
            lookups.push(lookup);
        }
        let has_block = !matches!(site.block, CallBlock::Absent);
        let primary_index = lookups
            .iter()
            .position(|lookup| matches!(lookup, Lookup::Found(_)));
        let mut primary_prepared = primary_index.and_then(|index| match &lookups[index] {
            Lookup::Found(found) => Some(PreparedCall::new(
                self.environment,
                found,
                arguments,
                has_block,
            )),
            _ => None,
        });
        for (value_index, lambda_node) in &site.deferred_lambdas {
            let signature = primary_prepared
                .as_ref()
                .and_then(|prepared| {
                    prepared.parameter_proc_signature(self.environment, arguments, *value_index)
                })
                .unwrap_or_else(BlockSignature::untyped);
            if let Some(block) = super::arguments::lambda_literal_block(lambda_node) {
                self.infer_lambda_with(&block, &signature, context, locals);
            }
        }
        let block_result = match &site.block {
            CallBlock::Literal(block_node) => {
                let signature = primary_prepared
                    .as_ref()
                    .map(PreparedCall::block_signature)
                    .unwrap_or_else(BlockSignature::untyped);
                Some(self.infer_block(block_node, &signature, context, locals))
            }
            CallBlock::Argument {
                argument_type,
                symbol,
                span,
            } => {
                let first_parameter = primary_prepared
                    .as_ref()
                    .and_then(PreparedCall::block_first_parameter_type);
                let symbol_receiver_error = symbol
                    .as_ref()
                    .and(first_parameter.as_ref())
                    .and_then(|first_parameter| self.definition_error_of(first_parameter));
                let (block_type, symbol_is_missing) = if let Some(error) = symbol_receiver_error {
                    self.record_unexpected_error(*span, error);
                    (Type::Untyped, false)
                } else {
                    self.block_argument_result(
                        argument_type,
                        symbol.as_deref(),
                        first_parameter,
                        context,
                    )
                };
                if symbol_is_missing
                    && site.reports_diagnostics
                    && let Some(expected) = primary_prepared
                        .as_ref()
                        .and_then(PreparedCall::block_proc_type)
                {
                    let given = Type::instance("Proc");
                    let message = steep_relation::with_detail_lines(
                        format!(
                            "Cannot pass a value of type `{}` as a block-pass-argument of type `{}`",
                            given.steep_display(),
                            expected.steep_display()
                        ),
                        self.environment,
                        &given,
                        &expected,
                    );
                    self.record_diagnostic(*span, Severity::Warning, "Ruby::BlockTypeMismatch", message);
                }
                Some(block_type)
            }
            CallBlock::Absent => None,
        };
        let mut results = Vec::new();
        let mut has_unknown = false;
        let mut has_reportable_missing = false;
        let mut has_unreportable_missing = false;
        for (index, lookup) in lookups.iter().enumerate() {
            match lookup {
                Lookup::Found(found) => {
                    has_reportable_missing |= found.missing_from_signature;
                    let result = if Some(index) == primary_index {
                        let prepared = primary_prepared.as_mut().expect("primary call is prepared");
                        self.found_method_result(found, prepared, arguments, block_result.as_ref())
                    } else {
                        let mut prepared =
                            PreparedCall::new(self.environment, found, arguments, has_block);
                        self.found_method_result(
                            found,
                            &mut prepared,
                            arguments,
                            block_result.as_ref(),
                        )
                    };
                    results.push(result);
                }
                Lookup::Missing { reportable } => {
                    if *reportable {
                        has_reportable_missing = true;
                    } else {
                        has_unreportable_missing = true;
                    }
                }
                Lookup::Unknown => has_unknown = true,
                Lookup::Unreachable => results.push(Type::Bottom),
            }
        }
        if has_unknown || has_unreportable_missing {
            results.push(Type::Untyped);
        }
        let result = if results.is_empty() {
            Type::Untyped
        } else {
            Type::union(results)
        };
        // Steep's hover on `receiver&.name` shows what the method returns, without the `nil` that `&.` adds
        let displayed_result = result.clone();
        if site.reports_diagnostics {
            if has_reportable_missing && !has_unknown {
                self.record_diagnostic(
                    site.no_method_span,
                    Severity::Error,
                    "Ruby::NoMethod",
                    format!(
                        "Type `{}` does not have method `{}`",
                        displayed_receiver_type.steep_display(),
                        site.method_name
                    ),
                );
            }
            if let (Some(index), Some(prepared)) = (primary_index, &primary_prepared)
                && let Lookup::Found(found) = &lookups[index]
            {
                self.report_argument_errors(
                    found,
                    prepared,
                    site,
                    arguments,
                    displayed_receiver_type,
                );
            }
        }
        if let Some(index) = primary_index
            && let Lookup::Found(found) = &lookups[index]
        {
            self.record_hover(
                site.name_span,
                HoverKind::Method {
                    receiver_type: displayed_receiver_type.clone(),
                    owner: found.owner.clone(),
                    method_name: site.method_name.clone(),
                    singleton: found.singleton,
                    overloads: found.overloads.clone(),
                    return_type: displayed_result,
                    comment: found.comment.clone(),
                },
            );
            let mut targets: Vec<Location> = Vec::new();
            for lookup in &lookups {
                if let Lookup::Found(found) = lookup
                    && let Some(location) = &found.location
                    && !targets.contains(location)
                {
                    targets.push(location.clone());
                }
            }
            self.record_definition(site.name_span, targets);
        } else {
            self.record_type_hover(site.name_span, &displayed_result);
        }
        result
    }

    /// Return type of the overload these arguments select, before binding its type parameters
    pub(crate) fn selected_return_type(
        &self,
        found: &FoundMethod,
        arguments: &CallArguments,
    ) -> Option<Type> {
        if let Some(fixed_return_type) = &found.fixed_return_type {
            return Some(fixed_return_type.clone());
        }
        if found.overloads.is_empty() {
            return None;
        }
        let prepared = PreparedCall::new(self.environment, found, arguments, false);
        prepared
            .overload
            .map(|overload| overload.function.return_type)
    }

    fn found_method_result(
        &mut self,
        found: &FoundMethod,
        prepared: &mut PreparedCall,
        arguments: &CallArguments,
        block_result: Option<&Type>,
    ) -> Type {
        if let Some(fixed_return_type) = &found.fixed_return_type {
            return fixed_return_type.clone();
        }
        if let Some(file_target) = &found.file_target {
            return match &file_target.method {
                FileMethod::Definition(offset) => self
                    .infer_file_method_return_type(*offset)
                    .unwrap_or(Type::Untyped),
                FileMethod::AttributeReader(variable_name) => self.instance_variable_type(
                    &file_target.owner,
                    file_target.singleton,
                    variable_name,
                ),
                FileMethod::AttributeWriter(_) => arguments
                    .values
                    .first()
                    .map(|value| value.argument_type.clone())
                    .unwrap_or(Type::Untyped),
                FileMethod::Alias(_) => Type::Untyped,
            };
        }
        prepared.finish(self.environment, block_result)
    }

    fn report_argument_errors(
        &mut self,
        found: &FoundMethod,
        _prepared: &PreparedCall,
        site: &CallSite<'pr>,
        arguments: &CallArguments,
        receiver_type: &Type,
    ) {
        if !found.diagnostics_allowed || found.overloads.is_empty() {
            return;
        }
        let has_block = !matches!(site.block, CallBlock::Absent);
        let mut failures: Vec<Vec<OverloadError>> = Vec::new();
        for overload in &found.overloads {
            let errors = self.overload_errors(overload, arguments, has_block);
            if errors.is_empty() {
                return;
            }
            failures.push(errors);
        }
        let non_arity: Vec<Vec<OverloadError>> = failures
            .iter()
            .filter(|errors| !errors.iter().any(OverloadError::is_arity))
            .cloned()
            .collect();
        if !non_arity.is_empty() {
            failures = non_arity;
        }
        if let [errors] = failures.as_slice() {
            for error in errors.clone() {
                self.record_overload_error(&error, site);
            }
            return;
        }
        self.report_unresolved_overloading(found, site, receiver_type);
    }

    /// Steep's `try_method_type` for one overload: block presence, arity, then argument types
    fn overload_errors(
        &self,
        overload: &MethodType,
        arguments: &CallArguments,
        has_block: bool,
    ) -> Vec<OverloadError> {
        let mut errors = Vec::new();
        match &overload.block {
            None if has_block => errors.push(OverloadError::UnexpectedBlockGiven),
            Some(block) if block.required && !has_block => {
                errors.push(OverloadError::RequiredBlockMissing)
            }
            _ => {}
        }
        match match_arguments(&overload.function, arguments) {
            Err(mismatch) => errors.push(OverloadError::Arity(mismatch)),
            Ok(pairs) => {
                let type_parameter_names: Vec<String> = overload
                    .type_parameters
                    .iter()
                    .map(|type_parameter| type_parameter.name.clone())
                    .collect();
                for pair in pairs {
                    if contains_variable_in(&pair.parameter_type, &type_parameter_names)
                        || contains_uncertain_part(&pair.parameter_type)
                    {
                        continue;
                    }
                    let literal_fits = pair.literal_type.as_ref().is_some_and(|literal| {
                        !self.is_certain_mismatch(literal, &pair.parameter_type)
                    });
                    if !literal_fits
                        && self.is_certain_mismatch(&pair.argument_type, &pair.parameter_type)
                    {
                        errors.push(OverloadError::TypeMismatch(pair));
                    }
                }
            }
        }
        errors
    }

    fn record_overload_error(&mut self, error: &OverloadError, site: &CallSite<'pr>) {
        match error {
            OverloadError::RequiredBlockMissing => self.record_diagnostic(
                site.name_span,
                Severity::Error,
                "Ruby::RequiredBlockMissing",
                "The method cannot be called without a block".to_string(),
            ),
            OverloadError::UnexpectedBlockGiven => self.record_diagnostic(
                site.block.span().unwrap_or(site.name_span),
                Severity::Warning,
                "Ruby::UnexpectedBlockGiven",
                "The method cannot be called with a block".to_string(),
            ),
            OverloadError::Arity(ArityMismatch::InsufficientPositional) => self.record_diagnostic(
                site.send_span,
                Severity::Error,
                "Ruby::InsufficientPositionalArguments",
                "More positional arguments are required".to_string(),
            ),
            OverloadError::Arity(ArityMismatch::UnexpectedPositional(span)) => self
                .record_diagnostic(
                    *span,
                    Severity::Error,
                    "Ruby::UnexpectedPositionalArgument",
                    "Unexpected positional argument".to_string(),
                ),
            OverloadError::Arity(ArityMismatch::InsufficientKeywords(names)) => self
                .record_diagnostic(
                    site.send_span,
                    Severity::Error,
                    "Ruby::InsufficientKeywordArguments",
                    format!("More keyword arguments are required: {}", names.join(", ")),
                ),
            OverloadError::Arity(ArityMismatch::UnexpectedKeyword(span)) => self.record_diagnostic(
                *span,
                Severity::Error,
                "Ruby::UnexpectedKeywordArgument",
                "Unexpected keyword argument".to_string(),
            ),
            OverloadError::TypeMismatch(pair) => {
                let message = steep_relation::with_detail_lines(
                    format!(
                        "Cannot pass a value of type `{}` as an argument of type `{}`",
                        pair.argument_type.steep_display(),
                        pair.parameter_type.steep_display()
                    ),
                    self.environment,
                    &pair.argument_type,
                    &pair.parameter_type,
                );
                self.record_diagnostic(pair.span, Severity::Error, "Ruby::ArgumentTypeMismatch", message)
            }
        }
    }

    fn report_unresolved_overloading(
        &mut self,
        found: &FoundMethod,
        site: &CallSite<'pr>,
        receiver_type: &Type,
    ) {
        let alignment = " ".repeat("  def ".len() + site.method_name.len());
        let method_types: Vec<String> = found
            .overloads
            .iter()
            .enumerate()
            .map(|(index, overload)| {
                if index == 0 {
                    format!("  def {}: {}", site.method_name, overload.steep_display())
                } else {
                    format!("{alignment}| {}", overload.steep_display())
                }
            })
            .collect();
        self.record_diagnostic(
            site.expression_span,
            Severity::Error,
            "Ruby::UnresolvedOverloading",
            format!(
                "Cannot find compatible overloading of method `{}` of type `{}`\nMethod types:\n{}",
                site.method_name,
                receiver_type.steep_display(),
                method_types.join("\n")
            ),
        );
    }

    /// What `&:name` makes the block return, and whether `name` is missing on the yielded value, in which case
    /// Steep sees a plain `Proc` that does not fit the block type (`BlockTypeMismatch`)
    fn block_argument_result(
        &mut self,
        argument_type: &Type,
        symbol: Option<&str>,
        first_parameter: Option<Type>,
        context: &Context,
    ) -> (Type, bool) {
        if let Type::Proc(proc_type) = argument_type {
            return (proc_type.function.return_type.clone(), false);
        }
        let (Some(symbol), Some(first_parameter)) = (symbol, first_parameter) else {
            return (Type::Untyped, false);
        };
        let mut results = Vec::new();
        for member in members_of(&first_parameter) {
            match self.lookup_member(&member, symbol, context, 0) {
                Lookup::Found(found) => {
                    let mut prepared = PreparedCall::new(
                        self.environment,
                        &found,
                        &CallArguments::default(),
                        false,
                    );
                    results.push(self.found_method_result(
                        &found,
                        &mut prepared,
                        &CallArguments::default(),
                        None,
                    ));
                    if found.missing_from_signature {
                        return (Type::Untyped, true);
                    }
                }
                Lookup::Unreachable => {}
                Lookup::Missing { reportable: true } => return (Type::Untyped, true),
                _ => return (Type::Untyped, false),
            }
        }
        if results.is_empty() {
            (Type::Untyped, false)
        } else {
            (Type::union(results), false)
        }
    }

    pub(crate) fn operator_result_type(
        &mut self,
        current: &Type,
        operator: &str,
        value_type: Type,
        operator_span: Span,
        context: &Context,
    ) -> Type {
        let site = CallSite {
            method_name: operator.to_string(),
            name_span: operator_span,
            block: CallBlock::Absent,
            is_self_call: false,
            reports_diagnostics: true,
            deferred_lambdas: Vec::new(),
            expression_span: operator_span,
            send_span: operator_span,
            no_method_span: operator_span,
        };
        let arguments = CallArguments::single(value_type, operator_span);
        let mut no_locals = Locals::new();
        self.dispatch_call(current, current, &site, &arguments, context, &mut no_locals)
    }

    pub(crate) fn lookup_member(
        &mut self,
        member: &Type,
        method_name: &str,
        context: &Context,
        depth: usize,
    ) -> Lookup {
        if depth > MAXIMUM_LOOKUP_DEPTH {
            return Lookup::Unknown;
        }
        match member {
            Type::Bottom => Lookup::Unreachable,
            Type::Untyped | Type::Top | Type::Void | Type::Variable(_) | Type::Class => {
                Lookup::Unknown
            }
            Type::SelfType | Type::Instance => {
                if matches!(context.self_type, Type::SelfType | Type::Instance) {
                    Lookup::Unknown
                } else {
                    let self_type = context.self_type.clone();
                    self.lookup_member(&self_type, method_name, context, depth + 1)
                }
            }
            Type::ClassInstance { name, arguments } => {
                self.lookup_instance(name, arguments, member, method_name, context, depth)
            }
            Type::ClassSingleton { name } => {
                self.lookup_singleton(name, member, method_name, context, depth)
            }
            Type::Interface { name, arguments } => {
                self.lookup_interface(name, arguments, member, method_name)
            }
            Type::Alias { name, arguments } => match self.environment.expand_alias(name, arguments)
            {
                Some(expanded) => {
                    let expanded_members = members_of(&expanded);
                    match expanded_members.as_slice() {
                        [single] => self.lookup_member(single, method_name, context, depth + 1),
                        _ => Lookup::Unknown,
                    }
                }
                None => Lookup::Unknown,
            },
            Type::Literal(literal) => self.lookup_instance(
                literal_class_name(literal),
                &[],
                member,
                method_name,
                context,
                depth,
            ),
            Type::Bool => {
                self.lookup_instance("TrueClass", &[], &Type::Bool, method_name, context, depth)
            }
            Type::Nil => {
                self.lookup_instance("NilClass", &[], &Type::Nil, method_name, context, depth)
            }
            Type::Tuple(elements) => {
                let element = if elements.is_empty() {
                    Type::Untyped
                } else {
                    Type::union(elements.clone())
                };
                let array = Type::generic("Array", vec![element.clone()]);
                self.lookup_instance("Array", &[element], &array, method_name, context, depth)
            }
            Type::Record(fields) => {
                let value = Type::union(
                    fields
                        .iter()
                        .map(|field| field.field_type.clone())
                        .collect(),
                );
                let hash = hash_of(Type::instance("Symbol"), value.clone());
                self.lookup_instance(
                    "Hash",
                    &[Type::instance("Symbol"), value],
                    &hash,
                    method_name,
                    context,
                    depth,
                )
            }
            Type::Proc(proc_type) => {
                if matches!(method_name, "call" | "()" | "[]" | "yield" | "===") {
                    return Lookup::Found(Box::new(FoundMethod {
                        owner: "Proc".to_string(),
                        singleton: false,
                        overloads: vec![MethodType {
                            type_parameters: Vec::new(),
                            function: proc_type.function.clone(),
                            block: proc_type.block.clone(),
                        }],
                        comment: None,
                        location: None,
                        diagnostics_allowed: false,
                        is_private: false,
                        is_pure: false,
                        missing_from_signature: false,
                        file_target: None,
                        fixed_return_type: None,
                    }));
                }
                self.lookup_instance("Proc", &[], member, method_name, context, depth)
            }
            Type::Intersection(members) => {
                for intersection_member in members {
                    if let Lookup::Found(found) =
                        self.lookup_member(intersection_member, method_name, context, depth + 1)
                    {
                        return Lookup::Found(found);
                    }
                }
                Lookup::Unknown
            }
            Type::Union(_) => Lookup::Unknown,
        }
    }

    fn find_rbs_method(
        &self,
        class_name: &str,
        method_name: &str,
        singleton: bool,
    ) -> Option<ResolvedMethod<'environment>> {
        let environment = self.environment;
        let mut current = method_name.to_string();
        for _ in 0..MAXIMUM_LOOKUP_DEPTH {
            let resolved = if singleton {
                environment.find_singleton_method(class_name, &current)?
            } else {
                environment.find_instance_method(class_name, &current)?
            };
            if resolved.overloads.is_empty()
                && let Some(target) = &resolved.definition.alias_of
            {
                current = target.clone();
                continue;
            }
            return Some(resolved);
        }
        None
    }

    fn found_from_rbs(
        resolved: &ResolvedMethod<'_>,
        singleton: bool,
        substitution: &Substitution,
        diagnostics_allowed: bool,
    ) -> FoundMethod {
        FoundMethod {
            owner: resolved.definition.owner.clone(),
            singleton,
            overloads: resolved
                .overloads
                .iter()
                .map(|overload| substitute_overload(overload, substitution))
                .collect(),
            comment: resolved.definition.comment.clone(),
            location: resolved.definition.location.clone(),
            diagnostics_allowed,
            is_private: resolved.definition.visibility == rbs_environment::Visibility::Private,
            is_pure: is_pure_method(resolved.definition),
            missing_from_signature: false,
            file_target: None,
            fixed_return_type: None,
        }
    }

    fn declared_class_name(&self, name: &str) -> Option<String> {
        let declared = self.environment.resolve_class_name(name)?;
        self.environment
            .classes
            .contains_key(declared)
            .then(|| declared.to_string())
    }

    fn lookup_instance(
        &mut self,
        name: &str,
        arguments: &[Type],
        receiver: &Type,
        method_name: &str,
        context: &Context,
        depth: usize,
    ) -> Lookup {
        if let Some(declared) = self.declared_class_name(name) {
            if let Some(resolved) = self.find_rbs_method(&declared, method_name, false) {
                let substitution =
                    class_substitution(self.environment, &declared, arguments, receiver);
                return Lookup::Found(Box::new(Checker::found_from_rbs(
                    &resolved,
                    false,
                    &substitution,
                    true,
                )));
            }
            if let Some(found) = self.module_self_type_method(&declared, receiver, method_name) {
                return Lookup::Found(Box::new(found));
            }
            let reportable = self.is_no_method_reportable(&declared, false);
            if self.options.steep_compatible {
                return Lookup::Missing { reportable };
            }
            if let Some(lookup) =
                self.lookup_file_method(&declared, false, receiver, method_name, context, depth)
            {
                return mark_missing_from_signature(lookup, reportable);
            }
            return Lookup::Missing { reportable };
        }
        if let Some(lookup) =
            self.lookup_file_method(name, false, receiver, method_name, context, depth)
        {
            return lookup;
        }
        let fallback = self.rbs_fallback_class(name);
        if let Some(resolved) = self.find_rbs_method(&fallback, method_name, false) {
            let substitution = class_substitution(self.environment, &fallback, &[], receiver);
            return Lookup::Found(Box::new(Checker::found_from_rbs(
                &resolved,
                false,
                &substitution,
                false,
            )));
        }
        Lookup::Unknown
    }

    /// Steep's shape of a module instance includes the module's self types, `Object` when none is declared
    fn module_self_type_method(
        &self,
        module_name: &str,
        receiver: &Type,
        method_name: &str,
    ) -> Option<FoundMethod> {
        let declaration = self.environment.classes.get(module_name)?;
        if declaration.kind != ClassKind::Module {
            return None;
        }
        let self_types: Vec<String> = if declaration.module_self_types.is_empty() {
            vec!["Object".to_string()]
        } else {
            declaration
                .module_self_types
                .iter()
                .map(|ancestor| ancestor.name.clone())
                .collect()
        };
        for self_type in self_types {
            if let Some(resolved) = self.find_rbs_method(&self_type, method_name, false) {
                let substitution = class_substitution(self.environment, &self_type, &[], receiver);
                return Some(Checker::found_from_rbs(
                    &resolved,
                    false,
                    &substitution,
                    true,
                ));
            }
        }
        None
    }

    fn lookup_singleton(
        &mut self,
        name: &str,
        receiver: &Type,
        method_name: &str,
        context: &Context,
        depth: usize,
    ) -> Lookup {
        if let Some(declared) = self.declared_class_name(name) {
            if method_name == "new" {
                return self.lookup_new(&declared, receiver);
            }
            if let Some(resolved) = self.find_rbs_method(&declared, method_name, true) {
                let substitution = singleton_substitution(self.environment, &declared, receiver);
                return Lookup::Found(Box::new(Checker::found_from_rbs(
                    &resolved,
                    true,
                    &substitution,
                    true,
                )));
            }
            let reportable = self.is_no_method_reportable(&declared, true);
            if self.options.steep_compatible {
                return Lookup::Missing { reportable };
            }
            if let Some(lookup) =
                self.lookup_file_method(&declared, true, receiver, method_name, context, depth)
            {
                return mark_missing_from_signature(lookup, reportable);
            }
            return Lookup::Missing { reportable };
        }
        if let Some(lookup) =
            self.lookup_file_method(name, true, receiver, method_name, context, depth)
        {
            return lookup;
        }
        if method_name == "new" {
            return Lookup::Found(Box::new(FoundMethod {
                owner: name.to_string(),
                singleton: true,
                overloads: Vec::new(),
                comment: None,
                location: None,
                diagnostics_allowed: false,
                is_private: false,
                is_pure: false,
                missing_from_signature: false,
                file_target: None,
                fixed_return_type: Some(Type::instance(name)),
            }));
        }
        let fallback = self.rbs_fallback_class(name);
        if let Some(resolved) = self.find_rbs_method(&fallback, method_name, true) {
            let mut substitution = singleton_substitution(self.environment, &fallback, receiver);
            substitution.instance_type = Some(Type::instance(name));
            return Lookup::Found(Box::new(Checker::found_from_rbs(
                &resolved,
                true,
                &substitution,
                false,
            )));
        }
        Lookup::Unknown
    }

    /// `Foo.new` is derived from `Foo#initialize` by the environment. Arguments are not checked against an
    /// `initialize` inherited from `Object` or `BasicObject`, since signatures often leave `initialize` out
    /// RBS derives `Foo.new` from the `initialize` that `Foo`'s instances see, which may come from an included
    /// module; the environment only synthesizes `new` on classes that declare `initialize` themselves, so a derived
    /// `new` found up the singleton chain is rebuilt from the nearest `initialize` instead
    fn lookup_new(&mut self, class_name: &str, receiver: &Type) -> Lookup {
        let substitution = singleton_substitution(self.environment, class_name, receiver);
        let Some(resolved) = self.find_rbs_method(class_name, "new", true) else {
            return Lookup::Missing {
                reportable: self.is_no_method_reportable(class_name, true),
            };
        };
        if let Some(explicit_owner) = self.explicit_new_owner(class_name)
            && let Some(explicit) = self.find_rbs_method(&explicit_owner, "new", true)
        {
            return Lookup::Found(Box::new(Checker::found_from_rbs(
                &explicit,
                true,
                &substitution,
                true,
            )));
        }
        if !self.is_derived_new(&resolved) {
            return Lookup::Found(Box::new(Checker::found_from_rbs(
                &resolved,
                true,
                &substitution,
                true,
            )));
        }
        let Some(initialize) = self.find_rbs_method(class_name, "initialize", false) else {
            return Lookup::Found(Box::new(Checker::found_from_rbs(
                &resolved,
                true,
                &substitution,
                true,
            )));
        };
        let class_parameters: Vec<rbs_environment::TypeParameter> = self
            .environment
            .classes
            .get(class_name)
            .map(|declaration| declaration.type_parameters.clone())
            .unwrap_or_default();
        let instance = self_instance_type(self.environment, class_name);
        let instance_substitution = Substitution {
            self_type: Some(instance.clone()),
            instance_type: Some(instance.clone()),
            class_type: Some(receiver.clone()),
            ..Substitution::default()
        };
        let overloads = initialize
            .overloads
            .iter()
            .map(|overload| {
                let mut type_parameters = class_parameters.clone();
                type_parameters.extend(overload.type_parameters.iter().cloned());
                let mut function = overload.function.substitute(&instance_substitution);
                function.return_type = instance.clone();
                MethodType {
                    type_parameters,
                    function,
                    block: overload
                        .block
                        .as_ref()
                        .map(|block| block.substitute(&instance_substitution)),
                }
            })
            .collect();
        let mut found = Checker::found_from_rbs(&initialize, true, &Substitution::default(), true);
        found.owner = class_name.to_string();
        found.overloads = overloads;
        found.is_private = false;
        found.is_pure = false;
        Lookup::Found(Box::new(found))
    }

    /// Steep's `deep_expand_alias` for the top level of a receiver type
    pub(crate) fn expand_alias_deeply(&self, target: &Type, depth: usize) -> Type {
        expand_aliases(self.environment, target, depth)
    }

    /// The nearest class up the superclass chain that declares `self.new` itself. RBS keeps such an inherited
    /// `new` even for a subclass that declares its own `initialize`
    fn explicit_new_owner(&self, class_name: &str) -> Option<String> {
        let mut current = Some(class_name.to_string());
        for _ in 0..64 {
            let name = current?;
            let declaration = self.environment.classes.get(&name)?;
            if let Some(new_method) = declaration.singleton_methods.get("new") {
                let synthesized =
                    declaration
                        .instance_methods
                        .get("initialize")
                        .is_some_and(|initialize| {
                            initialize.location.is_some()
                                && initialize.location == new_method.location
                        });
                if !synthesized {
                    return Some(name);
                }
            }
            current = declaration
                .super_class
                .as_ref()
                .map(|superclass| superclass.name.clone());
        }
        None
    }

    fn is_derived_new(&self, resolved: &ResolvedMethod<'_>) -> bool {
        let owner = resolved.definition.owner.as_str();
        if owner == "Class" {
            return true;
        }
        let Some(declaration) = self.environment.classes.get(owner) else {
            return false;
        };
        declaration
            .instance_methods
            .get("initialize")
            .is_some_and(|initialize| {
                initialize.location.is_some() && initialize.location == resolved.definition.location
            })
    }

    fn lookup_interface(
        &mut self,
        name: &str,
        arguments: &[Type],
        receiver: &Type,
        method_name: &str,
    ) -> Lookup {
        let Some(resolved) = self.environment.find_interface_method(name, method_name) else {
            return if self.environment.interfaces.contains_key(name) {
                Lookup::Missing { reportable: true }
            } else {
                Lookup::Unknown
            };
        };
        let mut substitution = Substitution {
            self_type: Some(receiver.clone()),
            instance_type: Some(receiver.clone()),
            ..Substitution::default()
        };
        if let Some(interface) = self.environment.interfaces.get(name) {
            for (index, type_parameter) in interface.type_parameters.iter().enumerate() {
                substitution.variables.insert(
                    type_parameter.name.clone(),
                    arguments.get(index).cloned().unwrap_or(Type::Untyped),
                );
            }
        }
        Lookup::Found(Box::new(Checker::found_from_rbs(
            &resolved,
            false,
            &substitution,
            false,
        )))
    }

    fn lookup_file_method(
        &mut self,
        class_name: &str,
        singleton: bool,
        receiver: &Type,
        method_name: &str,
        context: &Context,
        depth: usize,
    ) -> Option<Lookup> {
        let (owner, owner_singleton, method) =
            self.find_file_method(class_name, singleton, method_name)?;
        if let FileMethod::Alias(target) = &method {
            if target == method_name {
                return Some(Lookup::Unknown);
            }
            return Some(self.lookup_member(receiver, target, context, depth + 1));
        }
        Some(Lookup::Found(Box::new(FoundMethod {
            owner: owner.clone(),
            singleton: owner_singleton,
            overloads: Vec::new(),
            comment: None,
            location: None,
            diagnostics_allowed: false,
            is_private: false,
            is_pure: false,
            missing_from_signature: false,
            file_target: Some(FileTarget {
                owner,
                singleton: owner_singleton,
                method,
            }),
            fixed_return_type: None,
        })))
    }

    pub(crate) fn find_file_method(
        &self,
        class_name: &str,
        singleton: bool,
        method_name: &str,
    ) -> Option<(String, bool, FileMethod)> {
        let mut queue: VecDeque<(String, bool)> =
            VecDeque::from([(class_name.to_string(), singleton)]);
        let mut visited: HashSet<(String, bool)> = HashSet::new();
        while let Some((current, is_singleton)) = queue.pop_front() {
            if visited.len() > 64 || !visited.insert((current.clone(), is_singleton)) {
                continue;
            }
            if let Some(method) = self
                .file_index
                .find_method(&current, is_singleton, method_name)
            {
                return Some((current, is_singleton, method.clone()));
            }
            if let Some(file_class) = self.file_index.classes.get(&current) {
                let modules = if is_singleton {
                    &file_class.extends
                } else {
                    &file_class.includes
                };
                for module in modules {
                    queue.push_back((module.clone(), false));
                }
                if let Some(superclass) = &file_class.superclass {
                    queue.push_back((superclass.clone(), is_singleton));
                }
            }
            if let Some(declaration) = self.environment.classes.get(&current) {
                if is_singleton {
                    for extended in &declaration.extends {
                        queue.push_back((extended.name.clone(), false));
                    }
                    if let Some(superclass) = &declaration.super_class {
                        queue.push_back((superclass.name.clone(), true));
                    }
                } else {
                    for ancestor in self.environment.ancestors(&current) {
                        queue.push_back((ancestor.name, false));
                    }
                }
            }
            if queue.is_empty() && !visited.contains(&("Object".to_string(), false)) {
                queue.push_back(("Object".to_string(), false));
            }
        }
        None
    }

    fn rbs_fallback_class(&self, class_name: &str) -> String {
        let mut current = class_name.to_string();
        for _ in 0..32 {
            let Some(file_class) = self.file_index.classes.get(&current) else {
                break;
            };
            if file_class.kind == ClassKind::Module {
                return "Object".to_string();
            }
            let Some(superclass) = &file_class.superclass else {
                break;
            };
            if let Some(declared) = self.declared_class_name(superclass) {
                return declared;
            }
            current = superclass.clone();
        }
        "Object".to_string()
    }

    fn is_no_method_reportable(&mut self, class_name: &str, singleton: bool) -> bool {
        if !self.is_fully_known(class_name) {
            return false;
        }
        if singleton {
            let extends_known =
                self.environment
                    .singleton_ancestors(class_name)
                    .iter()
                    .all(|ancestor| match ancestor {
                        rbs_environment::SingletonAncestor::Singleton(name) => {
                            self.environment.classes.contains_key(name)
                        }
                        rbs_environment::SingletonAncestor::Instance(ancestor) => {
                            self.environment.classes.contains_key(&ancestor.name)
                        }
                    });
            if !extends_known {
                return false;
            }
        }
        true
    }

    pub(crate) fn infer_call_write(
        &mut self,
        node: &Node<'pr>,
        context: &Context,
        locals: &mut Locals,
    ) -> Type {
        let (receiver, read_name, message_location, index_arguments, value, operator) = match node {
            Node::CallOrWriteNode { .. } => {
                let Some(write) = node.as_call_or_write_node() else {
                    return Type::Untyped;
                };
                (
                    write.receiver(),
                    constant_text(&write.read_name()),
                    write.message_loc(),
                    None,
                    write.value(),
                    WriteOperator::Or,
                )
            }
            Node::CallAndWriteNode { .. } => {
                let Some(write) = node.as_call_and_write_node() else {
                    return Type::Untyped;
                };
                (
                    write.receiver(),
                    constant_text(&write.read_name()),
                    write.message_loc(),
                    None,
                    write.value(),
                    WriteOperator::And,
                )
            }
            Node::CallOperatorWriteNode { .. } => {
                let Some(write) = node.as_call_operator_write_node() else {
                    return Type::Untyped;
                };
                let operator = WriteOperator::Binary(
                    constant_text(&write.binary_operator()),
                    Span::of_location(&write.binary_operator_loc()),
                );
                (
                    write.receiver(),
                    constant_text(&write.read_name()),
                    write.message_loc(),
                    None,
                    write.value(),
                    operator,
                )
            }
            Node::IndexOrWriteNode { .. } => {
                let Some(write) = node.as_index_or_write_node() else {
                    return Type::Untyped;
                };
                (
                    write.receiver(),
                    "[]".to_string(),
                    Some(write.opening_loc()),
                    Some(write.arguments()),
                    write.value(),
                    WriteOperator::Or,
                )
            }
            Node::IndexAndWriteNode { .. } => {
                let Some(write) = node.as_index_and_write_node() else {
                    return Type::Untyped;
                };
                (
                    write.receiver(),
                    "[]".to_string(),
                    Some(write.opening_loc()),
                    Some(write.arguments()),
                    write.value(),
                    WriteOperator::And,
                )
            }
            Node::IndexOperatorWriteNode { .. } => {
                let Some(write) = node.as_index_operator_write_node() else {
                    return Type::Untyped;
                };
                let operator = WriteOperator::Binary(
                    constant_text(&write.binary_operator()),
                    Span::of_location(&write.binary_operator_loc()),
                );
                (
                    write.receiver(),
                    "[]".to_string(),
                    Some(write.opening_loc()),
                    Some(write.arguments()),
                    write.value(),
                    operator,
                )
            }
            _ => return Type::Untyped,
        };
        let receiver_type = match &receiver {
            Some(receiver) => self.infer(receiver, context, locals),
            None => context.self_type.clone(),
        };
        let arguments = match index_arguments {
            Some(index_arguments) => self.infer_arguments(index_arguments, context, locals),
            None => CallArguments::default(),
        };
        let name_span = message_location
            .map(|location| Span::of_location(&location))
            .unwrap_or(Span::of_node(node));
        let site = CallSite {
            method_name: read_name,
            name_span,
            block: CallBlock::Absent,
            is_self_call: receiver.is_none(),
            reports_diagnostics: true,
            deferred_lambdas: Vec::new(),
            expression_span: name_span,
            send_span: name_span,
            no_method_span: name_span,
        };
        let current = self.dispatch_call(
            &remove_nil(&receiver_type),
            &receiver_type,
            &site,
            &arguments,
            context,
            locals,
        );
        let value_type = self.infer(&value, context, locals);
        match operator {
            WriteOperator::Or => super::variables::or_assignment_type(&current, value_type),
            WriteOperator::And => Type::union(vec![
                crate::type_utilities::falsy_part(&current),
                value_type,
            ]),
            WriteOperator::Binary(operator, operator_span) => {
                self.operator_result_type(&current, &operator, value_type, operator_span, context)
            }
        }
    }

    pub(crate) fn infer_super(
        &mut self,
        node: &Node<'pr>,
        context: &Context,
        locals: &mut Locals,
    ) -> Type {
        let (arguments, block_node, keyword_span) = match node {
            Node::SuperNode { .. } => {
                let Some(super_node) = node.as_super_node() else {
                    return Type::Untyped;
                };
                (
                    self.infer_arguments(super_node.arguments(), context, locals),
                    super_node.block(),
                    Span::of_location(&super_node.keyword_loc()),
                )
            }
            _ => {
                let Some(forwarding) = node.as_forwarding_super_node() else {
                    return Type::Untyped;
                };
                let forwarded = CallArguments {
                    values: vec![super::arguments::ArgumentValue {
                        argument_type: Type::Untyped,
                        span: Span::of_node(node),
                        kind: super::arguments::ArgumentKind::Forwarding,
                        literal_type: None,
                    }],
                };
                (
                    forwarded,
                    forwarding.block().map(|block| block.as_node()),
                    Span::of_node(node),
                )
            }
        };
        let block = self.call_block(block_node, context, locals);
        let Some(method) = context.method.clone() else {
            if let CallBlock::Literal(block_node) = &block {
                self.infer_block(block_node, &BlockSignature::untyped(), context, locals);
            }
            return Type::Untyped;
        };
        if !self.super_method_exists(&method.owner, method.singleton, &method.name) {
            self.record_diagnostic(
                Span::of_node(node),
                Severity::Information,
                "Ruby::UnexpectedSuper",
                format!("No superclass method `{}` defined", method.name),
            );
        }
        let superclass = self
            .environment
            .classes
            .get(&method.owner)
            .and_then(|declaration| {
                declaration
                    .super_class
                    .as_ref()
                    .map(|ancestor| ancestor.name.clone())
            })
            .or_else(|| {
                self.file_index
                    .classes
                    .get(&method.owner)
                    .and_then(|class| class.superclass.clone())
            });
        let receiver = match (superclass, method.singleton) {
            (Some(superclass), true) => Type::ClassSingleton { name: superclass },
            (Some(superclass), false) => instance_type_of_class(self.environment, &superclass),
            (None, _) => Type::Untyped,
        };
        let site = CallSite {
            method_name: method.name.clone(),
            name_span: keyword_span,
            block,
            is_self_call: true,
            reports_diagnostics: false,
            deferred_lambdas: Vec::new(),
            expression_span: keyword_span,
            send_span: keyword_span,
            no_method_span: keyword_span,
        };
        self.dispatch_call(&receiver, &receiver, &site, &arguments, context, locals)
    }

    /// Steep's `super_method`: the method the class's own declaration overrides, or the inherited one when the
    /// class does not declare it
    fn super_method_exists(&self, owner: &str, singleton: bool, method_name: &str) -> bool {
        let Some(declaration) = self.environment.classes.get(owner) else {
            return false;
        };
        let declared_here = if singleton {
            declaration.singleton_methods.contains_key(method_name)
        } else {
            declaration.instance_methods.contains_key(method_name)
        };
        if !declared_here {
            return if singleton {
                self.environment
                    .find_singleton_method(owner, method_name)
                    .is_some()
            } else {
                self.environment
                    .find_instance_method(owner, method_name)
                    .is_some()
            };
        }
        if singleton {
            return declaration.super_class.as_ref().is_some_and(|superclass| {
                self.environment
                    .find_singleton_method(&superclass.name, method_name)
                    .is_some()
            });
        }
        self.environment
            .ancestors(owner)
            .iter()
            .skip_while(|ancestor| ancestor.name != owner)
            .skip(1)
            .any(|ancestor| {
                self.environment
                    .classes
                    .get(&ancestor.name)
                    .is_some_and(|ancestor_declaration| {
                        ancestor_declaration
                            .instance_methods
                            .contains_key(method_name)
                    })
            })
    }

    pub(crate) fn infer_yield(
        &mut self,
        node: &Node<'pr>,
        context: &Context,
        locals: &mut Locals,
    ) -> Type {
        let Some(yield_node) = node.as_yield_node() else {
            return Type::Untyped;
        };
        self.infer_arguments(yield_node.arguments(), context, locals);
        match context
            .method
            .as_ref()
            .and_then(|method| method.block.as_ref())
        {
            Some(block) => {
                let return_type = block.function.return_type.clone();
                if contains_uncertain_part(&return_type) {
                    Type::Untyped
                } else {
                    return_type
                }
            }
            None => Type::Untyped,
        }
    }

    fn record_attribute_hints(
        &mut self,
        call: &CallNode<'pr>,
        method_name: &str,
        context: &Context,
    ) {
        let Type::ClassSingleton { name: class_name } = &context.self_type else {
            return;
        };
        let class_name = class_name.clone();
        let Some(arguments) = call.arguments() else {
            return;
        };
        for argument in arguments.arguments().iter() {
            let Some(attribute_name) = crate::file_index::symbol_name(&argument) else {
                continue;
            };
            let attribute_type = self.attribute_type(&class_name, &attribute_name, method_name);
            self.record_hint(
                TypeHintKind::AttributeType,
                argument.location().start_offset(),
                &attribute_type,
                false,
            );
        }
    }

    fn attribute_type(
        &mut self,
        class_name: &str,
        attribute_name: &str,
        method_name: &str,
    ) -> Type {
        let self_type = self_instance_type(self.environment, class_name);
        let substitution = class_substitution(self.environment, class_name, &[], &self_type);
        if method_name != "attr_writer"
            && let Some(resolved) = self.find_rbs_method(class_name, attribute_name, false)
            && !resolved.overloads.is_empty()
        {
            return Type::union(
                resolved
                    .overloads
                    .iter()
                    .map(|overload| overload.function.return_type.substitute(&substitution))
                    .collect(),
            );
        }
        if let Some(resolved) =
            self.find_rbs_method(class_name, &format!("{attribute_name}="), false)
            && let Some(parameter) = resolved
                .overloads
                .first()
                .and_then(|overload| overload.function.required_positionals.first())
        {
            return parameter.parameter_type.substitute(&substitution);
        }
        let variable_type =
            self.instance_variable_type(class_name, false, &format!("@{attribute_name}"));
        if is_bottom(&variable_type) {
            Type::Untyped
        } else {
            variable_type
        }
    }
}

fn mark_missing_from_signature(lookup: Lookup, reportable: bool) -> Lookup {
    match lookup {
        Lookup::Found(mut found) => {
            found.missing_from_signature = reportable;
            Lookup::Found(found)
        }
        Lookup::Missing { .. } => Lookup::Missing { reportable },
        other => other,
    }
}

/// Steep's `MethodCall::Typed#pure?` plus `KNOWN_PURE_METHODS`
fn is_pure_method(definition: &rbs_environment::MethodDefinition) -> bool {
    if definition.name.ends_with('=') {
        return false;
    }
    definition.is_attribute_reader
        || definition
            .annotations
            .iter()
            .any(|annotation| annotation == "pure")
        || (definition.name == "[]" && matches!(definition.owner.as_str(), "Array" | "Hash"))
}

enum WriteOperator {
    Or,
    And,
    Binary(String, Span),
}

fn tuple_element(elements: &[Type], method_name: &str, call: &CallNode<'_>) -> Option<Type> {
    let arguments: Vec<Node<'_>> = call
        .arguments()
        .map(|arguments| arguments.arguments().iter().collect())
        .unwrap_or_default();
    let index: i64 = match (method_name, arguments.as_slice()) {
        ("first", []) => 0,
        ("last", []) => -1,
        ("[]", [argument]) => {
            let text = String::from_utf8_lossy(argument.location().as_slice()).replace('_', "");
            if !matches!(argument, Node::IntegerNode { .. } | Node::CallNode { .. }) {
                return None;
            }
            text.parse().ok()?
        }
        _ => return None,
    };
    let length = elements.len() as i64;
    let position = if index < 0 { length + index } else { index };
    if position < 0 || position >= length {
        return Some(Type::Nil);
    }
    elements.get(position as usize).cloned()
}

/// `record[:key]` and `record.fetch(:key)` on a record type such as `{ name: String }` give the field type
fn record_field(
    fields: &[rbs_environment::RecordField],
    method_name: &str,
    call: &CallNode<'_>,
) -> Option<Type> {
    if !matches!(method_name, "[]" | "fetch") {
        return None;
    }
    let arguments: Vec<Node<'_>> = call
        .arguments()
        .map(|arguments| arguments.arguments().iter().collect())
        .unwrap_or_default();
    let [key] = arguments.as_slice() else {
        return None;
    };
    let key_name = String::from_utf8_lossy(key.as_symbol_node()?.unescaped()).into_owned();
    let field = fields.iter().find(|field| field.name == key_name)?;
    Some(if field.required {
        field.field_type.clone()
    } else {
        Type::optional(field.field_type.clone())
    })
}

/// Steep's special case for `Array#compact` and `Hash#compact`, which drop `nil` from the element type
fn compact_result(
    method_name: &str,
    arguments: &CallArguments,
    call: &CallNode<'_>,
    result: &Type,
) -> Option<Type> {
    if method_name != "compact" || !arguments.values.is_empty() || call.block().is_some() {
        return None;
    }
    match result {
        Type::ClassInstance { name, arguments } if name == "Array" && arguments.len() == 1 => {
            Some(Type::generic("Array", vec![remove_nil(&arguments[0])]))
        }
        Type::ClassInstance { name, arguments } if name == "Hash" && arguments.len() == 2 => {
            Some(Type::generic(
                "Hash",
                vec![arguments[0].clone(), remove_nil(&arguments[1])],
            ))
        }
        _ => None,
    }
}

/// Expands aliases at the top level and inside unions, the way Steep's `deep_expand_alias` does
pub(crate) fn expand_aliases(
    environment: &rbs_environment::Environment,
    target: &Type,
    depth: usize,
) -> Type {
    if depth > MAXIMUM_LOOKUP_DEPTH {
        return target.clone();
    }
    match target {
        Type::Alias { name, arguments } => match environment.expand_alias(name, arguments) {
            Some(expanded) => expand_aliases(environment, &expanded, depth + 1),
            None => target.clone(),
        },
        Type::Union(members) => Type::union(
            members
                .iter()
                .map(|member| expand_aliases(environment, member, depth + 1))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn class_of(receiver: &Type) -> Option<Type> {
    let name = match receiver {
        Type::ClassInstance { name, .. } => name.clone(),
        Type::Literal(literal) => literal_class_name(literal).to_string(),
        Type::Nil => "NilClass".to_string(),
        Type::Tuple(_) => "Array".to_string(),
        Type::Record(_) => "Hash".to_string(),
        _ => return None,
    };
    Some(Type::ClassSingleton { name })
}

fn singleton_substitution(
    environment: &rbs_environment::Environment,
    class_name: &str,
    receiver: &Type,
) -> Substitution {
    Substitution {
        self_type: Some(receiver.clone()),
        instance_type: Some(instance_type_of_class(environment, class_name)),
        class_type: Some(receiver.clone()),
        ..Substitution::default()
    }
}
