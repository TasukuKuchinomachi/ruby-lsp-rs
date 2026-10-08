use std::collections::HashMap;
use std::sync::Arc;

use indexmap::IndexMap;

use super::configuration::Configuration;
use crate::position::Location;
use super::entry::{Entry, EntryId, EntryKind, MethodAliasTarget, MethodAliasTargetKind, ModuleOperation, Namespace, NamespaceKind};
use super::jaro_winkler;
use super::prefix_tree::PrefixTree;

/// Ruby LSP stores indexer columns through Prism's `code_units_offset`, which counts characters rather than bytes for
/// UTF-8, so UTF-8 columns are character columns like UTF-32
pub fn indexer_column_encoding(encoding: crate::position::Encoding) -> crate::position::Encoding {
    match encoding {
        crate::position::Encoding::Utf8 => crate::position::Encoding::Utf32,
        other => other,
    }
}

type AncestorHash = (Vec<String>, Option<String>);

const ENTRY_SIMILARITY_THRESHOLD: f64 = 0.7;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("No entry found for namespace")]
pub struct NonExistingNamespaceError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct UnresolvableAliasError;

#[derive(Clone)]
pub struct Index {
    entries: Vec<Option<Entry>>,
    free_entry_ids: Vec<EntryId>,
    names: IndexMap<String, Vec<EntryId>>,
    entries_tree: PrefixTree<Arc<str>>,
    uris_to_entries: HashMap<Arc<str>, Vec<EntryId>>,
    require_paths_tree: PrefixTree<(Arc<str>, Arc<str>)>,
    require_paths_by_uri: HashMap<Arc<str>, Arc<str>>,
    ancestors: HashMap<String, Vec<String>>,
    pub configuration: Arc<Configuration>,
    pub initial_indexing_completed: bool,
}

impl Default for Index {
    fn default() -> Self {
        Self::new(Arc::new(Configuration::default()))
    }
}

/// Returns the real nesting of a constant name taking into account top level references that may be included anywhere in
/// the name or nesting where that constant was found
pub fn actual_nesting(stack: &[String], name: Option<&str>) -> Vec<String> {
    let mut nesting: Vec<&str> = stack.iter().map(String::as_str).collect();
    if let Some(name) = name {
        nesting.push(name);
    }

    let mut corrected = Vec::new();
    for part in nesting.iter().rev() {
        corrected.push(part.strip_prefix("::").unwrap_or(part).to_string());
        if part.starts_with("::") {
            break;
        }
    }
    corrected.reverse();
    corrected
}

fn split_name(name: &str) -> Vec<&str> {
    // Ruby's `String#split` drops trailing empty strings
    let mut parts: Vec<&str> = name.split("::").collect();
    while parts.last() == Some(&"") {
        parts.pop();
    }
    parts
}

impl Index {
    pub fn new(configuration: Arc<Configuration>) -> Self {
        Index {
            entries: Vec::new(),
            free_entry_ids: Vec::new(),
            names: IndexMap::new(),
            entries_tree: PrefixTree::new(),
            uris_to_entries: HashMap::new(),
            require_paths_tree: PrefixTree::new(),
            require_paths_by_uri: HashMap::new(),
            ancestors: HashMap::new(),
            configuration,
            initial_indexing_completed: false,
        }
    }

    pub fn entry(&self, id: EntryId) -> &Entry {
        self.entries[id].as_ref().expect("entry was deleted")
    }

    pub fn entry_mut(&mut self, id: EntryId) -> &mut Entry {
        self.entries[id].as_mut().expect("entry was deleted")
    }

    pub fn try_entry(&self, id: EntryId) -> Option<&Entry> {
        self.entries.get(id).and_then(Option::as_ref)
    }

    pub fn add(&mut self, entry: Entry, skip_prefix_tree: bool) -> EntryId {
        let name = entry.name.clone();
        let uri = entry.uri.clone();
        let id = match self.free_entry_ids.pop() {
            Some(id) => {
                self.entries[id] = Some(entry);
                id
            }
            None => {
                self.entries.push(Some(entry));
                self.entries.len() - 1
            }
        };
        self.names.entry(name.clone()).or_default().push(id);
        self.uris_to_entries.entry(uri).or_default().push(id);
        if !skip_prefix_tree {
            self.entries_tree.insert(&name, Arc::from(name.as_str()));
        }
        id
    }

    pub fn delete(&mut self, uri: &str, skip_require_paths_tree: bool) {
        if let Some(ids) = self.uris_to_entries.remove(uri) {
            for id in ids {
                let Some(entry) = self.entries.get_mut(id).and_then(Option::take) else { continue };
                self.free_entry_ids.push(id);
                let name = entry.name;
                let Some(list) = self.names.get_mut(&name) else { continue };
                list.retain(|existing| *existing != id);
                if list.is_empty() {
                    self.names.swap_remove(&name);
                    self.entries_tree.delete(&name);
                } else {
                    self.entries_tree.insert(&name, Arc::from(name.as_str()));
                }
            }
        }

        if skip_require_paths_tree {
            return;
        }
        if let Some(require_path) = self.require_paths_by_uri.remove(uri) {
            self.require_paths_tree.delete(&require_path);
        }
    }

