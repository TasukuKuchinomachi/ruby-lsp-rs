mod support;

use lsp_server::{ErrorCode, Message, Notification, Request, RequestId};
use serde_json::{Value, json};
use support::{DEFAULT_URI, TestOptions, uninitialized_server, with_server};

fn initialize(server: &mut support::TestServer, params: Value) -> Value {
    server.result("initialize", params)
}

fn temporary_directory(name: &str) -> std::path::PathBuf {
    let directory = std::env::temp_dir().join(format!("ruby-lsp-rs-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).unwrap();
    directory
}

#[test]
fn initialize_enabled_features_with_array() {
    let mut server = uninitialized_server(Vec::new());
    let result = initialize(&mut server, json!({ "initializationOptions": { "enabledFeatures": ["semanticHighlighting"] } }));
    let capabilities = &result["capabilities"];
    assert!(capabilities.get("semanticTokensProvider").is_some());
    assert!(capabilities.get("hoverProvider").is_none());
    assert!(capabilities.get("definitionProvider").is_none());
}

#[test]
fn initialize_enabled_features_with_hash() {
    let mut server = uninitialized_server(Vec::new());
    let result = initialize(&mut server, json!({ "initializationOptions": { "enabledFeatures": { "semanticHighlighting": false } } }));
    let capabilities = &result["capabilities"];
    assert!(capabilities.get("semanticTokensProvider").is_none());
    assert_eq!(capabilities["definitionProvider"], json!(true));
    assert_eq!(capabilities["hoverProvider"], json!({}));
}

#[test]
fn initialize_enabled_features_with_no_configuration() {
    let mut server = uninitialized_server(Vec::new());
    let result = initialize(&mut server, json!({ "initializationOptions": {} }));
    let capabilities = &result["capabilities"];
    assert!(capabilities.get("semanticTokensProvider").is_some());
    assert_eq!(capabilities["definitionProvider"], json!(true));
    assert_eq!(capabilities["textDocumentSync"], json!({ "change": 2, "openClose": true }));
}

#[test]
fn initialize_negotiates_position_encoding() {
    for (encodings, expected) in [(json!(["utf-8", "utf-16"]), "utf-8"), (json!(["utf-16"]), "utf-16"), (json!([]), "utf-16")] {
        let mut server = uninitialized_server(Vec::new());
        let result = initialize(&mut server, json!({ "capabilities": { "general": { "positionEncodings": encodings } } }));
        assert_eq!(result["capabilities"]["positionEncoding"], json!(expected));
    }
}

#[test]
fn server_info_includes_version_and_formatter() {
    let mut server = uninitialized_server(Vec::new());
    let result = initialize(&mut server, json!({ "initializationOptions": { "formatter": "syntax_tree" } }));
    assert_eq!(result["serverInfo"]["name"], json!("Ruby LSP"));
    assert!(result["serverInfo"]["version"].is_string());
    assert_eq!(result["formatter"], json!("syntax_tree"));
}

#[test]
fn initialize_sets_client_name_and_applies_indexing_configuration() {
    let workspace = temporary_directory("indexing-configuration");
    let mut server = uninitialized_server(Vec::new());
    initialize(
        &mut server,
        json!({
            "workspaceFolders": [{ "uri": ruby_lsp_rs::uri::from_path_buf(&workspace), "name": "workspace" }],
            "clientInfo": { "name": "Visual Studio Code" },
            "initializationOptions": { "indexing": { "excludedGems": ["rubocop"], "includedPatterns": ["**/bin/**/*"] } },
        }),
    );
    let index = server.index();
    assert_eq!(index.configuration.workspace_path, workspace);
    assert!(index.configuration.excluded_gems.contains(&"rubocop".to_string()));
    assert!(index.configuration.included_patterns.contains(&"**/bin/**/*".to_string()));
}

#[test]
fn unrecognized_request_returns_method_not_found() {
    let mut server = with_server(None, DEFAULT_URI, TestOptions::default());
    let response = server.request("textDocument/doesNotExist", json!({}));
    let error = response.response_result.unwrap_err();
    assert_eq!(error.code, ErrorCode::MethodNotFound as i32);
    assert_eq!(error.message, "Method not found: textDocument/doesNotExist");
}

#[test]
fn did_close_clears_diagnostics() {
    let server = with_server(Some("class Foo\nend\n"), DEFAULT_URI, TestOptions::default());
    server.notify("textDocument/didClose", json!({ "textDocument": { "uri": DEFAULT_URI } }));
    let message = server.wait_for_message(
        |message| matches!(message, Message::Notification(notification) if notification.method == "textDocument/publishDiagnostics"),
    );
    let Message::Notification(notification) = message else { unreachable!() };
    assert_eq!(notification.params, json!({ "uri": DEFAULT_URI, "diagnostics": [] }));
}

#[test]
fn cancelling_requests_returns_expected_error_code() {
    let definition_params = json!({ "textDocument": { "uri": DEFAULT_URI }, "position": { "line": 0, "character": 0 } });
    let server = uninitialized_server(vec![
        Message::Notification(Notification::new(
            "textDocument/didOpen".to_string(),
            json!({ "textDocument": { "uri": DEFAULT_URI, "text": "class Foo\nend\n", "version": 1, "languageId": "ruby" } }),
        )),
        Message::Request(Request::new(RequestId::from(1), "textDocument/definition".to_string(), definition_params.clone())),
        Message::Notification(Notification::new("$/cancelRequest".to_string(), json!({ "id": 1 }))),
        Message::Request(Request::new(RequestId::from(2), "textDocument/definition".to_string(), definition_params)),
    ]);

    let cancelled = server.wait_for_response(&RequestId::from(1));
    let error = cancelled.response_result.unwrap_err();
    assert_eq!(error.code, ErrorCode::RequestCanceled as i32);
    assert_eq!(error.message, "Request 1 was cancelled");
    assert!(server.wait_for_response(&RequestId::from(2)).response_result.is_ok());
}

#[test]
fn diagnose_state_lists_documents() {
    let mut server = with_server(Some("class Foo\nend\n"), DEFAULT_URI, TestOptions::default());
    let result = server.result("rubyLsp/diagnoseState", Value::Null);
    assert_eq!(result["documents"], json!({ DEFAULT_URI: "class Foo\nend\n" }));
    assert_eq!(result["workerAlive"], json!(true));
}

#[test]
fn incremental_edits_are_applied() {
    let mut server = with_server(Some("class Foo\nend\n"), DEFAULT_URI, TestOptions::default());
    server.notify(
        "textDocument/didChange",
        json!({
            "textDocument": { "uri": DEFAULT_URI, "version": 2 },
            "contentChanges": [{ "range": { "start": { "line": 0, "character": 9 }, "end": { "line": 0, "character": 9 } }, "text": "Bar" }],
        }),
    );
    let result = server.result("rubyLsp/diagnoseState", Value::Null);
    assert_eq!(result["documents"][DEFAULT_URI], json!("class FooBar\nend\n"));
}

fn change_text(server: &support::TestServer, version: i32, start: (u32, u32), end: (u32, u32), text: &str) {
    server.notify(
        "textDocument/didChange",
        json!({
            "textDocument": { "uri": DEFAULT_URI, "version": version },
            "contentChanges": [{
                "range": { "start": { "line": start.0, "character": start.1 }, "end": { "line": end.0, "character": end.1 } },
                "text": text,
            }],
        }),
    );
}

#[test]
fn unsaved_changes_are_indexed_when_computing_automatic_features() {
    let mut server = with_server(Some("class Foo\nend\n"), DEFAULT_URI, TestOptions::default());
    change_text(&server, 2, (1, 0), (1, 0), "  def bar; end\n");
    server.result("textDocument/documentSymbol", json!({ "textDocument": { "uri": DEFAULT_URI } }));

    let index = server.index();
    let entries = index.entries_named("bar");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].owner(), Some("Foo"));
}

