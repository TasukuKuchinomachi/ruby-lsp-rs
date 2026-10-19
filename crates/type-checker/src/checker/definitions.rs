use std::rc::Rc;

use rbs_environment::{MethodType, ProcType, Substitution, Type};
use ruby_prism::{DefNode, Node, ParametersNode};

use super::variables::is_obvious_value;
use super::{CacheState, Checker, Context, Locals, MethodContext};
use crate::file_index::InstanceVariableKey;
use crate::source::{ConstantPath, Span, constant_text};
use crate::steep_relation;
use crate::type_utilities::{array_of, hash_of, self_instance_type};
use crate::{HoverKind, Severity, TypeHintKind};

struct DefinitionOwner {
    owner: String,
    singleton: bool,
    nesting: Vec<String>,
}

impl<'environment, 'pr> Checker<'environment, 'pr> {
    pub(crate) fn infer_definition_node(
        &mut self,
        node: &Node<'pr>,
        context: &Context,
        locals: &mut Locals,
    ) -> Type {
        let Some(definition) = node.as_def_node() else {
            return Type::Untyped;
        };
        let offset = node.location().start_offset();
        let owner = match self.file_index.definitions.get(&offset) {
            Some(info) => DefinitionOwner {
                owner: info.owner.clone(),
                singleton: info.singleton,
                nesting: info.nesting.clone(),
            },
            None => {
                if let Some(receiver) = definition.receiver() {
                    self.infer(&receiver, context, locals);
                }
                let mut unknown_context = context.clone();
                unknown_context.self_type = Type::Untyped;
                unknown_context.method = None;
                let mut definition_locals = Locals::new();
                self.infer_optional_node(
                    definition.body(),
                    &unknown_context,
                    &mut definition_locals,
                );
                return Type::instance("Symbol");
            }
        };
        let is_first_inference = !self.method_return_cache.contains_key(&offset);
        if is_first_inference {
            self.method_return_cache
                .insert(offset, CacheState::InProgress);
        }
        let (return_type, writes) = self.check_definition(&definition, &owner);
        self.store_instance_variable_writes(writes);
        if is_first_inference
            || matches!(
                self.method_return_cache.get(&offset),
                Some(CacheState::InProgress)
            )
        {
            self.method_return_cache
                .insert(offset, CacheState::Done(return_type));
        }
        Type::instance("Symbol")
    }

    pub(crate) fn infer_file_method_return_type(&mut self, offset: usize) -> Option<Type> {
        match self.method_return_cache.get(&offset) {
            Some(CacheState::Done(return_type)) => return Some(return_type.clone()),
            Some(CacheState::InProgress) => return None,
            None => {}
        }
        let info = self.file_index.definitions.get(&offset)?;
        let node = prism_child_nodes::copy_node(&info.node);
        let owner = DefinitionOwner {
            owner: info.owner.clone(),
            singleton: info.singleton,
            nesting: info.nesting.clone(),
        };
        let definition = node.as_def_node()?;
        self.method_return_cache
            .insert(offset, CacheState::InProgress);
        let result = self.silently(|checker| checker.check_definition(&definition, &owner));
        let Some((return_type, writes)) = result else {
            self.method_return_cache.remove(&offset);
            return None;
        };
        self.store_instance_variable_writes(writes);
        self.method_return_cache
            .insert(offset, CacheState::Done(return_type.clone()));
        Some(return_type)
    }

    fn store_instance_variable_writes(&mut self, writes: Vec<(InstanceVariableKey, Type)>) {
        for (key, written_type) in writes {
            let types = self
                .inferred_instance_variable_writes
                .entry(key)
                .or_default();
            if !types.contains(&written_type) {
                types.push(written_type);
            }
        }
    }