    pub fn delete_require_path(&mut self, require_path: &str) {
        self.require_paths_tree.delete(require_path);
    }

    pub fn insert_require_path(&mut self, require_path: &str, uri: Arc<str>) {
        let require_path: Arc<str> = Arc::from(require_path);
        self.require_paths_by_uri.insert(uri.clone(), require_path.clone());
        self.require_paths_tree.insert(&require_path, (require_path.clone(), uri));
    }

    pub fn get(&self, fully_qualified_name: &str) -> Option<&[EntryId]> {
        let name = fully_qualified_name.strip_prefix("::").unwrap_or(fully_qualified_name);
        self.names.get(name).map(Vec::as_slice)
    }

    pub fn get_owned(&self, fully_qualified_name: &str) -> Option<Vec<EntryId>> {
        self.get(fully_qualified_name).map(<[EntryId]>::to_vec)
    }

    pub fn entries_named(&self, fully_qualified_name: &str) -> Vec<&Entry> {
        self.get(fully_qualified_name).map(|ids| ids.iter().map(|id| self.entry(*id)).collect()).unwrap_or_default()
    }

    pub fn search_require_paths(&self, query: &str) -> Vec<(Arc<str>, Arc<str>)> {
        self.require_paths_tree.search(query)
    }

    pub fn first_unqualified_const(&self, name: &str) -> Option<Vec<EntryId>> {
        let suffix = format!("::{name}");
        let found = self
            .names
            .iter()
            .find(|(constant_name, _)| *constant_name == name || constant_name.ends_with(&suffix))
            .or_else(|| self.names.iter().find(|(constant_name, _)| constant_name.ends_with(name)));
        found.map(|(_, ids)| ids.clone())
    }

    fn tree_search(&self, query: &str) -> Vec<Vec<EntryId>> {
        self.entries_tree.search(query).into_iter().filter_map(|name| self.names.get(name.as_ref()).cloned()).collect()
    }

    pub fn prefix_search(&self, query: &str, nesting: Option<&[String]>) -> Vec<Vec<EntryId>> {
        let mut results = match nesting {
            None => self.tree_search(query),
            Some(nesting) => {
                let mut results = Vec::new();
                for i in (0..=nesting.len()).rev() {
                    let prefix = nesting[..i].join("::");
                    let namespaced_query = if prefix.is_empty() { query.to_string() } else { format!("{prefix}::{query}") };
                    results.extend(self.tree_search(&namespaced_query));
                }
                results
            }
        };
        uniq(&mut results);
        results
    }

    pub fn fuzzy_search(&self, query: Option<&str>, condition: impl Fn(&Entry) -> bool) -> Vec<EntryId> {
        let Some(query) = query else {
            let mut results = Vec::new();
            for ids in self.names.values() {
                if ids.first().is_some_and(|id| self.entry(*id).is_singleton_class()) {
                    continue;
                }
                results.extend(ids.iter().copied().filter(|id| condition(self.entry(*id))));
            }
            return results;
        };

        let normalized_query = query.replace("::", "").to_lowercase();
        let mut results: Vec<(Vec<EntryId>, f64)> = Vec::new();
        for (name, ids) in &self.names {
            if ids.first().is_some_and(|id| self.entry(*id).is_singleton_class()) {
                continue;
            }
            let selected: Vec<EntryId> = ids.iter().copied().filter(|id| condition(self.entry(*id))).collect();
            if selected.is_empty() {
                continue;
            }
            let similarity = jaro_winkler::distance(&name.replace("::", "").to_lowercase(), &normalized_query);
            if similarity > ENTRY_SIMILARITY_THRESHOLD {
                results.push((selected, -similarity));
            }
        }
        results.sort_by(|left, right| left.1.partial_cmp(&right.1).unwrap_or(std::cmp::Ordering::Equal));
        results.into_iter().flat_map(|(ids, _)| ids).collect()
    }

    pub fn method_completion_candidates(&mut self, name: Option<&str>, receiver_name: &str) -> Vec<EntryId> {
        let ancestors = self.linearized_ancestors_of(receiver_name).unwrap_or_default();
        let candidates: Vec<EntryId> = match name {
            Some(name) => self.prefix_search(name, None).into_iter().flatten().collect(),
            None => self.names.values().flatten().copied().collect(),
        };

        let mut completion_items: IndexMap<String, (EntryId, usize)> = IndexMap::new();
        for id in candidates {
            let Some(entry) = self.try_entry(id) else { continue };
            let is_unresolved_alias = matches!(entry.kind, EntryKind::UnresolvedMethodAlias { .. });
            if !(entry.is_member() || matches!(entry.kind, EntryKind::MethodAlias { .. }) || is_unresolved_alias) {
                continue;
            }

            let entry_name = entry.name.clone();
            let Some(ancestor_index) =
                entry.owner().and_then(|owner| ancestors.iter().position(|ancestor| ancestor == owner))
            else {
                continue;
            };
            if let Some((_, existing_index)) = completion_items.get(&entry_name)
                && ancestor_index >= *existing_index
            {
                continue;
            }

            if is_unresolved_alias {
                let mut seen_names = Vec::new();
                if let Some(resolved) = self.resolve_method_alias(id, receiver_name, &mut seen_names)
                    && matches!(self.entry(resolved).kind, EntryKind::MethodAlias { .. })
                {
                    completion_items.insert(entry_name, (resolved, ancestor_index));
                }
            } else {
                completion_items.insert(entry_name, (id, ancestor_index));
            }
        }

        completion_items.into_values().map(|(id, _)| id).collect()
    }

