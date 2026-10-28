use std::path::{Path, PathBuf};

use serde_json::Value;
use type_checker::HoverKind;

use super::support::common::delegate_request_if_needed;
use super::{RequestContext, RequestResult, TextDocumentPositionParams, parse_params, to_result};
use crate::requests::support::implementation_lookup;
use crate::requests::support::interface::LocationLink;
use crate::type_checking::{self, DefinitionTarget, NamedType};

pub fn handle(context: &mut RequestContext<'_>, params: Value) -> RequestResult {
    let params: TextDocumentPositionParams = parse_params(params)?;
    let document = context.store.get(&params.text_document.uri)?;
    if !document.is_ruby_or_erb() {
        return Ok(Value::Null);
    }

    let analysis = context.global_state.type_checking.analysis_for(document);
    let global_state = &*context.global_state;
    let offset = document.parse().offset_of(params.position)?;
    delegate_request_if_needed(global_state, document, offset)?;

    let (Some(analysis), Some(environment)) = (analysis, global_state.type_checking.environment_for(document.uri()))
    else {
        return Ok(Value::Null);
    };
    let Some(hover_entry) = analysis.hover_at(offset) else { return Ok(Value::Null) };
    let shown_type = match &hover_entry.kind {
        HoverKind::Type(shown_type) => shown_type,
        HoverKind::Method { return_type, .. } => return_type,
        HoverKind::Constant { constant_type, .. } => constant_type,
    };

    let definition_target = global_state.type_checking.settings.definition_target;
    let workspace_path = PathBuf::from(global_state.workspace_path());
    let core_directory = global_state.ruby_environment.rbs_core_path.as_deref().map(Path::new);
    let index = global_state.index();
    let mut links: Vec<LocationLink> = Vec::new();
    for named_type in type_checking::referenced_named_types(shown_type) {
        let signature = type_checking::type_declaration(environment, &named_type)
            .map(|location| type_checking::signature_location_link(location, global_state.encoding));
        let implementations: Vec<LocationLink> = match &named_type {
            NamedType::Class { name, .. } => {
                let core_class = type_checking::is_core_class(environment, name, core_directory);
                implementation_lookup::preferred_class_implementations(&index, name, core_class.then_some(&workspace_path))
                    .into_iter()
                    .map(|id| implementation_lookup::implementation_link(index.entry(id)))
                    .collect()
            }
            NamedType::Interface(_) | NamedType::Alias(_) => Vec::new(),
        };
        let chosen: Vec<LocationLink> = match definition_target {
            DefinitionTarget::Implementation if !implementations.is_empty() => implementations,
            DefinitionTarget::Signature if signature.is_none() => implementations,
            DefinitionTarget::Both => implementations.into_iter().chain(signature).collect(),
            DefinitionTarget::Implementation | DefinitionTarget::Signature => signature.into_iter().collect(),
        };
        for link in chosen {
            if !links.contains(&link) {
                links.push(link);
            }
        }
    }
    to_result(links)
}
