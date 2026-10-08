use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::support::common::{kind_for_entry, range_from_location};
use super::support::interface::Location;
use super::{RequestContext, RequestResult, parse_params, to_result};

#[derive(Debug, Clone, Default, Deserialize)]
struct WorkspaceSymbolParams {
    #[serde(default)]
    query: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSymbol {
    pub name: String,
    pub container_name: String,
    pub kind: u32,
    pub location: Location,
}

pub fn handle(context: &mut RequestContext<'_>, params: Value) -> RequestResult {
    let params: WorkspaceSymbolParams = if params.is_null() { WorkspaceSymbolParams::default() } else { parse_params(params)? };
    let global_state = &*context.global_state;
    let index = global_state.index();

    let ids = index.fuzzy_search(params.query.as_deref(), |entry| {
        let in_dependencies = crate::uri::to_path(&entry.uri).is_some_and(|path| !global_state.not_in_dependencies(&path));
        !in_dependencies && !entry.is_private()
    });

    let symbols: Vec<WorkspaceSymbol> = ids
        .into_iter()
        .map(|id| {
            let entry = index.entry(id);
            WorkspaceSymbol {
                name: entry.name.clone(),
                container_name: container_name(&entry.name),
                kind: kind_for_entry(&index, id),
                location: Location { uri: entry.uri.to_string(), range: range_from_location(&entry.location) },
            }
        })
        .collect();
    to_result(symbols)
}

fn container_name(name: &str) -> String {
    let mut parts: Vec<&str> = name.split("::").collect();
    while parts.last().is_some_and(|part| part.is_empty()) {
        parts.pop();
    }
    parts.pop();
    parts.join("::")
}
