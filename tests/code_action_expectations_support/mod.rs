use serde_json::{Value, json};

use crate::support::{DEFAULT_URI, TestOptions, with_server};
use crate::{ExpectationCase, response_result};

const CODE_ACTION_URI: &str = "file:///fake";

fn non_empty_params(case: &ExpectationCase) -> Option<Value> {
    case.params.clone().filter(|params| match params {
        Value::Object(object) => !object.is_empty(),
        Value::Array(array) => !array.is_empty(),
        Value::Null => false,
        _ => true,
    })
}

pub fn run_prepare_rename(case: &ExpectationCase) -> Result<Value, String> {
    let mut server = with_server(Some(&case.source), DEFAULT_URI, TestOptions::default());
    let position = non_empty_params(case)
        .and_then(|params| params.get("position").cloned())
        .unwrap_or(json!({ "line": 0, "character": 0 }));
    response_result(
        server.request("textDocument/prepareRename", json!({ "textDocument": { "uri": DEFAULT_URI }, "position": position })),
    )
}

pub fn run_code_actions(case: &ExpectationCase) -> Result<Value, String> {
    let mut server = with_server(Some(&case.source), CODE_ACTION_URI, TestOptions::default());
    let mut params = non_empty_params(case).unwrap_or_else(|| {
        let end_line = if case.source.split_inclusive('\n').count() > 1 { 1 } else { 0 };
        let end_character = if case.source.is_empty() { 0 } else { 1 };
        json!({
            "range": { "start": { "line": 0, "character": 0 }, "end": { "line": end_line, "character": end_character } },
            "context": { "diagnostics": [] },
        })
    });
    params["textDocument"] = json!({ "uri": CODE_ACTION_URI });
    response_result(server.request("textDocument/codeAction", params))
}

pub fn run_code_action_resolve(case: &ExpectationCase) -> Result<Value, String> {
    let mut server = with_server(Some(&case.source), CODE_ACTION_URI, TestOptions::default());
    let params = non_empty_params(case).unwrap_or_else(|| {
        json!({
            "data": {
                "range": { "start": { "line": 0, "character": 1 }, "end": { "line": 0, "character": 2 } },
                "uri": CODE_ACTION_URI,
            },
        })
    });
    let has_title = params.get("title").is_some_and(|title| !title.is_null());
    let response = server.request("codeAction/resolve", params);
    match response.response_result {
        Ok(result) => Ok(result),
        Err(error) if !has_title && error.code == lsp_server::ErrorCode::RequestFailed as i32 => Ok(Value::Null),
        Err(error) => Err(format!("{} ({})", error.message, error.code)),
    }
}
