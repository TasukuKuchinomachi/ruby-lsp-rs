use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::Value;

use crate::position::{Position, Range};

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TextEdit {
    pub range: Range,
    pub new_text: String,
}

impl TextEdit {
    pub fn new(range: Range, new_text: impl Into<String>) -> TextEdit {
        TextEdit { range, new_text: new_text.into() }
    }

    pub fn insert(position: Position, new_text: impl Into<String>) -> TextEdit {
        TextEdit::new(Range::new(position, position), new_text)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct VersionedTextDocumentIdentifier {
    pub uri: String,
    pub version: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TextDocumentEdit {
    pub text_document: VersionedTextDocumentIdentifier,
    pub edits: Vec<TextEdit>,
}

impl TextDocumentEdit {
    pub fn new(uri: impl Into<String>, edits: Vec<TextEdit>) -> TextDocumentEdit {
        TextDocumentEdit { text_document: VersionedTextDocumentIdentifier { uri: uri.into(), version: None }, edits }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RenameFile {
    pub kind: &'static str,
    pub old_uri: String,
    pub new_uri: String,
}

impl RenameFile {
    pub fn new(old_uri: impl Into<String>, new_uri: impl Into<String>) -> RenameFile {
        RenameFile { kind: "rename", old_uri: old_uri.into(), new_uri: new_uri.into() }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum DocumentChange {
    Edit(TextDocumentEdit),
    Rename(RenameFile),
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceEdit {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub changes: Option<BTreeMap<String, Vec<TextEdit>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub document_changes: Option<Vec<DocumentChange>>,
}

impl WorkspaceEdit {
    pub fn from_changes(changes: BTreeMap<String, Vec<TextEdit>>) -> WorkspaceEdit {
        WorkspaceEdit { changes: Some(changes), document_changes: None }
    }

    pub fn from_document_changes(document_changes: Vec<DocumentChange>) -> WorkspaceEdit {
        WorkspaceEdit { changes: None, document_changes: Some(document_changes) }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CodeAction {
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edit: Option<WorkspaceEdit>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}
