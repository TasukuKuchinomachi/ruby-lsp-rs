mod formatter_support;
mod support;

use formatter_support::{workspace, workspace_without_bundle};
use serde_json::{Value, json};
use support::{TestOptions, TestServer, repository_path, with_server};

fn rubocop_server(workspace: &formatter_support::FormatterWorkspace, source: &str, uri: &str) -> TestServer {
    let initialization_options = workspace.server_options(json!({ "linters": ["rubocop_internal"] }));
    with_server(Some(source), uri, TestOptions { initialization_options: Some(initialization_options), ..TestOptions::default() })
}

fn prism_server(source: &str) -> (TestServer, String) {
    let uri = ruby_lsp_rs::uri::from_path_buf(&repository_path().join("fake/file.rb"));
    (with_server(Some(source), &uri, TestOptions::default()), uri)
}

fn diagnostics(server: &mut TestServer, uri: &str) -> Vec<Value> {
    let result = server.result("textDocument/diagnostic", json!({ "textDocument": { "uri": uri } }));
    assert_eq!(result["kind"], "full");
    result["items"].as_array().cloned().unwrap_or_else(|| panic!("Expected items, got {result}"))
}

fn messages(diagnostics: &[Value]) -> Vec<String> {
    diagnostics.iter().map(|diagnostic| diagnostic["message"].as_str().unwrap().to_string()).collect()
}

#[test]
fn empty_diagnostics_for_ignored_file() {
    let Some(workspace) = workspace("diagnostics", &[]) else { return };
    let source = std::fs::read_to_string(repository_path().join("tests/fixtures/def_multiline_params.rb")).unwrap();
    let uri = workspace.uri_for("test/fixtures/def_multiline_params.rb");
    let mut server = rubocop_server(&workspace, &source, &uri);

    assert!(diagnostics(&mut server, &uri).is_empty());
}

#[test]
fn returns_syntax_error_diagnostics() {
    let (mut server, uri) = prism_server("def foo\n");

    let diagnostics = diagnostics(&mut server, &uri);
    assert_eq!(diagnostics.len(), 2);
    assert_eq!(diagnostics.last().unwrap()["message"], "expected an `end` to close the `def` statement");
}

#[test]
fn empty_diagnostics_without_rubocop() {
    let Some(workspace) = workspace_without_bundle("diagnostics_without_rubocop") else { return };
    let uri = workspace.uri_for("file.rb");
    let mut server = rubocop_server(&workspace, "def foo\n  \"Hello, world!\"\nend\n", &uri);

    assert!(diagnostics(&mut server, &uri).is_empty());
}

#[test]
fn diagnostics_with_rubocop() {
    let Some(workspace) = workspace("diagnostics", &[]) else { return };
    let uri = workspace.uri_for("fake/file.rb");
    let mut server = rubocop_server(&workspace, "def foo\n  \"Hello, world!\"\nend\n", &uri);

    let diagnostics = diagnostics(&mut server, &uri);
    assert!(!diagnostics.is_empty());
    assert!(diagnostics.iter().all(|diagnostic| diagnostic["source"] == "RuboCop"));
}

#[test]
fn ambiguous_syntax_warnings() {
    let (mut server, uri) = prism_server("b +a\nb -a\nb *a\nb /a/");

    let messages = messages(&diagnostics(&mut server, &uri));
    assert!(messages[0].contains("ambiguous first argument"), "{messages:?}");
    assert!(messages[1].contains("ambiguous first argument"), "{messages:?}");
    assert!(messages[2].contains("ambiguous `*`"), "{messages:?}");
    assert!(messages[3].contains("ambiguous `/`"), "{messages:?}");
}

#[test]
fn end_inside_method_definition_warning() {
    let (mut server, uri) = prism_server("def m; END{}; end");

    assert_eq!(messages(&diagnostics(&mut server, &uri))[0], "END in method; use at_exit");
}

#[test]
fn syntax_error_diagnostic() {
    let (mut server, uri) = prism_server("def foo");

    let messages = messages(&diagnostics(&mut server, &uri));
    assert_eq!(messages[0], "expected a delimiter to close the parameters");
    assert_eq!(messages[1], "unexpected end-of-input, assuming it is closing the parent top level context");
}

#[test]
fn syntax_diagnostics_use_byte_columns_and_severities() {
    let (mut server, uri) = prism_server("\"é\"; def foo");

    let diagnostics = diagnostics(&mut server, &uri);
    assert_eq!(diagnostics[0]["severity"], 1);
    assert_eq!(diagnostics[0]["source"], "Prism");
    assert_eq!(diagnostics[0]["range"]["start"], json!({ "line": 0, "character": 13 }));
}

#[test]
fn returns_nil_diagnostics_for_files_outside_workspace() {
    let mut server = with_server(Some("def foo; end"), "file:///foo.rb", TestOptions::default());

    let result = server.result("textDocument/diagnostic", json!({ "textDocument": { "uri": "file:///foo.rb" } }));
    assert_eq!(result, Value::Null);
}

#[test]
fn returns_nil_diagnostics_for_erb_documents() {
    let uri = ruby_lsp_rs::uri::from_path_buf(&repository_path().join("fake/file.erb"));
    let mut server = with_server(Some("<%= foo %>"), &uri, TestOptions::default());

    let result = server.result("textDocument/diagnostic", json!({ "textDocument": { "uri": uri } }));
    assert_eq!(result, Value::Null);
}

