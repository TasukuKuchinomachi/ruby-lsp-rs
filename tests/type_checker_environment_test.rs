mod support;

use std::path::PathBuf;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use lsp_server::{Connection, Message, Notification, Request, RequestId};
use rbs_environment::WorkspaceSources;
use ruby_environment::RubyEnvironment;
use ruby_lsp_rs::indexer::indexing::index_single;
use ruby_lsp_rs::server::{Server, ServerOptions};
use ruby_lsp_rs::type_checking;
use serde_json::{Value, json};

const RESPONSE_TIMEOUT: Duration = Duration::from_secs(60);

struct TypeCheckedServer {
    client: Connection,
    thread: Option<JoinHandle<i32>>,
    next_id: i32,
}

impl TypeCheckedServer {
    fn open(&self, relative_path: &str) -> String {
        let source = std::fs::read_to_string(workspace_path().join(relative_path)).unwrap();
        self.open_with_source(relative_path, &source)
    }

    fn open_with_source(&self, relative_path: &str, source: &str) -> String {
        let language_id = if relative_path.ends_with(".rbs") { "rbs" } else { "ruby" };
        self.open_as(relative_path, source, language_id)
    }

    fn open_as(&self, relative_path: &str, source: &str, language_id: &str) -> String {
        let path = workspace_path().join(relative_path);
        let uri = ruby_lsp_rs::uri::from_path_buf(&path);
        let params = json!({ "textDocument": { "uri": uri, "text": source, "version": 1, "languageId": language_id } });
        self.client.sender.send(Message::Notification(Notification::new("textDocument/didOpen".to_string(), params))).unwrap();
        uri
    }

    fn configure(&self, type_checker_settings: Value) {
        let params = json!({ "settings": { "typeChecker": type_checker_settings } });
        self.client
            .sender
            .send(Message::Notification(Notification::new("workspace/didChangeConfiguration".to_string(), params)))
            .unwrap();
    }

    fn locations(&mut self, method: &str, uri: &str, line: u32, character: u32) -> Vec<Value> {
        let result = self.result(method, json!({ "textDocument": { "uri": uri }, "position": position(line, character) }));
        result.as_array().cloned().unwrap_or_else(|| panic!("{method} returned no locations: {result}"))
    }

    fn result(&mut self, method: &str, params: Value) -> Value {
        let id = RequestId::from(self.next_id);
        self.next_id += 1;
        self.client.sender.send(Message::Request(Request::new(id.clone(), method.to_string(), params))).unwrap();
        loop {
            let message = self.client.receiver.recv_timeout(RESPONSE_TIMEOUT).expect("No message received from server");
            if let Message::Response(response) = message
                && response.id == id
            {
                return response.response_result.unwrap_or_else(|error| panic!("{method} failed: {}", error.message));
            }
        }
    }
}