#[test]
fn edits_outside_of_declarations_do_not_trigger_indexing() {
    let source = "class Foo\n  def bar\n    x = 1\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    server.result("textDocument/documentSymbol", json!({ "textDocument": { "uri": DEFAULT_URI } }));
    server.index().delete(DEFAULT_URI, true);

    change_text(&server, 2, (2, 8), (2, 9), "2");
    server.result("textDocument/documentSymbol", json!({ "textDocument": { "uri": DEFAULT_URI } }));
    assert!(server.index().entries_named("Foo").is_empty());
}

#[test]
fn did_change_watched_files_handles_creations_and_deletions() {
    let workspace = temporary_directory("watched-files");
    let path = workspace.join("watched.rb");
    std::fs::write(&path, "class Watched\nend\n").unwrap();
    let uri = ruby_lsp_rs::uri::from_path_buf(&path);

    let mut server = with_server(None, DEFAULT_URI, TestOptions::default());
    server.index().initial_indexing_completed = true;
    server.notify(
        "workspace/didChangeWatchedFiles",
        json!({ "changes": [{ "uri": uri, "type": 1 }, { "uri": uri, "type": 1 }, { "uri": "file:///does/not/exist.rb", "type": 1 }] }),
    );
    server.result("rubyLsp/diagnoseState", Value::Null);
    assert_eq!(server.index().entries_named("Watched").len(), 1);

    server.notify("workspace/didChangeWatchedFiles", json!({ "changes": [{ "uri": uri, "type": 3 }] }));
    server.result("rubyLsp/diagnoseState", Value::Null);
    assert!(server.index().entries_named("Watched").is_empty());
}

