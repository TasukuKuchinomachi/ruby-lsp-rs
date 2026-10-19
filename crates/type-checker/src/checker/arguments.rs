use rbs_environment::{Environment, Function, MethodType, Type};
use ruby_prism::{ArgumentsNode, Node};

use super::{Checker, Context, Locals};
use crate::source::{Span, bytes_text};
use crate::subtyping::{Certainty, is_subtype};
use crate::type_utilities::{contains_variable_in, element_type, hash_of};

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ArgumentKind {
    Positional,
    Splat,
    Keyword { name: String, key_span: Span },
    KeywordSplat,
    Forwarding,
}

#[derive(Debug, Clone)]
pub(crate) struct ArgumentValue {
    pub(crate) argument_type: Type,
    pub(crate) span: Span,
    pub(crate) kind: ArgumentKind,
    /// The literal type Steep gives a literal argument when the parameter expects one, e.g. `:desc` for `:asc | :desc`
    pub(crate) literal_type: Option<Type>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct CallArguments {
    pub(crate) values: Vec<ArgumentValue>,
}

impl CallArguments {
    pub(crate) fn single(argument_type: Type, span: Span) -> CallArguments {
        CallArguments {
            values: vec![ArgumentValue {
                argument_type,
                span,
                kind: ArgumentKind::Positional,
                literal_type: None,
            }],
        }
    }

