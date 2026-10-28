#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use lsp_server::{Connection, Message, Notification, Request, RequestId, Response};
use rbs_environment::Environment;
use ruby_environment::RubyEnvironment;
use ruby_lsp_rs::indexer::Index;
use ruby_lsp_rs::indexer::indexing::{index_single, index_single_with_require_path};
use ruby_lsp_rs::server::{Server, ServerOptions};
use ruby_type_check::project::{LoadedEnvironment, LoadedTarget};
use serde_json::{Value, json};

pub const DEFAULT_URI: &str = "file:///fake.rb";
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(20);

/// Stands in for `RubyLsp::BUNDLE_PATH`, which is always present when Ruby LSP's own test suite runs
pub const FAKE_BUNDLE_PATH: &str = "/ruby-lsp-rs-test-bundle";

#[derive(Default, Clone)]
pub struct TestOptions {
    /// Ruby LSP's own bundle depends on Sorbet, so tests run with a type checker unless this is set
    pub stub_no_typechecker: bool,
    pub rubylibdir: Option<String>,
    pub initialization_options: Option<Value>,
    /// Stands in for Ruby LSP's `stub_test_library`, since detection depends on the bundle's dependencies
    pub test_library: Option<String>,
    /// RBS environment of the built-in type checker, which is inactive without one
    pub type_checking_environment: Option<Arc<Environment>>,
}

pub struct TestServer {
    pub client: Connection,
    pub uri: String,
    index: Arc<Mutex<Index>>,
    thread: Option<JoinHandle<i32>>,
    next_id: i32,
}

pub fn with_server(source: Option<&str>, uri: &str, options: TestOptions) -> TestServer {
    let (server_connection, client) = Connection::memory();
    let mut server = Server::new(server_connection, ServerOptions { test_mode: true });
    let initialization_options = options
        .initialization_options
        .clone()
        .unwrap_or_else(|| json!({ "initializationOptions": { "experimentalFeaturesEnabled": true } }));
    server.apply_options(&initialization_options, Arc::new(RubyEnvironment::default()));
    let global_state = server.global_state_mut();
    global_state.has_type_checker = !options.stub_no_typechecker;
    global_state.dependency_paths = vec![FAKE_BUNDLE_PATH.to_string()];
    global_state.rubylibdir = options.rubylibdir.clone();
    if let Some(test_library) = &options.test_library {
        global_state.test_library = test_library.clone();
    }
    if let Some(environment) = &options.type_checking_environment {
        let workspace_path = std::path::PathBuf::from(global_state.workspace_path());
        global_state.type_checking.set_environment(&workspace_path, LoadedEnvironment {
            targets: vec![LoadedTarget { name: None, environment: environment.clone() }],
            steepfile: None,
            duration: Duration::ZERO,
        });
    }
    let index = server.global_state().index.clone();
    let thread = std::thread::spawn(move || server.run());

    let test_server = TestServer { client, uri: uri.to_string(), index, thread: Some(thread), next_id: 1 };
    if let Some(source) = source {
        let language_id = if uri.ends_with(".erb") { "erb" } else { "ruby" };
        test_server.did_open(uri, source, language_id);
        index_single(&mut test_server.index(), uri, source, true);
    }
    test_server
}

pub fn uninitialized_server(queued_messages: Vec<Message>) -> TestServer {
    let (server_connection, client) = Connection::memory();
    let server = Server::new(server_connection, ServerOptions { test_mode: true });
    let index = server.global_state().index.clone();
    for message in queued_messages {
        client.sender.send(message).unwrap();
    }
    let thread = std::thread::spawn(move || server.run());
    TestServer { client, uri: DEFAULT_URI.to_string(), index, thread: Some(thread), next_id: 1000 }
}

impl TestServer {
    pub fn index(&self) -> MutexGuard<'_, Index> {
        self.index.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn index_source(&self, uri: &str, source: &str) {
        index_single(&mut self.index(), uri, source, true);
    }

