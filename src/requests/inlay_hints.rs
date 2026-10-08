use serde_json::Value;

use super::{RequestContext, RequestResult, TextDocumentRangeParams, parse_params, to_result};
use crate::global_state::RequestConfig;
use crate::listeners::inlay_hints::InlayHints;

pub const CACHE_KEY: &str = "textDocument/inlayHint";

pub fn handle(context: &mut RequestContext<'_>, params: Value) -> RequestResult {
    let params: TextDocumentRangeParams = parse_params(params)?;
    let document = context.store.get(&params.text_document.uri)?;
    let lines = params.range.start.line..=params.range.end.line;
    let in_range = |hint: &&Value| hint["position"]["line"].as_u64().is_some_and(|line| lines.contains(&(line as u32)));

    if let Some(Value::Array(cached)) = document.cache_get(CACHE_KEY) {
        return to_result(cached.iter().filter(in_range).cloned().collect::<Vec<Value>>());
    }

    if !document.is_ruby_or_erb() {
        return Ok(Value::Null);
    }

    let default_configuration = RequestConfig::default();
    let hints_configuration = context.global_state.feature_configuration("inlayHint").unwrap_or(&default_configuration);
    let parsed = document.parse();
    let mut listener = InlayHints::new(&parsed, hints_configuration);
    listener.visit(&parsed.ast());
    let hints = to_result(listener.into_response())?;
    drop(parsed);

    let hints = document.cache_set(CACHE_KEY, hints);
    let filtered: Vec<Value> = hints.as_array().map(|hints| hints.iter().filter(in_range).cloned().collect()).unwrap_or_default();
    to_result(filtered)
}
