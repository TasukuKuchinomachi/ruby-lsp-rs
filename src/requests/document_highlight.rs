use serde_json::Value;

use super::support::common::delegate_request_if_needed;
use super::{RequestContext, RequestResult, TextDocumentPositionParams, parse_params, to_result};
use crate::listeners::document_highlight::DocumentHighlightListener;

pub fn handle(context: &mut RequestContext<'_>, params: Value) -> RequestResult {
    let params: TextDocumentPositionParams = parse_params(params)?;
    let document = context.store.get(&params.text_document.uri)?;
    if !document.is_ruby_or_erb() {
        return Ok(Value::Null);
    }

    let parsed = document.parse();
    let offset = parsed.offset_of(params.position)?;
    delegate_request_if_needed(context.global_state, document, offset)?;

    let node_context = parsed.locate(offset, &[]);
    let mut listener =
        DocumentHighlightListener::new(&parsed, node_context.node.as_ref(), node_context.parent.as_ref(), params.position);
    listener.visit(&parsed.ast());
    to_result(listener.into_response())
}