#[test]
fn initialized_indexes_the_workspace_and_reports_progress() {
    let workspace = temporary_directory("initial-indexing");
    std::fs::create_dir_all(workspace.join("lib")).unwrap();
    std::fs::write(workspace.join("lib/foo.rb"), "class Foo\n  def bar; end\nend\n").unwrap();
    let main_uri = ruby_lsp_rs::uri::from_path_buf(&workspace.join("main.rb"));

    let mut server = uninitialized_server(Vec::new());
    initialize(
        &mut server,
        json!({
            "workspaceFolders": [{ "uri": ruby_lsp_rs::uri::from_path_buf(&workspace), "name": "workspace" }],
            "capabilities": { "window": { "workDoneProgress": true } },
            "initializationOptions": {},
        }),
    );
    server.notify("initialized", json!({}));
    server.wait_for_message(|message| {
        matches!(message, Message::Notification(notification) if notification.method == "$/progress" && notification.params["value"]["kind"] == "end")
    });

    server.did_open(&main_uri, "Foo.new.bar\n", "ruby");
    let response = server.result(
        "textDocument/definition",
        json!({ "textDocument": { "uri": main_uri }, "position": { "line": 0, "character": 9 } }),
    );
    let expected_uri = ruby_lsp_rs::uri::from_path_buf(&workspace.join("lib/foo.rb"));
    assert_eq!(response[0]["targetUri"], json!(expected_uri));
    assert!(server.index().initial_indexing_completed);
}

#[test]
fn requests_for_missing_documents_return_invalid_params() {
    let mut server = with_server(None, DEFAULT_URI, TestOptions::default());
    let response = server.request(
        "textDocument/definition",
        json!({ "textDocument": { "uri": "untitled:Untitled-9" }, "position": { "line": 0, "character": 0 } }),
    );
    assert_eq!(response.response_result.unwrap_err().code, ErrorCode::InvalidParams as i32);
}

#[test]
fn invalid_positions_return_invalid_params() {
    let mut server = with_server(Some("foo\n"), DEFAULT_URI, TestOptions::default());
    let response = server.request(
        "textDocument/definition",
        json!({ "textDocument": { "uri": DEFAULT_URI }, "position": { "line": 10, "character": 0 } }),
    );
    assert_eq!(response.response_result.unwrap_err().code, ErrorCode::InvalidParams as i32);
}

#[test]
fn shutdown_responds_with_null() {
    let mut server = with_server(None, DEFAULT_URI, TestOptions::default());
    assert_eq!(server.result("shutdown", Value::Null), Value::Null);
}
