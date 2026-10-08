use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;
use serde_json::Value;

use super::prepare_rename::{NODE_TYPES, constant_target};
use super::references::{for_each_workspace_document, resolve_constant_target};
use super::support::common::range_from_location;
use super::support::workspace_edit::{DocumentChange, RenameFile, TextDocumentEdit, TextEdit, WorkspaceEdit};
use super::{RequestContext, RequestError, RequestResult, TextDocumentIdentifier, parse_params, to_result};
use crate::global_state::GlobalState;
use crate::indexer::Index;
use crate::indexer::reference_finder::{Target, find_references};
use crate::position::Position;
use crate::prism::constant_name;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RenameParams {
    text_document: TextDocumentIdentifier,
    position: Position,
    new_name: String,
}

pub fn handle(context: &mut RequestContext<'_>, params: Value) -> RequestResult {
    let params: RenameParams = parse_params(params)?;
    let global_state = &*context.global_state;
    let mut index = global_state.index();

    let (name, fully_qualified_name) = {
        let document = context.store.get(&params.text_document.uri)?;
        if !document.is_ruby() {
            return Ok(Value::Null);
        }

        let parsed = document.parse();
        let offset = parsed.offset_of(params.position)?;
        let node_context = parsed.locate(offset, NODE_TYPES);
        let Some(target) = constant_target(&parsed, &node_context, params.position) else { return Ok(Value::Null) };
        let Some(name) = constant_name(&target) else { return Ok(Value::Null) };
        let Some(Target::Constant { fully_qualified_name }) =
            resolve_constant_target(&mut index, &target, &node_context.nesting)
        else {
            return Ok(Value::Null);
        };

        if let Some(conflict_entries) = index.resolve(&params.new_name, &node_context.nesting) {
            let conflict_name =
                conflict_entries.first().map(|id| index.entry(*id).name.clone()).unwrap_or_default();
            return Err(RequestError::RequestFailed(format!("The new name is already in use by {conflict_name}")));
        }
        (name, fully_qualified_name)
    };

    let reference_target = Target::Constant { fully_qualified_name: fully_qualified_name.clone() };
    let mut changes: Vec<(String, Vec<TextEdit>)> = Vec::new();
    for_each_workspace_document(&global_state.workspace_path(), context.store, |uri, source| {
        let edits: Vec<TextEdit> = find_references(&reference_target, &mut index, source, uri, true)
            .into_iter()
            .map(|reference| {
                TextEdit::new(range_from_location(&reference.location), reference.name.replacen(&name, &params.new_name, 1))
            })
            .collect();
        if !edits.is_empty() {
            changes.push((uri.to_string(), edits));
        }
    });

    if !global_state.client_capabilities.supports_rename() {
        return to_result(WorkspaceEdit::from_changes(changes.into_iter().collect::<BTreeMap<_, _>>()));
    }

    let mut document_changes: Vec<DocumentChange> =
        changes.into_iter().map(|(uri, edits)| DocumentChange::Edit(TextDocumentEdit::new(uri, edits))).collect();
    collect_file_renames(global_state, &index, &fully_qualified_name, &params.new_name, &mut document_changes);
    to_result(WorkspaceEdit::from_document_changes(document_changes))
}

fn collect_file_renames(
    global_state: &GlobalState,
    index: &Index,
    fully_qualified_name: &str,
    new_name: &str,
    document_changes: &mut Vec<DocumentChange>,
) {
    let short_name = last_name_part(fully_qualified_name);
    let workspace_path = global_state.workspace_path();
    let Some(entries) = index.get(fully_qualified_name) else { return };

    for id in entries {
        let entry = index.entry(*id);
        let Some(file_path) = entry.file_path() else { continue };
        if !file_path.starts_with(&workspace_path) || !entry.is_constant_like() {
            continue;
        }

        if format!("{}.rb", file_from_constant_name(short_name)) != entry.file_name() {
            continue;
        }

        let new_file_name = file_from_constant_name(last_name_part(new_name));
        let directory = Path::new(&file_path).parent().unwrap_or_else(|| Path::new("/"));
        let new_uri = crate::uri::from_path_buf(&directory.join(format!("{new_file_name}.rb")));
        document_changes.push(DocumentChange::Rename(RenameFile::new(entry.uri.to_string(), new_uri)));
    }
}

/// Mirrors `name.split("::").last`, which ignores trailing empty parts
fn last_name_part(name: &str) -> &str {
    name.split("::").filter(|part| !part.is_empty()).last().unwrap_or("")
}

/// Mirrors `gsub(/([a-z])([A-Z])|([A-Z])([A-Z][a-z])/, '\1\3_\2\4').downcase`
pub fn file_from_constant_name(constant_name: &str) -> String {
    let characters: Vec<char> = constant_name.chars().collect();
    let mut result = String::new();
    let mut index = 0;
    while index < characters.len() {
        let current = characters[index];
        let next = characters.get(index + 1).copied();
        let after_next = characters.get(index + 2).copied();
        if current.is_ascii_lowercase() && next.is_some_and(|character| character.is_ascii_uppercase()) {
            result.push(current);
            result.push('_');
            result.push(next.expect("checked above"));
            index += 2;
        } else if current.is_ascii_uppercase()
            && next.is_some_and(|character| character.is_ascii_uppercase())
            && after_next.is_some_and(|character| character.is_ascii_lowercase())
        {
            result.push(current);
            result.push('_');
            result.push(next.expect("checked above"));
            result.push(after_next.expect("checked above"));
            index += 3;
        } else {
            result.push(current);
            index += 1;
        }
    }
    result.to_lowercase()
}
