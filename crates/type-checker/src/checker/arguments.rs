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
}

struct PositionalArgument {
    argument_type: Type,
    span: Span,
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
        // Steep types the keywords passed to a positional parameter as a hash literal, a record unless a `**`
        // splat is among them
        let record_fields: Option<Vec<rbs_environment::RecordField>> = keyword_values
            .iter()
            .map(|value| match &value.kind {
                ArgumentKind::Keyword { name, .. } => Some(rbs_environment::RecordField {
                    name: name.clone(),
                    field_type: value.argument_type.clone(),
                    required: true,
                }),
                _ => None,
            })
            .collect();
        let folded_type = match record_fields {
            Some(fields) => Type::Record(fields),
            None => hash_of(
                Type::instance("Symbol"),
                Type::union(keyword_values.iter().map(|value| value.argument_type.clone()).collect()),
            ),
        };
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
            argument_type: folded_type,
            span: Span { start, end },
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
        match is_subtype(environment, &pair.argument_type, &pair.parameter_type) {
            Certainty::No => return Certainty::No,
            Certainty::Unknown => combined = Certainty::Unknown,
            Certainty::Yes => {}
        }
    }
    combined
}

/// `arguments_for(index)` are the arguments as typed against the overload at `index`, since Steep types the
/// arguments of each overload it tries with that overload's parameters as hints
pub(crate) fn select_overload<'arguments>(
    environment: &Environment,
    overloads: &[MethodType],
    arguments_for: impl Fn(usize) -> &'arguments CallArguments,
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
        let Ok(pairs) = match_arguments(&overload.function, arguments_for(index)) else {
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
        self.infer_arguments_deferring(arguments, context, locals, None)
    }

    /// Arguments whose type depends on the hint are left for the caller, which types them with the parameter of
    /// each overload it tries, as Steep's `type_check_argument` does. They stand as `untyped` until then, a lambda
    /// literal as an untyped proc
    pub(crate) fn infer_arguments_deferring(
        &mut self,
        arguments: Option<ArgumentsNode<'pr>>,
        context: &Context,
        locals: &mut Locals,
        mut deferred: Option<&mut Vec<(usize, Node<'pr>)>>,
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
                    });
                }
                Node::ForwardingArgumentsNode { .. } => {
                    call_arguments.values.push(ArgumentValue {
                        argument_type: Type::Untyped,
                        span: Span::of_node(&argument),
                        kind: ArgumentKind::Forwarding,
                    });
                }
                Node::KeywordHashNode { .. } => self.infer_keyword_arguments(
                    &argument,
                    context,
                    locals,
                    &mut call_arguments,
                    deferred.as_deref_mut(),
                ),
                _ if deferred.is_some() && depends_on_hint(&argument) => {
                    if let Some(deferred) = deferred.as_deref_mut() {
                        deferred.push((
                            call_arguments.values.len(),
                            prism_child_nodes::copy_node(&argument),
                        ));
                    }
                    call_arguments.values.push(ArgumentValue {
                        argument_type: deferred_placeholder(&argument),
                        span: Span::of_node(&argument),
                        kind: ArgumentKind::Positional,
                    });
                }
                _ => {
                    let argument_type = self.infer(&argument, context, locals);
                    call_arguments.values.push(ArgumentValue {
                        argument_type,
                        span: Span::of_node(&argument),
                        kind: ArgumentKind::Positional,
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
        mut deferred: Option<&mut Vec<(usize, Node<'pr>)>>,
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
                    let argument_type = if deferred.is_some() && depends_on_hint(&value) {
                        if let Some(deferred) = deferred.as_deref_mut() {
                            deferred.push((call_arguments.values.len(), prism_child_nodes::copy_node(&value)));
                        }
                        deferred_placeholder(&value)
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
                    });
                }
                Node::AssocSplatNode { .. } => {
                    let Some(splat) = element.as_assoc_splat_node() else {
                        continue;
                    };
                    let argument_type = self.infer_optional_node(splat.value(), None, context, locals);
                    call_arguments.values.push(ArgumentValue {
                        argument_type,
                        span: Span::of_node(&element),
                        kind: ArgumentKind::KeywordSplat,
                    });
                }
                _ => {}
            }
        }
    }
}

/// Whether the type of an argument depends on the hint, so that it is typed only once the parameter is known. A
/// call is not deferred although a hint can bind the type parameter it returns: typing it again for every overload
/// would double the work at every argument nested in another
fn depends_on_hint(node: &Node<'_>) -> bool {
    match node {
        Node::SymbolNode { .. }
        | Node::StringNode { .. }
        | Node::IntegerNode { .. }
        | Node::TrueNode { .. }
        | Node::FalseNode { .. }
        | Node::ArrayNode { .. }
        | Node::HashNode { .. }
        | Node::LambdaNode { .. } => true,
        Node::CallNode { .. } => lambda_literal_block(node).is_some(),
        Node::ParenthesesNode { .. } => node
            .as_parentheses_node()
            .and_then(|parentheses| parentheses.body())
            .and_then(|body| super::narrowing::single_statement(&body))
            .is_some_and(|statement| depends_on_hint(&statement)),
        Node::AndNode { .. } | Node::OrNode { .. } | Node::IfNode { .. } | Node::UnlessNode { .. } => true,
        _ => false,
    }
}

/// What a deferred argument stands as until it is typed: a lambda literal is a proc whatever its hint
fn deferred_placeholder(node: &Node<'_>) -> Type {
    if lambda_literal_block(node).is_none() {
        return Type::Untyped;
    }
    let mut function = rbs_environment::Function::returning(Type::Untyped);
    function.untyped_parameters = true;
    Type::Proc(Box::new(rbs_environment::ProcType {
        function,
        block: None,
        self_type: None,
    }))
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