    pub fn constant_completion_candidates(&mut self, name: &str, nesting: &[String]) -> Vec<Vec<EntryId>> {
        if let Some(stripped) = name.strip_prefix("::") {
            return self.tree_search(stripped);
        }

        let direct_query = if nesting.is_empty() { name.to_string() } else { format!("{}::{name}", nesting.join("::")) };
        let mut entries = self.tree_search(&direct_query);

        for i in (1..=nesting.len()).rev() {
            let namespace = nesting[..i].join("::");
            entries.extend(self.tree_search(&format!("{namespace}::{name}")));
        }

        if name.ends_with("::") {
            let mut extended = nesting.to_vec();
            extended.push(name.to_string());
            entries.extend(self.inherited_constant_completion_candidates(None, &extended));
        } else {
            entries.extend(self.inherited_constant_completion_candidates(Some(name), nesting));
        }

        entries.extend(self.tree_search(name));

        let mut filtered: Vec<Vec<EntryId>> = entries
            .into_iter()
            .map(|definitions| {
                definitions.into_iter().filter(|id| self.try_entry(*id).is_some_and(Entry::is_constant_like)).collect()
            })
            .filter(|definitions: &Vec<EntryId>| !definitions.is_empty())
            .collect();
        uniq(&mut filtered);
        filtered
    }

    /// Resolve a constant to its declaration based on its name and the nesting where the reference was found
    pub fn resolve(&mut self, name: &str, nesting: &[String]) -> Option<Vec<EntryId>> {
        let mut seen_names = Vec::new();
        self.resolve_with_seen(name, nesting, &mut seen_names)
    }

    fn resolve_with_seen(&mut self, name: &str, nesting: &[String], seen_names: &mut Vec<String>) -> Option<Vec<EntryId>> {
        self.try_resolve(name, nesting, seen_names).ok().flatten()
    }

    fn try_resolve(
        &mut self,
        name: &str,
        nesting: &[String],
        seen_names: &mut Vec<String>,
    ) -> Result<Option<Vec<EntryId>>, UnresolvableAliasError> {
        if let Some(stripped) = name.strip_prefix("::")
            && let Some(entries) = self.direct_or_aliased_constant(stripped, seen_names)?
        {
            return Ok(Some(entries));
        }

        let full_name = if nesting.is_empty() { name.to_string() } else { format!("{}::{name}", nesting.join("::")) };

        if let Some(entries) = self.direct_or_aliased_constant(&full_name, seen_names)? {
            return Ok(Some(entries));
        }

        if let Some(entries) = self.lookup_enclosing_scopes(name, nesting, seen_names)? {
            return Ok(Some(entries));
        }

        if let Some(entries) = self.lookup_ancestor_chain(name, nesting, seen_names)? {
            return Ok(Some(entries));
        }

        self.direct_or_aliased_constant(name, seen_names)
    }

    pub fn follow_aliased_namespace(&mut self, name: &str) -> Option<String> {
        let mut seen_names = Vec::new();
        self.try_follow_aliased_namespace(name, &mut seen_names).ok()
    }

    fn try_follow_aliased_namespace(
        &mut self,
        name: &str,
        seen_names: &mut Vec<String>,
    ) -> Result<String, UnresolvableAliasError> {
        let parts = split_name(name);
        let mut real_parts: Vec<&str> = Vec::new();

        for i in (0..parts.len()).rev() {
            let current_name = parts[..=i].join("::");
            let entry_id = if seen_names.contains(&current_name) {
                None
            } else {
                self.names.get(&current_name).and_then(|ids| ids.first().copied())
            };

            let target = match entry_id.map(|id| &self.entry(id).kind) {
                Some(EntryKind::ConstantAlias { target }) => Some(target.clone()),
                Some(EntryKind::UnresolvedConstantAlias { .. }) => {
                    let resolved = self.resolve_alias(entry_id.unwrap(), seen_names);
                    match &self.entry(resolved).kind {
                        EntryKind::ConstantAlias { target } => Some(target.clone()),
                        _ => return Err(UnresolvableAliasError),
                    }
                }
                _ => None,
            };

            match target {
                Some(target) => {
                    let rest = real_parts.join("::");
                    return self.try_follow_aliased_namespace(&format!("{target}::{rest}"), seen_names);
                }
                None => real_parts.insert(0, parts[i]),
            }
        }

        Ok(real_parts.join("::"))
    }

    /// Attempts to find methods for a resolved fully qualified receiver name
    pub fn resolve_method(&mut self, method_name: &str, receiver_name: &str, inherited_only: bool) -> Option<Vec<EntryId>> {
        let mut seen_names = Vec::new();
        self.resolve_method_with_seen(method_name, receiver_name, &mut seen_names, inherited_only)
    }