    fn check_definition(
        &mut self,
        definition: &DefNode<'pr>,
        owner: &DefinitionOwner,
    ) -> (Type, Vec<(InstanceVariableKey, Type)>) {
        let method_name = constant_text(&definition.name());
        let scope_start = definition.location().start_offset();
        let nesting = Rc::new(owner.nesting.clone());
        let mut self_type = if owner.singleton {
            Type::ClassSingleton {
                name: owner.owner.clone(),
            }
        } else {
            self_instance_type(self.environment, &owner.owner)
        };
        if let Some(annotated_self) = self.annotated_self_type(scope_start, &nesting) {
            self_type = annotated_self;
        }
        let overloads =
            self.declared_overloads(&owner.owner, owner.singleton, &method_name, &self_type);
        // Steep checks a `def` against all overloads joined into one method type, whose return type is the union
        let single_return_type = if overloads.is_empty() {
            None
        } else {
            Some(Type::union(
                overloads
                    .iter()
                    .map(|overload| overload.function.return_type.clone())
                    .collect(),
            ))
        };
        self.report_definition_declaration(definition, owner, &method_name, &overloads);
        let block = match overloads.as_slice() {
            [overload] => overload.block.clone(),
            _ => overloads.iter().find_map(|overload| overload.block.clone()),
        };
        let context = Context {
            self_type: self_type.clone(),
            nesting: nesting.clone(),
            method: Some(Rc::new(MethodContext {
                name: method_name.clone(),
                owner: owner.owner.clone(),
                singleton: owner.singleton,
                declared_return_type: single_return_type.clone(),
                block,
            })),
            declared_locals: Rc::new(self.local_variable_annotations(scope_start, &nesting)),
        };
        let mut locals = Locals::new();
        if let Some(parameters) = definition.parameters() {
            let first_parameter_diagnostic = self.analysis.diagnostics.len();
            self.assign_definition_parameters(&parameters, &overloads, &context, &mut locals);
            // Steep's `synthesize` of `defs` types the parameters twice, so it reports what their default values
            // contain twice
            if self.options.steep_compatible && definition.receiver().is_some() {
                let parameter_diagnostics =
                    self.analysis.diagnostics[first_parameter_diagnostic..].to_vec();
                self.repeated_diagnostics.extend(parameter_diagnostics);
            }
        }
        self.instance_variable_write_frames.push(Vec::new());
        self.method_return_frames.push(Vec::new());
        let body_type = self.infer_optional_node(definition.body(), &context, &mut locals);
        let returned_types = self.method_return_frames.pop().unwrap_or_default();
        let writes = self
            .instance_variable_write_frames
            .pop()
            .unwrap_or_default();
        let declared_return_type = if overloads.is_empty() {
            None
        } else {
            Some(Type::union(
                overloads
                    .iter()
                    .map(|overload| overload.function.return_type.clone())
                    .collect(),
            ))
        };
        let return_type = match &declared_return_type {
            Some(declared) => declared.clone(),
            None => {
                let mut candidates = returned_types;
                candidates.push(body_type.clone());
                Type::union(candidates)
            }
        };
        if let Some(declared) = &single_return_type
            && !matches!(declared, Type::Void)
            && method_name != "initialize"
            && self.is_certain_mismatch(&body_type, declared)
            && self.last_statement_mismatches(
                definition.body(),
                declared,
                &body_type,
                &context,
                &locals,
            )
        {
            let is_setter = method_name.ends_with('=')
                && !matches!(method_name.as_str(), "==" | "!=" | "<=" | ">=" | "===");
            let (body, declared_type) = (body_type.steep_display(), declared.steep_display());
            if is_setter {
                let message = steep_relation::with_detail_lines(
                    format!("Setter method `{method_name}` cannot have type `{body}` because declared as type `{declared_type}`"),
                    self.environment,
                    &body_type,
                    declared,
                );
                self.record_diagnostic(
                    Span::of_location(&definition.name_loc()),
                    Severity::Information,
                    "Ruby::SetterBodyTypeMismatch",
                    message,
                );
            } else {
                let message = steep_relation::with_detail_lines(
                    format!("Cannot allow method body have type `{body}` because declared as type `{declared_type}`"),
                    self.environment,
                    &body_type,
                    declared,
                );
                self.record_diagnostic(
                    Span::of_location(&definition.name_loc()),
                    Severity::Error,
                    "Ruby::MethodBodyTypeMismatch",
                    message,
                );
            }
        }
        let hint_offset = match (definition.rparen_loc(), definition.parameters()) {
            (Some(right_parenthesis), _) => right_parenthesis.end_offset(),
            (None, Some(parameters)) => parameters.location().end_offset(),
            (None, None) => definition.name_loc().end_offset(),
        };
        self.record_hint(
            TypeHintKind::MethodReturnType,
            hint_offset,
            &return_type,
            false,
        );
        let name_span = Span::of_location(&definition.name_loc());
        let (comment, location) =
            self.declared_method_documentation(&owner.owner, owner.singleton, &method_name);
        self.record_hover(
            name_span,
            HoverKind::Method {
                receiver_type: self_type,
                owner: owner.owner.clone(),
                method_name,
                singleton: owner.singleton,
                overloads,
                return_type: return_type.clone(),
                comment,
            },
        );
        if let Some(location) = location {
            self.record_definition(name_span, vec![location]);
        }
        (return_type, writes)
    }

