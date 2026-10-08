use serde_json::Value;

use super::{RequestContext, RequestResult, TextDocumentParams, parse_params};
use crate::indexer::indexing::index_single;
use crate::listeners::test_discovery;

pub const CACHE_KEY: &str = "rubyLsp/discoverTests";

pub fn handle(context: &mut RequestContext<'_>, params: Value) -> RequestResult {
    let params: TextDocumentParams = parse_params(params)?;
    let document = context.store.get(&params.text_document.uri)?;
    if !document.is_ruby() {
        return Ok(Value::Null);
    }
    if let Some(cached) = document.cache_get(CACHE_KEY) {
        return Ok(cached.clone());
    }

    let uri = document.uri().to_string();
    let parsed = document.parse();
    let mut index = context.global_state.index();
    if index.entries_for(&uri).is_empty() {
        index_single(&mut index, &uri, document.source(), true);
    }
    let items = Value::Array(test_discovery::discover(&parsed, &mut index, &uri).response());
    drop(index);
    drop(parsed);

    Ok(document.cache_set(CACHE_KEY, items))
}
