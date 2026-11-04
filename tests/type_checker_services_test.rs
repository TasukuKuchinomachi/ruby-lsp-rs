mod support;

use std::path::{Path, PathBuf};
use std::thread::JoinHandle;
use std::time::Duration;

use lsp_server::{Connection, Message, Notification, Request, RequestId};
use ruby_lsp_rs::server::{Server, ServerOptions};
use serde_json::{Value, json};

const RESPONSE_TIMEOUT: Duration = Duration::from_secs(120);

struct TypeCheckedServer {
    client: Connection,
    workspace_path: PathBuf,
    thread: Option<JoinHandle<i32>>,
    next_id: i32,
}

fn workspace_path() -> PathBuf {
    support::repository_path().join("tests/type_checker_workspace")
}

fn rbs_gem_directory() -> PathBuf {
    support::repository_path().join("tests/type_checker_rbs_gem")
}

fn position(line: u32, character: u32) -> Value {
    json!({ "line": line, "character": character })
}

fn start_server_in(workspace_path: &Path, initialization_options: Value) -> TypeCheckedServer {
    let (server_connection, client) = Connection::memory();
    let options = ServerOptions {
        test_mode: true,
        rbs_gem_directory: Some(rbs_gem_directory()),
    };
    let server = Server::new(server_connection, options);
    let thread = std::thread::spawn(move || server.run());
    let mut server = TypeCheckedServer {
        client,
        workspace_path: workspace_path.to_path_buf(),
        thread: Some(thread),
        next_id: 1,
    };
    let workspace_uri = ruby_lsp_rs::uri::from_path_buf(workspace_path);
    server.result(
        "initialize",
        json!({
            "workspaceFolders": [{ "uri": workspace_uri, "name": "workspace" }],
            "capabilities": {
                "general": { "positionEncodings": ["utf-16"] },
                "workspace": { "inlayHint": { "refreshSupport": true }, "diagnostics": { "refreshSupport": true } },
            },
            "initializationOptions": initialization_options,
        }),
    );
    server.notify("initialized", json!({}));
    server.wait_for_log("Loaded the signatures");
    server
}

fn start_server() -> TypeCheckedServer {
    start_server_in(&workspace_path(), json!({}))
}

impl TypeCheckedServer {
    fn uri(&self, relative_path: &str) -> String {
        ruby_lsp_rs::uri::from_path_buf(&self.workspace_path.join(relative_path))
    }

    fn notify(&self, method: &str, params: Value) {
        self.client
            .sender
            .send(Message::Notification(Notification::new(method.to_string(), params)))
            .unwrap();
    }

    fn wait_for_log(&self, prefix: &str) -> String {
        loop {
            let message = self
                .client
                .receiver
                .recv_timeout(RESPONSE_TIMEOUT)
                .expect("No message received from server");
            if let Message::Notification(notification) = message
                && notification.method == "window/logMessage"
                && let Some(text) = notification.params["message"].as_str()
            {
                assert!(!text.starts_with("Failed to load the RBS signatures"), "{text}");
                if text.starts_with(prefix) {
                    return text.to_string();
                }
            }
        }
    }

    fn wait_for_request(&self, method: &str) {
        loop {
            let message = self
                .client
                .receiver
                .recv_timeout(RESPONSE_TIMEOUT)
                .expect("No message received from server");
            if let Message::Request(request) = message
                && request.method == method
            {
                return;
            }
        }
    }

    fn open(&self, relative_path: &str) -> String {
        let source = std::fs::read_to_string(self.workspace_path.join(relative_path)).unwrap();
        self.open_with_source(relative_path, &source)
    }

    fn open_with_source(&self, relative_path: &str, source: &str) -> String {
        let language_id = if relative_path.ends_with(".rbs") { "rbs" } else { "ruby" };
        let uri = self.uri(relative_path);
        self.notify(
            "textDocument/didOpen",
            json!({ "textDocument": { "uri": uri, "text": source, "version": 1, "languageId": language_id } }),
        );
        uri
    }

