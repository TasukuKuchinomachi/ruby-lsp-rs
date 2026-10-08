use serde_json::{Value, json};

use crate::ExpectationCase;
use crate::support::{DEFAULT_URI, TestOptions, with_server};

pub fn run_document_highlight(case: &ExpectationCase) -> Result<Value, String> {
    let mut server = with_server(Some(&case.source), DEFAULT_URI, TestOptions { stub_no_typechecker: true, ..TestOptions::default() });
    let position = match &case.params {
        Some(Value::Array(params)) if !params.is_empty() => params[0].clone(),
        _ => json!({ "line": 0, "character": 0 }),
    };
    let response = server.request(
        "textDocument/documentHighlight",
        json!({ "textDocument": { "uri": DEFAULT_URI }, "position": position }),
    );
    response.response_result.map_err(|error| format!("{} ({})", error.message, error.code))
}