    fn resolve_method_with_seen(
        &mut self,
        method_name: &str,
        receiver_name: &str,
        seen_names: &mut Vec<String>,
        inherited_only: bool,
    ) -> Option<Vec<EntryId>> {
        let method_entries = self.get_owned(method_name)?;
        let receiver = receiver_name.strip_prefix("::").unwrap_or(receiver_name);
        let ancestors = self.linearized_ancestors_of(receiver).ok()?;

        for ancestor in &ancestors {
            if inherited_only && ancestor == receiver_name {
                continue;
            }

            let mut found = Vec::new();
            for id in &method_entries {
                let Some(entry) = self.try_entry(*id) else { continue };
                let owned_by_ancestor = entry.owner() == Some(ancestor.as_str());
                match entry.kind {
                    EntryKind::Accessor { .. } | EntryKind::Method { .. } | EntryKind::MethodAlias { .. } => {
                        if owned_by_ancestor {
                            found.push(*id);
                        }
                    }
                    EntryKind::UnresolvedMethodAlias { .. } => {
                        if owned_by_ancestor
                            && let Some(resolved) = self.resolve_method_alias(*id, receiver_name, seen_names)
                            && matches!(self.entry(resolved).kind, EntryKind::MethodAlias { .. })
                        {
                            found.push(resolved);
                        }
                    }
                    _ => {}
                }
            }

            if !found.is_empty() {
                return Some(found);
            }
        }

        None
    }

    /// Linearizes the ancestors for a given name, returning the order of namespaces in which Ruby will search for method
    /// or constant declarations.
    ///
    /// The order of ancestors is [linearized_prepends, self, linearized_includes, linearized_superclass]
    pub fn linearized_ancestors_of(&mut self, fully_qualified_name: &str) -> Result<Vec<String>, NonExistingNamespaceError> {
        if let Some(cached) = self.ancestors.get(fully_qualified_name) {
            return Ok(cached.clone());
        }

        let mut parts: Vec<String> = split_name(fully_qualified_name).into_iter().map(str::to_string).collect();
        let mut singleton_levels = 0;
        while parts.last().is_some_and(|part| part.contains("<Class:")) {
            singleton_levels += 1;
            parts.pop();
        }
        let attached_class_name = parts.join("::");

        let mut entries = self.get_owned(fully_qualified_name);
        if singleton_levels > 0 && entries.is_none() && self.is_indexed(&attached_class_name) {
            entries = Some(vec![self.existing_or_new_singleton_class(&attached_class_name)]);
        }
        let entries = entries.ok_or(NonExistingNamespaceError)?;

        let mut ancestors = vec![fully_qualified_name.to_string()];
        self.ancestors.insert(fully_qualified_name.to_string(), ancestors.clone());

        let mut namespaces: Vec<EntryId> = Vec::new();
        for id in &entries {
            match &self.entry(*id).kind {
                EntryKind::Namespace(_) => namespaces.push(*id),
                EntryKind::ConstantAlias { target } => {
                    if let Some(target_ids) = self.get(target) {
                        namespaces.extend(target_ids.iter().copied().filter(|target_id| self.entry(*target_id).is_namespace()));
                    }
                }
                _ => {}
            }
        }

        if namespaces.is_empty() {
            return Err(NonExistingNamespaceError);
        }

        let mut nesting: Vec<String> = self
            .entry(namespaces[0])
            .namespace()
            .map(|namespace| namespace.nesting.iter().flat_map(|part| split_name(part).into_iter().map(str::to_string)).collect())
            .unwrap_or_default();

        if !nesting.is_empty() {
            for _ in 0..singleton_levels {
                let last = nesting.last().cloned().unwrap_or_default();
                nesting.push(format!("<Class:{last}>"));
            }
        }

        self.linearize_mixins(&mut ancestors, &namespaces, &nesting, fully_qualified_name)?;
        self.linearize_superclass(
            &mut ancestors,
            &attached_class_name,
            fully_qualified_name,
            &namespaces,
            &nesting,
            singleton_levels,
        )?;

        Ok(ancestors)
    }

    pub fn resolve_instance_variable(&mut self, variable_name: &str, owner_name: &str) -> Option<Vec<EntryId>> {
        let entries = self.get_owned(variable_name)?;
        let ancestors = self.linearized_ancestors_of(owner_name).ok()?;
        if ancestors.is_empty() {
            return None;
        }
        Some(
            entries
                .into_iter()
                .filter(|id| self.entry(*id).owner().is_some_and(|owner| ancestors.iter().any(|ancestor| ancestor == owner)))
                .collect(),
        )
    }

    pub fn resolve_class_variable(&mut self, variable_name: &str, owner_name: &str) -> Option<Vec<EntryId>> {
        let entries: Vec<EntryId> = self
            .get_owned(variable_name)?
            .into_iter()
            .filter(|id| matches!(self.entry(*id).kind, EntryKind::ClassVariable { .. }))
            .collect();
        if entries.is_empty() {
            return None;
        }
        let ancestors = self.linearized_attached_ancestors(owner_name).ok()?;
        if ancestors.is_empty() {
            return None;
        }
        Some(
            entries
                .into_iter()
                .filter(|id| self.entry(*id).owner().is_some_and(|owner| ancestors.iter().any(|ancestor| ancestor == owner)))
                .collect(),
        )
    }