    fn change(&self, uri: &str, version: i32, source: &str) {
        self.notify(
            "textDocument/didChange",
            json!({ "textDocument": { "uri": uri, "version": version }, "contentChanges": [{ "text": source }] }),
        );
    }

    fn configure(&self, type_checker_settings: Value) {
        self.notify(
            "workspace/didChangeConfiguration",
            json!({ "settings": { "typeChecker": type_checker_settings } }),
        );
    }

    fn result(&mut self, method: &str, params: Value) -> Value {
        let id = RequestId::from(self.next_id);
        self.next_id += 1;
        self.client
            .sender
            .send(Message::Request(Request::new(id.clone(), method.to_string(), params)))
            .unwrap();
        loop {
            let message = self
                .client
                .receiver
                .recv_timeout(RESPONSE_TIMEOUT)
                .expect("No message received from server");
            if let Message::Response(response) = message
                && response.id == id
            {
                return response
                    .response_result
                    .unwrap_or_else(|error| panic!("{method} failed: {}", error.message));
            }
        }
    }

    fn at(&mut self, method: &str, uri: &str, line: u32, character: u32) -> Value {
        self.result(
            method,
            json!({ "textDocument": { "uri": uri }, "position": position(line, character) }),
        )
    }

    fn locations(&mut self, method: &str, uri: &str, line: u32, character: u32) -> Vec<(String, u64)> {
        let result = self.at(method, uri, line, character);
        result
            .as_array()
            .unwrap_or_else(|| panic!("{method} returned no locations: {result}"))
            .iter()
            .map(|location| {
                let uri = location["targetUri"]
                    .as_str()
                    .or(location["uri"].as_str())
                    .unwrap()
                    .to_string();
                let range = if location["targetSelectionRange"].is_object() {
                    &location["targetSelectionRange"]
                } else {
                    &location["range"]
                };
                (uri, range["start"]["line"].as_u64().unwrap())
            })
            .collect()
    }

    fn diagnostics(&mut self, uri: &str) -> Value {
        self.result("textDocument/diagnostic", json!({ "textDocument": { "uri": uri } }))
    }

    fn type_diagnostics(&mut self, uri: &str) -> Vec<Value> {
        let diagnostics = self.diagnostics(uri);
        diagnostics["items"]
            .as_array()
            .unwrap_or_else(|| panic!("No diagnostics: {diagnostics}"))
            .iter()
            .filter(|diagnostic| diagnostic["source"] == json!("ruby-lsp-rs"))
            .cloned()
            .collect()
    }

    fn hover_markdown(&mut self, uri: &str, line: u32, character: u32) -> String {
        let hover = self.at("textDocument/hover", uri, line, character);
        hover["contents"]["value"]
            .as_str()
            .unwrap_or_else(|| panic!("No hover: {hover}"))
            .to_string()
    }

