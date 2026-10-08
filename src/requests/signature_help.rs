use ruby_prism::Node;
use serde_json::Value;

use super::support::common::delegate_request_if_needed;
use super::support::sorbet_level::SorbetLevel;
use super::{RequestContext, RequestResult, TextDocumentPositionParams, parse_params, to_result};
use crate::document::ParsedDocument;
use crate::listeners::signature_help::SignatureHelpListener;
use crate::position::{Position, range_covers};
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

    let node_context = parsed.locate(offset, &["CallNode"]);
    let Some(target) = adjust_for_nested_target(&parsed, node_context.node.as_ref(), node_context.parent.as_ref(), position)
    else {
        return Ok(Value::Null);
    };

    let sorbet_level = SorbetLevel::for_document(global_state, &parsed);
    let mut index = global_state.index();
    let mut listener = SignatureHelpListener::new(&mut index, document.ruby_source().as_bytes(), &node_context, sorbet_level);
    listener.dispatch(&target);
    to_result(listener.into_response())
}

fn adjust_for_nested_target<'pr>(
    parsed: &ParsedDocument<'_>,
    target: Option<&Node<'pr>>,
    parent: Option<&Node<'pr>>,
    position: Position,
) -> Option<Node<'pr>> {
    let Some(parent) = parent.filter(|parent| parent.as_call_node().is_some()) else {
        return target.map(NodeExt::clone_node);
    };
    let Some(target_call) = target.and_then(|target| target.as_call_node()) else {
        return Some(parent.clone_node());
    };

    match target_call.arguments() {
        Some(arguments) if range_covers(&parsed.range_from_node(&arguments.as_node()), position) => {
            target.map(NodeExt::clone_node)
        }
        _ => Some(parent.clone_node()),
    }
}
