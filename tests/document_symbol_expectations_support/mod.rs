use serde_json::{Value, json};

use crate::ExpectationCase;
use crate::support::{TestOptions, fixture_path, with_server};

pub fn run_document_symbol(case: &ExpectationCase) -> Result<Value, String> {
    let rake_fixture = fixture_path(&format!("{}.rake", case.fixture_name));
    let path = if rake_fixture.is_file() { rake_fixture } else { fixture_path(&format!("{}.rb", case.fixture_name)) };
    let uri = ruby_lsp_rs::uri::from_path_buf(&path);
    let mut server = with_server(Some(&case.source), &uri, TestOptions { stub_no_typechecker: true, ..TestOptions::default() });
    let response = server.request("textDocument/documentSymbol", json!({ "textDocument": { "uri": uri } }));
    response.response_result.map_err(|error| format!("{} ({})", error.message, error.code))
}