    pub fn instance_variable_completion_candidates(&mut self, name: &str, owner_name: &str) -> Vec<EntryId> {
        let entries: Vec<EntryId> = self.prefix_search(name, None).into_iter().flatten().collect();
        if entries.is_empty() {
            return entries;
        }

        let ancestors = self.linearized_ancestors_of(owner_name).unwrap_or_default();
        let (instance_variables, class_variables): (Vec<EntryId>, Vec<EntryId>) =
            entries.into_iter().partition(|id| matches!(self.entry(*id).kind, EntryKind::InstanceVariable { .. }));

        let owned_by = |index: &Index, id: &EntryId, ancestors: &[String]| {
            index.entry(*id).owner().is_some_and(|owner| ancestors.iter().any(|ancestor| ancestor == owner))
        };

        let mut variables: Vec<EntryId> =
            instance_variables.into_iter().filter(|id| owned_by(self, id, &ancestors)).collect();

        if !class_variables.is_empty() {
            let name_parts = split_name(owner_name);
            if name_parts.last().is_some_and(|part| part.starts_with("<Class:")) {
                let attached_name = name_parts[..name_parts.len() - 1].join("::");
                let attached_ancestors = self.linearized_ancestors_of(&attached_name).unwrap_or_default();
                variables.extend(class_variables.into_iter().filter(|id| owned_by(self, id, &attached_ancestors)));
            } else {
                variables.extend(class_variables.into_iter().filter(|id| owned_by(self, id, &ancestors)));
            }
        }

        let mut seen = std::collections::HashSet::new();
        variables.retain(|id| seen.insert(self.entry(*id).name.clone()));
        variables
    }

    pub fn class_variable_completion_candidates(&mut self, name: &str, owner_name: &str) -> Vec<EntryId> {
        let entries: Vec<EntryId> = self.prefix_search(name, None).into_iter().flatten().collect();
        if entries.is_empty() {
            return entries;
        }
        let ancestors = self.linearized_attached_ancestors(owner_name).unwrap_or_default();
        let mut variables: Vec<EntryId> = entries
            .into_iter()
            .filter(|id| self.entry(*id).owner().is_some_and(|owner| ancestors.iter().any(|ancestor| ancestor == owner)))
            .collect();
        let mut seen = std::collections::HashSet::new();
        variables.retain(|id| seen.insert(self.entry(*id).name.clone()));
        variables
    }

    /// Synchronizes a change made to the given URI, clearing the ancestors cache if any namespace's ancestry changed
    pub fn handle_change(&mut self, uri: &str, update: impl FnOnce(&mut Index)) {
        let original = self.namespace_ancestor_hashes(uri);
        update(self);
        let updated = self.namespace_ancestor_hashes(uri);

        if let (Some(original), Some(updated)) = (original, updated)
            && original.iter().any(|(name, hash)| updated.get(name) != Some(hash))
        {
            self.ancestors.clear();
        }
    }

    /// Mirrors `Index#handle_change(uri, source)`: deletes the URI's entries and indexes the new source with comments
    pub fn handle_change_with_source(&mut self, uri: &str, source: &str) {
        self.handle_change(uri, |index| {
            index.delete(uri, false);
            super::indexing::index_single(index, uri, source, true);
        });
    }

    fn namespace_ancestor_hashes(&self, uri: &str) -> Option<HashMap<String, AncestorHash>> {
        let ids = self.uris_to_entries.get(uri)?;
        Some(
            ids.iter()
                .filter_map(|id| {
                    let entry = self.try_entry(*id)?;
                    entry.namespace().map(|namespace| (entry.name.clone(), namespace.ancestor_hash()))
                })
                .collect(),
        )
    }

    pub fn has_cached_ancestors(&self, name: &str) -> bool {
        self.ancestors.contains_key(name)
    }

    pub fn is_ancestors_cache_empty(&self) -> bool {
        self.ancestors.is_empty()
    }

    pub fn clear_ancestors(&mut self) {
        self.ancestors.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    pub fn names(&self) -> impl Iterator<Item = &String> {
        self.names.keys()
    }

    pub fn is_indexed(&self, name: &str) -> bool {
        self.names.contains_key(name)
    }

    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn existing_or_new_singleton_class(&mut self, name: &str) -> EntryId {
        let unqualified_name = split_name(name).last().copied().unwrap_or("").to_string();
        let full_singleton_name = format!("{name}::<Class:{unqualified_name}>");
        if let Some(id) = self.get(&full_singleton_name).and_then(|ids| ids.first().copied()) {
            return id;
        }

        let attached = self.get(name).and_then(|ids| ids.first().copied()).map(|id| self.entry(id));
        let uri = attached.map(|entry| entry.uri.clone()).unwrap_or_else(|| Arc::from(""));
        let empty_location = Location { start_line: 1, end_line: 1, start_column: 0, end_column: 0 };
        let location = attached.map(|entry| entry.location).unwrap_or(empty_location);
        let name_location = attached.map(Entry::name_location).unwrap_or(empty_location);
        let entry = Entry::new(
            full_singleton_name.clone(),
            uri,
            location,
            None,
            EntryKind::Namespace(Namespace {
                kind: NamespaceKind::SingletonClass,
                nesting: vec![full_singleton_name],
                name_location,
                mixin_operations: Vec::new(),
                parent_class: None,
            }),
        );
        self.add(entry, true)
    }

    pub fn entries_for(&self, uri: &str) -> Vec<EntryId> {
        self.uris_to_entries.get(uri).cloned().unwrap_or_default()
    }

    pub fn all_entry_ids(&self) -> impl Iterator<Item = EntryId> + '_ {
        self.entries.iter().enumerate().filter_map(|(id, entry)| entry.as_ref().map(|_| id))
    }