#[test]
fn diagnostics_are_cached_until_the_document_changes() {
    let Some(workspace) = workspace("diagnostics", &[]) else { return };
    let uri = workspace.uri_for("test/requests/cached_test.rb");
    let mut server = rubocop_server(&workspace, "# typed: true\n# frozen_string_literal: true\n\nfoo = 1\n", &uri);

    let first = diagnostics(&mut server, &uri);
    assert!(first.iter().any(|diagnostic| diagnostic["code"] == "Lint/UselessAssignment"));
    assert_eq!(diagnostics(&mut server, &uri), first);

    server.notify(
        "textDocument/didChange",
        json!({
            "textDocument": { "uri": uri, "version": 2 },
            "contentChanges": [{ "text": "# typed: true\n# frozen_string_literal: true\n\nputs 1\n" }],
        }),
    );
    let after_change = diagnostics(&mut server, &uri);
    assert!(after_change.is_empty(), "{after_change:?}");
}

#[test]
fn restarts_the_worker_when_it_crashes() {
    let Some(workspace) = workspace("worker_restart", &[]) else { return };
    let uri = workspace.uri_for("test/requests/restart_test.rb");
    let mut server = rubocop_server(&workspace, "# typed: true\n# frozen_string_literal: true\n\nfoo = 1\n", &uri);
    let first = diagnostics(&mut server, &uri);

    let mut global_state = ruby_lsp_rs::global_state::GlobalState::new();
    global_state.workspace_uri = ruby_lsp_rs::uri::from_path_buf(&workspace.path);
    global_state.ruby_command = workspace.ruby_command.clone();
    let process_id = ruby_lsp_rs::requests::support::formatter::worker_process_id(&global_state).expect("worker is running");
    assert!(std::process::Command::new("kill").args(["-9", &process_id.to_string()]).status().unwrap().success());

    server.notify(
        "textDocument/didChange",
        json!({ "textDocument": { "uri": uri, "version": 2 }, "contentChanges": [{ "text": "# typed: true\n# frozen_string_literal: true\n\nfoo = 1\n\n" }] }),
    );
    let after_restart = diagnostics(&mut server, &uri);
    assert!(after_restart.iter().any(|diagnostic| diagnostic["code"] == "Lint/UselessAssignment"), "{after_restart:?}");
    assert!(first.iter().any(|diagnostic| diagnostic["code"] == "Lint/UselessAssignment"));
    assert_ne!(ruby_lsp_rs::requests::support::formatter::worker_process_id(&global_state), Some(process_id));
}

#[test]
fn shows_error_if_rubocop_configuration_is_unavailable() {
    let Some(workspace) = workspace("unavailable_configuration", &[(".rubocop", "-c .i_dont_exist.yml")]) else { return };
    let uri = workspace.uri_for("fake/file.rb");
    let mut server = rubocop_server(&workspace, "puts 1\n", &uri);

    let id = server.send_request("textDocument/diagnostic", json!({ "textDocument": { "uri": uri } }));
    let message = server.wait_for_message(|message| {
        matches!(message, lsp_server::Message::Notification(notification) if notification.method == "window/showMessage")
    });
    let lsp_server::Message::Notification(notification) = message else { unreachable!() };
    let expected = format!(
        "RuboCop configuration error: Configuration file not found: {}/.i_dont_exist.yml",
        std::fs::canonicalize(&workspace.path).unwrap().display()
    );
    let actual = notification.params["message"].as_str().unwrap();
    assert!(
        actual.starts_with(&expected) || actual.starts_with(&expected.replace("/private", "")),
        "{actual} does not start with {expected}"
    );

    let response = server.wait_for_response(&id);
    let result = response.response_result.unwrap();
    assert_eq!(result["items"], json!([]));
}

#[test]
fn rubocop_config_changes_reload_configuration_and_trigger_workspace_diagnostic_refresh() {
    let Some(workspace) = workspace("configuration_reload", &[]) else { return };
    let uri = workspace.uri_for("test/requests/reload_test.rb");
    let source = "# typed: true\n# frozen_string_literal: true\n\nputs \"hello\"\n";
    let initialization_options = workspace.server_options(json!({ "linters": ["rubocop_internal"] }));
    let mut options = initialization_options.clone();
    options["capabilities"] = json!({ "workspace": { "diagnostics": { "refreshSupport": true } } });
    let mut server = with_server(Some(source), &uri, TestOptions { initialization_options: Some(options), ..TestOptions::default() });
    server.index().initial_indexing_completed = true;

    let before_change = diagnostics(&mut server, &uri);
    assert!(before_change.is_empty(), "{before_change:?}");

    let configuration_path = workspace.file_path(".rubocop.yml");
    let original_configuration = std::fs::read_to_string(&configuration_path).unwrap();
    std::fs::write(
        &configuration_path,
        format!("{original_configuration}\nStyle/StringLiterals:\n  EnforcedStyle: single_quotes\n"),
    )
    .unwrap();

    for configuration_file in [".rubocop.yml", ".rubocop", ".rubocop_todo.yml"] {
        server.notify(
            "workspace/didChangeWatchedFiles",
            json!({ "changes": [{ "uri": workspace.uri_for(configuration_file), "type": 2 }] }),
        );
        let message = server.wait_for_message(|message| matches!(message, lsp_server::Message::Request(_)));
        let lsp_server::Message::Request(request) = message else { unreachable!() };
        assert_eq!(request.method, "workspace/diagnostic/refresh");
    }

    let codes: Vec<Value> = diagnostics(&mut server, &uri).iter().map(|diagnostic| diagnostic["code"].clone()).collect();
    assert_eq!(codes, vec![json!("Style/StringLiterals")]);
    std::fs::write(&configuration_path, original_configuration).unwrap();
}