    /// Steep's `UndeclaredMethodDefinition` / `MethodDefinitionInUndeclaredModule`, and the parameter checks of
    /// `MethodParams.build` against a single declared overload
    fn report_definition_declaration(
        &mut self,
        definition: &DefNode<'pr>,
        owner: &DefinitionOwner,
        method_name: &str,
        overloads: &[MethodType],
    ) {
        let name_span = Span::of_location(&definition.name_loc());
        if overloads.is_empty() {
            if self.environment.classes.contains_key(&owner.owner) {
                let separator = if owner.singleton { "." } else { "#" };
                self.record_diagnostic(
                    name_span,
                    Severity::Warning,
                    "Ruby::UndeclaredMethodDefinition",
                    format!(
                        "Method `::{}{separator}{method_name}` is not declared in RBS",
                        owner.owner
                    ),
                );
            } else {
                self.record_diagnostic(
                    name_span,
                    Severity::Information,
                    "Ruby::MethodDefinitionInUndeclaredModule",
                    format!("Method `{method_name}` is defined in undeclared module"),
                );
            }
            return;
        }
        let [overload] = overloads else {
            return;
        };
        let parameter_span = definition
            .parameters()
            .map(|parameters| Span::of_location(&parameters.location()))
            .unwrap_or(name_span);
        let overload = overload.steep_display();
        for (kind, span) in parameter_kind_errors(definition.parameters(), &overloads[0].function) {
            let (code, severity, message) = match kind {
                ParameterKindError::Mismatch => (
                    "Ruby::MethodParameterMismatch",
                    Severity::Error,
                    format!(
                        "The method parameter is incompatible with the declaration `{overload}`"
                    ),
                ),
                ParameterKindError::DifferentKind => (
                    "Ruby::DifferentMethodParameterKind",
                    Severity::Hint,
                    format!(
                        "The method parameter has different kind from the declaration `{overload}`"
                    ),
                ),
                ParameterKindError::Arity => (
                    "Ruby::MethodArityMismatch",
                    Severity::Error,
                    format!("Method parameters are incompatible with declaration `{overload}`"),
                ),
            };
            let span = if matches!(kind, ParameterKindError::Arity) {
                parameter_span
            } else {
                span
            };
            self.record_diagnostic(span, severity, code, message);
        }
    }

    fn last_statement_mismatches(
        &mut self,
        body: Option<Node<'pr>>,
        declared: &Type,
        body_type: &Type,
        context: &Context,
        locals: &Locals,
    ) -> bool {
        let hinted = self.hinted_last_statement_type(body, declared, body_type, context, locals);
        self.is_certain_mismatch(&hinted, declared)
    }

    fn hinted_last_statement_type(
        &mut self,
        body: Option<Node<'pr>>,
        declared: &Type,
        body_type: &Type,
        context: &Context,
        locals: &Locals,
    ) -> Type {
        let last_statement = body.and_then(|body| match body {
            Node::StatementsNode { .. } => body
                .as_statements_node()
                .and_then(|statements| statements.body().last()),
            _ => None,
        });
        match last_statement {
            Some(last_statement) => {
                self.hinted_value_type(&last_statement, declared, body_type, context, locals, 0)
            }
            None => body_type.clone(),
        }
    }

    /// The method type Steep checks a `def` against: the class's definition, inherited declarations included
    fn declared_overloads(
        &self,
        owner: &str,
        singleton: bool,
        method_name: &str,
        self_type: &Type,
    ) -> Vec<MethodType> {
        if !self.environment.classes.contains_key(owner) {
            return Vec::new();
        }
        let resolved = if singleton {
            self.environment.find_singleton_method(owner, method_name)
        } else {
            self.environment.find_instance_method(owner, method_name)
        };
        let Some(resolved) = resolved else {
            return Vec::new();
        };
        let instance = self_instance_type(self.environment, owner);
        let substitution = Substitution {
            self_type: Some(self_type.clone()),
            instance_type: Some(instance),
            class_type: Some(Type::ClassSingleton {
                name: owner.to_string(),
            }),
            ..Substitution::default()
        };
        resolved
            .overloads
            .iter()
            .map(|overload| overload.substitute(&substitution))
            .collect()
    }