    /// Moves every entry from a separately built index into this one, used to merge files indexed in parallel
    pub fn absorb(&mut self, other: Index) {
        for entry in other.entries.into_iter().flatten() {
            let skip_prefix_tree = entry.is_singleton_class();
            self.add(entry, skip_prefix_tree);
        }
        let mut require_paths: Vec<(Arc<str>, Arc<str>)> = other.require_paths_by_uri.into_iter().collect();
        require_paths.sort();
        for (uri, require_path) in require_paths {
            self.insert_require_path(&require_path, uri);
        }
    }

    pub fn comments_for(&mut self, id: EntryId) -> String {
        if let Some(comments) = &self.entry(id).comments {
            return comments.clone();
        }
        let entry = self.entry(id);
        let comments = super::comments::comments_from_file(entry, &self.configuration);
        self.entry_mut(id).comments = Some(comments.clone());
        comments
    }

    fn linearized_attached_ancestors(&mut self, name: &str) -> Result<Vec<String>, NonExistingNamespaceError> {
        let name_parts = split_name(name);
        if name_parts.last().is_some_and(|part| part.starts_with("<Class:")) {
            let attached_name = name_parts[..name_parts.len() - 1].join("::");
            self.linearized_ancestors_of(&attached_name)
        } else {
            self.linearized_ancestors_of(name)
        }
    }

    fn linearize_mixins(
        &mut self,
        ancestors: &mut Vec<String>,
        namespaces: &[EntryId],
        nesting: &[String],
        cache_key: &str,
    ) -> Result<(), NonExistingNamespaceError> {
        let mixin_operations: Vec<ModuleOperation> = namespaces
            .iter()
            .flat_map(|id| self.entry(*id).namespace().map(|namespace| namespace.mixin_operations.clone()).unwrap_or_default())
            .collect();
        let mut main_namespace_index = 0;

        for operation in mixin_operations {
            let Some(resolved) = self.resolve(operation.module_name(), nesting) else { continue };
            let module_fully_qualified_name = self.entry(resolved[0]).name.clone();

            match operation {
                ModuleOperation::Prepend(_) => {
                    let linearized_prepends = self.linearized_ancestors_of(&module_fully_qualified_name)?;
                    let prepended_ancestors: Vec<String> = ancestors[..main_namespace_index.min(ancestors.len())].to_vec();
                    let unique_prepends: Vec<String> =
                        linearized_prepends.iter().filter(|name| !prepended_ancestors.contains(name)).cloned().collect();
                    let insert_position = (linearized_prepends.len() - unique_prepends.len()).min(ancestors.len());
                    for (offset, name) in unique_prepends.into_iter().enumerate() {
                        ancestors.insert(insert_position + offset, name);
                    }
                    main_namespace_index += linearized_prepends.len();
                }
                ModuleOperation::Include(_) => {
                    let linearized_includes = self.linearized_ancestors_of(&module_fully_qualified_name)?;
                    let new_includes: Vec<String> =
                        linearized_includes.into_iter().filter(|name| !ancestors.contains(name)).collect();
                    let insert_position = (main_namespace_index + 1).min(ancestors.len());
                    for (offset, name) in new_includes.into_iter().enumerate() {
                        ancestors.insert(insert_position + offset, name);
                    }
                }
            }
            self.ancestors.insert(cache_key.to_string(), ancestors.clone());
        }
        Ok(())
    }

    fn singleton_name_parts(base: &str, singleton_levels: usize) -> String {
        let mut parts: Vec<String> = split_name(base).into_iter().map(str::to_string).collect();
        for _ in 0..singleton_levels {
            let last = parts.last().cloned().unwrap_or_default();
            parts.push(format!("<Class:{last}>"));
        }
        parts.join("::")
    }

