use serde_json::Value;

use super::support::common::{delegate_request_if_needed, range_from_location};
use super::{RequestContext, RequestResult, TextDocumentPositionParams, parse_params, to_result};
use crate::document::LanguageId;
use crate::indexer::{EntryKind, Index};
use crate::requests::support::interface::LocationLink;
use crate::type_checking::DefinitionTarget;
use crate::type_checking::worker::GotoKind;

pub fn handle(context: &mut RequestContext<'_>, params: Value) -> RequestResult {
    let params: TextDocumentPositionParams = parse_params(params)?;
    let document = context.store.get(&params.text_document.uri)?;
    let global_state = &*context.global_state;
    if document.language_id() != LanguageId::Rbs {
        if !document.is_ruby_or_erb() {
            return Ok(Value::Null);
        }
        let offset = document.parse().offset_of(params.position)?;
        delegate_request_if_needed(global_state, document, offset)?;
    }

    let Some(answer) = global_state
        .type_checking
        .goto(GotoKind::TypeDefinition, document.uri(), params.position)
    else {
        return Ok(Value::Null);
    };
    let index = global_state.index();
    let implementations: Vec<LocationLink> = answer
        .type_names
        .iter()
        .flat_map(|class_name| class_implementations(&index, class_name))
        .collect();
    let links = match global_state.type_checking.settings.definition_target {
        DefinitionTarget::Implementation if !implementations.is_empty() => implementations,
        DefinitionTarget::Signature if answer.links.is_empty() => implementations,
        DefinitionTarget::Both => implementations.into_iter().chain(answer.links).collect(),
        DefinitionTarget::Implementation | DefinitionTarget::Signature => answer.links,
    };
    let mut unique: Vec<LocationLink> = Vec::new();
    for link in links {
        if !unique.contains(&link) {
            unique.push(link);
        }
    }
    to_result(unique)
}

pub fn class_implementations(index: &Index, class_name: &str) -> Vec<LocationLink> {
    let mut implementations: Vec<LocationLink> = index
        .get(class_name)
        .unwrap_or_default()
        .iter()
        .map(|id| index.entry(*id))
        .filter(|entry| matches!(entry.kind, EntryKind::Namespace(_)) && !entry.uri.ends_with(".rbs"))
        .map(|entry| LocationLink {
            target_uri: entry.uri.to_string(),
            target_range: range_from_location(&entry.location),
            target_selection_range: range_from_location(&entry.name_location()),
        })
        .collect();
    implementations.sort_by(|left, right| {
        (
            &left.target_uri,
            left.target_range.start.line,
            left.target_range.start.character,
        )
            .cmp(&(
                &right.target_uri,
                right.target_range.start.line,
                right.target_range.start.character,
            ))
    });
    implementations
}
