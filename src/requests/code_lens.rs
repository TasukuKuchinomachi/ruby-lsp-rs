use serde_json::Value;

use ruby_file_glob::fnmatch;
use super::{RequestContext, RequestResult, TextDocumentParams, parse_params};
use crate::listeners::code_lens::CodeLens;
use crate::listeners::test_discovery;
use crate::requests::support::test_collection::TestCollection;
use crate::requests::support::test_command::TEST_PATH_PATTERN;

pub const CACHE_KEY: &str = "textDocument/codeLens";

pub fn handle(context: &mut RequestContext<'_>, params: Value) -> RequestResult {
    let params: TextDocumentParams = parse_params(params)?;
    let document = context.store.get(&params.text_document.uri)?;
    if !document.is_ruby_or_erb() {
        return Ok(Value::Null);
    }
    if let Some(cached) = document.cache_get(CACHE_KEY) {
        return Ok(cached.clone());
    }

    let global_state = &*context.global_state;
    let parsed = document.parse();
    let uri = document.uri().to_string();
    let file_path = crate::uri::to_path(&uri);
    let test_lenses_enabled = global_state
        .feature_configuration("codeLens")
        .is_none_or(|configuration| configuration.is_enabled("enableTestCodeLens"))
        && file_path.as_deref().is_some_and(|path| fnmatch(TEST_PATH_PATTERN, path));

    let mut response = Vec::new();
    let mut test_collection = TestCollection::new();
    if global_state.is_feature_enabled("fullTestDiscovery") {
        if test_lenses_enabled {
            let mut index = global_state.index();
            test_collection = test_discovery::discover(&parsed, &mut index, &uri);
        }
    } else {
        let mut listener = CodeLens::new(&parsed, global_state, &uri);
        listener.visit(&parsed.ast());
        response = listener.into_response();
    }

    let discovered_tests = Value::Array(test_collection.response());
    response.extend(test_collection.into_code_lens());
    let result = Value::Array(response);
    drop(parsed);

    document.cache_set(super::discover_tests::CACHE_KEY, discovered_tests);
    Ok(document.cache_set(CACHE_KEY, result))
}
