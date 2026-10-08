use std::rc::Rc;

use rbs_environment::{Function, ProcType, Type};
use ruby_prism::{Node, ParametersNode};

use super::variables::IT_PARAMETER_KEY;
use super::{Checker, Context, Locals};
use crate::TypeHintKind;
use crate::source::{Span, constant_text};
use crate::type_utilities::{array_of, hash_of, is_bottom};

pub(crate) struct BlockSignature {
    pub(crate) function: Option<Function>,
    pub(crate) self_type: Option<Type>,
    /// What `break` inside the block makes the method call return, checked like Steep's `BreakTypeMismatch`
    pub(crate) break_type: Option<Type>,
}

impl BlockSignature {
    pub(crate) fn untyped() -> BlockSignature {
        BlockSignature {
            function: None,
            self_type: None,
            break_type: None,
        }
    }

    fn positional_types(&self) -> Option<(Vec<Type>, Option<Type>)> {
        let function = self.function.as_ref()?;
        if function.untyped_parameters {
            return None;
        }
        let positionals = function
            .required_positionals
            .iter()
            .chain(function.optional_positionals.iter())
            .chain(function.trailing_positionals.iter())
            .map(|parameter| parameter.parameter_type.clone())
            .collect();
        let rest = function
            .rest_positionals
            .as_ref()
            .map(|parameter| parameter.parameter_type.clone());
        Some((positionals, rest))
    }

    fn keyword_type(&self, name: &str) -> Type {
        let Some(function) = &self.function else {
            return Type::Untyped;
        };
        function
            .required_keywords
            .iter()
            .chain(function.optional_keywords.iter())
            .find(|(keyword, _)| keyword == name)
            .map(|(_, parameter)| parameter.parameter_type.clone())
            .unwrap_or(Type::Untyped)
    }
}

struct YieldedValues {
    values: Vec<Type>,
    rest: Option<Type>,
    /// Steep gives `T?` to each parameter of a block that spreads one `Array[T]`, since the array may be short
    spreads_array: bool,
}

impl YieldedValues {
    fn new(signature: &BlockSignature, parameter_count: usize) -> YieldedValues {
        let Some((positionals, rest)) = signature.positional_types() else {
            return YieldedValues {
                values: Vec::new(),
                rest: Some(Type::Untyped),
                spreads_array: false,
            };
        };
        if parameter_count > 1 && positionals.len() == 1 && rest.is_none() {
            match &positionals[0] {
                Type::Tuple(elements) => {
                    return YieldedValues {
                        values: elements.clone(),
                        rest: None,
                        spreads_array: false,
                    };
                }
                Type::ClassInstance { name, arguments } if name == "Array" => {
                    return YieldedValues {
                        values: Vec::new(),
                        rest: Some(arguments.first().cloned().unwrap_or(Type::Untyped)),
                        spreads_array: true,
                    };
                }
                Type::ClassInstance { .. }
                | Type::Nil
                | Type::Bool
                | Type::Literal(_)
                | Type::ClassSingleton { .. } => {}
                _ => {
                    return YieldedValues {
                        values: Vec::new(),
                        rest: Some(Type::Untyped),
                        spreads_array: false,
                    };
                }
            }
        }
        YieldedValues {
            values: positionals,
            rest,
            spreads_array: false,
        }
    }

    fn at(&self, index: usize) -> Type {
        match self.values.get(index) {
            Some(value) => value.clone(),
            None => match &self.rest {
                Some(rest) if self.spreads_array => Type::optional(rest.clone()),
                Some(rest) => rest.clone(),
                None => Type::Nil,
            },
        }
    }

    fn remaining_from(&self, index: usize) -> Type {
        let mut remaining: Vec<Type> = self.values.iter().skip(index).cloned().collect();
        if let Some(rest) = &self.rest {
            remaining.push(rest.clone());
        }
        if remaining.is_empty() {
            Type::Untyped
        } else {
            Type::union(remaining)
        }
    }
}