    fn declared_method_documentation(
        &self,
        owner: &str,
        singleton: bool,
        method_name: &str,
    ) -> (Option<String>, Option<rbs_environment::Location>) {
        if !self.environment.classes.contains_key(owner) {
            return (None, None);
        }
        let resolved = if singleton {
            self.environment.find_singleton_method(owner, method_name)
        } else {
            self.environment.find_instance_method(owner, method_name)
        };
        match resolved {
            Some(resolved) => (
                resolved.definition.comment.clone(),
                resolved.definition.location.clone(),
            ),
            None => (None, None),
        }
    }

    fn assign_definition_parameters(
        &mut self,
        parameters: &ParametersNode<'pr>,
        overloads: &[MethodType],
        context: &Context,
        locals: &mut Locals,
    ) {
        let parameter_type = |select: &dyn Fn(&MethodType) -> Option<Type>| -> Type {
            if overloads.is_empty() {
                return Type::Untyped;
            }
            let types: Option<Vec<Type>> = overloads.iter().map(select).collect();
            match types {
                Some(types) => Type::union(types),
                None => Type::Untyped,
            }
        };
        for (index, required) in parameters.requireds().iter().enumerate() {
            let required_type = parameter_type(&|overload| {
                overload
                    .function
                    .required_positionals
                    .get(index)
                    .map(|parameter| parameter.parameter_type.clone())
            });
            match required {
                Node::RequiredParameterNode { .. } => {
                    let Some(required_parameter) = required.as_required_parameter_node() else {
                        continue;
                    };
                    let span = Span::of_node(&required);
                    self.define_parameter(
                        &constant_text(&required_parameter.name()),
                        span,
                        required_type,
                        ParameterHint::after_name(span),
                        locals,
                    );
                }
                Node::MultiTargetNode { .. } => {
                    self.assign_target(&required, required_type, context, locals);
                }
                _ => {}
            }
        }
        for (index, optional) in parameters.optionals().iter().enumerate() {
            let Some(optional_parameter) = optional.as_optional_parameter_node() else {
                continue;
            };
            let default_value = optional_parameter.value();
            let default_type = self.infer(&default_value, context, locals);
            let optional_type = parameter_type(&|overload| {
                overload
                    .function
                    .optional_positionals
                    .get(index)
                    .map(|parameter| parameter.parameter_type.clone())
            });
            let span = Span::of_location(&optional_parameter.name_loc());
            let hint = ParameterHint {
                is_obvious: is_obvious_value(&default_value) && default_type == optional_type,
                ..ParameterHint::after_name(span)
            };
            self.define_parameter(
                &constant_text(&optional_parameter.name()),
                span,
                optional_type,
                hint,
                locals,
            );
        }
        if let Some(rest) = parameters.rest()
            && let Some(rest_parameter) = rest.as_rest_parameter_node()
            && let (Some(name), Some(name_location)) =
                (rest_parameter.name(), rest_parameter.name_loc())
        {
            let element = parameter_type(&|overload| {
                overload
                    .function
                    .rest_positionals
                    .as_ref()
                    .map(|parameter| parameter.parameter_type.clone())
            });
            let span = Span::of_location(&name_location);
            self.define_parameter(
                &constant_text(&name),
                span,
                array_of(element),
                ParameterHint::after_name(span),
                locals,
            );
        }
        for (index, post) in parameters.posts().iter().enumerate() {
            let post_type = parameter_type(&|overload| {
                overload
                    .function
                    .trailing_positionals
                    .get(index)
                    .map(|parameter| parameter.parameter_type.clone())
            });
            if let Some(post_parameter) = post.as_required_parameter_node() {
                let span = Span::of_node(&post);
                self.define_parameter(
                    &constant_text(&post_parameter.name()),
                    span,
                    post_type,
                    ParameterHint::after_name(span),
                    locals,
                );
            }
        }
        for keyword in parameters.keywords().iter() {
            let (name, name_location, default) = match keyword {
                Node::RequiredKeywordParameterNode { .. } => {
                    let Some(required) = keyword.as_required_keyword_parameter_node() else {
                        continue;
                    };
                    (constant_text(&required.name()), required.name_loc(), None)
                }
                Node::OptionalKeywordParameterNode { .. } => {
                    let Some(optional) = keyword.as_optional_keyword_parameter_node() else {
                        continue;
                    };
                    let default_value = optional.value();
                    let default_type = self.infer(&default_value, context, locals);
                    (
                        constant_text(&optional.name()),
                        optional.name_loc(),
                        Some((is_obvious_value(&default_value), default_type)),
                    )
                }
                _ => continue,
            };
            let keyword_type = parameter_type(&|overload| {
                overload
                    .function
                    .required_keywords
                    .iter()
                    .chain(overload.function.optional_keywords.iter())
                    .find(|(keyword_name, _)| *keyword_name == name)
                    .map(|(_, parameter)| parameter.parameter_type.clone())
            });
            let span = Span::of_location(&name_location);
            let name_span = Span {
                start: span.start,
                end: span.end.saturating_sub(1).max(span.start),
            };
            let hint = ParameterHint {
                kind: TypeHintKind::KeywordParameterType,
                offset: span.end,
                is_obvious: default.is_some_and(|(is_obvious, default_type)| {
                    is_obvious && default_type == keyword_type
                }),
            };
            self.define_parameter(&name, name_span, keyword_type, hint, locals);
        }
        if let Some(keyword_rest) = parameters.keyword_rest()
            && let Some(keyword_rest_parameter) = keyword_rest.as_keyword_rest_parameter_node()
            && let (Some(name), Some(name_location)) = (
                keyword_rest_parameter.name(),
                keyword_rest_parameter.name_loc(),
            )
        {
            let value = parameter_type(&|overload| {
                overload
                    .function
                    .rest_keywords
                    .as_ref()
                    .map(|parameter| parameter.parameter_type.clone())
            });
            let span = Span::of_location(&name_location);
            self.define_parameter(
                &constant_text(&name),
                span,
                hash_of(Type::instance("Symbol"), value),
                ParameterHint::after_name(span),
                locals,
            );
        }
        if let Some(block_parameter) = parameters.block()
            && let (Some(name), Some(name_location)) =
                (block_parameter.name(), block_parameter.name_loc())
        {
            let block_type = parameter_type(&|overload| {
                Some(match &overload.block {
                    Some(block) => {
                        let proc_type = Type::Proc(Box::new(ProcType {
                            function: block.function.clone(),
                            block: None,
                            self_type: block.self_type.clone(),
                        }));
                        if block.required {
                            proc_type
                        } else {
                            Type::optional(proc_type)
                        }
                    }
                    None => Type::Nil,
                })
            });
            let span = Span::of_location(&name_location);
            self.define_parameter(
                &constant_text(&name),
                span,
                block_type,
                ParameterHint::after_name(span),
                locals,
            );
        }
    }

