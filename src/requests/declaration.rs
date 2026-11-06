use serde_json::Value;

use super::support::common::delegate_request_if_needed;
use super::{RequestContext, RequestResult, TextDocumentPositionParams, parse_params, to_result};
use crate::type_checking::worker::GotoKind;

pub fn handle(context: &mut RequestContext<'_>, params: Value) -> RequestResult {
    let params: TextDocumentPositionParams = parse_params(params)?;
    let document = context.store.get(&params.text_document.uri)?;
    if !document.is_ruby_or_erb() {
        return Ok(Value::Null);
    }

    let global_state = &*context.global_state;
    let offset = document.parse().offset_of(params.position)?;
    delegate_request_if_needed(global_state, document, offset)?;

    match global_state
        .type_checking
        .goto(GotoKind::Definition, document.uri(), params.position)
    {
        Some(answer) => to_result(answer.links),
        None => Ok(Value::Null),
    }
}
