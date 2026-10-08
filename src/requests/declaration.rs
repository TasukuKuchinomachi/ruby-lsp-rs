use serde_json::Value;

use super::support::common::delegate_request_if_needed;
use super::{RequestContext, RequestResult, TextDocumentPositionParams, parse_params, to_result};
use crate::type_checking;

pub fn handle(context: &mut RequestContext<'_>, params: Value) -> RequestResult {
    let params: TextDocumentPositionParams = parse_params(params)?;
    let document = context.store.get(&params.text_document.uri)?;
    if !document.is_ruby_or_erb() {
        return Ok(Value::Null);
    }

    let analysis = context.global_state.type_checking.analysis_for(document);
    let global_state = &*context.global_state;
    let offset = document.parse().offset_of(params.position)?;
    delegate_request_if_needed(global_state, document, offset)?;

    let Some(analysis) = analysis else { return Ok(Value::Null) };
    to_result(type_checking::definition_links(&analysis, offset, global_state.encoding))
}