    pub fn index_file(&self, path: &Path, require_path: Option<&str>) {
        let source = std::fs::read(path).unwrap_or_else(|error| panic!("Failed to read {}: {error}", path.display()));
        let uri = ruby_lsp_rs::uri::from_path_buf(path);
        index_single_with_require_path(&mut self.index(), Arc::from(uri.as_str()), &source, true, require_path);
    }

    pub fn notify(&self, method: &str, params: Value) {
        self.client.sender.send(Message::Notification(Notification::new(method.to_string(), params))).unwrap();
    }

    pub fn did_open(&self, uri: &str, source: &str, language_id: &str) {
        self.notify(
            "textDocument/didOpen",
            json!({ "textDocument": { "uri": uri, "text": source, "version": 1, "languageId": language_id } }),
        );
    }

    pub fn send_request(&mut self, method: &str, params: Value) -> RequestId {
        let id = RequestId::from(self.next_id);
        self.next_id += 1;
        self.client.sender.send(Message::Request(Request::new(id.clone(), method.to_string(), params))).unwrap();
        id
    }

    pub fn wait_for_response(&self, id: &RequestId) -> Response {
        loop {
            let message = self.client.receiver.recv_timeout(RESPONSE_TIMEOUT).expect("No message received from server");
            if let Message::Response(response) = message
                && &response.id == id
            {
                return response;
            }
        }
    }

    pub fn wait_for_message(&self, mut predicate: impl FnMut(&Message) -> bool) -> Message {
        loop {
            let message = self.client.receiver.recv_timeout(RESPONSE_TIMEOUT).expect("No message received from server");
            if predicate(&message) {
                return message;
            }
        }
    }

    pub fn request(&mut self, method: &str, params: Value) -> Response {
        let id = self.send_request(method, params);
        self.wait_for_response(&id)
    }

    pub fn result(&mut self, method: &str, params: Value) -> Value {
        let response = self.request(method, params);
        match response.response_result {
            Ok(result) => result,
            Err(error) => panic!("Failed to execute request {method}: {} ({})", error.message, error.code),
        }
    }

    pub fn definition(&mut self, line: u32, character: u32) -> Vec<Value> {
        let uri = self.uri.clone();
        let result = self.result(
            "textDocument/definition",
            json!({ "textDocument": { "uri": uri }, "position": { "line": line, "character": character } }),
        );
        result.as_array().cloned().unwrap_or_else(|| panic!("Expected an array, got {result}"))
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        let id = RequestId::from(-1);
        let _ = self.client.sender.send(Message::Request(Request::new(id, "shutdown".to_string(), Value::Null)));
        let _ = self.client.sender.send(Message::Notification(Notification::new("exit".to_string(), Value::Null)));
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

pub fn repository_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

pub fn fixture_path(name: &str) -> PathBuf {
    repository_path().join("tests/fixtures").join(name)
}

pub fn ruby_config(key: &str) -> Option<String> {
    let output = std::process::Command::new("ruby").args(["-e", &format!("print RbConfig::CONFIG[{key:?}]")]).output().ok()?;
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

pub fn rbs_core_path() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("RUBY_LSP_RBS_CORE_PATH") {
        return Some(PathBuf::from(path));
    }
    let output = std::process::Command::new("ruby")
        .args(["-e", "print Gem::Specification.find_by_name('rbs').full_gem_path"])
        .output()
        .ok()?;
    output.status.success().then(|| PathBuf::from(String::from_utf8_lossy(&output.stdout).into_owned()).join("core"))
}

pub fn fake_uri(uri: &str) -> String {
    let segments: Vec<&str> = uri.split('/').collect();
    let start = segments.len().saturating_sub(2);
    format!("file:///{}", segments[start..].join("/"))
}

pub fn range_json(start_line: u32, start_character: u32, end_line: u32, end_character: u32) -> Value {
    json!({
        "start": { "line": start_line, "character": start_character },
        "end": { "line": end_line, "character": end_character },
    })
}
