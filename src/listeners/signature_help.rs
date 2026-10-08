use ruby_prism::{CallNode, Node};
use serde::Serialize;

use crate::indexer::entry::Signature;
use crate::indexer::{EntryId, Index};
use crate::node_context::NodeContext;
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
pub struct ParameterInformation {
    pub label: String,
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
                    .map(|parameter| ParameterInformation { label: parameter.name().to_string() })
                    .collect(),
                documentation: MarkupContent {
                    kind: "markdown".to_string(),
                    value: markdown_from_index_entries(self.index, title, methods, None, extra_links),
                },
            })
            .collect()
    }
}