    fn completion_items(&mut self, uri: &str, line: u32, character: u32) -> Vec<Value> {
        let result = self.result(
            "textDocument/completion",
            json!({
                "textDocument": { "uri": uri },
                "position": position(line, character),
                "context": { "triggerKind": 2, "triggerCharacter": "." },
            }),
        );
        match result {
            Value::Array(items) => items,
            Value::Object(list) => list["items"].as_array().cloned().unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    fn inlay_hints(&mut self, uri: &str) -> Vec<Value> {
        let hints = self.result(
            "textDocument/inlayHint",
            json!({ "textDocument": { "uri": uri }, "range": { "start": position(0, 0), "end": position(100, 0) } }),
        );
        hints.as_array().cloned().unwrap_or_default()
    }
}

impl Drop for TypeCheckedServer {
    fn drop(&mut self) {
        let shutdown = Request::new(RequestId::from(-1), "shutdown".to_string(), Value::Null);
        let _ = self.client.sender.send(Message::Request(shutdown));
        let _ = self.client.sender.send(Message::Notification(Notification::new(
            "exit".to_string(),
            Value::Null,
        )));
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn temporary_workspace(name: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(format!("ruby-lsp-rs-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    copy_directory(&workspace_path(), &directory);
    directory
}

fn copy_directory(source: &Path, destination: &Path) {
    std::fs::create_dir_all(destination).unwrap();
    for entry in std::fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let target = destination.join(entry.file_name());
        if entry.path().is_dir() {
            copy_directory(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

#[test]
fn reports_type_errors_of_checked_files_with_the_steep_version_of_the_lockfile() {
    let mut server = start_server();
    let uri = server.open("lib/user.rb");
    let diagnostics = server.type_diagnostics(&uri);
    assert_eq!(
        diagnostics,
        vec![json!({
            "code": "Ruby::NoMethod",
            "codeDescription": {
                "href": "https://github.com/soutaro/steep/tree/v2.1.0/manual/ruby-diagnostics.md#Ruby%3A%3ANoMethod",
            },
            "message": "Type `::User` does not have method `unknown_method`",
            "range": { "start": position(18, 5), "end": position(18, 19) },
            "severity": 1,
            "source": "ruby-lsp-rs",
        })]
    );

    let ignored = server.open("lib/ignored.rb");
    assert_eq!(server.type_diagnostics(&ignored), Vec::<Value>::new());
}

#[test]
fn checks_the_unsaved_text_of_a_ruby_file() {
    let mut server = start_server();
    let uri = server.open("lib/shapes.rb");
    assert_eq!(server.type_diagnostics(&uri), Vec::<Value>::new());

    let source = std::fs::read_to_string(workspace_path().join("lib/shapes.rb")).unwrap();
    server.change(&uri, 2, &format!("{source}circle.perimeter\n"));
    let codes: Vec<Value> = server
        .type_diagnostics(&uri)
        .iter()
        .map(|diagnostic| diagnostic["code"].clone())
        .collect();
    assert_eq!(codes, vec![json!("Ruby::NoMethod")]);
}

#[test]
fn reports_signature_problems_in_signature_files() {
    let mut server = start_server();
    let uri = server.open("sig/invalid_signature.rbs");
    assert_eq!(
        server.type_diagnostics(&uri),
        vec![json!({
            "code": "RBS::UnknownTypeName",
            "message": "Cannot find type `MissingType`",
            "range": { "start": position(1, 21), "end": position(1, 32) },
            "severity": 1,
            "source": "ruby-lsp-rs",
        })]
    );

    let valid = server.open("sig/user.rbs");
    assert_eq!(server.type_diagnostics(&valid), Vec::<Value>::new());

    let outside = server.open_with_source(
        "lib/outside.rbs",
        "class Outside\n  def missing: () -> MissingType\nend\n",
    );
    assert_eq!(server.diagnostics(&outside), Value::Null);
}

#[test]
fn reports_type_errors_and_signature_problems_in_inline_files() {
    let mut server = start_server();
    let uri = server.open("inline/calculator.rb");
    let found: Vec<(Value, Value)> = server
        .type_diagnostics(&uri)
        .iter()
        .map(|diagnostic| (diagnostic["code"].clone(), diagnostic["range"]["start"].clone()))
        .collect();
    assert_eq!(
        found,
        vec![
            (json!("Ruby::ArgumentTypeMismatch"), position(8, 28)),
            (json!("RBS::InlineDiagnostic"), position(4, 4))
        ]
    );
}

#[test]
fn reports_syntax_errors_of_unsaved_signatures() {
    let mut server = start_server();
    let uri = server.open_with_source("sig/user.rbs", "class User\n  def greeting: ( -> String\nend\n");
    let diagnostics = server.type_diagnostics(&uri);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(diagnostics[0]["code"], json!("RBS::SyntaxError"));
    assert_eq!(diagnostics[0]["range"]["start"], position(1, 18));
}

#[test]
fn uses_unsaved_signatures_for_ruby_files() {
    let mut server = start_server();
    let signature = server.open("sig/user.rbs");
    let user = server.open("lib/user.rb");
    assert_eq!(server.type_diagnostics(&user).len(), 1);

    let source = std::fs::read_to_string(workspace_path().join("sig/user.rbs")).unwrap();
    server.change(
        &signature,
        2,
        &source.replace("end\n", "  def unknown_method: () -> void\nend\n"),
    );
    assert_eq!(server.type_diagnostics(&user), Vec::<Value>::new());
}

#[test]
fn reloads_signatures_changed_on_disk() {
    let workspace = temporary_workspace("reload");
    let mut server = start_server_in(&workspace, json!({}));
    let user = server.open("lib/user.rb");
    assert_eq!(server.type_diagnostics(&user).len(), 1);

    let signature_path = workspace.join("sig/user.rbs");
    let source = std::fs::read_to_string(&signature_path).unwrap();
    std::fs::write(
        &signature_path,
        source.replace("end\n", "  def unknown_method: () -> void\nend\n"),
    )
    .unwrap();
    server.notify(
        "workspace/didChangeWatchedFiles",
        json!({ "changes": [{ "uri": server.uri("sig/user.rbs"), "type": 2 }] }),
    );
    server.wait_for_request("workspace/diagnostic/refresh");
    assert_eq!(server.type_diagnostics(&user), Vec::<Value>::new());
    let _ = std::fs::remove_dir_all(workspace);
}

#[test]
fn rebuilds_the_service_when_the_steepfile_changes() {
    let workspace = temporary_workspace("steepfile");
    let mut server = start_server_in(&workspace, json!({}));
    let ignored = server.open("lib/ignored.rb");
    assert_eq!(server.type_diagnostics(&ignored), Vec::<Value>::new());

    let steepfile = workspace.join("Steepfile");
    let source = std::fs::read_to_string(&steepfile).unwrap();
    std::fs::write(&steepfile, source.replace("  ignore \"lib/ignored.rb\"\n", "")).unwrap();
    server.notify(
        "workspace/didChangeWatchedFiles",
        json!({ "changes": [{ "uri": server.uri("Steepfile"), "type": 2 }] }),
    );
    server.wait_for_log("Loaded the signatures");
    let codes: Vec<Value> = server
        .type_diagnostics(&ignored)
        .iter()
        .map(|diagnostic| diagnostic["code"].clone())
        .collect();
    assert_eq!(codes, vec![json!("Ruby::NoMethod")]);
    let _ = std::fs::remove_dir_all(workspace);
}

#[test]
fn checks_a_workspace_without_steepfile_as_one_target() {
    let workspace = temporary_workspace("without-steepfile");
    std::fs::remove_file(workspace.join("Steepfile")).unwrap();
    let mut server = start_server_in(&workspace, json!({}));
    let uri = server.open("lib/user.rb");
    let codes: Vec<Value> = server
        .type_diagnostics(&uri)
        .iter()
        .map(|diagnostic| diagnostic["code"].clone())
        .collect();
    assert_eq!(codes, vec![json!("Ruby::NoMethod")]);
    let _ = std::fs::remove_dir_all(workspace);
}

#[test]
fn shows_method_calls_on_hover_as_steep_does_before_the_ruby_lsp_hover() {
    let mut server = start_server();
    let uri = server.open("lib/user.rb");
    let hover = server.at("textDocument/hover", &uri, 17, 17);
    let markdown = hover["contents"]["value"].as_str().unwrap();
    let (type_markdown, index_markdown) = markdown
        .split_once("\n\n---\n\n")
        .unwrap_or_else(|| panic!("{markdown}"));
    assert_eq!(
        type_markdown,
        "```rbs\n::String\n```\n\n----\n**Method type**:\n```rbs\n() -> ::String\n```\n----\n### 📚 User#greeting\n\nReturns a greeting for the user\n\n"
    );
    assert!(index_markdown.starts_with("```ruby\ngreeting()"), "{index_markdown}");
    assert_eq!(
        hover["range"],
        json!({ "start": position(17, 15), "end": position(17, 23) })
    );

    assert_eq!(
        server.hover_markdown(&uri, 16, 1),
        "**Local variable** `user: ::User`\n"
    );
}

#[test]
fn shows_type_names_on_hover_in_signature_files() {
    let mut server = start_server();
    let uri = server.open("sig/user.rbs");
    assert_eq!(
        server.hover_markdown(&uri, 1, 20),
        "```rbs\nclass String\n```\n----\n### 📚 String\n\nA sequence of characters\n\n"
    );
}

#[test]
fn completes_methods_of_typed_receivers_in_incomplete_code() {
    let mut server = start_server();
    let uri = server.open("lib/user.rb");
    let source = std::fs::read_to_string(workspace_path().join("lib/user.rb")).unwrap();
    server.change(&uri, 2, &format!("{source}user.\n"));
    let items = server.completion_items(&uri, 20, 5);
    let greeting = items
        .iter()
        .find(|item| item["label"] == json!("greeting"))
        .unwrap_or_else(|| panic!("No greeting in {items:?}"));
    assert_eq!(greeting["kind"], json!(3));
    assert_eq!(greeting["labelDetails"], json!({ "description": "User#greeting" }));
    assert_eq!(greeting["insertText"], json!("greeting"));
    assert_eq!(
        greeting["documentation"]["value"],
        json!(
            "**Method type**:\n```rbs\n() -> ::String\n```\n----\n### 📚 User#greeting\n\nReturns a greeting for the user\n\n"
        )
    );
    assert!(items.iter().any(|item| item["label"] == json!("frozen?")));
    assert!(!items.iter().any(|item| item["label"] == json!("initialize")));

    let resolved = server.result("completionItem/resolve", greeting.clone());
    let documentation = resolved["documentation"]["value"].as_str().unwrap();
    assert!(documentation.starts_with("**Method type**:"), "{documentation}");
    assert!(
        documentation.contains("\n\n---\n\n```ruby\ngreeting()"),
        "{documentation}"
    );
}

#[test]
fn completes_type_names_in_signature_files() {
    let mut server = start_server();
    let uri = server.open_with_source(
        "sig/fleet.rbs",
        "class Garage::Truck\n  def load: () -> Garage::C\nend\n",
    );
    let items = server.completion_items(&uri, 1, 26);
    let car = items
        .iter()
        .find(|item| item["label"] == json!("Garage::Car"))
        .unwrap_or_else(|| panic!("No Garage::Car in {items:?}"));
    assert_eq!(car["kind"], json!(7));
    assert_eq!(
        car["labelDetails"],
        json!({ "description": "class Garage::Car < ::Object" })
    );
    assert_eq!(
        car["textEdit"]["range"],
        json!({ "start": position(1, 18), "end": position(1, 26) })
    );
    assert!(
        items
            .iter()
            .any(|item| item["label"] == json!("untyped") && item["detail"] == json!("(builtin type)"))
    );
}

#[test]
fn helps_with_the_signature_of_a_call() {
    let mut server = start_server();
    let uri = server.open("lib/user.rb");
    assert_eq!(
        server.at("textDocument/signatureHelp", &uri, 19, 16),
        json!({
            "activeSignature": 0,
            "signatures": [{
                "activeParameter": 1,
                "label": "(::Integer, ?::String) -> ::String",
                "parameters": [{ "label": "::Integer" }, { "label": "?::String" }],
            }],
        })
    );
}

#[test]
fn shows_types_as_inlay_hints() {
    let mut server = start_server();
    let uri = server.open("lib/user.rb");
    let labels: Vec<(Value, Value)> = server
        .inlay_hints(&uri)
        .iter()
        .map(|hint| (hint["position"].clone(), hint["label"].clone()))
        .collect();
    assert_eq!(
        labels,
        vec![
            (position(1, 14), json!("::String")),
            (position(3, 21), json!(": ::String")),
            (position(3, 22), json!("→ void")),
            (position(4, 9), json!(": ::String")),
            (position(7, 14), json!("→ ::String")),
            (position(11, 18), json!(": ::Integer")),
            (position(11, 34), json!("→ ::String")),
            (position(17, 7), json!(": ::String")),
        ]
    );

    server
        .configure(json!({ "inlayHints": { "hideObviousTypes": false, "methodReturnTypes": false, "maxLength": 5 } }));
    let labels: Vec<Value> = server
        .inlay_hints(&uri)
        .iter()
        .map(|hint| hint["label"].clone())
        .collect();
    assert!(labels.contains(&json!(": ::Us…")), "{labels:?}");
    assert!(
        !labels.iter().any(|label| label.as_str().unwrap().starts_with('→')),
        "{labels:?}"
    );
}

#[test]
fn jumps_to_implementations_and_signatures_of_methods() {
    let mut server = start_server();
    let uri = server.open("lib/shapes.rb");
    let shapes = server.uri("lib/shapes.rb");
    let signature = server.uri("sig/shapes.rbs");
    assert_eq!(
        server.locations("textDocument/definition", &uri, 21, 7),
        vec![(shapes.clone(), 1)]
    );
    assert_eq!(
        server.locations("textDocument/definition", &uri, 23, 7),
        vec![(shapes.clone(), 15)]
    );
    assert_eq!(
        server.locations("textDocument/definition", &uri, 22, 16),
        vec![(shapes.clone(), 11)]
    );
    assert_eq!(
        server.locations("textDocument/implementation", &uri, 21, 7),
        vec![(shapes.clone(), 1)]
    );
    assert_eq!(
        server.locations("textDocument/declaration", &uri, 21, 7),
        vec![(signature.clone(), 1)]
    );

    let core = server.locations("textDocument/definition", &uri, 24, 7);
    assert_eq!(core.len(), 1, "{core:?}");
    assert!(
        core[0].0.ends_with("/type_checker_rbs_gem/core/builtin.rbs"),
        "{core:?}"
    );

    server.configure(json!({ "definition": { "target": "signature" } }));
    assert_eq!(
        server.locations("textDocument/definition", &uri, 21, 7),
        vec![(signature.clone(), 1)]
    );
    server.configure(json!({ "definition": { "target": "both" } }));
    assert_eq!(
        server.locations("textDocument/definition", &uri, 21, 7),
        vec![(shapes, 1), (signature, 1)]
    );
}

#[test]
fn jumps_to_type_definitions() {
    let mut server = start_server();
    let uri = server.open("lib/garage.rb");
    let car_implementations = vec![(server.uri("lib/garage.rb"), 1), (server.uri("lib/garage/car.rb"), 1)];
    assert_eq!(
        server.locations("textDocument/typeDefinition", &uri, 18, 5),
        car_implementations
    );

    server.configure(json!({ "definition": { "target": "signature" } }));
    let nil_class = server
        .locations("textDocument/typeDefinition", &uri, 18, 5)
        .pop()
        .unwrap();
    assert!(
        nil_class.0.ends_with("/type_checker_rbs_gem/core/builtin.rbs"),
        "{nil_class:?}"
    );
    assert_eq!(
        server.locations("textDocument/typeDefinition", &uri, 18, 5)[..2],
        [(server.uri("sig/garage/car.rbs"), 1), (server.uri("sig/garage.rbs"), 1)]
    );
}

#[test]
fn jumps_to_the_declarations_of_instance_variables() {
    let mut server = start_server();
    let uri = server.open("lib/garage/car.rb");
    let car = server.uri("lib/garage/car.rb");
    assert_eq!(
        server.locations("textDocument/definition", &uri, 11, 6),
        vec![(car.clone(), 3), (car, 7), (server.uri("sig/garage/car.rbs"), 2)]
    );
}

#[test]
fn jumps_from_signatures_to_implementations() {
    let mut server = start_server();
    let uri = server.open("sig/user.rbs");
    let user = server.uri("lib/user.rb");
    assert_eq!(
        server.locations("textDocument/definition", &uri, 5, 6),
        vec![(user.clone(), 7)]
    );
    assert_eq!(
        server.locations("textDocument/implementation", &uri, 0, 6),
        vec![(user, 0)]
    );
}

#[test]
fn turns_the_type_checker_features_off_with_the_settings() {
    let mut server = start_server();
    let uri = server.open("lib/user.rb");
    server.configure(json!({ "diagnostics": { "enabled": false } }));
    assert_eq!(server.type_diagnostics(&uri), Vec::<Value>::new());
    assert!(server.hover_markdown(&uri, 17, 17).starts_with("```rbs\n::String"));

    server.configure(json!({ "enabled": false }));
    assert!(server.hover_markdown(&uri, 17, 17).starts_with("```ruby\ngreeting()"));
    assert_eq!(server.inlay_hints(&uri), Vec::<Value>::new());
}
