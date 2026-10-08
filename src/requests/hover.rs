use ruby_prism::Node;
use serde_json::{Value, json};

use super::support::common::{delegate_request_if_needed, determine_target, location_covers_position};
use super::support::hover_response_builder::HoverResponseBuilder;
use super::support::sorbet_level::SorbetLevel;
use super::{RequestContext, RequestResult, TextDocumentPositionParams, parse_params};
use crate::document::ParsedDocument;
use crate::listeners::hover::{ALLOWED_TARGETS, Hover};
use crate::position::Position;
use crate::prism::NodeExt;

pub fn handle(context: &mut RequestContext<'_>, params: Value) -> RequestResult {
    let params: TextDocumentPositionParams = parse_params(params)?;
    let document = context.store.get(&params.text_document.uri)?;
    if !document.is_ruby_or_erb() {
        return Ok(Value::Null);
    }

    let global_state = &*context.global_state;
    let parsed = document.parse();
    let position = params.position;
    let offset = parsed.offset_of(position)?;
    delegate_request_if_needed(global_state, document, offset)?;

    let node_context = parsed.locate(offset, ALLOWED_TARGETS);
    let Some(target) = refine_target(&parsed, node_context.node.as_ref(), node_context.parent.as_ref(), position) else {
        return Ok(Value::Null);
    };

    let sorbet_level = SorbetLevel::for_document(global_state, &parsed);
    let mut response_builder = HoverResponseBuilder::default();
    {
        let mut index = global_state.index();
        let mut listener =
            Hover::new(&mut response_builder, global_state, &mut index, document.uri(), &node_context, sorbet_level);
        listener.dispatch(&target);
    }

    if response_builder.is_empty() {
        return Ok(Value::Null);
    }
    Ok(json!({ "contents": { "kind": "markdown", "value": response_builder.response() } }))
}

fn refine_target<'pr>(
    parsed: &ParsedDocument<'_>,
    target: Option<&Node<'pr>>,
    parent: Option<&Node<'pr>>,
    position: Position,
) -> Option<Node<'pr>> {
    let target = target?;

    if let Some(parent) = parent
        && should_refine_target(parent, target)
    {
        return Some(determine_target(parsed, target.clone_node(), parent.clone_node(), position));
    }

    if position_outside_target(parsed, target, position) {
        return None;
    }

    Some(target.clone_node())
}

fn should_refine_target(parent: &Node<'_>, target: &Node<'_>) -> bool {
    (ALLOWED_TARGETS.contains(&parent.type_name()) && !ALLOWED_TARGETS.contains(&target.type_name()))
        || (parent.as_constant_path_node().is_some() && target.as_constant_read_node().is_some())
}

fn position_outside_target(parsed: &ParsedDocument<'_>, target: &Node<'_>, position: Position) -> bool {
    let name_location = match target {
        Node::GlobalVariableAndWriteNode { .. } => target.as_global_variable_and_write_node().map(|node| node.name_loc()),
        Node::GlobalVariableOperatorWriteNode { .. } => {
            target.as_global_variable_operator_write_node().map(|node| node.name_loc())
        }
        Node::GlobalVariableOrWriteNode { .. } => target.as_global_variable_or_write_node().map(|node| node.name_loc()),
        Node::GlobalVariableWriteNode { .. } => target.as_global_variable_write_node().map(|node| node.name_loc()),
        Node::CallNode { .. } => target.as_call_node().and_then(|call| call.message_loc()),
        _ => return false,
    };
    !location_covers_position(parsed, name_location.as_ref(), position)
}
