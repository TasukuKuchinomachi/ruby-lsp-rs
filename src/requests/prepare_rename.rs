use ruby_prism::Node;
use serde_json::Value;

use super::support::common::determine_target;
use super::{RequestContext, RequestResult, TextDocumentPositionParams, parse_params, to_result};
use crate::document::ParsedDocument;
use crate::node_context::NodeContext;
use crate::position::Position;
use crate::prism::NodeExt;

pub const NODE_TYPES: &[&str] = &["ConstantReadNode", "ConstantPathNode", "ConstantPathTargetNode"];

pub fn handle(context: &mut RequestContext<'_>, params: Value) -> RequestResult {
    let params: TextDocumentPositionParams = parse_params(params)?;
    let document = context.store.get(&params.text_document.uri)?;
    if !document.is_ruby() {
        return Ok(Value::Null);
    }

    let parsed = document.parse();
    let offset = parsed.offset_of(params.position)?;
    let node_context = parsed.locate(offset, NODE_TYPES);
    let Some(target) = constant_target(&parsed, &node_context, params.position) else { return Ok(Value::Null) };
    to_result(parsed.range_from_node(&target))
}

pub fn constant_target<'pr>(
    parsed: &ParsedDocument<'_>,
    node_context: &NodeContext<'pr>,
    position: Position,
) -> Option<Node<'pr>> {
    let target = node_context.node.as_ref()?;
    if target.as_program_node().is_some() {
        return None;
    }

    if target.as_constant_read_node().is_some()
        && let Some(parent) = node_context.parent.as_ref().filter(|parent| parent.as_constant_path_node().is_some())
    {
        return Some(determine_target(parsed, target.clone_node(), parent.clone_node(), position));
    }

    Some(target.clone_node())
}