impl Drop for TypeCheckedServer {
    fn drop(&mut self) {
        let _ = self.client.sender.send(Message::Request(Request::new(RequestId::from(-1), "shutdown".to_string(), Value::Null)));
        let _ = self.client.sender.send(Message::Notification(Notification::new("exit".to_string(), Value::Null)));
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn workspace_path() -> PathBuf {
    support::repository_path().join("tests/type_checker_workspace")
}

fn start_server() -> Option<TypeCheckedServer> {
    let Some(core_directory) = support::rbs_core_path() else {
        eprintln!("Skipping: the rbs gem was not found");
        return None;
    };
    let sources = WorkspaceSources {
        stdlib_directory: core_directory.parent().map(|rbs_gem| rbs_gem.join("stdlib")),
        core_directory: Some(core_directory.clone()),
        gem_directories: Default::default(),
    };
    let loaded = type_checking::load_environment(&workspace_path(), &sources, &["ruby".to_string()]).expect("Failed to load the RBS environment");

    let (server_connection, client) = Connection::memory();
    let mut server = Server::new(server_connection, ServerOptions { test_mode: true });
    server.apply_options(
        &json!({ "workspaceFolders": [{ "uri": ruby_lsp_rs::uri::from_path_buf(&workspace_path()), "name": "workspace" }] }),
        Arc::new(RubyEnvironment { rbs_core_path: Some(core_directory.to_string_lossy().into_owned()), ..RubyEnvironment::default() }),
    );
    server.global_state_mut().type_checking.set_environment(loaded);
    let gem_path = support::repository_path().join("tests/type_checker_gem/lib");
    for path in ruby_files(&workspace_path().join("lib")).into_iter().chain(ruby_files(&gem_path)) {
        let source = std::fs::read_to_string(&path).unwrap();
        index_single(&mut server.global_state().index(), &ruby_lsp_rs::uri::from_path_buf(&path), &source, true);
    }
    let thread = std::thread::spawn(move || server.run());
    Some(TypeCheckedServer { client, thread: Some(thread), next_id: 1 })
}

fn ruby_files(directory: &std::path::Path) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(directory).unwrap().filter_map(Result::ok).map(|entry| entry.path()).collect();
    paths.sort();
    paths
        .into_iter()
        .flat_map(|path| {
            if path.is_dir() {
                ruby_files(&path)
            } else if path.extension().is_some_and(|extension| extension == "rb") {
                vec![path]
            } else {
                Vec::new()
            }
        })
        .collect()
}

fn position(line: u32, character: u32) -> Value {
    json!({ "line": line, "character": character })
}

#[test]
fn shows_inferred_types_as_inlay_hints() {
    let Some(mut server) = start_server() else { return };
    let uri = server.open("lib/user.rb");
    let hints = server.result(
        "textDocument/inlayHint",
        json!({ "textDocument": { "uri": uri }, "range": { "start": position(0, 0), "end": position(20, 0) } }),
    );
    let message_hint = hints
        .as_array()
        .unwrap()
        .iter()
        .find(|hint| hint["position"]["line"] == json!(13) && hint["kind"] == json!(1))
        .unwrap_or_else(|| panic!("No type hint for `message`: {hints}"));
    assert_eq!(message_hint["label"], json!(": String"));
    assert_eq!(message_hint["position"], position(13, 7));
}

#[test]
fn shows_method_signatures_on_hover() {
    let Some(mut server) = start_server() else { return };
    let uri = server.open("lib/user.rb");
    let hover = server.result("textDocument/hover", json!({ "textDocument": { "uri": uri }, "position": position(13, 17) }));
    let markdown = hover["contents"]["value"].as_str().unwrap_or_else(|| panic!("No hover: {hover}"));
    assert!(markdown.starts_with("```rbs\ndef greeting: () -> String\n```"), "{markdown}");
    assert!(markdown.contains("Defined in `User`"), "{markdown}");
}

fn workspace_uri(relative_path: &str) -> String {
    ruby_lsp_rs::uri::from_path_buf(&workspace_path().join(relative_path))
}

fn targets(locations: &[Value]) -> Vec<(String, u64)> {
    locations
        .iter()
        .map(|location| {
            (location["targetUri"].as_str().unwrap().to_string(), location["targetRange"]["start"]["line"].as_u64().unwrap())
        })
        .collect()
}

#[test]
fn jumps_to_the_implementation_of_the_receiver_type() {
    let Some(mut server) = start_server() else { return };
    let uri = server.open("lib/shapes.rb");
    let shapes = workspace_uri("lib/shapes.rb");
    assert_eq!(targets(&server.locations("textDocument/definition", &uri, 21, 7)), vec![(shapes.clone(), 1)]);
    assert_eq!(targets(&server.locations("textDocument/definition", &uri, 23, 7)), vec![(shapes.clone(), 15)]);
    assert_eq!(targets(&server.locations("textDocument/definition", &uri, 22, 16)), vec![(shapes.clone(), 11)]);
    assert_eq!(targets(&server.locations("textDocument/definition", &uri, 20, 9)), vec![(shapes, 0)]);

    let user_uri = server.open("lib/user.rb");
    assert_eq!(targets(&server.locations("textDocument/definition", &user_uri, 13, 17)), vec![(workspace_uri("lib/user.rb"), 7)]);
}

#[test]
fn jumps_to_the_signature_of_methods_without_ruby_implementations() {
    let Some(mut server) = start_server() else { return };
    let uri = server.open("lib/shapes.rb");
    let definitions = targets(&server.locations("textDocument/definition", &uri, 24, 7));
    assert_eq!(definitions.len(), 1, "{definitions:?}");
    assert!(definitions[0].0.ends_with("/core/string.rbs"), "{definitions:?}");
}

#[test]
fn jumps_to_declarations_and_type_definitions_in_signatures() {
    let Some(mut server) = start_server() else { return };
    let uri = server.open("lib/shapes.rb");
    let signature = workspace_uri("sig/shapes.rbs");
    assert_eq!(targets(&server.locations("textDocument/declaration", &uri, 21, 7)), vec![(signature.clone(), 1)]);
    assert_eq!(targets(&server.locations("textDocument/declaration", &uri, 23, 7)), vec![(signature.clone(), 7)]);
    assert_eq!(targets(&server.locations("textDocument/typeDefinition", &uri, 21, 0)), vec![(workspace_uri("lib/shapes.rb"), 0)]);
    assert_eq!(targets(&server.locations("textDocument/typeDefinition", &uri, 22, 16)), vec![(workspace_uri("lib/shapes.rb"), 10)]);

    server.configure(json!({ "definition": { "target": "signature" } }));
    assert_eq!(targets(&server.locations("textDocument/typeDefinition", &uri, 21, 0)), vec![(signature.clone(), 0)]);
    assert_eq!(targets(&server.locations("textDocument/typeDefinition", &uri, 22, 16)), vec![(signature, 5)]);
}

#[test]
fn switches_the_definition_target_with_the_configuration() {
    let Some(mut server) = start_server() else { return };
    let uri = server.open("lib/shapes.rb");
    let implementation = (workspace_uri("lib/shapes.rb"), 1);
    let signature = (workspace_uri("sig/shapes.rbs"), 1);

    server.configure(json!({ "definition": { "target": "signature" } }));
    assert_eq!(targets(&server.locations("textDocument/definition", &uri, 21, 7)), vec![signature.clone()]);

    server.configure(json!({ "definition": { "target": "both" } }));
    assert_eq!(targets(&server.locations("textDocument/definition", &uri, 21, 7)), vec![implementation.clone(), signature]);

    server.configure(json!({ "definition": { "target": "implementation" } }));
    assert_eq!(targets(&server.locations("textDocument/definition", &uri, 21, 7)), vec![implementation]);
}

#[test]
fn links_to_the_signature_and_the_implementation_on_hover() {
    let Some(mut server) = start_server() else { return };
    let uri = server.open("lib/shapes.rb");
    let hover = server.result("textDocument/hover", json!({ "textDocument": { "uri": uri }, "position": position(21, 7) }));
    let markdown = hover["contents"]["value"].as_str().unwrap_or_else(|| panic!("No hover: {hover}"));
    let signature_link = format!("**Signature**: [sig/shapes.rbs:2]({}#L2,3)", workspace_uri("sig/shapes.rbs"));
    let implementation_link = format!("**Implementation**: [lib/shapes.rb:2]({}#L2,7)", workspace_uri("lib/shapes.rb"));
    assert!(markdown.contains(&signature_link), "{markdown}");
    assert!(markdown.contains(&implementation_link), "{markdown}");

    let hover = server.result("textDocument/hover", json!({ "textDocument": { "uri": uri }, "position": position(1, 7) }));
    let markdown = hover["contents"]["value"].as_str().unwrap_or_else(|| panic!("No hover: {hover}"));
    assert!(markdown.contains(&signature_link), "{markdown}");
    assert!(!markdown.contains("**Implementation**"), "{markdown}");
}

#[test]
fn reports_type_errors_only_for_checked_files() {
    let Some(mut server) = start_server() else { return };
    let uri = server.open("lib/user.rb");
    let diagnostics = server.result("textDocument/diagnostic", json!({ "textDocument": { "uri": uri } }));
    let no_method = diagnostics["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|diagnostic| diagnostic["code"] == json!("Ruby::NoMethod"))
        .unwrap_or_else(|| panic!("No Ruby::NoMethod diagnostic: {diagnostics}"));
    assert_eq!(no_method["source"], json!("ruby-lsp-rs"));
    assert_eq!(no_method["range"]["start"]["line"], json!(14));

    let ignored_uri = server.open("lib/ignored.rb");
    let diagnostics = server.result("textDocument/diagnostic", json!({ "textDocument": { "uri": ignored_uri } }));
    assert!(
        !diagnostics["items"].as_array().unwrap().iter().any(|diagnostic| diagnostic["source"] == json!("ruby-lsp-rs")),
        "{diagnostics}"
    );
}

#[test]
fn reports_signature_problems_in_signature_files() {
    let Some(mut server) = start_server() else { return };
    let uri = server.open("sig/invalid_signature.rbs");
    let diagnostics = server.result("textDocument/diagnostic", json!({ "textDocument": { "uri": uri } }));
    let items = diagnostics["items"].as_array().unwrap_or_else(|| panic!("No diagnostics: {diagnostics}"));
    assert_eq!(items.len(), 1, "{diagnostics}");
    assert_eq!(items[0]["code"], json!("RBS::UnknownTypeName"));
    assert_eq!(items[0]["severity"], json!(1));
    assert_eq!(items[0]["source"], json!("ruby-lsp-rs"));
    assert_eq!(items[0]["message"], json!("Cannot find type `MissingType`"));
    assert_eq!(items[0]["range"], json!({ "start": position(1, 21), "end": position(1, 32) }));

    let valid_uri = server.open("sig/user.rbs");
    let diagnostics = server.result("textDocument/diagnostic", json!({ "textDocument": { "uri": valid_uri } }));
    assert_eq!(diagnostics["items"], json!([]));
}

#[test]
fn reports_type_errors_and_signature_problems_in_inline_files() {
    let Some(mut server) = start_server() else { return };
    let uri = server.open("inline/calculator.rb");
    let diagnostics = server.result("textDocument/diagnostic", json!({ "textDocument": { "uri": uri } }));
    let items = diagnostics["items"].as_array().unwrap_or_else(|| panic!("No diagnostics: {diagnostics}"));
    let found: Vec<(Value, Value)> = items
        .iter()
        .filter(|diagnostic| diagnostic["source"] == json!("ruby-lsp-rs"))
        .map(|diagnostic| (diagnostic["code"].clone(), diagnostic["range"]["start"].clone()))
        .collect();
    assert_eq!(
        found,
        vec![
            (json!("Ruby::ArgumentTypeMismatch"), position(8, 28)),
            (json!("RBS::InlineDiagnostic"), position(4, 4)),
        ],
        "{diagnostics}"
    );
}

#[test]
fn reports_syntax_errors_of_unsaved_signatures() {
    let Some(mut server) = start_server() else { return };
    let uri = server.open_with_source("sig/user.rbs", "class User\n  def greeting: () ->\nend\n");
    let diagnostics = server.result("textDocument/diagnostic", json!({ "textDocument": { "uri": uri } }));
    let items = diagnostics["items"].as_array().unwrap_or_else(|| panic!("No diagnostics: {diagnostics}"));
    assert_eq!(items.len(), 1, "{diagnostics}");
    assert_eq!(items[0]["code"], json!("RBS::SyntaxError"));
    assert_eq!(items[0]["range"]["start"], position(2, 0));
}

#[test]
fn does_not_validate_signatures_outside_signature_directories() {
    let Some(mut server) = start_server() else { return };
    let uri = server.open_with_source("lib/outside.rbs", "class Outside\n  def missing: () -> MissingType\nend\n");
    let diagnostics = server.result("textDocument/diagnostic", json!({ "textDocument": { "uri": uri } }));
    assert_eq!(diagnostics, Value::Null);
}

fn hover_markdown(server: &mut TypeCheckedServer, uri: &str, line: u32, character: u32) -> String {
    let hover = server.result("textDocument/hover", json!({ "textDocument": { "uri": uri }, "position": position(line, character) }));
    hover["contents"]["value"].as_str().unwrap_or_else(|| panic!("No hover: {hover}")).to_string()
}

fn markdown_link(relative_path: &str, line: u32, column: u32) -> String {
    format!("[{relative_path}:{line}]({}#L{line},{column})", workspace_uri(relative_path))
}

#[test]
fn links_to_the_classes_of_a_variable_type_on_hover() {
    let Some(mut server) = start_server() else { return };
    let uri = server.open("lib/garage.rb");
    let car_signature = markdown_link("sig/garage/car.rbs", 2, 3);
    let car_implementations = format!("{} | {}", markdown_link("lib/garage/car.rb", 2, 9), markdown_link("lib/garage.rb", 2, 9));

    let optional = hover_markdown(&mut server, &uri, 18, 5);
    assert!(optional.contains(&format!("**Signature**: {car_signature}")), "{optional}");
    assert!(optional.contains(&format!("**Implementation**: {car_implementations}")), "{optional}");

    let type_argument = hover_markdown(&mut server, &uri, 11, 5);
    assert!(type_argument.contains("**Signature**: `Array` [array.rbs:"), "{type_argument}");
    assert!(type_argument.contains(&format!(" · `Garage::Car` {car_signature}")), "{type_argument}");
    assert!(type_argument.contains(&format!("**Implementation**: `Garage::Car` {car_implementations}")), "{type_argument}");

    let union = hover_markdown(&mut server, &uri, 17, 5);
    let truck_signature = markdown_link("sig/garage.rbs", 5, 3);
    assert!(union.contains(&format!("**Signature**: `Garage::Car` {car_signature} · `Garage::Truck` {truck_signature}")), "{union}");
    assert!(union.contains(&format!("**Implementation**: `Garage::Car` {car_implementations}")), "{union}");
    assert!(!union.contains("`Garage::Truck` [lib/"), "{union}");

    let core = hover_markdown(&mut server, &uri, 19, 5);
    assert!(core.contains("**Signature**: [integer.rbs:"), "{core}");
    assert!(!core.contains("**Implementation**"), "{core}");
}

#[test]
fn jumps_to_the_signature_of_core_classes_reopened_by_gems() {
    let Some(mut server) = start_server() else { return };
    let uri = server.open("lib/garage.rb");
    let definitions = targets(&server.locations("textDocument/typeDefinition", &uri, 19, 5));
    assert_eq!(definitions.len(), 1, "{definitions:?}");
    assert!(definitions[0].0.ends_with("/core/integer.rbs"), "{definitions:?}");
}

#[test]
fn switches_the_type_definition_target_with_the_configuration() {
    let Some(mut server) = start_server() else { return };
    let uri = server.open("lib/garage.rb");
    let car_implementation = (workspace_uri("lib/garage/car.rb"), 1);
    let car_signature = (workspace_uri("sig/garage/car.rbs"), 1);
    let truck_signature = (workspace_uri("sig/garage.rbs"), 4);

    assert_eq!(targets(&server.locations("textDocument/typeDefinition", &uri, 18, 5)), vec![car_implementation.clone()]);
    assert_eq!(
        targets(&server.locations("textDocument/typeDefinition", &uri, 17, 5)),
        vec![car_implementation.clone(), truck_signature.clone()]
    );

    server.configure(json!({ "definition": { "target": "signature" } }));
    assert_eq!(targets(&server.locations("textDocument/typeDefinition", &uri, 18, 5)), vec![car_signature.clone()]);

    server.configure(json!({ "definition": { "target": "both" } }));
    assert_eq!(
        targets(&server.locations("textDocument/typeDefinition", &uri, 17, 5)),
        vec![car_implementation, car_signature, truck_signature]
    );
}

#[test]
fn chooses_the_primary_declaration_of_a_reopened_class() {
    let Some(mut server) = start_server() else { return };
    let uri = server.open("lib/garage.rb");
    server.configure(json!({ "definition": { "target": "signature" } }));
    assert_eq!(
        targets(&server.locations("textDocument/typeDefinition", &uri, 17, 5)),
        vec![(workspace_uri("sig/garage/car.rbs"), 1), (workspace_uri("sig/garage.rbs"), 4)]
    );
}

fn target_positions(locations: &[Value]) -> Vec<(String, Value)> {
    locations
        .iter()
        .map(|location| match location["targetUri"].as_str() {
            Some(target_uri) => (target_uri.to_string(), location["targetSelectionRange"]["start"].clone()),
            None => (location["uri"].as_str().unwrap().to_string(), location["range"]["start"].clone()),
        })
        .collect()
}

#[test]
fn jumps_to_the_declarations_of_variables() {
    let Some(mut server) = start_server() else { return };
    let uri = server.open("lib/garage.rb");
    let garage = workspace_uri("lib/garage.rb");
    let definition = |server: &mut TypeCheckedServer, line: u32, character: u32| {
        target_positions(&server.locations("textDocument/definition", &uri, line, character))
    };
    assert_eq!(definition(&mut server, 19, 5), vec![(garage.clone(), position(10, 4))]);
    assert_eq!(definition(&mut server, 13, 7), vec![(garage.clone(), position(10, 4))]);
    assert_eq!(definition(&mut server, 11, 5), vec![(garage.clone(), position(9, 19))]);
    assert_eq!(definition(&mut server, 16, 15), vec![(garage.clone(), position(9, 25))]);
    assert_eq!(definition(&mut server, 12, 7), vec![(garage, position(11, 18))]);

    let car_uri = server.open("lib/garage/car.rb");
    let car = workspace_uri("lib/garage/car.rb");
    assert_eq!(
        target_positions(&server.locations("textDocument/definition", &car_uri, 11, 6)),
        vec![(car.clone(), position(3, 6)), (car, position(7, 6)), (workspace_uri("sig/garage/car.rbs"), position(2, 4))]
    );
}

#[test]
fn jumps_from_names_declared_in_signatures_to_their_implementations() {
    let Some(mut server) = start_server() else { return };
    let uri = server.open("sig/user.rbs");
    let user = workspace_uri("lib/user.rb");
    assert_eq!(targets(&server.locations("textDocument/definition", &uri, 0, 6)), vec![(user.clone(), 0)]);
    assert_eq!(targets(&server.locations("textDocument/definition", &uri, 1, 14)), vec![(user.clone(), 1)]);
    assert_eq!(targets(&server.locations("textDocument/definition", &uri, 5, 6)), vec![(user, 7)]);
    assert_eq!(targets(&server.locations("textDocument/definition", &uri, 5, 2)), Vec::new());
}

#[test]
fn jumps_from_type_names_in_signatures_to_their_implementations_and_declarations() {
    let Some(mut server) = start_server() else { return };
    let uri = server.open("sig/garage.rbs");
    assert_eq!(
        targets(&server.locations("textDocument/definition", &uri, 9, 37)),
        vec![(workspace_uri("lib/garage/car.rb"), 1), (workspace_uri("sig/garage/car.rbs"), 1)]
    );

    let definitions = targets(&server.locations("textDocument/definition", &uri, 9, 71));
    assert_eq!(definitions.len(), 1, "{definitions:?}");
    assert!(definitions[0].0.ends_with("/core/integer.rbs"), "{definitions:?}");
}

#[test]
fn jumps_from_unsaved_signatures() {
    let Some(mut server) = start_server() else { return };
    let uri = server.open_with_source("sig/user.rbs", "class User\n  def greeting: () -> String\nend\n");
    assert_eq!(targets(&server.locations("textDocument/definition", &uri, 1, 6)), vec![(workspace_uri("lib/user.rb"), 7)]);
}

#[test]
fn reads_signatures_opened_as_plain_text() {
    let Some(mut server) = start_server() else { return };
    let source = std::fs::read_to_string(workspace_path().join("sig/user.rbs")).unwrap();
    let uri = server.open_as("sig/user.rbs", &source, "plaintext");
    assert_eq!(targets(&server.locations("textDocument/definition", &uri, 5, 6)), vec![(workspace_uri("lib/user.rb"), 7)]);
}
