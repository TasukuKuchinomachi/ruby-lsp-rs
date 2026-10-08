mod formatter_support;
mod support;

use formatter_support::workspace;
use ruby_lsp_rs::document::{ContentChange, Document, LanguageId};
use ruby_lsp_rs::position::Encoding;
use serde_json::{Value, json};
use support::{TestOptions, repository_path, with_server};

const TEST_PATH: &str = "test/requests/code_actions_formatting_test.rb";

/// Reads the code actions from the diagnostic data instead of requesting `textDocument/codeAction`, which only forwards
/// them, so that the RuboCop integration is tested on its own
fn diagnostic_code_actions(source: &str, diagnostic_code: &str) -> Option<(String, Vec<Value>)> {
    let workspace = workspace("code_actions_formatting", &[])?;
    let uri = workspace.uri_for(TEST_PATH);
    let initialization_options = workspace.server_options(json!({ "linters": ["rubocop_internal"] }));
    let mut server =
        with_server(Some(source), &uri, TestOptions { initialization_options: Some(initialization_options), ..TestOptions::default() });

    let result = server.result("textDocument/diagnostic", json!({ "textDocument": { "uri": uri } }));
    let diagnostic = result["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|diagnostic| diagnostic["source"] == "RuboCop" && diagnostic["code"] == diagnostic_code)
        .unwrap_or_else(|| panic!("Expected {diagnostic_code} diagnostic to be present in {result}"))
        .clone();
    Some((uri, diagnostic["data"]["code_actions"].as_array().cloned().unwrap_or_default()))
}

fn assert_disable_line(fixture: &str, cop_name: &str) {
    let name = format!("rubocop_{fixture}");
    let source = std::fs::read_to_string(repository_path().join(format!("tests/fixtures/{name}.rb"))).unwrap();
    let expected =
        std::fs::read_to_string(repository_path().join(format!("tests/expectations/code_actions_formatting/{name}.exp.rb"))).unwrap();
    let Some((uri, code_actions)) = diagnostic_code_actions(&source, cop_name) else { return };

    let title = format!("Disable {cop_name} for this line");
    let selected_action = code_actions
        .iter()
        .find(|code_action| code_action["title"] == title.as_str())
        .unwrap_or_else(|| panic!("Expected to find code action with title '{title}'"));

    let edits: Vec<ContentChange> = selected_action["edit"]["documentChanges"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|document_change| document_change["edits"].as_array().cloned().unwrap_or_default())
        .map(|edit| serde_json::from_value(json!({ "range": edit["range"], "text": edit["newText"] })).unwrap())
        .collect();

    let mut document = Document::new(&uri, source, 1, LanguageId::Ruby, Encoding::Utf16);
    document.push_edits(&edits, 2).unwrap();
    assert_eq!(document.source(), expected);
}

#[test]
fn disable_line_emoji() {
    assert_disable_line("emoji", "Lint/UselessAssignment");
}

#[test]
fn disable_line_lambda_indentation() {
    assert_disable_line("lambda_indentation", "Layout/IndentationConsistency");
}

#[test]
fn disable_line_comment_on_comment() {
    assert_disable_line("comment_on_comment", "Layout/CommentIndentation");
}

#[test]
fn disable_line_multiline_array() {
    assert_disable_line("multiline_array", "Layout/IndentationConsistency");
}

#[test]
fn disable_line_line_disable_existing() {
    assert_disable_line("line_disable_existing", "Lint/UselessAssignment");
}

#[test]
#[ignore = "Incompatible with inline RuboCop comment (skipped in ruby-lsp too)"]
fn disable_line_multiline_string() {
    assert_disable_line("multiline_string", "Lint/Void");
}

#[test]
#[ignore = "Incompatible with inline RuboCop comment (skipped in ruby-lsp too)"]
fn disable_line_continuation() {
    assert_disable_line("continuation", "Layout/SpaceAroundOperators");
}

#[test]
fn no_disable_line_for_self_resolving_cops() {
    let Some((_, code_actions)) = diagnostic_code_actions("#\ndef foo; end\n", "Layout/EmptyComment") else { return };
    assert!(
        !code_actions.iter().any(|code_action| code_action["title"] == "Disable Layout/EmptyComment for this line"),
        "Should not offer disable action for self-resolving cops"
    );
}

#[test]
fn code_action_request_offers_rubocop_quick_fixes() {
    let Some(workspace) = workspace("code_actions_formatting", &[]) else { return };
    let uri = workspace.uri_for(TEST_PATH);
    let source = "# typed: true\n# frozen_string_literal: true\n\ndef foo\n  bar = 1\nend\n";
    let initialization_options = workspace.server_options(json!({ "linters": ["rubocop_internal"] }));
    let mut server =
        with_server(Some(source), &uri, TestOptions { initialization_options: Some(initialization_options), ..TestOptions::default() });

    let diagnostics = server.result("textDocument/diagnostic", json!({ "textDocument": { "uri": uri } }))["items"].clone();
    let diagnostic = diagnostics
        .as_array()
        .unwrap()
        .iter()
        .find(|diagnostic| diagnostic["code"] == "Lint/UselessAssignment")
        .unwrap()
        .clone();
    let code_actions = server.result(
        "textDocument/codeAction",
        json!({ "textDocument": { "uri": uri }, "range": diagnostic["range"], "context": { "diagnostics": [diagnostic] } }),
    );
    let titles: Vec<&str> = code_actions.as_array().unwrap().iter().filter_map(|code_action| code_action["title"].as_str()).collect();
    assert!(titles.contains(&"Autocorrect Lint/UselessAssignment"), "{titles:?}");
    assert!(titles.contains(&"Disable Lint/UselessAssignment for this line"), "{titles:?}");
}