    fn define_parameter(
        &mut self,
        name: &str,
        span: Span,
        parameter_type: Type,
        hint: ParameterHint,
        locals: &mut Locals,
    ) {
        self.record_hint(hint.kind, hint.offset, &parameter_type, hint.is_obvious);
        self.record_type_hover(span, &parameter_type);
        locals.insert(name.to_string(), parameter_type);
    }

    pub(crate) fn infer_class_node(&mut self, node: &Node<'pr>, context: &Context) -> Type {
        let start = node.location().start_offset();
        let (constant_path, superclass, body) = match node {
            Node::ClassNode { .. } => {
                let Some(class_node) = node.as_class_node() else {
                    return Type::Untyped;
                };
                (
                    class_node.constant_path(),
                    class_node.superclass(),
                    class_node.body(),
                )
            }
            _ => {
                let Some(module_node) = node.as_module_node() else {
                    return Type::Untyped;
                };
                (module_node.constant_path(), None, module_node.body())
            }
        };
        let Some(name) = self.file_index.class_names_by_offset.get(&start).cloned() else {
            return Type::Untyped;
        };
        if let Some(error) = self.class_definition_error(&name) {
            self.record_unexpected_error(Span::of_node(node), error);
            return Type::Untyped;
        }
        let mut outer_locals = Locals::new();
        if let Some(superclass) = superclass {
            self.infer(&superclass, context, &mut outer_locals);
        }
        if let Node::ConstantPathNode { .. } = constant_path
            && let Some(parent) = constant_path
                .as_constant_path_node()
                .and_then(|path| path.parent())
        {
            self.infer(&parent, context, &mut outer_locals);
        }
        self.record_class_name_reference(&constant_path, &name);
        let mut nesting = (*context.nesting).clone();
        nesting.push(name.clone());
        let nesting = Rc::new(nesting);
        let self_type = self
            .annotated_self_type(start, &nesting)
            .unwrap_or(Type::ClassSingleton { name });
        let class_context = Context {
            self_type,
            declared_locals: Rc::new(self.local_variable_annotations(start, &nesting)),
            nesting,
            method: None,
        };
        let mut locals = Locals::new();
        self.infer_optional_node(body, &class_context, &mut locals)
    }

