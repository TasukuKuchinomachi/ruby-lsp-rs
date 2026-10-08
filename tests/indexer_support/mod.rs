#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use ruby_lsp_rs::indexer::entry::NamespaceKind;
use ruby_lsp_rs::indexer::indexing::index_single;
use ruby_lsp_rs::indexer::rbs_indexer::RbsIndexer;
use ruby_lsp_rs::indexer::{Configuration, Entry, EntryId, EntryKind, Index};

pub const DEFAULT_URI: &str = "file:///fake/path/foo.rb";

/// Finds the `core` directory of the RBS gem: `RUBY_LSP_RBS_CORE_PATH` wins, otherwise RubyGems is asked
pub fn rbs_core_path() -> PathBuf {
    if let Ok(path) = std::env::var("RUBY_LSP_RBS_CORE_PATH") {
        return PathBuf::from(path);
    }
    let output = std::process::Command::new("ruby")
        .args(["-e", "print Gem::Specification.find_by_name('rbs').full_gem_path"])
        .output()
        .expect("ruby must be available to locate the RBS gem");
    PathBuf::from(String::from_utf8_lossy(&output.stdout).into_owned()).join("core")
}

fn core_index() -> &'static Index {
    static CORE_INDEX: OnceLock<Index> = OnceLock::new();
    CORE_INDEX.get_or_init(|| {
        let mut index = Index::new(Arc::new(Configuration::default()));
        RbsIndexer::new(&mut index).index_core_directory(&rbs_core_path());
        index
    })
}

/// Mirrors `RubyIndexer::TestCase`: every test starts from an index that already contains Ruby core RBS entries
pub struct TestCase {
    pub index: Index,
    default_names: Vec<String>,
}

impl TestCase {
    pub fn new() -> Self {
        let index = core_index().clone();
        let default_names = index.names().cloned().collect();
        TestCase { index, default_names }
    }

    pub fn empty() -> Self {
        TestCase { index: Index::new(Arc::new(Configuration::default())), default_names: Vec::new() }
    }

    pub fn index(&mut self, source: &str) {
        index_single(&mut self.index, DEFAULT_URI, source, true);
    }

    pub fn index_with_uri(&mut self, source: &str, uri: &str) {
        index_single(&mut self.index, uri, source, true);
    }

    pub fn entries(&self, name: &str) -> Vec<&Entry> {
        self.index.entries_named(name)
    }

    pub fn ids(&self, name: &str) -> Vec<EntryId> {
        self.index.get_owned(name).unwrap_or_default()
    }

    pub fn first(&self, name: &str) -> &Entry {
        let entries = self.entries(name);
        assert!(!entries.is_empty(), "Expected {name} to be indexed");
        entries[0]
    }

    pub fn assert_entry(&self, expected_name: &str, kind: &str, expected_location: &str) {
        let entries = self.entries(expected_name);
        assert!(!entries.is_empty(), "Expected {expected_name} to be indexed");
        let entry = entries[0];
        assert_eq!(kind, kind_name(entry), "Expected {expected_name} to be a {kind}");
        assert_eq!(expected_location, location_string(entry), "Unexpected location for {expected_name}");
    }

    pub fn assert_entry_with_visibility(&self, expected_name: &str, kind: &str, expected_location: &str, visibility: &str) {
        self.assert_entry(expected_name, kind, expected_location);
        assert_eq!(visibility, self.first(expected_name).visibility.as_str());
    }

    pub fn refute_entry(&self, expected_name: &str) {
        assert!(self.index.get(expected_name).is_none(), "Expected {expected_name} to not be indexed");
    }

    pub fn assert_no_indexed_entries(&self) {
        let names: Vec<String> = self.index.names().cloned().collect();
        assert_eq!(self.default_names, names);
    }

    pub fn assert_no_entry(&self, name: &str) {
        assert!(!self.index.is_indexed(name), "Expected '{name}' to not be indexed");
    }
}

impl Default for TestCase {
    fn default() -> Self {
        Self::new()
    }
}

/// The Ruby class name of the equivalent `RubyIndexer::Entry` subclass
pub fn kind_name(entry: &Entry) -> &'static str {
    match &entry.kind {
        EntryKind::Namespace(namespace) => match namespace.kind {
            NamespaceKind::Module => "Module",
            NamespaceKind::Class => "Class",
            NamespaceKind::SingletonClass => "SingletonClass",
        },
        EntryKind::Constant => "Constant",
        EntryKind::UnresolvedConstantAlias { .. } => "UnresolvedConstantAlias",
        EntryKind::ConstantAlias { .. } => "ConstantAlias",
        EntryKind::GlobalVariable => "GlobalVariable",
        EntryKind::ClassVariable { .. } => "ClassVariable",
        EntryKind::InstanceVariable { .. } => "InstanceVariable",
        EntryKind::Accessor { .. } => "Accessor",
        EntryKind::Method { .. } => "Method",
        EntryKind::UnresolvedMethodAlias { .. } => "UnresolvedMethodAlias",
        EntryKind::MethodAlias { .. } => "MethodAlias",
    }
}

/// Formats a location like `RubyIndexer::TestCase#assert_entry`: `path:start_line-start_column:end_line-end_column`
/// with 0-based lines
pub fn location_string(entry: &Entry) -> String {
    let location = entry.location;
    format!(
        "{}:{}-{}:{}-{}",
        entry.file_path().unwrap_or_default(),
        location.start_line - 1,
        location.start_column,
        location.end_line - 1,
        location.end_column
    )
}

pub fn names_of(index: &Index, ids: &[EntryId]) -> Vec<String> {
    ids.iter().map(|id| index.entry(*id).name.clone()).collect()
}

pub fn owner_names_of(index: &Index, ids: &[EntryId]) -> Vec<Option<String>> {
    ids.iter().map(|id| index.entry(*id).owner().map(str::to_string)).collect()
}

/// The Ruby class name of the equivalent `RubyIndexer::Entry::Parameter` subclass
pub fn parameter_kind(parameter: &ruby_lsp_rs::indexer::entry::Parameter) -> &'static str {
    use ruby_lsp_rs::indexer::entry::Parameter;
    match parameter {
        Parameter::Required(_) => "RequiredParameter",
        Parameter::Optional(_) => "OptionalParameter",
        Parameter::Keyword(_) => "KeywordParameter",
        Parameter::OptionalKeyword(_) => "OptionalKeywordParameter",
        Parameter::Rest(_) => "RestParameter",
        Parameter::KeywordRest(_) => "KeywordRestParameter",
        Parameter::Block(_) => "BlockParameter",
        Parameter::Forwarding => "ForwardingParameter",
    }
}

pub fn parameter_names(parameters: &[ruby_lsp_rs::indexer::entry::Parameter]) -> Vec<String> {
    parameters.iter().map(|parameter| parameter.name().to_string()).collect()
}

pub fn parameter_kinds(parameters: &[ruby_lsp_rs::indexer::entry::Parameter]) -> Vec<&'static str> {
    parameters.iter().map(parameter_kind).collect()
}

impl TestCase {
    /// The kind and name of the first entry named like the owner of `name`'s first entry, mirroring `entry.owner`
    pub fn owner_of(&self, name: &str) -> Option<(&'static str, String)> {
        let owner = self.first(name).owner()?.to_string();
        let owner_entry = self.first(&owner);
        Some((kind_name(owner_entry), owner))
    }
}
