use ruby_prism::Node;
use serde_json::Value;

use super::support::common::{delegate_request_if_needed, determine_target, location_covers_position};
use super::support::sorbet_level::SorbetLevel;
use super::{RequestContext, RequestResult, TextDocumentPositionParams, parse_params, to_result};
use crate::document::ParsedDocument;
use crate::listeners::definition::Definition;
use crate::position::Position;
use crate::prism::NodeExt;

pub const NODE_TYPES: &[&str] = &[
    "CallNode",
    "ConstantReadNode",
    "ConstantPathNode",
    "BlockArgumentNode",
    "GlobalVariableAndWriteNode",
    "GlobalVariableOperatorWriteNode",
    "GlobalVariableOrWriteNode",
    "GlobalVariableReadNode",
    "GlobalVariableTargetNode",
    "GlobalVariableWriteNode",
    "InstanceVariableReadNode",
    "InstanceVariableAndWriteNode",
    "InstanceVariableOperatorWriteNode",
    "InstanceVariableOrWriteNode",
    "InstanceVariableTargetNode",
    "InstanceVariableWriteNode",
    "SymbolNode",
    "StringNode",
    "SuperNode",
    "ForwardingSuperNode",
    "ClassVariableAndWriteNode",
    "ClassVariableOperatorWriteNode",
    "ClassVariableOrWriteNode",
    "ClassVariableReadNode",
    "ClassVariableTargetNode",
    "ClassVariableWriteNode",
];

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

    let node_context = parsed.locate(offset, NODE_TYPES);
    let target = refine_target(&parsed, node_context.node.as_ref(), node_context.parent.as_ref(), position);

    let Some(target) = target else { return to_result(Vec::<Value>::new()) };

    let sorbet_level = SorbetLevel::for_document(global_state, &parsed);
    let mut index = global_state.index();
    let mut listener =
        Definition::new(global_state, &mut index, document.language_id(), document.uri(), &node_context, sorbet_level);
    listener.dispatch(&target);
    to_result(listener.into_response())
}

fn refine_target<'pr>(
    parsed: &ParsedDocument<'_>,
    target: Option<&Node<'pr>>,
    parent: Option<&Node<'pr>>,
    position: Position,
) -> Option<Node<'pr>> {
    let target = target?;

    if target.as_constant_read_node().is_some()
        && let Some(parent) = parent.filter(|parent| parent.as_constant_path_node().is_some())
    {
        return Some(determine_target(parsed, target.clone_node(), parent.clone_node(), position));
    }

    if position_outside_target(parsed, target, position) {
        return None;
    }

    if target.as_symbol_node().is_some()
        && let Some(parent) = parent.filter(|parent| parent.as_block_argument_node().is_some())
    {
        return Some(parent.clone_node());
    }

    Some(target.clone_node())
}

fn position_outside_target(parsed: &ParsedDocument<'_>, target: &Node<'_>, position: Position) -> bool {
    let name_location = match target {
        Node::GlobalVariableAndWriteNode { .. } => target.as_global_variable_and_write_node().map(|node| node.name_loc()),
        Node::GlobalVariableOperatorWriteNode { .. } => {
            target.as_global_variable_operator_write_node().map(|node| node.name_loc())
        }
        Node::GlobalVariableOrWriteNode { .. } => target.as_global_variable_or_write_node().map(|node| node.name_loc()),
        Node::GlobalVariableWriteNode { .. } => target.as_global_variable_write_node().map(|node| node.name_loc()),
        Node::InstanceVariableAndWriteNode { .. } => target.as_instance_variable_and_write_node().map(|node| node.name_loc()),
        Node::InstanceVariableOperatorWriteNode { .. } => {
            target.as_instance_variable_operator_write_node().map(|node| node.name_loc())
        }
        Node::InstanceVariableOrWriteNode { .. } => target.as_instance_variable_or_write_node().map(|node| node.name_loc()),
        Node::InstanceVariableWriteNode { .. } => target.as_instance_variable_write_node().map(|node| node.name_loc()),
        Node::CallNode { .. } => {
            let message_location = target.as_call_node().and_then(|call| call.message_loc());
            return !location_covers_position(parsed, message_location.as_ref(), position);
        }
        _ => return false,
    };
    !location_covers_position(parsed, name_location.as_ref(), position)
}
