use rbs_environment::{Function, MethodType, Parameter};
use ruby_prism::{CallNode, Node};
use serde::Serialize;

use crate::indexer::entry::Signature;
use crate::indexer::{EntryId, Index};
use crate::node_context::NodeContext;
use crate::position::Encoding;
use crate::prism::{NodeExt, location_slice};
use crate::requests::support::common::markdown_from_index_entries;
use crate::requests::support::sorbet_level::SorbetLevel;
use crate::requests::support::static_docs::GUESSED_TYPES_URL;
use crate::type_inferrer::TypeInferrer;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MarkupContent {
    pub kind: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum ParameterLabel {
    Text(String),
    /// Start and end within the signature label, so that a parameter whose text appears twice is still highlighted
    Offsets([usize; 2]),
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ParameterInformation {
    pub label: ParameterLabel,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SignatureInformation {
    pub label: String,
    pub documentation: MarkupContent,
    pub parameters: Vec<ParameterInformation>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SignatureHelp {
    pub signatures: Vec<SignatureInformation>,
    pub active_signature: usize,
    pub active_parameter: usize,
}

pub struct SignatureHelpListener<'a, 'pr> {
    response: Option<SignatureHelp>,
    index: &'a mut Index,
    source: &'a [u8],
    node_context: &'a NodeContext<'pr>,
    sorbet_level: SorbetLevel,
}

impl<'a, 'pr> SignatureHelpListener<'a, 'pr> {
    pub fn new(
        index: &'a mut Index,
        source: &'a [u8],
        node_context: &'a NodeContext<'pr>,
        sorbet_level: SorbetLevel,
    ) -> SignatureHelpListener<'a, 'pr> {
        SignatureHelpListener { response: None, index, source, node_context, sorbet_level }
    }

    pub fn into_response(self) -> Option<SignatureHelp> {
        self.response
    }

    pub fn dispatch(&mut self, target: &Node<'_>) {
        if let Some(call) = target.as_call_node() {
            self.on_call_node_enter(&call);
        }
    }

    fn on_call_node_enter(&mut self, node: &CallNode<'_>) {
        if self.sorbet_level.is_true_or_higher() {
            return;
        }
        let Some(message) = node.message_loc().map(|location| location_slice(&location)) else { return };
        let Some(receiver_type) = TypeInferrer::new(self.index).infer_receiver_type(self.node_context) else { return };
        let Some(methods) = self.index.resolve_method(&message, &receiver_type.name, false) else { return };
        let Some(target_method) = methods.first().map(|id| self.index.entry(*id)) else { return };

        let signatures = target_method.signatures();
        if signatures.is_empty() {
            return;
        }

        let name = target_method.name.clone();
        let mut title = String::new();
        let extra_links = if receiver_type.guessed {
            title.push_str(&format!("\n\nGuessed receiver: {}", receiver_type.name));
            Some(format!("[Learn more about guessed types]({GUESSED_TYPES_URL})"))
        } else {
            None
        };

        let (active_signature, active_parameter) = self.determine_active_signature_and_parameter(node, &signatures);
        let signatures = self.generate_signatures(&signatures, &name, &methods, &title, extra_links.as_deref());
        self.response = Some(SignatureHelp { signatures, active_signature, active_parameter });
    }

    fn determine_active_signature_and_parameter(&self, node: &CallNode<'_>, signatures: &[Signature]) -> (usize, usize) {
        let arguments_node = node.arguments();
        let arguments: Vec<Node<'_>> =
            arguments_node.as_ref().map(|arguments| arguments.arguments().iter().collect()).unwrap_or_default();

        let active_signature_index = signatures.iter().position(|signature| signature.matches(&arguments)).unwrap_or(0);

        let parameter_length = signatures[active_signature_index].parameters.len().saturating_sub(1);
        let mut active_parameter = arguments.len().saturating_sub(1).min(parameter_length);

        if let Some(arguments_node) = &arguments_node {
            let arguments_end = arguments_node.location().end_offset();
            if arguments_end < node.as_node().end_offset() && self.source.get(arguments_end) == Some(&b',') {
                active_parameter += 1;
            }
        }

        (active_signature_index, active_parameter)
    }

    fn generate_signatures(
        &mut self,
        signatures: &[Signature],
        method_name: &str,
        methods: &[EntryId],
        title: &str,
        extra_links: Option<&str>,
    ) -> Vec<SignatureInformation> {
        signatures
            .iter()
            .map(|signature| SignatureInformation {
                label: format!("{method_name}({})", signature.format()),
                parameters: signature
                    .parameters
                    .iter()
                    .map(|parameter| ParameterInformation { label: ParameterLabel::Text(parameter.name().to_string()) })
                    .collect(),
                documentation: MarkupContent {
                    kind: "markdown".to_string(),
                    value: markdown_from_index_entries(self.index, title, methods, None, extra_links),
                },
            })
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq)]
enum ParameterKind {
    Positional,
    Rest,
    Keyword(String),
    KeywordRest,
}

struct RenderedParameter {
    text: String,
    kind: ParameterKind,
}

#[derive(Debug, Clone, PartialEq)]
enum ActiveArgument {
    Positional(usize),
    /// `None` after a comma that follows a keyword argument, when the next keyword is not written yet
    Keyword(Option<String>),
}

struct WrittenArguments {
    positional_count: usize,
    keyword_names: Vec<String>,
    active: ActiveArgument,
}

/// Signature help built from the RBS overloads of the method the type checker resolved for `call`
pub fn overload_signature_help(
    call: &CallNode<'_>,
    method_name: &str,
    overloads: &[MethodType],
    documentation: &str,
    source: &[u8],
    offset: usize,
    encoding: Encoding,
) -> Option<SignatureHelp> {
    if overloads.is_empty() {
        return None;
    }
    let written_arguments = written_arguments(call, source, offset);
    let signatures = overloads
        .iter()
        .map(|overload| overload_signature(method_name, overload, documentation, encoding))
        .collect();
    let active_signature = overloads
        .iter()
        .position(|overload| accepts(&overload.function, &written_arguments))
        .unwrap_or(0);
    let active_parameter =
        active_parameter_index(&rendered_parameters(&overloads[active_signature].function), &written_arguments);
    Some(SignatureHelp { signatures, active_signature, active_parameter })
}

fn overload_signature(
    method_name: &str,
    overload: &MethodType,
    documentation: &str,
    encoding: Encoding,
) -> SignatureInformation {
    let label = format!("{method_name}{overload}");
    let type_parameters_length = if overload.type_parameters.is_empty() {
        0
    } else {
        let names: Vec<&str> = overload.type_parameters.iter().map(|type_parameter| type_parameter.name.as_str()).collect();
        format!("[{}] ", names.join(", ")).len()
    };
    let parameters_start = method_name.len() + type_parameters_length + "(".len();
    let mut parameters = Vec::new();
    if !overload.function.untyped_parameters {
        let mut start = parameters_start;
        for parameter in rendered_parameters(&overload.function) {
            let end = start + parameter.text.len();
            let label_part = if label.get(start..end) == Some(parameter.text.as_str()) {
                ParameterLabel::Offsets([encoding.code_units_of(&label[..start]), encoding.code_units_of(&label[..end])])
            } else {
                ParameterLabel::Text(parameter.text.clone())
            };
            parameters.push(ParameterInformation { label: label_part });
            start = end + ", ".len();
        }
    }
    SignatureInformation {
        label,
        documentation: MarkupContent { kind: "markdown".to_string(), value: documentation.to_string() },
        parameters,
    }
}

/// Mirrors how RBS writes the parameters of a method type, so that each text is found in the signature label
fn rendered_parameters(function: &Function) -> Vec<RenderedParameter> {
    let describe = |parameter: &Parameter| match &parameter.name {
        Some(name) => format!("{} {name}", parameter.parameter_type),
        None => parameter.parameter_type.to_string(),
    };
    let rendered = |text: String, kind: ParameterKind| RenderedParameter { text, kind };
    let mut parameters = Vec::new();
    let positional = |parameter: &Parameter| rendered(describe(parameter), ParameterKind::Positional);
    parameters.extend(function.required_positionals.iter().map(positional));
    parameters.extend(
        function
            .optional_positionals
            .iter()
            .map(|parameter| rendered(format!("?{}", describe(parameter)), ParameterKind::Positional)),
    );
    if let Some(parameter) = &function.rest_positionals {
        parameters.push(rendered(format!("*{}", describe(parameter)), ParameterKind::Rest));
    }
    parameters.extend(function.trailing_positionals.iter().map(positional));
    parameters.extend(function.required_keywords.iter().map(|(name, parameter)| {
        rendered(format!("{name}: {}", describe(parameter)), ParameterKind::Keyword(name.clone()))
    }));
    parameters.extend(function.optional_keywords.iter().map(|(name, parameter)| {
        rendered(format!("?{name}: {}", describe(parameter)), ParameterKind::Keyword(name.clone()))
    }));
    if let Some(parameter) = &function.rest_keywords {
        parameters.push(rendered(format!("**{}", describe(parameter)), ParameterKind::KeywordRest));
    }
    parameters
}

fn written_arguments(call: &CallNode<'_>, source: &[u8], offset: usize) -> WrittenArguments {
    let arguments: Vec<Node<'_>> =
        call.arguments().map(|arguments| arguments.arguments().iter().collect()).unwrap_or_default();
    let mut positional_count = 0;
    let mut keyword_names = Vec::new();
    let mut active = ActiveArgument::Positional(0);
    let mut last_end_before_offset = None;
    for argument in &arguments {
        if let Some(keyword_hash) = argument.as_keyword_hash_node() {
            for element in keyword_hash.elements().iter() {
                let name = keyword_name(&element);
                if element.start_offset() <= offset {
                    active = ActiveArgument::Keyword(name.clone());
                    last_end_before_offset = Some(element.end_offset());
                }
                keyword_names.extend(name);
            }
            continue;
        }
        if argument.as_block_argument_node().is_some() || argument.as_missing_node().is_some() {
            continue;
        }
        if argument.start_offset() <= offset {
            active = ActiveArgument::Positional(positional_count);
            last_end_before_offset = Some(argument.end_offset());
        }
        positional_count += 1;
    }
    let comma_follows = last_end_before_offset
        .and_then(|end| source.get(end..offset.max(end)))
        .is_some_and(|between| between.contains(&b','));
    if comma_follows {
        active = match active {
            ActiveArgument::Positional(index) => {
                positional_count = positional_count.max(index + 2);
                ActiveArgument::Positional(index + 1)
            }
            ActiveArgument::Keyword(_) => ActiveArgument::Keyword(None),
        };
    }
    WrittenArguments { positional_count, keyword_names, active }
}

fn keyword_name(element: &Node<'_>) -> Option<String> {
    let symbol = element.as_assoc_node()?.key().as_symbol_node()?;
    Some(location_slice(&symbol.value_loc()?))
}

/// Fewer arguments than required are accepted, since the call is still being written
fn accepts(function: &Function, written_arguments: &WrittenArguments) -> bool {
    if function.untyped_parameters {
        return true;
    }
    let maximum_positional_count =
        function.required_positionals.len() + function.optional_positionals.len() + function.trailing_positionals.len();
    let positionals_fit =
        function.rest_positionals.is_some() || written_arguments.positional_count <= maximum_positional_count;
    let keywords_fit = function.rest_keywords.is_some()
        || written_arguments.keyword_names.iter().all(|name| {
            function.required_keywords.iter().chain(&function.optional_keywords).any(|(keyword, _)| keyword == name)
        });
    positionals_fit && keywords_fit
}

fn active_parameter_index(parameters: &[RenderedParameter], written_arguments: &WrittenArguments) -> usize {
    let position_of = |kind: &ParameterKind| parameters.iter().position(|parameter| parameter.kind == *kind);
    let found = match &written_arguments.active {
        ActiveArgument::Positional(index) => parameters
            .iter()
            .enumerate()
            .filter(|(_, parameter)| parameter.kind == ParameterKind::Positional)
            .map(|(position, _)| position)
            .nth(*index)
            .filter(|position| position_of(&ParameterKind::Rest).is_none_or(|rest| *position < rest))
            .or_else(|| position_of(&ParameterKind::Rest)),
        ActiveArgument::Keyword(Some(name)) => {
            position_of(&ParameterKind::Keyword(name.clone())).or_else(|| position_of(&ParameterKind::KeywordRest))
        }
        ActiveArgument::Keyword(None) => parameters
            .iter()
            .position(|parameter| {
                matches!(&parameter.kind, ParameterKind::Keyword(name) if !written_arguments.keyword_names.contains(name))
            })
            .or_else(|| position_of(&ParameterKind::KeywordRest)),
    };
    found.unwrap_or_else(|| parameters.len().saturating_sub(1))
}
