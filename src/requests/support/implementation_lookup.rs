use std::path::{Path, PathBuf};

use rbs_environment::Type;
use type_checker::{HoverEntry, HoverKind};

use crate::indexer::{Entry, EntryId, EntryKind, Index};
use crate::requests::support::common::range_from_location;
use crate::requests::support::interface::LocationLink;
use crate::type_checking::{self, NamedType};

/// `None` when the type checker doesn't know what is at the position, so the caller has to fall back to name based
/// lookups. `Some` with no entries means the method or constant is known but has no Ruby implementation
pub fn implementation_entries(index: &mut Index, hover_entry: &HoverEntry) -> Option<Vec<EntryId>> {
    match &hover_entry.kind {
        HoverKind::Method { receiver_type, owner, method_name, singleton, .. } => {
            Some(method_implementations(index, receiver_type, owner, method_name, *singleton))
        }
        HoverKind::Constant { name, .. } => Some(constant_implementations(index, name)),
        HoverKind::Type(_) => None,
    }
}

/// The owner is only a fallback, since the receiver finds overrides in subclasses that the owner in RBS doesn't know about
fn method_implementations(index: &mut Index, receiver_type: &Type, owner: &str, method_name: &str, singleton: bool) -> Vec<EntryId> {
    let mut implementations = Vec::new();
    for named_type in type_checking::named_types(receiver_type) {
        if let NamedType::Class { name, singleton } = named_type {
            for id in resolve_ruby_method(index, method_name, &name, singleton) {
                if !implementations.contains(&id) {
                    implementations.push(id);
                }
            }
        }
    }
    if implementations.is_empty() {
        implementations = resolve_ruby_method(index, method_name, owner.strip_prefix("::").unwrap_or(owner), singleton);
    }
    implementations
}

fn resolve_ruby_method(index: &mut Index, method_name: &str, class_name: &str, singleton: bool) -> Vec<EntryId> {
    let receiver_name = if singleton {
        let unqualified_name = class_name.rsplit("::").next().unwrap_or(class_name);
        format!("{class_name}::<Class:{unqualified_name}>")
    } else {
        class_name.to_string()
    };
    let methods = index.resolve_method(method_name, &receiver_name, false).unwrap_or_default();
    methods.into_iter().filter(|id| is_ruby_entry(index.entry(*id))).collect()
}

fn constant_implementations(index: &Index, name: &str) -> Vec<EntryId> {
    let name = name.strip_prefix("::").unwrap_or(name);
    let entries = index.get(name).unwrap_or_default();
    entries.iter().copied().filter(|id| is_ruby_entry(index.entry(*id))).collect()
}

/// The definitions in the file named after the class come first, since a model reopened in concerns and initializers
/// would otherwise often list those first
pub fn class_implementations(index: &Index, class_name: &str, only_within: Option<&Path>) -> Vec<EntryId> {
    let class_file_path = class_file_path(class_name);
    let mut implementations: Vec<EntryId> = index
        .get(class_name)
        .unwrap_or_default()
        .iter()
        .copied()
        .filter(|id| {
            let entry = index.entry(*id);
            matches!(entry.kind, EntryKind::Namespace(_))
                && is_ruby_entry(entry)
                && only_within.is_none_or(|directory| {
                    crate::uri::to_path_buf(&entry.uri).is_some_and(|path| path.starts_with(directory))
                })
        })
        .collect();
    implementations.sort_by_key(|id| !is_in_file(index.entry(*id), &class_file_path));
    implementations
}

/// Only the definitions in the file named after the class when there is one, so that jumping to the type opens it
/// directly instead of listing every reopening
pub fn preferred_class_implementations(index: &Index, class_name: &str, only_within: Option<&Path>) -> Vec<EntryId> {
    let class_file_path = class_file_path(class_name);
    let implementations = class_implementations(index, class_name, only_within);
    let in_class_file: Vec<EntryId> =
        implementations.iter().copied().filter(|id| is_in_file(index.entry(*id), &class_file_path)).collect();
    if in_class_file.is_empty() { implementations } else { in_class_file }
}

fn class_file_path(class_name: &str) -> PathBuf {
    PathBuf::from(format!("{}.rb", type_checking::underscored_path(class_name)))
}

fn is_in_file(entry: &Entry, file_path: &Path) -> bool {
    crate::uri::to_path_buf(&entry.uri).is_some_and(|path| path.ends_with(file_path))
}

/// The index also holds the core RBS signatures, which are not implementations
fn is_ruby_entry(entry: &Entry) -> bool {
    !entry.uri.ends_with(".rbs")
}

pub fn implementation_link(entry: &Entry) -> LocationLink {
    LocationLink {
        target_uri: entry.uri.to_string(),
        target_range: range_from_location(&entry.location),
        target_selection_range: range_from_location(&entry.name_location()),
    }
}

pub fn implementation_markdown_link(entry: &Entry, workspace_path: &Path) -> String {
    let name_location = entry.name_location();
    let line = name_location.start_line;
    let path = crate::uri::to_path_buf(&entry.uri).map(|path| type_checking::displayed_path(&path, workspace_path));
    format!(
        "[{}:{line}]({}#L{line},{})",
        path.unwrap_or_else(|| entry.file_name()),
        entry.uri,
        name_location.start_column + 1
    )
}
