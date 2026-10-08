use ruby_prism::{CallNode, Node};

use crate::document::{Document, ParsedDocument};
use crate::global_state::GlobalState;
use crate::indexer::{EntryId, EntryKind, Index};
use crate::position::{Location, Position, Range, range_covers};
use crate::prism::NodeExt;
use crate::requests::RequestError;

pub use crate::prism::constant_name;

pub fn range_from_location(location: &Location) -> Range {
    location.to_range()
}

pub fn self_receiver(node: &CallNode<'_>) -> bool {
    node.receiver().is_none_or(|receiver| receiver.as_self_node().is_some())
}

pub fn delegate_request_if_needed(global_state: &GlobalState, document: &Document, offset: usize) -> Result<(), RequestError> {
    if global_state.client_capabilities.supports_request_delegation && document.inside_host_language(offset) {
        return Err(RequestError::Delegate);
    }
    Ok(())
}

/// Ruby LSP compares Prism's byte columns with editor positions, which is only correct for ASCII lines. Columns are
/// converted to the negotiated encoding instead
pub fn covers_position(parsed: &ParsedDocument<'_>, start_offset: usize, end_offset: usize, position: Position) -> bool {
    range_covers(&parsed.code_units_range(start_offset, end_offset), position)
}

pub fn location_covers_position(parsed: &ParsedDocument<'_>, location: Option<&ruby_prism::Location<'_>>, position: Position) -> bool {
    location.is_some_and(|location| covers_position(parsed, location.start_offset(), location.end_offset(), position))
}

pub fn determine_target<'pr>(parsed: &ParsedDocument<'_>, target: Node<'pr>, parent: Node<'pr>, position: Position) -> Node<'pr> {
    let Some(parent_path) = parent.as_constant_path_node() else { return target };

    let mut target = parent.clone_node();
    let mut parent = parent_path.parent();

    while let Some(candidate) = parent {
        if !covers_position(parsed, candidate.start_offset(), candidate.end_offset(), position) {
            break;
        }
        parent = candidate.as_constant_path_node().and_then(|path| path.parent());
        target = candidate;
    }

    target
}

pub fn namespace_constant_name(node: &Node<'_>) -> Option<String> {
    let path = if let Some(class) = node.as_class_node() {
        class.constant_path()
    } else {
        node.as_module_node()?.constant_path()
    };
    if path.as_constant_path_node().is_some() || path.as_constant_read_node().is_some() { constant_name(&path) } else { None }
}

pub fn each_constant_path_part<'pr>(node: &Node<'pr>, mut block: impl FnMut(&Node<'pr>)) {
    let mut current = Some(node.clone_node());
    while let Some(part) = current {
        let Some(path) = part.as_constant_path_node() else { break };
        block(&part);
        current = path.parent();
    }
}

pub mod symbol_kind {
    pub const NAMESPACE: u32 = 3;
    pub const CLASS: u32 = 5;
    pub const METHOD: u32 = 6;
    pub const PROPERTY: u32 = 7;
    pub const FIELD: u32 = 8;
    pub const CONSTRUCTOR: u32 = 9;
    pub const VARIABLE: u32 = 13;
    pub const CONSTANT: u32 = 14;
    pub const NULL: u32 = 21;
}

pub fn kind_for_entry(index: &Index, id: EntryId) -> u32 {
    let entry = index.entry(id);
    match &entry.kind {
        EntryKind::Namespace(_) if entry.is_class() => symbol_kind::CLASS,
        EntryKind::Namespace(_) => symbol_kind::NAMESPACE,
        EntryKind::Constant | EntryKind::UnresolvedConstantAlias { .. } | EntryKind::ConstantAlias { .. } => {
            symbol_kind::CONSTANT
        }
        EntryKind::Method { .. } | EntryKind::UnresolvedMethodAlias { .. } | EntryKind::MethodAlias { .. } => {
            if entry.name == "initialize" { symbol_kind::CONSTRUCTOR } else { symbol_kind::METHOD }
        }
        EntryKind::Accessor { .. } => symbol_kind::PROPERTY,
        EntryKind::InstanceVariable { .. } | EntryKind::ClassVariable { .. } => symbol_kind::FIELD,
        EntryKind::GlobalVariable => symbol_kind::VARIABLE,
    }
}

pub struct CategorizedMarkdown {
    pub title: String,
    pub links: String,
    pub documentation: String,
}

pub fn categorized_markdown_from_index_entries(
    index: &mut Index,
    title: &str,
    entries: &[EntryId],
    max_entries: Option<usize>,
) -> CategorizedMarkdown {
    let markdown_title = format!("```ruby\n{title}\n```");
    let mut definitions = Vec::new();
    let mut content = String::new();
    let entries_to_format = match max_entries {
        Some(max_entries) => &entries[..entries.len().min(max_entries)],
        None => entries,
    };

    for id in entries_to_format {
        let entry = index.entry(*id);
        let location = entry.location;
        let uri = format!(
            "{}#L{},{}-{},{}",
            entry.uri,
            location.start_line,
            location.start_column + 1,
            location.end_line,
            location.end_column + 1
        );
        definitions.push(format!("[{}]({uri})", entry.file_name()));
        let comments = index.comments_for(*id);
        if !comments.is_empty() {
            content.push_str(&format!("\n\n{comments}"));
        }
    }

    let additional_entries_text = match max_entries {
        Some(max_entries) if entries.len() > max_entries => {
            let additional = entries.len() - max_entries;
            format!(" | {additional} other{}", if additional > 1 { "s" } else { "" })
        }
        _ => String::new(),
    };

    CategorizedMarkdown {
        title: markdown_title,
        links: format!("**Definitions**: {}{additional_entries_text}", definitions.join(" | ")),
        documentation: content,
    }
}

pub fn markdown_from_index_entries(
    index: &mut Index,
    title: &str,
    entries: &[EntryId],
    max_entries: Option<usize>,
    extra_links: Option<&str>,
) -> String {
    let categorized = categorized_markdown_from_index_entries(index, title, entries, max_entries);
    let mut markdown = categorized.title;
    if let Some(extra_links) = extra_links {
        markdown.push_str(&format!("\n\n{extra_links}"));
    }
    format!("{markdown}\n\n{}\n\n{}", categorized.links, categorized.documentation)
}
