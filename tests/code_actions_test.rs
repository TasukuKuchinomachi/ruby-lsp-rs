mod support;

use lsp_server::Message;
use serde_json::{Value, json};
use support::{TestOptions, with_server};

fn resolve(source: &str, uri: &str, language_id: &str, params: Value) -> (lsp_server::Response, Vec<Message>) {
    let mut server = with_server(None, uri, TestOptions::default());
    server.did_open(uri, source, language_id);
    let id = server.send_request("codeAction/resolve", params);
    let mut messages = Vec::new();
    loop {
        let message = server.wait_for_message(|_| true);
        if let Message::Response(response) = &message
            && response.id == id
        {
            return (response.clone(), messages);
        }
        messages.push(message);
    }
}

fn shown_messages(messages: &[Message]) -> Vec<String> {
    messages
        .iter()
        .filter_map(|message| match message {
            Message::Notification(notification) if notification.method == "window/showMessage" => {
                notification.params["message"].as_str().map(str::to_string)
            }
            _ => None,
        })
        .collect()
}

#[test]
fn resolve_fails_for_non_ruby_documents() {
    let params = json!({
        "title": "Refactor: Extract Variable",
        "data": { "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 3 } }, "uri": "file:///fake.erb" },
    });
    let (response, messages) = resolve("<%= foo %>", "file:///fake.erb", "erb", params);

    let error = response.response_result.expect_err("resolve should fail");
    assert_eq!(lsp_server::ErrorCode::RequestFailed as i32, error.code);
    assert_eq!("Code actions are currently only available for Ruby documents", error.message);
    assert_eq!(vec![error.message.clone()], shown_messages(&messages));
}

#[test]
fn resolve_fails_for_unknown_code_actions() {
    let params = json!({
        "title": "Something else",
        "data": { "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 3 } }, "uri": "file:///fake.rb" },
    });
    let (response, messages) = resolve("foo", "file:///fake.rb", "ruby", params);

    let error = response.response_result.expect_err("resolve should fail");
    assert_eq!("Unknown code action: Something else", error.message);
    assert_eq!(vec![error.message.clone()], shown_messages(&messages));
}

#[test]
fn resolve_fails_for_empty_extract_selection() {
    let params = json!({
        "title": "Refactor: Extract Variable",
        "data": { "range": { "start": { "line": 0, "character": 1 }, "end": { "line": 0, "character": 1 } }, "uri": "file:///fake.rb" },
    });
    let (response, _) = resolve("foo", "file:///fake.rb", "ruby", params);
    assert_eq!("Invalid selection for refactor", response.response_result.expect_err("resolve should fail").message);
}

#[test]
fn resolve_extracts_variable() {
    let params = json!({
        "title": "Refactor: Extract Variable",
        "data": { "range": { "start": { "line": 3, "character": 6 }, "end": { "line": 3, "character": 11 } }, "uri": "file:///fake.rb" },
    });
    let (response, _) = resolve("def foo\n  a = 1\n  b = 2\n  c = a + b\nend\n", "file:///fake.rb", "ruby", params);

    assert_eq!(
        json!({
            "title": "Refactor: Extract Variable",
            "edit": { "documentChanges": [{
                "textDocument": { "uri": "file:///fake.rb", "version": null },
                "edits": [
                    { "range": { "start": { "line": 3, "character": 6 }, "end": { "line": 3, "character": 11 } }, "newText": "new_variable" },
                    { "range": { "start": { "line": 3, "character": 2 }, "end": { "line": 3, "character": 2 } }, "newText": "new_variable = a + b\n  " },
                ],
            }] },
        }),
        response.response_result.unwrap()
    );
}

#[test]
fn code_actions_skip_attribute_actions_for_erb() {
    let mut server = with_server(None, "file:///fake.erb", TestOptions::default());
    server.did_open("file:///fake.erb", "<% @foo %>", "erb");
    let range = json!({ "start": { "line": 0, "character": 4 }, "end": { "line": 0, "character": 4 } });
    let result = server.result(
        "textDocument/codeAction",
        json!({ "textDocument": { "uri": "file:///fake.erb" }, "range": range, "context": { "diagnostics": [] } }),
    );
    assert_eq!(json!([]), result);
}

#[test]
fn on_type_formatting_uses_the_client_name_of_the_store() {
    let mut server = with_server(Some("class Foo\n"), "file:///fake.rb", TestOptions::default());
    let result = server.result(
        "textDocument/onTypeFormatting",
        json!({ "textDocument": { "uri": "file:///fake.rb" }, "position": { "line": 1, "character": 0 }, "ch": "\n", "options": {} }),
    );
    assert_eq!(
        json!([
            { "range": { "start": { "line": 1, "character": 0 }, "end": { "line": 1, "character": 0 } }, "newText": "\n" },
            { "range": { "start": { "line": 1, "character": 0 }, "end": { "line": 1, "character": 0 } }, "newText": "end" },
        ]),
        result
    );
}