    fn linearize_superclass(
        &mut self,
        ancestors: &mut Vec<String>,
        attached_class_name: &str,
        fully_qualified_name: &str,
        namespaces: &[EntryId],
        nesting: &[String],
        singleton_levels: usize,
    ) -> Result<(), NonExistingNamespaceError> {
        let possible_parents: Vec<EntryId> =
            if singleton_levels > 0 { self.get_owned(attached_class_name).unwrap_or_default() } else { namespaces.to_vec() };

        let mut superclass: Option<String> = None;
        for id in &possible_parents {
            let entry = self.entry(*id);
            if !entry.is_class() {
                continue;
            }
            let Some(parent_class) = entry.namespace().and_then(|namespace| namespace.parent_class.clone()) else {
                continue;
            };
            let is_object = parent_class == "::Object";
            superclass = Some(parent_class);
            if !is_object {
                break;
            }
        }

        if let Some(parent_class) = superclass {
            let parent_class_name = self.resolve(&parent_class, nesting).map(|resolved| self.entry(resolved[0]).name.clone());

            if let Some(parent_class_name) = &parent_class_name
                && fully_qualified_name != parent_class_name
            {
                let parent_name = Self::singleton_name_parts(parent_class_name, singleton_levels);
                let parent_ancestors = self.linearized_ancestors_of(&parent_name)?;
                ancestors.extend(parent_ancestors);
                self.ancestors.insert(fully_qualified_name.to_string(), ancestors.clone());
            }

            if parent_class_name.as_deref().is_some_and(|name| name.starts_with("BasicObject")) && singleton_levels > 0 {
                let class_class_name = Self::singleton_name_parts("Class", singleton_levels - 1);
                let class_ancestors = self.linearized_ancestors_of(&class_class_name)?;
                ancestors.extend(class_ancestors);
                self.ancestors.insert(fully_qualified_name.to_string(), ancestors.clone());
            }
        } else if singleton_levels > 0 {
            let has_module = self.get(attached_class_name).is_some_and(|ids| ids.iter().any(|id| self.entry(*id).is_module()));
            if has_module {
                let module_class_name = Self::singleton_name_parts("Module", singleton_levels - 1);
                let module_ancestors = self.linearized_ancestors_of(&module_class_name)?;
                ancestors.extend(module_ancestors);
                self.ancestors.insert(fully_qualified_name.to_string(), ancestors.clone());
            }
        }
        Ok(())
    }

    /// Attempts to resolve an unresolved constant alias into a resolved one. Returns the same entry if the target does not
    /// exist
    fn resolve_alias(&mut self, id: EntryId, seen_names: &mut Vec<String>) -> EntryId {
        let entry = self.entry(id);
        let alias_name = entry.name.clone();
        if seen_names.contains(&alias_name) {
            return id;
        }
        seen_names.push(alias_name.clone());

        let EntryKind::UnresolvedConstantAlias { target, nesting } = entry.kind.clone() else { return id };
        let Some(resolved_target) = self.resolve_with_seen(&target, &nesting, seen_names) else { return id };
        if resolved_target[0] == id {
            return id;
        }

        let target_name = self.entry(resolved_target[0]).name.clone();
        let entry = self.entry_mut(id);
        entry.kind = EntryKind::ConstantAlias { target: target_name };

        // Ruby LSP replaces the unresolved entry by moving the resolved one to the end of the list
        if let Some(list) = self.names.get_mut(&alias_name) {
            list.retain(|existing| *existing != id);
            list.push(id);
        }
        id
    }

    fn lookup_enclosing_scopes(
        &mut self,
        name: &str,
        nesting: &[String],
        seen_names: &mut Vec<String>,
    ) -> Result<Option<Vec<EntryId>>, UnresolvableAliasError> {
        for i in (1..=nesting.len()).rev() {
            let namespace = nesting[..i].join("::");
            if let Some(entries) = self.direct_or_aliased_constant(&format!("{namespace}::{name}"), seen_names)? {
                return Ok(Some(entries));
            }
        }
        Ok(None)
    }

    fn lookup_ancestor_chain(
        &mut self,
        name: &str,
        nesting: &[String],
        seen_names: &mut Vec<String>,
    ) -> Result<Option<Vec<EntryId>>, UnresolvableAliasError> {
        let full_name = build_non_redundant_full_name(name, nesting);
        let mut parts: Vec<&str> = split_name(&full_name);
        let Some(constant_name) = parts.pop() else { return Ok(None) };
        if parts.is_empty() {
            return Ok(None);
        }

        let Some(namespace_entries) = self.try_resolve(&parts.join("::"), &[], seen_names)? else { return Ok(None) };
        let namespace_name = self.entry(namespace_entries[0]).name.clone();
        let Ok(ancestors) = self.linearized_ancestors_of(&namespace_name) else { return Ok(None) };

        for ancestor_name in ancestors {
            if let Some(entries) = self.direct_or_aliased_constant(&format!("{ancestor_name}::{constant_name}"), seen_names)? {
                return Ok(Some(entries));
            }
        }
        Ok(None)
    }