impl<'pr> Checker<'_, 'pr> {
    pub(crate) fn infer_block(
        &mut self,
        block_node: &Node<'pr>,
        signature: &BlockSignature,
        context: &Context,
        locals: &mut Locals,
    ) -> Type {
        let (parameters, body) = match block_node {
            Node::BlockNode { .. } => {
                let Some(block) = block_node.as_block_node() else {
                    return Type::Untyped;
                };
                (block.parameters(), block.body())
            }
            Node::LambdaNode { .. } => {
                let Some(lambda) = block_node.as_lambda_node() else {
                    return Type::Untyped;
                };
                (lambda.parameters(), lambda.body())
            }
            _ => return Type::Untyped,
        };
        let scope_start = block_node.location().start_offset();
        let mut block_context = context.clone();
        if let Some(self_type) = &signature.self_type {
            block_context.self_type = self_type.clone();
        }
        if let Some(annotated_self) = self.annotated_self_type(scope_start, &context.nesting) {
            block_context.self_type = annotated_self;
        }
        let mut parameter_locals = locals.clone();
        // A block can run after the code that follows it, so a variable that is still `nil` here is usually assigned
        // later; typing it as `nil` inside the block would report every use
        let mut relaxed_names = Vec::new();
        for (name, local_type) in parameter_locals.iter_mut() {
            if matches!(local_type, Type::Nil) && !self.options.steep_compatible {
                *local_type = Type::Untyped;
                relaxed_names.push(name.clone());
            }
        }
        let parameter_names = self.assign_block_parameters(
            parameters,
            signature,
            &block_context,
            &mut parameter_locals,
        );
        let (block_locals, (body_type, next_types)) = self.infer_repeatedly(
            &parameter_locals,
            &parameter_names,
            &relaxed_names,
            |checker, iteration_locals| {
                checker.block_next_frames.push(Vec::new());
                checker.jump_expectations.push(super::JumpExpectation {
                    break_type: signature.break_type.clone(),
                    next_type: signature
                        .function
                        .as_ref()
                        .map(|function| function.return_type.clone()),
                });
                let body = body.as_ref().map(prism_child_nodes::copy_node);
                let body_type = checker.infer_optional_node(body, &block_context, iteration_locals);
                checker.jump_expectations.pop();
                let next_types = checker.block_next_frames.pop().unwrap_or_default();
                (body_type, next_types)
            },
        );
        for (name, outer_type) in locals.iter_mut() {
            if parameter_names.contains(name) {
                continue;
            }
            if let Some(block_type) = block_locals.get(name)
                && block_type != outer_type
                && !(relaxed_names.contains(name) && block_type.is_untyped())
            {
                *outer_type = Type::union(vec![outer_type.clone(), block_type.clone()]);
            }
        }
        let mut result_types = next_types;
        if !is_bottom(&body_type) {
            result_types.push(body_type);
        }
        if result_types.is_empty() {
            Type::Bottom
        } else {
            Type::union(result_types)
        }
    }

    fn assign_block_parameters(
        &mut self,
        parameters: Option<Node<'pr>>,
        signature: &BlockSignature,
        context: &Context,
        locals: &mut Locals,
    ) -> Vec<String> {
        let Some(parameters) = parameters else {
            return Vec::new();
        };
        match parameters {
            Node::BlockParametersNode { .. } => {
                let Some(block_parameters) = parameters.as_block_parameters_node() else {
                    return Vec::new();
                };
                let Some(parameters_node) = block_parameters.parameters() else {
                    return Vec::new();
                };
                self.assign_parameters_from_signature(&parameters_node, signature, context, locals)
            }
            Node::NumberedParametersNode { .. } => {
                let Some(numbered) = parameters.as_numbered_parameters_node() else {
                    return Vec::new();
                };
                let maximum = numbered.maximum() as usize;
                let yielded = YieldedValues::new(signature, maximum);
                (1..=maximum)
                    .map(|number| {
                        let name = format!("_{number}");
                        locals.insert(name.clone(), yielded.at(number - 1));
                        name
                    })
                    .collect()
            }
            Node::ItParametersNode { .. } if self.options.steep_compatible => Vec::new(),
            Node::ItParametersNode { .. } => {
                let yielded = YieldedValues::new(signature, 1);
                locals.insert(IT_PARAMETER_KEY.to_string(), yielded.at(0));
                vec![IT_PARAMETER_KEY.to_string()]
            }
            _ => Vec::new(),
        }
    }

    fn assign_parameters_from_signature(
        &mut self,
        parameters: &ParametersNode<'pr>,
        signature: &BlockSignature,
        context: &Context,
        locals: &mut Locals,
    ) -> Vec<String> {
        let requireds: Vec<Node<'pr>> = parameters.requireds().iter().collect();
        let optionals: Vec<Node<'pr>> = parameters.optionals().iter().collect();
        let posts: Vec<Node<'pr>> = parameters.posts().iter().collect();
        let rest = parameters
            .rest()
            .filter(|rest| matches!(rest, Node::RestParameterNode { .. }));
        let positional_count =
            requireds.len() + optionals.len() + posts.len() + usize::from(rest.is_some());
        let yielded = YieldedValues::new(signature, positional_count);
        let mut names = Vec::new();
        let mut index = 0;
        for required in &requireds {
            self.assign_block_parameter(required, yielded.at(index), context, locals, &mut names);
            index += 1;
        }
        for optional in &optionals {
            let Some(optional_parameter) = optional.as_optional_parameter_node() else {
                continue;
            };
            let default_type = self.infer(&optional_parameter.value(), context, locals);
            let parameter_type = match yielded.at(index) {
                Type::Nil => default_type,
                given => given,
            };
            let name = constant_text(&optional_parameter.name());
            self.record_parameter(
                &name,
                Span::of_location(&optional_parameter.name_loc()),
                parameter_type,
                locals,
            );
            names.push(name);
            index += 1;
        }
        if let Some(rest) = rest
            && let Some(rest_parameter) = rest.as_rest_parameter_node()
            && let (Some(name), Some(name_location)) =
                (rest_parameter.name(), rest_parameter.name_loc())
        {
            let name = constant_text(&name);
            let rest_type = array_of(yielded.remaining_from(index));
            self.record_parameter(&name, Span::of_location(&name_location), rest_type, locals);
            names.push(name);
        }
        for post in &posts {
            self.assign_block_parameter(post, yielded.at(index), context, locals, &mut names);
            index += 1;
        }
        for keyword in parameters.keywords().iter() {
            let (name, name_location) = match keyword {
                Node::RequiredKeywordParameterNode { .. } => {
                    let Some(required) = keyword.as_required_keyword_parameter_node() else {
                        continue;
                    };
                    (constant_text(&required.name()), required.name_loc())
                }
                Node::OptionalKeywordParameterNode { .. } => {
                    let Some(optional) = keyword.as_optional_keyword_parameter_node() else {
                        continue;
                    };
                    self.infer(&optional.value(), context, locals);
                    (constant_text(&optional.name()), optional.name_loc())
                }
                _ => continue,
            };
            let keyword_type = signature.keyword_type(&name);
            let span = Span::of_location(&name_location);
            let name_span = Span {
                start: span.start,
                end: span.end.saturating_sub(1).max(span.start),
            };
            self.record_parameter(&name, name_span, keyword_type, locals);
            if signature.function.is_none() {
                locals.insert(super::variables::fallback_local_key(&name), Type::Untyped);
            }
            names.push(name);
        }
        if let Some(keyword_rest) = parameters.keyword_rest()
            && let Some(keyword_rest_parameter) = keyword_rest.as_keyword_rest_parameter_node()
            && let (Some(name), Some(name_location)) = (
                keyword_rest_parameter.name(),
                keyword_rest_parameter.name_loc(),
            )
        {
            let name = constant_text(&name);
            self.record_parameter(
                &name,
                Span::of_location(&name_location),
                hash_of(Type::instance("Symbol"), Type::Untyped),
                locals,
            );
            names.push(name);
        }
        if let Some(block_parameter) = parameters.block()
            && let (Some(name), Some(name_location)) =
                (block_parameter.name(), block_parameter.name_loc())
        {
            let name = constant_text(&name);
            self.record_parameter(
                &name,
                Span::of_location(&name_location),
                Type::Untyped,
                locals,
            );
            names.push(name);
        }
        names
    }

    fn assign_block_parameter(
        &mut self,
        parameter: &Node<'pr>,
        parameter_type: Type,
        context: &Context,
        locals: &mut Locals,
        names: &mut Vec<String>,
    ) {
        match parameter {
            Node::RequiredParameterNode { .. } => {
                let Some(required) = parameter.as_required_parameter_node() else {
                    return;
                };
                let name = constant_text(&required.name());
                self.record_parameter(&name, Span::of_node(parameter), parameter_type, locals);
                names.push(name);
            }
            Node::MultiTargetNode { .. } => {
                let Some(multiple_target) = parameter.as_multi_target_node() else {
                    return;
                };
                let lefts: Vec<Node<'pr>> = multiple_target.lefts().iter().collect();
                let rights: Vec<Node<'pr>> = multiple_target.rights().iter().collect();
                for target in lefts.iter().chain(rights.iter()) {
                    if let Some(required) = target.as_required_parameter_node() {
                        names.push(constant_text(&required.name()));
                    }
                }
                self.assign_destructured(
                    &lefts,
                    multiple_target.rest(),
                    &rights,
                    &parameter_type,
                    None,
                    context,
                    locals,
                );
            }
            _ => {}
        }
    }

    fn record_parameter(
        &mut self,
        name: &str,
        name_span: Span,
        parameter_type: Type,
        locals: &mut Locals,
    ) {
        self.record_hint(
            TypeHintKind::BlockParameterType,
            name_span.end,
            &parameter_type,
            false,
        );
        self.record_type_hover(name_span, &parameter_type);
        locals.insert(name.to_string(), parameter_type);
    }

    pub(crate) fn infer_lambda(
        &mut self,
        node: &Node<'pr>,
        context: &Context,
        locals: &mut Locals,
    ) -> Type {
        self.infer_lambda_with(node, &BlockSignature::untyped(), context, locals)
    }

    /// `-> (x) { ... }`, `lambda { ... }` and `proc { ... }`, typed with the proc type the receiving parameter
    /// expects when there is one (Steep's `type_lambda` with a hint)
    pub(crate) fn infer_lambda_with(
        &mut self,
        node: &Node<'pr>,
        signature: &BlockSignature,
        context: &Context,
        locals: &mut Locals,
    ) -> Type {
        let mut lambda_context = context.clone();
        lambda_context.method = context.method.as_ref().map(|method| {
            Rc::new(super::MethodContext {
                name: method.name.clone(),
                owner: method.owner.clone(),
                singleton: method.singleton,
                declared_return_type: None,
                block: method.block.clone(),
            })
        });
        self.method_return_frames.push(Vec::new());
        let body_type = self.infer_block(node, signature, &lambda_context, locals);
        let returned = self.method_return_frames.pop().unwrap_or_default();
        let mut result_types = returned;
        result_types.push(body_type);
        let return_type = Type::union(result_types);
        let return_type = if is_bottom(&return_type) {
            Type::Untyped
        } else {
            return_type
        };
        let mut function = match &signature.function {
            Some(function) => function.clone(),
            None => lambda_parameters_function(node),
        };
        function.return_type = return_type;
        Type::Proc(Box::new(ProcType {
            function,
            block: None,
            self_type: signature.self_type.clone(),
        }))
    }
}

/// Parameters of a lambda without a hint, all `untyped`, the way Steep shows `^(untyped, ?untyped) -> T`
fn lambda_parameters_function(node: &Node<'_>) -> Function {
    let mut function = Function::returning(Type::Untyped);
    let parameters = match node {
        Node::LambdaNode { .. } => node.as_lambda_node().and_then(|lambda| lambda.parameters()),
        Node::BlockNode { .. } => node.as_block_node().and_then(|block| block.parameters()),
        _ => None,
    };
    let Some(parameters) =
        parameters.and_then(|parameters| parameters.as_block_parameters_node()?.parameters())
    else {
        return function;
    };
    let untyped = || rbs_environment::Parameter::new(Type::Untyped);
    function.required_positionals = parameters.requireds().iter().map(|_| untyped()).collect();
    function.optional_positionals = parameters.optionals().iter().map(|_| untyped()).collect();
    if parameters
        .rest()
        .is_some_and(|rest| matches!(rest, Node::RestParameterNode { .. }))
    {
        function.rest_positionals = Some(untyped());
    }
    function.trailing_positionals = parameters.posts().iter().map(|_| untyped()).collect();
    function
}
