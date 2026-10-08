mod support;

use lsp_server::Message;
use serde_json::{Value, json};
use support::{TestServer, uninitialized_server};

fn initialized_server_with_refresh_support() -> TestServer {
    let mut server = uninitialized_server(Vec::new());
    server.result(
        "initialize",
        json!({
            "capabilities": {
                "workspace": { "inlayHint": { "refreshSupport": true }, "diagnostics": { "refreshSupport": true } },
            },
            "initializationOptions": { "typeChecker": { "inlayHints": { "maxLength": 30 } } },
        }),
    );
    server.notify("initialized", json!({}));
    server
}

fn outgoing_requests_until_response(server: &mut TestServer) -> Vec<String> {
    let id = server.send_request("rubyLsp/workspace/addons", json!({}));
    let mut methods = Vec::new();
    loop {
        match server.wait_for_message(|_| true) {
            Message::Request(request) => methods.push(request.method),
            Message::Response(response) if response.id == id => return methods,
            _ => {}
        }
    }
}

fn change_type_checker_configuration(server: &TestServer, type_checker: Value) {
    server.notify("workspace/didChangeConfiguration", json!({ "settings": { "typeChecker": type_checker } }));
}

#[test]
fn refreshes_inlay_hints_and_diagnostics_when_settings_change() {
    let mut server = initialized_server_with_refresh_support();
    change_type_checker_configuration(&server, json!({ "inlayHints": { "maxLength": null } }));
    let methods = outgoing_requests_until_response(&mut server);
    assert!(methods.contains(&"workspace/inlayHint/refresh".to_string()), "{methods:?}");
    assert!(methods.contains(&"workspace/diagnostic/refresh".to_string()), "{methods:?}");
}

#[test]
fn does_not_refresh_when_settings_are_unchanged() {
    let mut server = initialized_server_with_refresh_support();
    outgoing_requests_until_response(&mut server);
    change_type_checker_configuration(&server, json!({ "inlayHints": { "maxLength": 30 } }));
    server.notify("workspace/didChangeConfiguration", json!({ "settings": { "rubyLsp": {} } }));
    let methods = outgoing_requests_until_response(&mut server);
    assert!(!methods.iter().any(|method| method.ends_with("/refresh")), "{methods:?}");
}

#[test]
fn registers_watchers_for_signatures() {
    let mut server = uninitialized_server(Vec::new());
    server.result(
        "initialize",
        json!({
            "capabilities": {
                "workspace": { "didChangeWatchedFiles": { "dynamicRegistration": true, "relativePatternSupport": true } },
            },
        }),
    );
    let mut registration_ids = Vec::new();
    while registration_ids.len() < 4 {
        if let Message::Request(request) = server.wait_for_message(|message| matches!(message, Message::Request(_)))
            && request.method == "client/registerCapability"
        {
            registration_ids.push(request.params["registrations"][0]["id"].as_str().unwrap().to_string());
        }
    }
    assert!(registration_ids.contains(&"rbs-watcher".to_string()));
    assert!(registration_ids.contains(&"steepfile-watcher".to_string()));
}
