use std::collections::{HashMap, HashSet, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crossbeam_channel::{Receiver, Sender, select};
use lsp_server::{Connection, ErrorCode, Message, Notification, Request, RequestId, Response, ResponseError};
use serde_json::{Value, json};

use crate::document::{ContentChange, LanguageId};
use crate::global_state::{GlobalState, message_type};
use crate::indexer::{Index, indexing};
use crate::requests::{self, AUTOMATIC_REQUESTS, Outgoing, RequestContext, RequestError};
use crate::ruby_environment::RubyEnvironment;
use crate::store::Store;

pub const RUBY_LSP_VERSION: &str = "0.26.11";
const INDEXING_PROGRESS_TOKEN: &str = "indexing-progress";

pub mod file_change_type {
    pub const CREATED: u8 = 1;
    pub const CHANGED: u8 = 2;
    pub const DELETED: u8 = 3;
}

#[derive(Debug, Clone, Default)]
pub struct ServerOptions {
    pub test_mode: bool,
}

type IndexingOutcome = Result<Index, String>;

pub struct Server {
    receiver: Receiver<Message>,
    outgoing: Outgoing,
    global_state: GlobalState,
    store: Store,
    options: ServerOptions,
    pending_messages: VecDeque<Message>,
    cancelled_requests: HashSet<RequestId>,
    deferred_watched_file_notifications: Vec<Notification>,
    indexing_sender: Sender<IndexingOutcome>,
    indexing_receiver: Receiver<IndexingOutcome>,
    live_indexed_uris: HashSet<String>,
    next_outgoing_request_id: i32,
    shutdown_requested: bool,
}

impl Server {
    pub fn new(connection: Connection, options: ServerOptions) -> Server {
        let (indexing_sender, indexing_receiver) = crossbeam_channel::unbounded();
        Server {
            receiver: connection.receiver,
            outgoing: Outgoing::new(connection.sender),
            global_state: GlobalState::new(),
            store: Store::default(),
            options,
            pending_messages: VecDeque::new(),
            cancelled_requests: HashSet::new(),
            deferred_watched_file_notifications: Vec::new(),
            indexing_sender,
            indexing_receiver,
            live_indexed_uris: HashSet::new(),
            next_outgoing_request_id: 1,
            shutdown_requested: false,
        }
    }

    pub fn global_state(&self) -> &GlobalState {
        &self.global_state
    }

    pub fn apply_options(&mut self, options: &Value, environment: Arc<RubyEnvironment>) {
        let notifications = self.global_state.apply_options(options, environment);
        self.store.encoding = self.global_state.encoding;
        drop(notifications);
    }

    pub fn global_state_mut(&mut self) -> &mut GlobalState {
        &mut self.global_state
    }

    pub fn run(mut self) -> i32 {
        loop {
            if self.pending_messages.is_empty() {
                select! {
                    recv(self.receiver) -> message => match message {
                        Ok(message) => self.enqueue(message),
                        Err(_) => return if self.shutdown_requested { 0 } else { 1 },
                    },
                    recv(self.indexing_receiver) -> outcome => {
                        if let Ok(outcome) = outcome {
                            self.finish_initial_indexing(outcome);
                        }
                    },
                }
            }

            while let Ok(message) = self.receiver.try_recv() {
                self.enqueue(message);
            }
            while let Ok(outcome) = self.indexing_receiver.try_recv() {
                self.finish_initial_indexing(outcome);
            }

            let Some(message) = self.pending_messages.pop_front() else { continue };
            if let Some(exit_code) = self.process_message(message) {
                return exit_code;
            }
        }
    }

    fn enqueue(&mut self, message: Message) {
        if let Message::Notification(notification) = &message
            && notification.method == "$/cancelRequest"
        {
            self.cancel_request(&notification.params);
            return;
        }
        self.pending_messages.push_back(message);
    }

    fn cancel_request(&mut self, params: &Value) {
        if let Ok(id) = serde_json::from_value::<RequestId>(params["id"].clone()) {
            self.cancelled_requests.insert(id);
        }
    }

    fn process_message(&mut self, message: Message) -> Option<i32> {
        match message {
            Message::Request(request) => {
                if self.shutdown_requested {
                    return None;
                }
                self.process_request(request);
                None
            }
            Message::Notification(notification) => {
                if notification.method == "exit" {
                    return Some(if self.shutdown_requested { 0 } else { 1 });
                }
                if self.shutdown_requested {
                    return None;
                }
                let method = notification.method.clone();
                let outcome = catch_unwind(AssertUnwindSafe(|| self.process_notification(notification)));
                if let Err(payload) = outcome {
                    self.outgoing.log_message(
                        &format!("Error processing {method}: {}", panic_message(&payload)),
                        message_type::ERROR,
                    );
                }
                None
            }
            Message::Response(_) => None,
        }
    }

    fn process_request(&mut self, request: Request) {
        if self.cancelled_requests.remove(&request.id) {
            self.send_error(request.id.clone(), ErrorCode::RequestCanceled as i32, format!("Request {} was cancelled", request.id), None);
            return;
        }

        match request.method.as_str() {
            "initialize" => {
                let id = request.id.clone();
                let outcome = catch_unwind(AssertUnwindSafe(|| self.run_initialize(request)));
                if let Err(payload) = outcome {
                    let message = panic_message(&payload);
                    self.send_error(id, ErrorCode::InternalError as i32, message.clone(), None);
                    self.outgoing.log_message(&format!("Error processing initialize: {message}"), message_type::ERROR);
                }
            }
            "shutdown" => {
                self.outgoing.log("Shutting down Ruby LSP...");
                self.shutdown_requested = true;
                self.pending_messages.retain(|message| matches!(message, Message::Notification(notification) if notification.method == "exit"));
                self.cancelled_requests.clear();
                self.store.clear();
                self.outgoing.send(Message::Response(Response::new_ok(request.id, Value::Null)));
            }
            method => {
                let Some(handler) = requests::handler_for(method) else {
                    self.send_error(request.id, ErrorCode::MethodNotFound as i32, format!("Method not found: {method}"), None);
                    return;
                };

                let id = request.id.clone();
                let method = method.to_string();
                let outcome = catch_unwind(AssertUnwindSafe(|| {
                    self.prepare_document(&method, &request.params);
                    let mut context = RequestContext {
                        global_state: &mut self.global_state,
                        store: &mut self.store,
                        outgoing: &self.outgoing,
                        pending_message_count: self.pending_messages.len(),
                    };
                    handler(&mut context, request.params)
                }));

                match outcome {
                    Ok(Ok(result)) => self.outgoing.send(Message::Response(Response::new_ok(id, result))),
                    Ok(Err(RequestError::Delegate)) => {
                        self.send_error(id, RequestError::Delegate.code(), RequestError::Delegate.message(), None)
                    }
                    Ok(Err(error)) => {
                        self.send_error(id, error.code(), error.message(), None);
                        self.outgoing.log_message(&format!("Error processing {method}: {}", error.message()), message_type::ERROR);
                    }
                    Err(payload) => {
                        let message = panic_message(&payload);
                        let data = json!({ "errorClass": "Panic", "errorMessage": message, "backtrace": "" });
                        self.send_error(id, ErrorCode::InternalError as i32, message.clone(), Some(data));
                        self.outgoing.log_message(&format!("Error processing {method}: {message}"), message_type::ERROR);
                    }
                }
            }
        }
    }

    fn prepare_document(&mut self, method: &str, params: &Value) {
        let Some(uri) = params["textDocument"]["uri"].as_str() else { return };

        let supports_request_delegation = self.global_state.client_capabilities.supports_request_delegation;
        if let Ok(document) = self.store.get(uri)
            && document.take_parse_pending()
            && supports_request_delegation
            && document.language_id() == LanguageId::Erb
        {
            let host_language_source = document.host_language_source().unwrap_or_default().to_string();
            self.outgoing.notify(
                "delegate/textDocument/virtualState",
                json!({ "textDocument": { "uri": uri, "text": host_language_source } }),
            );
        }

        if AUTOMATIC_REQUESTS.contains(&method) {
            self.index_document_if_needed(uri);
        }
    }

    fn index_document_if_needed(&mut self, uri: &str) {
        let Ok(document) = self.store.get(uri) else { return };
        if !document.is_ruby() || !document.indexing_pending() {
            return;
        }
        document.mark_indexing_done();
        if !document.should_index() {
            return;
        }

        let document_uri = document.uri().to_string();
        let source = document.source().to_string();
        self.outgoing.log(&format!("Determined that document should be indexed: {document_uri}"));
        self.global_state.index().handle_change(&document_uri, |index| {
            index.delete(&document_uri, true);
            indexing::index_single(index, &document_uri, &source, true);
        });
        self.live_indexed_uris.insert(document_uri);
    }

    fn process_notification(&mut self, notification: Notification) {
        match notification.method.as_str() {
            "initialized" => self.run_initialized(),
            "textDocument/didOpen" => self.text_document_did_open(&notification.params),
            "textDocument/didClose" => self.text_document_did_close(&notification.params),
            "textDocument/didChange" => self.text_document_did_change(&notification.params),
            "workspace/didChangeWatchedFiles" => self.workspace_did_change_watched_files(notification),
            "$/cancelRequest" => self.cancel_request(&notification.params),
            _ => {}
        }
    }

    fn send_error(&self, id: RequestId, code: i32, message: String, data: Option<Value>) {
        self.outgoing.send(Message::Response(Response { id, response_result: Err(ResponseError { code, message, data }) }));
    }

    fn send_request(&mut self, method: &str, params: Value) {
        let id = RequestId::from(self.next_outgoing_request_id);
        self.next_outgoing_request_id += 1;
        self.outgoing.send(Message::Request(Request::new(id, method.to_string(), params)));
    }

    fn run_initialize(&mut self, request: Request) {
        let options = request.params;
        self.outgoing.log(&format!(
            "Initializing Ruby LSP v{RUBY_LSP_VERSION} (ruby-lsp-rs v{}) https://github.com/Shopify/ruby-lsp/releases/tag/v{RUBY_LSP_VERSION}....",
            env!("CARGO_PKG_VERSION")
        ));

        let workspace_path = GlobalState::workspace_path_from_options(&options)
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")));
        let environment = self.detect_ruby_environment(&workspace_path, &options["initializationOptions"]["rubyCommand"]);
        let environment_errors = environment.errors.clone();
        let global_state_notifications = self.global_state.apply_options(&options, Arc::new(environment));
        self.store.encoding = self.global_state.encoding;

        if let Some(client_name) = options["clientInfo"]["name"].as_str() {
            self.store.client_name = client_name.to_string();
        }

        let enabled_features = EnabledFeatures::from_configuration(&options["initializationOptions"]["enabledFeatures"]);
        let bundle_env = read_bundle_env(Path::new(&self.global_state.workspace_path()));
        let response = json!({
            "capabilities": self.server_capabilities(&enabled_features),
            "serverInfo": { "name": "Ruby LSP", "version": RUBY_LSP_VERSION },
            "formatter": self.global_state.formatter,
            "degraded_mode": false,
            "bundle_env": bundle_env,
        });
        self.outgoing.send(Message::Response(Response::new_ok(request.id, response)));

        if self.global_state.supports_watching_files() {
            self.register_watched_files("**/*.rb", "workspace-watcher");
            let rubocop_pattern = json!({
                "baseUri": self.global_state.workspace_uri,
                "pattern": "{.rubocop.yml,.rubocop,.rubocop_todo.yml}",
            });
            self.register_watched_files(rubocop_pattern, "rubocop-watcher");
        }

        self.process_indexing_configuration(&options["initializationOptions"]["indexing"]);
        self.begin_progress(INDEXING_PROGRESS_TOKEN, "Ruby LSP: indexing files");

        for notification in global_state_notifications {
            self.outgoing.notify(&notification.method, notification.params);
        }
        for error in environment_errors {
            self.outgoing.log_message(&error, message_type::WARNING);
        }
    }

    fn detect_ruby_environment(&self, workspace_path: &Path, ruby_command: &Value) -> RubyEnvironment {
        if self.options.test_mode {
            return RubyEnvironment::default();
        }
        let command: Vec<String> = ruby_command
            .as_array()
            .map(|parts| parts.iter().filter_map(|part| part.as_str().map(str::to_string)).collect())
            .filter(|parts: &Vec<String>| !parts.is_empty())
            .unwrap_or_else(|| vec!["ruby".to_string()]);
        match RubyEnvironment::detect(workspace_path, &command) {
            Ok(environment) => environment,
            Err(error) => {
                self.outgoing.log_message(&format!("Failed to detect the Ruby environment: {error}"), message_type::ERROR);
                RubyEnvironment::default()
            }
        }
    }

    fn server_capabilities(&self, enabled: &EnabledFeatures) -> Value {
        let has_type_checker = self.global_state.has_type_checker;
        let mut capabilities = serde_json::Map::new();
        let mut set = |key: &str, value: Value| {
            if !matches!(value, Value::Null | Value::Bool(false)) {
                capabilities.insert(key.to_string(), value);
            }
        };

        set("textDocumentSync", json!({ "change": 2, "openClose": true }));
        set("positionEncoding", json!(self.global_state.encoding_name()));
        set("selectionRangeProvider", json!(enabled.is_enabled("selectionRanges")));
        set("hoverProvider", enabled.provider("hover", json!({})));
        set("documentSymbolProvider", enabled.provider("documentSymbols", json!({})));
        set("documentLinkProvider", enabled.provider("documentLink", json!({ "resolveProvider": false })));
        set("foldingRangeProvider", enabled.provider("foldingRanges", json!(true)));
        set("semanticTokensProvider", enabled.provider("semanticHighlighting", semantic_tokens_provider()));
        set(
            "documentFormattingProvider",
            json!(enabled.is_enabled("formatting") && self.global_state.formatter != "none"),
        );
        set("documentHighlightProvider", json!(enabled.is_enabled("documentHighlights")));
        set("codeActionProvider", enabled.provider("codeActions", json!({ "documentSelector": null, "resolveProvider": true })));
        set(
            "documentOnTypeFormattingProvider",
            enabled.provider(
                "onTypeFormatting",
                json!({ "documentSelector": null, "firstTriggerCharacter": "{", "moreTriggerCharacter": ["\n", "|", "d"] }),
            ),
        );
        set(
            "diagnosticProvider",
            enabled.provider(
                "diagnostics",
                json!({ "documentSelector": null, "interFileDependencies": false, "workspaceDiagnostics": false }),
            ),
        );
        set("inlayHintProvider", enabled.provider("inlayHint", json!({ "resolveProvider": false })));
        set(
            "completionProvider",
            enabled.provider(
                "completion",
                json!({
                    "resolveProvider": true,
                    "triggerCharacters": ["/", "\"", "'", ":", "@", ".", "=", "<", "$"],
                    "completionItem": { "labelDetailsSupport": true },
                }),
            ),
        );
        set("codeLensProvider", enabled.provider("codeLens", json!({ "resolveProvider": true })));
        set("definitionProvider", json!(enabled.is_enabled("definition")));
        set("workspaceSymbolProvider", json!(enabled.is_enabled("workspaceSymbol") && !has_type_checker));
        set("signatureHelpProvider", enabled.provider("signatureHelp", json!({ "triggerCharacters": ["(", " ", ","] })));
        set("typeHierarchyProvider", enabled.provider("typeHierarchy", json!({})));
        set("renameProvider", if has_type_checker { Value::Null } else { json!({ "prepareProvider": true }) });
        set("referencesProvider", json!(!has_type_checker));
        set("documentRangeFormattingProvider", json!(true));
        set(
            "experimental",
            json!({ "addon_detection": true, "compose_bundle": true, "go_to_relevant_file": true, "full_test_discovery": true }),
        );
        Value::Object(capabilities)
    }

    fn register_watched_files(&mut self, pattern: impl Into<Value>, registration_id: &str) {
        let params = json!({
            "registrations": [{
                "id": registration_id,
                "method": "workspace/didChangeWatchedFiles",
                "registerOptions": { "watchers": [{ "globPattern": pattern.into(), "kind": 7 }] },
            }],
        });
        self.send_request("client/registerCapability", params);
    }

    fn process_indexing_configuration(&mut self, indexing_options: &Value) {
        let workspace_path = PathBuf::from(self.global_state.workspace_path());
        let environment = self.global_state.ruby_environment.clone();
        let index_configuration_path = workspace_path.join(".index.yml");
        let mut messages: Vec<(String, u8)> = Vec::new();
        {
            let mut index = self.global_state.index();
            let configuration = Arc::make_mut(&mut index.configuration);
            configuration.workspace_path = workspace_path;
            configuration.initialize_from_environment(&environment);

            if index_configuration_path.exists() {
                let parsed = std::fs::read_to_string(&index_configuration_path)
                    .map_err(|error| error.to_string())
                    .and_then(|content| serde_yaml::from_str::<Value>(&content).map_err(|error| error.to_string()));
                match parsed {
                    Ok(yaml) => {
                        if let Err(error) = configuration.apply_config(&yaml) {
                            messages.push((error, message_type::ERROR));
                        }
                        messages.push((
                            "The .index.yml configuration file is deprecated. Please use editor settings to configure the index"
                                .to_string(),
                            message_type::WARNING,
                        ));
                    }
                    Err(error) => {
                        messages.push((format!("Syntax error while loading configuration: {error}"), message_type::WARNING))
                    }
                }
            } else if indexing_options.is_object()
                && let Err(error) = configuration.apply_config(indexing_options)
            {
                messages.push((error, message_type::ERROR));
            }
        }
        for (message, message_type) in messages {
            self.outgoing.show_message(&message, message_type);
        }
    }

    fn run_initialized(&mut self) {
        if !self.options.test_mode {
            self.outgoing.log("Finished initializing Ruby LSP!");
        }
        self.perform_initial_indexing();
        self.check_formatter_is_available();
    }

    fn check_formatter_is_available(&mut self) {
        if self.global_state.formatter != "rubocop_internal" || self.global_state.ruby_environment.rubocop_version.is_some() {
            return;
        }
        self.global_state.formatter = "none".to_string();
        self.outgoing.show_message(
            "Ruby LSP formatter is set to `rubocop_internal` but RuboCop was not found in the Gemfile or gemspec.",
            message_type::ERROR,
        );
    }

    fn perform_initial_indexing(&mut self) {
        let configuration = self.global_state.index().configuration.clone();
        let environment = self.global_state.ruby_environment.clone();
        let outgoing = self.outgoing.clone();
        let supports_progress = self.global_state.client_capabilities.supports_progress;
        let indexing_sender = self.indexing_sender.clone();

        std::thread::spawn(move || {
            let outcome = catch_unwind(AssertUnwindSafe(|| {
                let paths = configuration.indexable_paths(&environment);
                let mut index = Index::new(configuration);
                let last_reported = std::sync::atomic::AtomicU32::new(0);
                let report_progress = |percentage: u32| {
                    if supports_progress && last_reported.swap(percentage, std::sync::atomic::Ordering::Relaxed) != percentage {
                        outgoing.notify("$/progress", progress_report(INDEXING_PROGRESS_TOKEN, percentage));
                    }
                };
                indexing::index_all(&mut index, &paths, environment.rbs_core_path.as_deref(), &report_progress);
                index
            }));
            let _ = indexing_sender.send(outcome.map_err(|payload| panic_message(&payload)));
        });
    }

    /// The initial index is built separately so that requests keep being answered while indexing. Once it's done, it
    /// replaces the live index and documents that were indexed from the editor's unsaved state are indexed again
    fn finish_initial_indexing(&mut self, outcome: IndexingOutcome) {
        match outcome {
            Ok(mut new_index) => {
                let live_uris: Vec<String> =
                    self.live_indexed_uris.iter().filter(|uri| self.store.contains(uri)).cloned().collect();
                for uri in live_uris {
                    let Some(document) = self.store.get_open(&uri) else { continue };
                    let source = document.source().to_string();
                    new_index.delete(&uri, true);
                    indexing::index_single(&mut new_index, &uri, &source, true);
                }
                new_index.initial_indexing_completed = true;
                new_index.clear_ancestors();
                *self.global_state.index() = new_index;
            }
            Err(message) => {
                let mut index = self.global_state.index();
                index.initial_indexing_completed = true;
                index.clear_ancestors();
                drop(index);
                self.outgoing.show_message(
                    &format!(
                        "Error while indexing (see [troubleshooting steps](https://shopify.github.io/ruby-lsp/troubleshooting#indexing)): {message}"
                    ),
                    message_type::ERROR,
                );
            }
        }

        for (_, document) in self.store.iter_mut() {
            document.clear_cache("textDocument/codeLens");
        }
        self.end_progress(INDEXING_PROGRESS_TOKEN);

        if self.global_state.client_capabilities.supports_code_lens_refresh {
            self.send_request("workspace/codeLens/refresh", Value::Null);
        }

        for notification in std::mem::take(&mut self.deferred_watched_file_notifications) {
            self.pending_messages.push_back(Message::Notification(notification));
        }
    }

    fn begin_progress(&mut self, token: &str, title: &str) {
        if !self.global_state.client_capabilities.supports_progress {
            return;
        }
        self.send_request("window/workDoneProgress/create", json!({ "token": token }));
        self.outgoing.notify(
            "$/progress",
            json!({ "token": token, "value": { "kind": "begin", "title": title, "percentage": 0, "message": "0% completed" } }),
        );
    }

    fn end_progress(&mut self, token: &str) {
        if !self.global_state.client_capabilities.supports_progress {
            return;
        }
        self.outgoing.notify("$/progress", json!({ "token": token, "value": { "kind": "end" } }));
    }

    fn text_document_did_open(&mut self, params: &Value) {
        let text_document = &params["textDocument"];
        let Some(uri) = text_document["uri"].as_str() else { return };
        let language_id = LanguageId::from_client_language_id(text_document["languageId"].as_str().unwrap_or("ruby"));
        let source = text_document["text"].as_str().unwrap_or_default().to_string();
        let version = text_document["version"].as_i64().unwrap_or(0) as i32;
        let document = self.store.set(uri, source, version, language_id);

        if document.past_expensive_limit() && crate::uri::scheme(uri) == "file" {
            let path = crate::uri::to_path(uri).unwrap_or_default();
            self.outgoing.log_message(
                &format!(
                    "The file {path} is too long. For performance reasons, semantic highlighting and\ndiagnostics will be disabled.\n"
                ),
                message_type::WARNING,
            );
        }
    }

    fn text_document_did_close(&mut self, params: &Value) {
        let Some(uri) = params["textDocument"]["uri"].as_str() else { return };
        self.store.delete(uri);
        self.outgoing.notify("textDocument/publishDiagnostics", json!({ "uri": uri, "diagnostics": [] }));
    }

    fn text_document_did_change(&mut self, params: &Value) {
        let Some(uri) = params["textDocument"]["uri"].as_str() else { return };
        let version = params["textDocument"]["version"].as_i64().unwrap_or(0) as i32;
        let edits: Vec<ContentChange> = serde_json::from_value(params["contentChanges"].clone()).unwrap_or_default();
        if let Err(error) = self.store.push_edits(uri, &edits, version) {
            self.outgoing.log_message(&format!("Error processing textDocument/didChange: {error}"), message_type::ERROR);
        }
    }

    fn workspace_did_change_watched_files(&mut self, notification: Notification) {
        if !self.global_state.index().initial_indexing_completed {
            self.deferred_watched_file_notifications.push(notification);
            return;
        }

        let mut changes: Vec<Value> = notification.params["changes"].as_array().cloned().unwrap_or_default();
        let mut seen = HashSet::new();
        changes.retain(|change| seen.insert(change.to_string()));

        for change in changes {
            let Some(uri) = change["uri"].as_str() else { continue };
            let Some(file_path) = crate::uri::to_path(uri) else { continue };
            if Path::new(&file_path).is_dir() {
                continue;
            }
            if file_path.ends_with(".rb") {
                let change_type = change["type"].as_u64().unwrap_or(0) as u8;
                self.handle_ruby_file_change(&file_path, change_type);
                continue;
            }
            let file_name = Path::new(&file_path).file_name().and_then(|name| name.to_str()).unwrap_or_default();
            if matches!(file_name, ".rubocop.yml" | ".rubocop" | ".rubocop_todo.yml") {
                self.handle_rubocop_config_change();
            }
        }
    }

    fn handle_rubocop_config_change(&mut self) {
        if !requests::support::formatter::reload_rubocop_configuration(&self.global_state, &self.outgoing) {
            return;
        }
        for (_, document) in self.store.iter_mut() {
            document.clear_cache(requests::diagnostics::CACHE_KEY);
        }
        if self.global_state.client_capabilities.supports_diagnostic_refresh {
            self.send_request("workspace/diagnostic/refresh", Value::Null);
        }
    }

    fn handle_ruby_file_change(&mut self, file_path: &str, change_type: u8) {
        let load_path_entry =
            self.global_state.ruby_environment.load_path.iter().find(|load_path| file_path.starts_with(load_path.as_str())).cloned();
        let uri = crate::uri::from_path(file_path);
        let require_path = load_path_entry.map(|entry| crate::uri::require_path(file_path, &entry));
        let managed_by_client = self.store.contains(&uri);

        let read_content = || std::fs::read(file_path).ok().map(|bytes| String::from_utf8_lossy(&bytes).into_owned());
        let index_from_disk = |index: &mut Index, content: &str| {
            index.delete(&uri, false);
            indexing::index_single_with_require_path(index, Arc::from(uri.as_str()), content.as_bytes(), true, require_path.as_deref());
        };

        match change_type {
            file_change_type::CREATED => {
                let Some(content) = read_content() else { return };
                let mut index = self.global_state.index();
                if managed_by_client {
                    index.handle_change(&uri, |index| index_from_disk(index, &content));
                } else {
                    index_from_disk(&mut index, &content);
                }
            }
            file_change_type::CHANGED => {
                if managed_by_client {
                    return;
                }
                let Some(content) = read_content() else { return };
                self.global_state.index().handle_change(&uri, |index| index_from_disk(index, &content));
            }
            file_change_type::DELETED => self.global_state.index().delete(&uri, false),
            _ => {}
        }
    }
}

struct EnabledFeatures {
    features: HashMap<String, bool>,
    default: bool,
}

impl EnabledFeatures {
    fn from_configuration(configuration: &Value) -> EnabledFeatures {
        match configuration {
            Value::Array(features) => EnabledFeatures {
                features: features.iter().filter_map(|feature| feature.as_str().map(|name| (name.to_string(), true))).collect(),
                default: false,
            },
            Value::Object(features) => EnabledFeatures {
                features: features
                    .iter()
                    .map(|(name, value)| (name.clone(), !matches!(value, Value::Null | Value::Bool(false))))
                    .collect(),
                default: true,
            },
            _ => EnabledFeatures { features: HashMap::new(), default: true },
        }
    }

    fn is_enabled(&self, feature: &str) -> bool {
        self.features.get(feature).copied().unwrap_or(self.default)
    }

    fn provider(&self, feature: &str, provider: Value) -> Value {
        if self.is_enabled(feature) { provider } else { Value::Null }
    }
}

fn semantic_tokens_provider() -> Value {
    json!({
        "documentSelector": null,
        "legend": {
            "tokenTypes": [
                "namespace", "type", "class", "enum", "interface", "struct", "typeParameter", "parameter", "variable",
                "property", "enumMember", "event", "function", "method", "macro", "keyword", "modifier", "comment",
                "string", "number", "regexp", "operator", "decorator"
            ],
            "tokenModifiers": [
                "declaration", "definition", "readonly", "static", "deprecated", "abstract", "async", "modification",
                "documentation", "defaultLibrary"
            ],
        },
        "range": true,
        "full": { "delta": true },
    })
}

fn progress_report(token: &str, percentage: u32) -> Value {
    json!({
        "token": token,
        "value": { "kind": "report", "percentage": percentage, "message": format!("{percentage}% completed") },
    })
}

fn read_bundle_env(workspace_path: &Path) -> Value {
    let bundle_env_path = workspace_path.join(".ruby-lsp").join("bundle_env");
    let Ok(content) = std::fs::read_to_string(&bundle_env_path) else { return Value::Null };
    let _ = std::fs::remove_file(&bundle_env_path);
    let environment: serde_json::Map<String, Value> = content
        .lines()
        .filter_map(|line| line.split_once('=').map(|(key, value)| (key.to_string(), Value::String(value.to_string()))))
        .collect();
    Value::Object(environment)
}

fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        message.to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "Unknown panic".to_string()
    }
}