    fn has(&self, predicate: impl Fn(&ArgumentKind) -> bool) -> bool {
        self.values.iter().any(|value| predicate(&value.kind))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ArityMismatch {
    InsufficientPositional,
    /// Every positional argument no parameter takes
    UnexpectedPositional(Vec<Span>),
    InsufficientKeywords(Vec<String>),
    UnexpectedKeyword(Span),
}

/// A parameter type paired with the argument passed to it. `argument_type` is cloned because keyword arguments may be
/// folded into a single positional hash
#[derive(Debug, Clone)]
pub(crate) struct ArgumentPair {
    pub(crate) parameter_type: Type,
    pub(crate) argument_type: Type,
    pub(crate) span: Span,
    pub(crate) literal_type: Option<Type>,
}

struct PositionalArgument {
    argument_type: Type,
    span: Span,
    literal_type: Option<Type>,
}

pub(crate) fn match_arguments(
    function: &Function,
    arguments: &CallArguments,
) -> Result<Vec<ArgumentPair>, ArityMismatch> {
    if function.untyped_parameters || arguments.has(|kind| *kind == ArgumentKind::Forwarding) {
        return Ok(Vec::new());
    }
    let accepts_keywords = !function.required_keywords.is_empty()
        || !function.optional_keywords.is_empty()
        || function.rest_keywords.is_some();
    let mut positionals: Vec<PositionalArgument> = arguments
        .values
        .iter()
        .filter(|value| value.kind == ArgumentKind::Positional)
        .map(|value| PositionalArgument {
            argument_type: value.argument_type.clone(),
            span: value.span,
            literal_type: value.literal_type.clone(),
        })
        .collect();
    let keyword_values: Vec<&ArgumentValue> = arguments
        .values
        .iter()
        .filter(|value| {
            matches!(
                value.kind,
                ArgumentKind::Keyword { .. } | ArgumentKind::KeywordSplat
            )
        })
        .collect();
    if !accepts_keywords && !keyword_values.is_empty() {
        let value_type = Type::union(
            keyword_values
                .iter()
                .map(|value| value.argument_type.clone())
                .collect(),
        );
        let start = keyword_values
            .iter()
            .map(|value| value.span.start)
            .min()
            .unwrap_or(0);
        let end = keyword_values
            .iter()
            .map(|value| value.span.end)
            .max()
            .unwrap_or(0);
        positionals.push(PositionalArgument {
            argument_type: hash_of(Type::instance("Symbol"), value_type),
            span: Span { start, end },
            literal_type: None,
        });
    }
    let mut pairs = Vec::new();
    if !arguments.has(|kind| *kind == ArgumentKind::Splat) {
        let required_count = function.required_positionals.len();
        let trailing_count = function.trailing_positionals.len();
        let optional_count = function.optional_positionals.len();
        let count = positionals.len();
        if count < required_count + trailing_count {
            return Err(ArityMismatch::InsufficientPositional);
        }
        let middle_count = count - required_count - trailing_count;
        if middle_count > optional_count && function.rest_positionals.is_none() {
            return Err(ArityMismatch::UnexpectedPositional(
                positionals[required_count + optional_count..count - trailing_count]
                    .iter()
                    .map(|positional| positional.span)
                    .collect(),
            ));
        }
        for (index, positional) in positionals.iter().enumerate() {
            let parameter = if index < required_count {
                Some(&function.required_positionals[index])
            } else if index < required_count + middle_count {
                let middle_index = index - required_count;
                function
                    .optional_positionals
                    .get(middle_index)
                    .or(function.rest_positionals.as_ref())
            } else {
                function
                    .trailing_positionals
                    .get(index - required_count - middle_count)
            };
            if let Some(parameter) = parameter {
                pairs.push(ArgumentPair {
                    parameter_type: parameter.parameter_type.clone(),
                    argument_type: positional.argument_type.clone(),
                    span: positional.span,
                    literal_type: positional.literal_type.clone(),
                });
            }
        }
    }
    if accepts_keywords {
        let has_keyword_splat = arguments.has(|kind| *kind == ArgumentKind::KeywordSplat);
        let mut given_names = Vec::new();
        for value in &arguments.values {
            let ArgumentKind::Keyword { name, key_span } = &value.kind else {
                continue;
            };
            given_names.push(name.clone());
            let parameter = function
                .required_keywords
                .iter()
                .chain(function.optional_keywords.iter())
                .find(|(keyword, _)| keyword == name)
                .map(|(_, parameter)| parameter)
                .or(function.rest_keywords.as_ref());
            match parameter {
                Some(parameter) => pairs.push(ArgumentPair {
                    parameter_type: parameter.parameter_type.clone(),
                    argument_type: value.argument_type.clone(),
                    span: value.span,
                    literal_type: value.literal_type.clone(),
                }),
                None => return Err(ArityMismatch::UnexpectedKeyword(*key_span)),
            }
        }
        if !has_keyword_splat {
            let missing: Vec<String> = function
                .required_keywords
                .iter()
                .filter(|(keyword, _)| !given_names.contains(keyword))
                .map(|(keyword, _)| keyword.clone())
                .collect();
            if !missing.is_empty() {
                return Err(ArityMismatch::InsufficientKeywords(missing));
            }
        }
    }
    Ok(pairs)
}

pub(crate) fn block_fits(overload: &MethodType, has_block: bool) -> bool {
    match &overload.block {
        Some(block) => has_block || !block.required,
        None => !has_block,
    }
}

fn arguments_fit(
    environment: &Environment,
    overload: &MethodType,
    pairs: &[ArgumentPair],
) -> Certainty {
    let type_parameter_names: Vec<String> = overload
        .type_parameters
        .iter()
        .map(|type_parameter| type_parameter.name.clone())
        .collect();
    let mut combined = Certainty::Yes;
    for pair in pairs {
        if contains_variable_in(&pair.parameter_type, &type_parameter_names) {
            continue;
        }
        let literal_result = pair
            .literal_type
            .as_ref()
            .map(|literal| is_subtype(environment, literal, &pair.parameter_type));
        if matches!(literal_result, Some(Certainty::Yes | Certainty::Unknown)) {
            continue;
        }
        match is_subtype(environment, &pair.argument_type, &pair.parameter_type) {
            Certainty::No => return Certainty::No,
            Certainty::Unknown => combined = Certainty::Unknown,
            Certainty::Yes => {}
        }
    }
    combined
}

pub(crate) fn select_overload(
    environment: &Environment,
    overloads: &[MethodType],
    arguments: &CallArguments,
    has_block: bool,
) -> usize {
    let any_block_fits = overloads
        .iter()
        .any(|overload| block_fits(overload, has_block));
    let mut first_arity_match = None;
    for (index, overload) in overloads.iter().enumerate() {
        if any_block_fits && !block_fits(overload, has_block) {
            continue;
        }
        let Ok(pairs) = match_arguments(&overload.function, arguments) else {
            continue;
        };
        // Steep takes the first overload that type checks, so an undecidable argument still selects it
        if arguments_fit(environment, overload, &pairs) != Certainty::No {
            return index;
        }
        first_arity_match.get_or_insert(index);
    }
    if let Some(index) = first_arity_match {
        return index;
    }
    overloads
        .iter()
        .position(|overload| block_fits(overload, has_block))
        .unwrap_or(0)
}

impl<'pr> Checker<'_, 'pr> {
    pub(crate) fn infer_arguments(
        &mut self,
        arguments: Option<ArgumentsNode<'pr>>,
        context: &Context,
        locals: &mut Locals,
    ) -> CallArguments {
        self.infer_arguments_deferring_lambdas(arguments, context, locals, None)
    }

    /// Lambda literals are left for the caller, which types them once the overload tells what proc type the
    /// parameter expects; they enter overload selection as an untyped proc
    pub(crate) fn infer_arguments_deferring_lambdas(
        &mut self,
        arguments: Option<ArgumentsNode<'pr>>,
        context: &Context,
        locals: &mut Locals,
        mut deferred_lambdas: Option<&mut Vec<(usize, Node<'pr>)>>,
    ) -> CallArguments {
        let mut call_arguments = CallArguments::default();
        let Some(arguments) = arguments else {
            return call_arguments;
        };
        for argument in arguments.arguments().iter() {
            match argument {
                Node::SplatNode { .. } => {
                    let splatted = self.infer(&argument, context, locals);
                    call_arguments.values.push(ArgumentValue {
                        argument_type: element_type(&splatted),
                        span: Span::of_node(&argument),
                        kind: ArgumentKind::Splat,
                        literal_type: None,
                    });
                }
                Node::ForwardingArgumentsNode { .. } => {
                    call_arguments.values.push(ArgumentValue {
                        argument_type: Type::Untyped,
                        span: Span::of_node(&argument),
                        kind: ArgumentKind::Forwarding,
                        literal_type: None,
                    });
                }
                Node::KeywordHashNode { .. } => self.infer_keyword_arguments(
                    &argument,
                    context,
                    locals,
                    &mut call_arguments,
                    deferred_lambdas.as_deref_mut(),
                ),
                _ if deferred_lambdas.is_some() && lambda_literal_block(&argument).is_some() => {
                    if let Some(deferred) = deferred_lambdas.as_deref_mut() {
                        deferred.push((
                            call_arguments.values.len(),
                            prism_child_nodes::copy_node(&argument),
                        ));
                    }
                    let mut function = rbs_environment::Function::returning(Type::Untyped);
                    function.untyped_parameters = true;
                    call_arguments.values.push(ArgumentValue {
                        argument_type: Type::Proc(Box::new(rbs_environment::ProcType {
                            function,
                            block: None,
                            self_type: None,
                        })),
                        span: Span::of_node(&argument),
                        kind: ArgumentKind::Positional,
                        literal_type: None,
                    });
                }
                _ => {
                    let argument_type = self.infer(&argument, context, locals);
                    call_arguments.values.push(ArgumentValue {
                        argument_type,
                        span: Span::of_node(&argument),
                        kind: ArgumentKind::Positional,
                        literal_type: literal_type_of(&argument),
                    });
                }
            }
        }
        call_arguments
    }

    fn infer_keyword_arguments(
        &mut self,
        node: &Node<'pr>,
        context: &Context,
        locals: &mut Locals,
        call_arguments: &mut CallArguments,
        mut deferred_lambdas: Option<&mut Vec<(usize, Node<'pr>)>>,
    ) {
        let Some(keyword_hash) = node.as_keyword_hash_node() else {
            return;
        };
        let all_symbol_keys = keyword_hash.elements().iter().all(|element| match element {
            Node::AssocNode { .. } => element
                .as_assoc_node()
                .is_some_and(|association| matches!(association.key(), Node::SymbolNode { .. })),
            Node::AssocSplatNode { .. } => true,
            _ => false,
        });
        if !all_symbol_keys {
            let argument_type = self.infer(node, context, locals);
            call_arguments.values.push(ArgumentValue {
                argument_type,
                span: Span::of_node(node),
                kind: ArgumentKind::Positional,
                literal_type: None,
            });
            return;
        }
        for element in keyword_hash.elements().iter() {
            match element {
                Node::AssocNode { .. } => {
                    let Some(association) = element.as_assoc_node() else {
                        continue;
                    };
                    let key = association.key();
                    let Some(symbol) = key.as_symbol_node() else {
                        continue;
                    };
                    let name = bytes_text(symbol.unescaped());
                    let value = association.value();
                    let deferred =
                        deferred_lambdas.is_some() && lambda_literal_block(&value).is_some();
                    let argument_type = if deferred {
                        if let Some(deferred_lambdas) = deferred_lambdas.as_deref_mut() {
                            deferred_lambdas.push((
                                call_arguments.values.len(),
                                prism_child_nodes::copy_node(&value),
                            ));
                        }
                        let mut function = rbs_environment::Function::returning(Type::Untyped);
                        function.untyped_parameters = true;
                        Type::Proc(Box::new(rbs_environment::ProcType {
                            function,
                            block: None,
                            self_type: None,
                        }))
                    } else if matches!(value, Node::ImplicitNode { .. }) {
                        self.infer_implicit_value(&value, context, locals)
                    } else {
                        self.infer(&value, context, locals)
                    };
                    let key_span = match symbol.value_loc() {
                        Some(value_location) => Span::of_location(&value_location),
                        None => Span::of_node(&key),
                    };
                    call_arguments.values.push(ArgumentValue {
                        argument_type,
                        span: Span::of_node(&value),
                        kind: ArgumentKind::Keyword { name, key_span },
                        literal_type: literal_type_of(&value),
                    });
                }
                Node::AssocSplatNode { .. } => {
                    let Some(splat) = element.as_assoc_splat_node() else {
                        continue;
                    };
                    let argument_type = self.infer_optional_node(splat.value(), context, locals);
                    call_arguments.values.push(ArgumentValue {
                        argument_type,
                        span: Span::of_node(&element),
                        kind: ArgumentKind::KeywordSplat,
                        literal_type: None,
                    });
                }
                _ => {}
            }
        }
    }
}

/// The node a lambda literal's body hangs off: the `LambdaNode` itself, or the block of `lambda { }` / `proc { }`
pub(crate) fn lambda_literal_block<'pr>(node: &Node<'pr>) -> Option<Node<'pr>> {
    match node {
        Node::LambdaNode { .. } => Some(prism_child_nodes::copy_node(node)),
        Node::CallNode { .. } => {
            let call = node.as_call_node()?;
            if call.receiver().is_some() || !matches!(call.name().as_slice(), b"lambda" | b"proc") {
                return None;
            }
            call.block()
                .filter(|block| matches!(block, Node::BlockNode { .. }))
        }
        _ => None,
    }
}

/// Literal type of a literal expression, or of an array or symbol-keyed hash made only of literals
pub(crate) fn literal_type_of(node: &Node<'_>) -> Option<Type> {
    use rbs_environment::Literal;
    match node {
        Node::SymbolNode { .. } => Some(Type::Literal(Literal::Symbol(bytes_text(
            node.as_symbol_node()?.unescaped(),
        )))),
        Node::StringNode { .. } => Some(Type::Literal(Literal::String(bytes_text(
            node.as_string_node()?.unescaped(),
        )))),
        Node::IntegerNode { .. } => Some(Type::Literal(Literal::Integer(
            bytes_text(node.location().as_slice()).replace('_', ""),
        ))),
        Node::TrueNode { .. } => Some(Type::Literal(Literal::True)),
        Node::FalseNode { .. } => Some(Type::Literal(Literal::False)),
        Node::ArrayNode { .. } => {
            let elements: Option<Vec<Type>> = node
                .as_array_node()?
                .elements()
                .iter()
                .map(|element| literal_type_of(&element))
                .collect();
            let elements = elements?;
            if elements.is_empty() {
                return None;
            }
            Some(crate::type_utilities::array_of(Type::union(elements)))
        }
        Node::HashNode { .. } | Node::KeywordHashNode { .. } => {
            let elements = match node {
                Node::HashNode { .. } => node.as_hash_node()?.elements(),
                _ => node.as_keyword_hash_node()?.elements(),
            };
            let mut fields = Vec::new();
            for element in elements.iter() {
                let association = element.as_assoc_node()?;
                let key = association.key();
                let name = bytes_text(key.as_symbol_node()?.unescaped());
                let field_type = literal_type_of(&association.value())?;
                fields.push(rbs_environment::RecordField {
                    name,
                    field_type,
                    required: true,
                });
            }
            if fields.is_empty() {
                return None;
            }
            Some(Type::Record(fields))
        }
        _ => None,
    }
}
