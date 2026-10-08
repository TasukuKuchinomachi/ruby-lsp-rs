pub mod support;

pub mod code_action_resolve;
pub mod code_actions;
pub mod code_lens;
pub mod code_lens_resolve;
pub mod completion;
pub mod completion_resolve;
pub mod compose_bundle;
pub mod definition;
pub mod diagnose_state;
pub mod diagnostics;
pub mod discover_tests;
pub mod document_highlight;
pub mod document_link;
pub mod document_symbol;
pub mod folding_ranges;
pub mod formatting;
pub mod go_to_relevant_file;
pub mod hover;
pub mod inlay_hints;
pub mod on_type_formatting;
pub mod prepare_rename;
pub mod prepare_type_hierarchy;
pub mod range_formatting;
pub mod references;
pub mod rename;
pub mod resolve_test_commands;
pub mod selection_ranges;
pub mod semantic_highlighting;
pub mod show_syntax_tree;
pub mod signature_help;
pub mod type_hierarchy_subtypes;
pub mod type_hierarchy_supertypes;
pub mod workspace_addons;
pub mod workspace_dependencies;
pub mod workspace_symbol;

use crossbeam_channel::Sender;
use lsp_server::{Message, Notification};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::global_state::{GlobalState, message_type};
use crate::position::{InvalidLocationError, Position, Range};
use crate::store::{NonExistingDocumentError, Store};

pub const DELEGATE_REQUEST_ERROR_CODE: i32 = -32900;

pub const AUTOMATIC_REQUESTS: &[&str] =
    &["textDocument/documentSymbol", "textDocument/documentLink", "textDocument/codeLens", "textDocument/foldingRange"];

#[derive(Debug, Clone, PartialEq)]
pub enum RequestError {
    InvalidParams(String),
    RequestFailed(String),
    Delegate,
    Internal(String),
}

impl RequestError {
    pub fn code(&self) -> i32 {
        match self {
            RequestError::InvalidParams(_) => lsp_server::ErrorCode::InvalidParams as i32,
            RequestError::RequestFailed(_) => lsp_server::ErrorCode::RequestFailed as i32,
            RequestError::Delegate => DELEGATE_REQUEST_ERROR_CODE,
            RequestError::Internal(_) => lsp_server::ErrorCode::InternalError as i32,
        }
    }

    pub fn message(&self) -> String {
        match self {
            RequestError::InvalidParams(message) | RequestError::RequestFailed(message) | RequestError::Internal(message) => {
                message.clone()
            }
            RequestError::Delegate => "DELEGATE_REQUEST".to_string(),
        }
    }
}

impl From<NonExistingDocumentError> for RequestError {
    fn from(error: NonExistingDocumentError) -> Self {
        RequestError::InvalidParams(format!("Non existing document: {}", error.0))
    }
}

impl From<InvalidLocationError> for RequestError {
    fn from(error: InvalidLocationError) -> Self {
        RequestError::InvalidParams(error.to_string())
    }
}

pub type RequestResult = Result<Value, RequestError>;
pub type RequestHandler = fn(&mut RequestContext<'_>, Value) -> RequestResult;

#[derive(Clone)]
pub struct Outgoing {
    sender: Sender<Message>,
}

impl Outgoing {
    pub fn new(sender: Sender<Message>) -> Outgoing {
        Outgoing { sender }
    }

    pub fn send(&self, message: Message) {
        let _ = self.sender.send(message);
    }

    pub fn notify(&self, method: &str, params: Value) {
        self.send(Message::Notification(Notification::new(method.to_string(), params)));
    }

    pub fn log_message(&self, message: &str, message_type: u8) {
        self.notify("window/logMessage", json!({ "type": message_type, "message": message }));
    }

    pub fn show_message(&self, message: &str, message_type: u8) {
        self.notify("window/showMessage", json!({ "type": message_type, "message": message }));
    }

    pub fn log(&self, message: &str) {
        self.log_message(message, message_type::LOG);
    }
}

pub struct RequestContext<'a> {
    pub global_state: &'a mut GlobalState,
    pub store: &'a mut Store,
    pub outgoing: &'a Outgoing,
    pub pending_message_count: usize,
}

pub fn parse_params<T: DeserializeOwned>(params: Value) -> Result<T, RequestError> {
    serde_json::from_value(params).map_err(|error| RequestError::InvalidParams(format!("Invalid params: {error}")))
}

pub fn to_result<T: serde::Serialize>(value: T) -> RequestResult {
    serde_json::to_value(value).map_err(|error| RequestError::Internal(error.to_string()))
}

#[derive(Debug, Clone, Deserialize)]
pub struct TextDocumentIdentifier {
    pub uri: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextDocumentParams {
    pub text_document: TextDocumentIdentifier,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextDocumentPositionParams {
    pub text_document: TextDocumentIdentifier,
    pub position: Position,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextDocumentRangeParams {
    pub text_document: TextDocumentIdentifier,
    pub range: Range,
}

pub fn handler_for(method: &str) -> Option<RequestHandler> {
    let handler: RequestHandler = match method {
        "textDocument/selectionRange" => selection_ranges::handle,
        "textDocument/documentSymbol" => document_symbol::handle,
        "textDocument/documentLink" => document_link::handle,
        "textDocument/codeLens" => code_lens::handle,
        "codeLens/resolve" => code_lens_resolve::handle,
        "textDocument/semanticTokens/full" => semantic_highlighting::handle_full,
        "textDocument/semanticTokens/full/delta" => semantic_highlighting::handle_delta,
        "textDocument/semanticTokens/range" => semantic_highlighting::handle_range,
        "textDocument/foldingRange" => folding_ranges::handle,
        "textDocument/formatting" => formatting::handle,
        "textDocument/rangeFormatting" => range_formatting::handle,
        "textDocument/documentHighlight" => document_highlight::handle,
        "textDocument/onTypeFormatting" => on_type_formatting::handle,
        "textDocument/hover" => hover::handle,
        "textDocument/inlayHint" => inlay_hints::handle,
        "textDocument/codeAction" => code_actions::handle,
        "codeAction/resolve" => code_action_resolve::handle,
        "textDocument/diagnostic" => diagnostics::handle,
        "textDocument/completion" => completion::handle,
        "completionItem/resolve" => completion_resolve::handle,
        "textDocument/signatureHelp" => signature_help::handle,
        "textDocument/definition" => definition::handle,
        "textDocument/prepareTypeHierarchy" => prepare_type_hierarchy::handle,
        "textDocument/rename" => rename::handle,
        "textDocument/prepareRename" => prepare_rename::handle,
        "textDocument/references" => references::handle,
        "typeHierarchy/supertypes" => type_hierarchy_supertypes::handle,
        "typeHierarchy/subtypes" => type_hierarchy_subtypes::handle,
        "workspace/symbol" => workspace_symbol::handle,
        "rubyLsp/textDocument/showSyntaxTree" => show_syntax_tree::handle,
        "rubyLsp/workspace/dependencies" => workspace_dependencies::handle,
        "rubyLsp/workspace/addons" => workspace_addons::handle,
        "rubyLsp/composeBundle" => compose_bundle::handle,
        "rubyLsp/diagnoseState" => diagnose_state::handle,
        "rubyLsp/discoverTests" => discover_tests::handle,
        "rubyLsp/resolveTestCommands" => resolve_test_commands::handle,
        "experimental/goToRelevantFile" => go_to_relevant_file::handle,
        _ => return None,
    };
    Some(handler)
}
