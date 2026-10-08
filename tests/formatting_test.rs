mod formatter_support;
mod support;

use formatter_support::{FormatterWorkspace, workspace};
use serde_json::{Value, json};
use support::{TestOptions, TestServer, repository_path, with_server};

const FORMATTING_TEST_PATH: &str = "test/requests/formatting_test.rb";
const FORMATTING_EXPECTATIONS_TEST_PATH: &str = "test/requests/formatting_expectations_test.rb";
const UNFORMATTED_SOURCE: &str = "class Foo\ndef foo\nend\nend\n";

fn formatting_server(workspace: &FormatterWorkspace, formatter: &str, source: &str, relative_path: &str) -> (TestServer, String) {
    let uri = workspace.uri_for(relative_path);
    let initialization_options = workspace.server_options(json!({ "formatter": formatter }));
    let server = with_server(Some(source), &uri, TestOptions { initialization_options: Some(initialization_options), ..TestOptions::default() });
    (server, uri)
}

fn formatting(server: &mut TestServer, uri: &str) -> Value {
    server.result("textDocument/formatting", json!({ "textDocument": { "uri": uri }, "options": { "tabSize": 2, "insertSpaces": true } }))
}

fn formatted_document(workspace: &FormatterWorkspace, formatter: &str, source: &str) -> Option<String> {
    let (mut server, uri) = formatting_server(workspace, formatter, source, FORMATTING_TEST_PATH);
    let result = formatting(&mut server, &uri);
    result[0]["newText"].as_str().map(str::to_string)
}

#[test]
fn formats_with_rubocop() {
    let Some(workspace) = workspace("formatting", &[]) else { return };
    assert_eq!(
        formatted_document(&workspace, "rubocop_internal", UNFORMATTED_SOURCE).as_deref(),
        Some("# typed: true\n# frozen_string_literal: true\n\nclass Foo\n  def foo\n  end\nend\n")
    );
}

#[test]
fn formats_with_syntax_tree() {
    let Some(workspace) = workspace("formatting", &[]) else { return };
    assert_eq!(
        formatted_document(&workspace, "syntax_tree", UNFORMATTED_SOURCE).as_deref(),
        Some("class Foo\n  def foo\n  end\nend\n")
    );
}

#[test]
fn formatting_edit_replaces_the_whole_document() {
    let Some(workspace) = workspace("formatting", &[]) else { return };
    let (mut server, uri) = formatting_server(&workspace, "syntax_tree", UNFORMATTED_SOURCE, FORMATTING_TEST_PATH);
    assert_eq!(
        formatting(&mut server, &uri),
        json!([{
            "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 4, "character": 0 } },
            "newText": "class Foo\n  def foo\n  end\nend\n",
        }])
    );
}

#[test]
fn does_not_format_with_formatter_is_none() {
    let Some(workspace) = workspace("formatting", &[]) else { return };
    assert_eq!(formatted_document(&workspace, "none", "def foo"), None);
}

#[test]
fn syntax_tree_formatting_uses_options_from_streerc() {
    let streerc = "--print-width=100\n--plugins=plugin/trailing_comma\n";
    let Some(workspace) = workspace("formatting_streerc", &[(".streerc", streerc)]) else { return };
    let source = format!(
        "class Foo\ndef foo\n{{one: \"{}\", two: \"{}\"}}\nSomeClass.with(arguments).and_more_methods.some_are_wordy.who_is_demeter_again?\nend\nend\n",
        "a".repeat(50),
        "b".repeat(50)
    );
    let expected = format!(
        "class Foo\n  def foo\n    {{\n      one: \"{}\",\n      two: \"{}\",\n    }}\n    SomeClass.with(arguments).and_more_methods.some_are_wordy.who_is_demeter_again?\n  end\nend\n",
        "a".repeat(50),
        "b".repeat(50)
    );
    assert_eq!(formatted_document(&workspace, "syntax_tree", &source), Some(expected));
}

#[test]
fn syntax_tree_formatting_ignores_syntax_invalid_documents() {
    let Some(workspace) = workspace("formatting", &[]) else { return };
    assert_eq!(formatted_document(&workspace, "syntax_tree", "def foo"), None);
}

#[test]
fn syntax_tree_formatting_returns_nil_if_file_matches_ignore_files_options_from_streerc() {
    let streerc = format!("--ignore-files={FORMATTING_TEST_PATH}\n");
    let Some(workspace) = workspace("formatting_streerc_ignore", &[(".streerc", &streerc)]) else { return };
    assert_eq!(formatted_document(&workspace, "syntax_tree", UNFORMATTED_SOURCE), None);
}