    fn inherited_constant_completion_candidates(&mut self, name: Option<&str>, nesting: &[String]) -> Vec<Vec<EntryId>> {
        let (namespace_entries, constant_name) = match name {
            Some(name) => {
                let full_name = build_non_redundant_full_name(name, nesting);
                let mut parts: Vec<String> = split_name(&full_name).into_iter().map(str::to_string).collect();
                let constant_name = parts.pop().unwrap_or_default();
                if parts.is_empty() {
                    return Vec::new();
                }
                (self.resolve(&parts.join("::"), &[]), constant_name)
            }
            None => (self.resolve(&nesting.join("::"), &[]), String::new()),
        };
        let Some(namespace_entries) = namespace_entries else { return Vec::new() };

        let namespace_name = self.entry(namespace_entries[0]).name.clone();
        let Ok(ancestors) = self.linearized_ancestors_of(&namespace_name) else { return Vec::new() };
        let candidates: Vec<Vec<EntryId>> = ancestors
            .iter()
            .flat_map(|ancestor_name| self.tree_search(&format!("{ancestor_name}::{constant_name}")))
            .collect();

        let mut completion_items: IndexMap<String, (Vec<EntryId>, usize)> = IndexMap::new();
        for entries in candidates {
            let Some(first) = entries.first() else { continue };
            let mut parts: Vec<&str> = split_name(&self.entry(*first).name);
            let short_name = parts.pop().unwrap_or("").to_string();
            let candidate_namespace = parts.join("::");
            let Some(ancestor_index) = ancestors.iter().position(|ancestor| *ancestor == candidate_namespace) else {
                continue;
            };
            if let Some((_, existing_index)) = completion_items.get(&short_name)
                && ancestor_index >= *existing_index
            {
                continue;
            }
            completion_items.insert(short_name, (entries, ancestor_index));
        }

        completion_items.into_values().map(|(entries, _)| entries).collect()
    }

    fn direct_or_aliased_constant(
        &mut self,
        full_name: &str,
        seen_names: &mut Vec<String>,
    ) -> Result<Option<Vec<EntryId>>, UnresolvableAliasError> {
        if let Some(ids) = self.names.get(full_name).cloned() {
            return Ok(Some(self.resolve_constant_aliases(ids, seen_names)));
        }

        let aliased = self.try_follow_aliased_namespace(full_name, seen_names)?;
        if full_name == aliased || seen_names.contains(&aliased) {
            return Ok(None);
        }

        Ok(self.names.get(&aliased).cloned().map(|ids| self.resolve_constant_aliases(ids, seen_names)))
    }

    fn resolve_constant_aliases(&mut self, ids: Vec<EntryId>, seen_names: &mut Vec<String>) -> Vec<EntryId> {
        ids.into_iter()
            .map(|id| match self.entry(id).kind {
                EntryKind::UnresolvedConstantAlias { .. } => self.resolve_alias(id, seen_names),
                _ => id,
            })
            .collect()
    }

    /// Attempt to resolve a given unresolved method alias, returning the same entry if the target cannot be found
    fn resolve_method_alias(&mut self, id: EntryId, receiver_name: &str, seen_names: &mut Vec<String>) -> Option<EntryId> {
        let EntryKind::UnresolvedMethodAlias { new_name, old_name, owner } = self.try_entry(id)?.kind.clone() else {
            return Some(id);
        };
        if new_name == old_name || seen_names.contains(&new_name) {
            return Some(id);
        }
        seen_names.push(new_name.clone());

        let Some(target_ids) = self.resolve_method_with_seen(&old_name, receiver_name, seen_names, false) else {
            return Some(id);
        };
        let target_id = target_ids[0];
        let target_name = self.entry(target_id).name.clone();
        let target_signatures = self.entry(target_id).signatures();
        let target_owner = self.entry(target_id).owner().map(str::to_string);
        let target_kind = match self.entry(target_id).kind {
            EntryKind::Accessor { .. } => MethodAliasTargetKind::Accessor,
            EntryKind::MethodAlias { .. } => MethodAliasTargetKind::MethodAlias,
            _ => MethodAliasTargetKind::Method,
        };
        let target_comments = self.comments_for(target_id);
        let alias_comments = self.comments_for(id);

        let entry = self.entry_mut(id);
        entry.comments = Some(format!("Alias for {target_name}\n{alias_comments}\n{target_comments}"));
        entry.kind = EntryKind::MethodAlias {
            target: MethodAliasTarget {
                name: target_name,
                signatures: target_signatures,
                owner: target_owner,
                kind: target_kind,
            },
            owner,
        };

        if let Some(list) = self.names.get_mut(&new_name) {
            list.retain(|existing| *existing != id);
            list.push(id);
        }
        Some(id)
    }
}

/// Removes redundancy from a constant reference's full name. For example, a reference to `A::B::Foo` inside the
/// ["A", "B"] nesting should not become `A::B::A::B::Foo`
pub fn build_non_redundant_full_name(name: &str, nesting: &[String]) -> String {
    if nesting.is_empty() {
        return name.to_string();
    }
    if !name.contains("::") {
        return format!("{}::{name}", nesting.join("::"));
    }

    let mut name_parts: Vec<String> = name.split("::").map(str::to_string).collect();
    let Some(first_redundant_part) = nesting.iter().position(|part| *part == name_parts[0]) else {
        return format!("{}::{name}", nesting.join("::"));
    };

    let mut parts: Vec<String> = nesting[..first_redundant_part].to_vec();
    parts.append(&mut name_parts);
    parts.join("::")
}

fn uniq(results: &mut Vec<Vec<EntryId>>) {
    let mut seen = std::collections::HashSet::new();
    results.retain(|ids| seen.insert(ids.clone()));
}
