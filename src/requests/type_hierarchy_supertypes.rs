use indexmap::IndexSet;
use serde_json::Value;

use super::prepare_type_hierarchy::TypeHierarchyItem;
use super::support::common::{kind_for_entry, range_from_location};
use super::{RequestContext, RequestResult, to_result};
use crate::indexer::EntryId;

pub fn handle(context: &mut RequestContext<'_>, params: Value) -> RequestResult {
    let name = params["item"]["name"].as_str().unwrap_or_default();
    let mut index = context.global_state.index();
    let entries = match index.get_owned(name) {
        Some(entries) if !entries.is_empty() => entries,
        _ => return Ok(Value::Null),
    };

    let mut parents: IndexSet<EntryId> = IndexSet::new();
    for id in entries {
        let Some(namespace) = index.entry(id).namespace().cloned() else { continue };

        if index.entry(id).is_class()
            && let Some(parent_class_name) = &namespace.parent_class
            && let Some(resolved_parent_entries) = index.resolve(parent_class_name, &namespace.nesting)
        {
            parents.extend(resolved_parent_entries.into_iter().filter(|parent| index.entry(*parent).is_class()));
        }

        for mixin_operation in &namespace.mixin_operations {
            let Some(resolved_mixin_entries) = index.resolve(mixin_operation.module_name(), &namespace.nesting) else {
                continue;
            };
            parents.extend(resolved_mixin_entries.into_iter().filter(|mixin| index.entry(*mixin).is_module()));
        }
    }

    let items: Vec<TypeHierarchyItem> = parents
        .into_iter()
        .map(|id| {
            let entry = index.entry(id);
            TypeHierarchyItem {
                name: entry.name.clone(),
                kind: kind_for_entry(&index, id),
                uri: entry.uri.to_string(),
                range: range_from_location(&entry.location),
                selection_range: range_from_location(&entry.name_location()),
                detail: Some(entry.file_name()),
            }
        })
        .collect();
    to_result(items)
}
