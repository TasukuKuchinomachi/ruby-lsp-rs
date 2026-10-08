use serde_json::{Value, json};

use crate::ExpectationCase;
use crate::formatter_support::{sanitize_code_action_uris, workspace};
use crate::support::{TestOptions, with_server};

pub fn available() -> bool {
    workspace("diagnostics_expectations", &[]).is_some()
}

pub fn run_diagnostics(case: &ExpectationCase) -> Result<Value, String> {
    let workspace = workspace("diagnostics_expectations", &[]).ok_or("The formatter test workspace is not available")?;
    let uri = workspace.uri_for("test/requests/diagnostics_expectations_test.rb");
    let initialization_options = workspace.server_options(json!({ "linters": ["rubocop_internal"] }));
    let mut server =
        with_server(Some(&case.source), &uri, TestOptions { initialization_options: Some(initialization_options), ..TestOptions::default() });

    let response = server.request("textDocument/diagnostic", json!({ "textDocument": { "uri": uri } }));
    let result = response.response_result.map_err(|error| format!("{} ({})", error.message, error.code))?;
    let mut items = result["items"].clone();
    if let Some(diagnostics) = items.as_array_mut() {
        diagnostics.retain(|diagnostic| !(diagnostic["source"] == "RuboCop" && diagnostic["code"] == "Layout/EndOfLine"));
    }
    sanitize_code_action_uris(&mut items);
    Ok(items)
}
