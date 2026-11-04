use serde_json::Value;

use super::{RequestContext, RequestResult, TextDocumentParams, parse_params};
use crate::listeners::document_symbol::DocumentSymbolListener;

const CACHE_KEY: &str = "textDocument/documentSymbol";

pub fn handle(context: &mut RequestContext<'_>, params: Value) -> RequestResult {
    let params: TextDocumentParams = parse_params(params)?;
    let document = context.store.get(&params.text_document.uri)?;
    if !document.is_ruby_or_erb() {
        return Ok(Value::Null);
    }

    let uri = params.text_document.uri;
    Ok(document.cache_fetch(CACHE_KEY, |document| {
        let parsed = document.parse();
        let mut listener = DocumentSymbolListener::new(&parsed, &uri);
        listener.visit(&parsed.ast());
        serde_json::to_value(listener.into_response()).unwrap_or(Value::Null)
    }))
}
