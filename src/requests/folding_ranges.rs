use serde_json::Value;

use super::{RequestContext, RequestResult, TextDocumentParams, parse_params, to_result};
use crate::listeners::folding_ranges::FoldingRanges;

pub fn handle(context: &mut RequestContext<'_>, params: Value) -> RequestResult {
    let params: TextDocumentParams = parse_params(params)?;
    let document = context.store.get(&params.text_document.uri)?;
    if !document.is_ruby_or_erb() {
        return Ok(Value::Null);
    }

    let parsed = document.parse();
    to_result(FoldingRanges::new(&parsed).perform())
}