    /// Steep's `for_class` and `for_module` build both definitions of the class to type its body
    fn class_definition_error(
        &self,
        name: &str,
    ) -> Option<&'environment rbs_environment::DefinitionError> {
        if !self.options.steep_compatible {
            return None;
        }
        self.environment
            .instance_definition_error(name)
            .or_else(|| self.environment.singleton_definition_error(name))
    }

    fn record_class_name_reference(&mut self, constant_path: &Node<'pr>, name: &str) {
        let name_span = match constant_path {
            Node::ConstantPathNode { .. } => match constant_path.as_constant_path_node() {
                Some(path) => Span::of_location(&path.name_loc()),
                None => return,
            },
            _ => Span::of_node(constant_path),
        };
        if ConstantPath::of_node(constant_path).is_none() {
            return;
        }
        let singleton = Type::ClassSingleton {
            name: name.to_string(),
        };
        let declaration = self.environment.classes.get(name);
        let comment = declaration.and_then(|declaration| declaration.comment.clone());
        let locations = declaration
            .map(|declaration| declaration.locations.clone())
            .unwrap_or_default();
        self.record_hover(
            name_span,
            HoverKind::Constant {
                name: name.to_string(),
                constant_type: singleton,
                comment,
            },
        );
        self.record_definition(name_span, locations);
    }

    pub(crate) fn infer_singleton_class_node(
        &mut self,
        node: &Node<'pr>,
        context: &Context,
        locals: &mut Locals,
    ) -> Type {
        let Some(singleton_class) = node.as_singleton_class_node() else {
            return Type::Untyped;
        };
        let expression_type = self.infer(&singleton_class.expression(), context, locals);
        let mut singleton_context = context.clone();
        singleton_context.method = None;
        singleton_context.self_type = match expression_type {
            Type::ClassSingleton { .. } => expression_type,
            _ => Type::Untyped,
        };
        let mut singleton_locals = Locals::new();
        self.infer_optional_node(
            singleton_class.body(),
            &singleton_context,
            &mut singleton_locals,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ParameterKindError {
    Mismatch,
    DifferentKind,
    Arity,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DeclaredPositional {
    Required,
    Optional,
    Rest,
}

/// Port of the positional and keyword part of Steep's `MethodParams.build`
fn parameter_kind_errors(
    parameters: Option<ParametersNode<'_>>,
    function: &rbs_environment::Function,
) -> Vec<(ParameterKindError, Span)> {
    let mut errors = Vec::new();
    if function.untyped_parameters {
        return errors;
    }
    let mut declared: std::collections::VecDeque<DeclaredPositional> =
        std::collections::VecDeque::new();
    declared.extend(
        function
            .required_positionals
            .iter()
            .map(|_| DeclaredPositional::Required),
    );
    declared.extend(
        function
            .optional_positionals
            .iter()
            .map(|_| DeclaredPositional::Optional),
    );
    if function.rest_positionals.is_some() {
        declared.push_back(DeclaredPositional::Rest);
    }
    declared.extend(
        function
            .trailing_positionals
            .iter()
            .map(|_| DeclaredPositional::Required),
    );
    let mut remaining_keywords: Vec<String> = function
        .required_keywords
        .iter()
        .chain(function.optional_keywords.iter())
        .map(|(name, _)| name.clone())
        .collect();
    let Some(parameters) = parameters else {
        if !declared.is_empty()
            || !remaining_keywords.is_empty()
            || function.rest_keywords.is_some()
        {
            errors.push((ParameterKindError::Arity, Span { start: 0, end: 0 }));
        }
        return errors;
    };
    if parameters
        .keyword_rest()
        .is_some_and(|rest| matches!(rest, Node::ForwardingParameterNode { .. }))
    {
        return errors;
    }
    let mut positional_exhausted = false;
    for required in parameters.requireds().iter() {
        let span = Span::of_node(&required);
        let head = if positional_exhausted {
            None
        } else {
            declared.pop_front()
        };
        match head {
            Some(DeclaredPositional::Required) => {}
            Some(DeclaredPositional::Rest) => {
                errors.push((ParameterKindError::Mismatch, span));
                positional_exhausted = true;
            }
            Some(DeclaredPositional::Optional) | None => {
                errors.push((ParameterKindError::Mismatch, span))
            }
        }
    }
    for optional in parameters.optionals().iter() {
        let span = optional
            .as_optional_parameter_node()
            .map(|optional| Span::of_location(&optional.name_loc()))
            .unwrap_or_else(|| Span::of_node(&optional));
        let head = if positional_exhausted {
            None
        } else {
            declared.pop_front()
        };
        match head {
            Some(DeclaredPositional::Optional) => {}
            Some(DeclaredPositional::Rest) => {
                errors.push((ParameterKindError::DifferentKind, span));
                positional_exhausted = true;
            }
            Some(DeclaredPositional::Required) | None => {
                errors.push((ParameterKindError::DifferentKind, span))
            }
        }
    }
    if let Some(rest) = parameters
        .rest()
        .filter(|rest| matches!(rest, Node::RestParameterNode { .. }))
    {
        let mut has_error = false;
        if positional_exhausted {
            has_error = true;
        } else {
            loop {
                match declared.pop_front() {
                    Some(DeclaredPositional::Rest) => break,
                    Some(_) => has_error = true,
                    None => {
                        has_error = true;
                        break;
                    }
                }
            }
        }
        if has_error {
            errors.push((ParameterKindError::DifferentKind, Span::of_node(&rest)));
        }
        positional_exhausted = true;
        declared.clear();
    }
    if !positional_exhausted && !declared.is_empty() && parameters.posts().is_empty() {
        errors.push((ParameterKindError::Arity, Span { start: 0, end: 0 }));
    }
    for keyword in parameters.keywords().iter() {
        let (name, span, is_required) = match keyword {
            Node::RequiredKeywordParameterNode { .. } => {
                let Some(required) = keyword.as_required_keyword_parameter_node() else {
                    continue;
                };
                (
                    constant_text(&required.name()),
                    Span::of_node(&keyword),
                    true,
                )
            }
            Node::OptionalKeywordParameterNode { .. } => {
                let Some(optional) = keyword.as_optional_keyword_parameter_node() else {
                    continue;
                };
                (
                    constant_text(&optional.name()),
                    Span::of_node(&keyword),
                    false,
                )
            }
            _ => continue,
        };
        let declared_required = function
            .required_keywords
            .iter()
            .any(|(declared, _)| *declared == name);
        let declared_optional = function
            .optional_keywords
            .iter()
            .any(|(declared, _)| *declared == name);
        remaining_keywords.retain(|declared| *declared != name);
        let kind = match (is_required, declared_required, declared_optional) {
            (true, true, _) | (false, _, true) => None,
            (true, _, _) => Some(ParameterKindError::Mismatch),
            (false, _, _) => Some(ParameterKindError::DifferentKind),
        };
        if let Some(kind) = kind {
            errors.push((kind, span));
        }
    }
    match parameters.keyword_rest() {
        Some(rest) if matches!(rest, Node::KeywordRestParameterNode { .. }) => {
            if !remaining_keywords.is_empty() || function.rest_keywords.is_none() {
                errors.push((ParameterKindError::DifferentKind, Span::of_node(&rest)));
            }
        }
        _ => {
            if !remaining_keywords.is_empty() || function.rest_keywords.is_some() {
                errors.push((ParameterKindError::Arity, Span { start: 0, end: 0 }));
            }
        }
    }
    errors
}

struct ParameterHint {
    kind: TypeHintKind,
    offset: usize,
    is_obvious: bool,
}

impl ParameterHint {
    fn after_name(name_span: Span) -> ParameterHint {
        ParameterHint {
            kind: TypeHintKind::MethodParameterType,
            offset: name_span.end,
            is_obvious: false,
        }
    }
}
