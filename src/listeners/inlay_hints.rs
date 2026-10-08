use ruby_prism::Node;
use serde::Serialize;

use crate::document::ParsedDocument;
use crate::global_state::RequestConfig;
use crate::position::Position;
use crate::prism::{NodeExt, child_nodes, constant_id_name};

const RESCUE_STRING_LENGTH: u32 = "rescue".len() as u32;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InlayHint {
    pub position: Position,
    pub label: String,
    pub padding_left: bool,
    pub tooltip: String,
}

pub struct InlayHints<'a, 'b> {
    response: Vec<InlayHint>,
    parsed: &'a ParsedDocument<'b>,
    hints_configuration: &'a RequestConfig,
}

impl<'a, 'b> InlayHints<'a, 'b> {
    pub fn new(parsed: &'a ParsedDocument<'b>, hints_configuration: &'a RequestConfig) -> InlayHints<'a, 'b> {
        InlayHints { response: Vec::new(), parsed, hints_configuration }
    }

    pub fn into_response(self) -> Vec<InlayHint> {
        self.response
    }

    pub fn visit(&mut self, node: &Node<'_>) {
        match node {
            Node::RescueNode { .. } => self.on_rescue_node_enter(node),
            Node::ImplicitNode { .. } => self.on_implicit_node_enter(node),
            _ => {}
        }
        for child in child_nodes(node) {
            self.visit(&child);
        }
    }

    fn on_rescue_node_enter(&mut self, node: &Node<'_>) {
        if !self.hints_configuration.is_enabled("implicitRescue") {
            return;
        }
        let rescue = node.as_rescue_node().expect("matched a rescue node");
        if rescue.exceptions().iter().next().is_some() {
            return;
        }

        let start = self.parsed.byte_position(node.start_offset());
        self.response.push(InlayHint {
            position: Position::new(start.line, start.character + RESCUE_STRING_LENGTH),
            label: "StandardError".to_string(),
            padding_left: true,
            tooltip: "StandardError is implied in a bare rescue".to_string(),
        });
    }

    fn on_implicit_node_enter(&mut self, node: &Node<'_>) {
        if !self.hints_configuration.is_enabled("implicitHashValue") {
            return;
        }

        let value = node.as_implicit_node().expect("matched an implicit node").value();
        let (node_name, tooltip) = if let Some(call) = value.as_call_node() {
            let name = constant_id_name(&call.name());
            let tooltip = format!("This is a method call. Method name: {name}");
            (name, tooltip)
        } else if let Some(constant) = value.as_constant_read_node() {
            let name = constant_id_name(&constant.name());
            let tooltip = format!("This is a constant: {name}");
            (name, tooltip)
        } else if let Some(local_variable) = value.as_local_variable_read_node() {
            let name = constant_id_name(&local_variable.name());
            let tooltip = format!("This is a local variable: {name}");
            (name, tooltip)
        } else {
            (String::new(), String::new())
        };

        let start = self.parsed.byte_position(node.start_offset());
        let name_length = node_name.chars().count() as u32;
        self.response.push(InlayHint {
            position: Position::new(start.line, start.character + name_length + 1),
            label: node_name,
            padding_left: true,
            tooltip,
        });
    }
}