#[test]
fn rubocop_formatting_ignores_syntax_invalid_documents() {
    let Some(workspace) = workspace("formatting", &[]) else { return };
    assert_eq!(formatted_document(&workspace, "rubocop_internal", "def foo"), None);
}

#[test]
fn returns_nil_if_document_is_already_formatted() {
    let Some(workspace) = workspace("formatting", &[]) else { return };
    let source = "# typed: strict\n# frozen_string_literal: true\n\nclass Foo\n  def foo\n  end\nend\n";
    assert_eq!(formatted_document(&workspace, "rubocop_internal", source), None);
}

#[test]
fn returns_nil_when_formatter_is_invalid() {
    let Some(workspace) = workspace("formatting", &[]) else { return };
    assert_eq!(formatted_document(&workspace, "invalid", UNFORMATTED_SOURCE), None);
}

#[test]
fn rubocop_formatter_uses_internal_integration_when_rubocop_add_on_is_unavailable() {
    let Some(workspace) = workspace("formatting", &[]) else { return };
    assert_eq!(
        formatted_document(&workspace, "rubocop", UNFORMATTED_SOURCE).as_deref(),
        Some("# typed: true\n# frozen_string_literal: true\n\nclass Foo\n  def foo\n  end\nend\n")
    );
}

#[test]
fn returns_nil_formatting_for_files_outside_workspace() {
    let mut server = with_server(
        Some(UNFORMATTED_SOURCE),
        "file:///foo.rb",
        TestOptions { initialization_options: Some(json!({ "initializationOptions": { "formatter": "syntax_tree" } })), ..TestOptions::default() },
    );
    assert_eq!(formatting(&mut server, "file:///foo.rb"), Value::Null);
}

#[test]
fn formatting_errors_push_window_notification() {
    let files = [(".rubocop.yml", "{}\n"), ("test/requests/.rubocop.yml", "Style/StringLiterals:\n  EnforcedStyle: invalid_style\n")];
    let Some(workspace) = workspace("formatting_error", &files) else { return };
    let (mut server, uri) = formatting_server(&workspace, "rubocop_internal", UNFORMATTED_SOURCE, FORMATTING_TEST_PATH);

    let id = server.send_request("textDocument/formatting", json!({ "textDocument": { "uri": uri }, "options": {} }));
    let message = server.wait_for_message(|message| {
        matches!(message, lsp_server::Message::Notification(notification) if notification.method == "window/showMessage")
    });
    let lsp_server::Message::Notification(notification) = message else { unreachable!() };
    assert_eq!(
        notification.params["message"],
        "Formatting error: invalid EnforcedStyle 'invalid_style' for Style/StringLiterals found in test/requests/.rubocop.yml\nValid choices are: single_quotes, double_quotes"
    );
    assert_eq!(server.wait_for_response(&id).response_result.unwrap(), Value::Null);
}

#[test]
fn formatting_expectations() {
    let Some(workspace) = workspace("formatting", &[]) else { return };
    let expectations_directory = repository_path().join("tests/expectations/formatting");
    let mut fixtures: Vec<_> = std::fs::read_dir(repository_path().join("tests/fixtures"))
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "rb" || extension == "rake"))
        .collect();
    fixtures.sort();

    let mut failures = Vec::new();
    let mut compared = 0;
    for fixture in fixtures {
        let fixture_name = fixture.file_stem().unwrap().to_string_lossy().into_owned();
        let source = std::fs::read_to_string(&fixture).unwrap();
        let (mut server, uri) = formatting_server(&workspace, "rubocop_internal", &source, FORMATTING_EXPECTATIONS_TEST_PATH);
        let response = server.request(
            "textDocument/formatting",
            json!({ "textDocument": { "uri": uri }, "options": { "tabSize": 2, "insertSpaces": true } }),
        );
        let result = match response.response_result {
            Ok(result) => result,
            Err(error) => {
                failures.push(format!("formatting__{fixture_name}__does_not_raise: {}", error.message));
                continue;
            }
        };

        let expectation_path = expectations_directory.join(format!("{fixture_name}.exp.rb"));
        if !expectation_path.is_file() {
            continue;
        }
        compared += 1;
        let expected = std::fs::read_to_string(&expectation_path).unwrap();
        let actual = result[0]["newText"].as_str().unwrap_or_default();
        if actual != expected {
            failures.push(format!(
                "formatting__{fixture_name}\n########## Expected ##########\n{expected}\n##########  Actual  ##########\n{actual}"
            ));
        }
    }

    assert_eq!(compared, 3);
    assert!(failures.is_empty(), "{} expectation(s) failed:\n\n{}", failures.len(), failures.join("\n\n"));
}
