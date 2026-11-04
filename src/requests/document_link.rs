use serde_json::Value;

use super::{RequestContext, RequestResult, TextDocumentParams, parse_params, to_result};
use crate::listeners::document_link::{DocumentLink, GemPaths};

pub fn handle(context: &mut RequestContext<'_>, params: Value) -> RequestResult {
    let params: TextDocumentParams = parse_params(params)?;
    let document = context.store.get(&params.text_document.uri)?;
    if !document.is_ruby_or_erb() {
        return Ok(Value::Null);
    }

    let gem_paths = GemPaths::from_environment(&context.global_state.ruby_environment);
    let parsed = document.parse();
    to_result(DocumentLink::new(&parsed, &params.text_document.uri, &gem_paths).perform())
}
