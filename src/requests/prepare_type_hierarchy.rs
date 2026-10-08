use serde::Serialize;
use serde_json::Value;

use super::support::common::{determine_target, kind_for_entry, range_from_location};
use super::{RequestContext, RequestResult, TextDocumentPositionParams, parse_params, to_result};
use crate::position::Range;
use crate::prism::{NodeExt, node_slice};

const NODE_TYPES: &[&str] = &["ConstantReadNode", "ConstantWriteNode", "ConstantPathNode"];

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TypeHierarchyItem {
    pub name: String,
    pub kind: u32,
    pub uri: String,
    pub range: Range,
    pub selection_range: Range,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

pub fn handle(context: &mut RequestContext<'_>, params: Value) -> RequestResult {
    let params: TextDocumentPositionParams = parse_params(params)?;
    let document = context.store.get(&params.text_document.uri)?;
    if !document.is_ruby_or_erb() {
        return Ok(Value::Null);
    }

    let parsed = document.parse();
    let node_context = parsed.locate_node(params.position, NODE_TYPES)?;
    let (Some(node), Some(parent)) = (node_context.node.as_ref(), node_context.parent.as_ref()) else {
        return Ok(Value::Null);
    };

    let target = determine_target(&parsed, node.clone_node(), parent.clone_node(), params.position);
    let mut index = context.global_state.index();
    let Some(entries) = index.resolve(&node_slice(&target), &node_context.nesting) else { return Ok(Value::Null) };
    let Some(first_entry) = entries.first().copied() else { return Ok(Value::Null) };

    let entry = index.entry(first_entry);
    let range = range_from_location(&entry.location);
    to_result(vec![TypeHierarchyItem {
        name: entry.name.clone(),
        kind: kind_for_entry(&index, first_entry),
        uri: entry.uri.to_string(),
        range,
        selection_range: range,
        detail: None,
    }])
}
