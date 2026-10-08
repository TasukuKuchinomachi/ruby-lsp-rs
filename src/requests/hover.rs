use std::path::{Path, PathBuf};

use rbs_environment::Type;
use ruby_prism::Node;
use serde_json::{Value, json};
use type_checker::{Analysis, HoverEntry, HoverKind};

use super::support::common::{delegate_request_if_needed, determine_target, location_covers_position};
use super::support::hover_response_builder::HoverResponseBuilder;
use super::support::sorbet_level::SorbetLevel;
use super::{RequestContext, RequestResult, TextDocumentPositionParams, parse_params};
use crate::document::ParsedDocument;
use crate::global_state::GlobalState;
use crate::indexer::Entry;
use crate::listeners::hover::{ALLOWED_TARGETS, Hover};
use crate::position::{Position, Range};
use crate::prism::NodeExt;
use crate::requests::support::common::range_from_location;
use crate::requests::support::implementation_lookup;
use crate::type_checking::{self, NamedType};

const MAXIMUM_LINKS_PER_SECTION: usize = 5;
const TYPE_GROUP_SEPARATOR: &str = " · ";

pub fn handle(context: &mut RequestContext<'_>, params: Value) -> RequestResult {
    let params: TextDocumentPositionParams = parse_params(params)?;
    let document = context.store.get(&params.text_document.uri)?;
    if !document.is_ruby_or_erb() {
        return Ok(Value::Null);
    }

    let analysis = context.global_state.type_checking.analysis_for(document);
    let global_state = &*context.global_state;
    let parsed = document.parse();
    let position = params.position;
    let offset = parsed.offset_of(position)?;
    delegate_request_if_needed(global_state, document, offset)?;

    let type_markdown = analysis.as_ref().and_then(|analysis| {
        let hover_entry = analysis.hover_at(offset)?;
        let markdown = type_checking::hover_markdown(hover_entry, document.source().as_bytes());
        let links = location_links_markdown(global_state, analysis, hover_entry, document.uri(), position);
        Some(if links.is_empty() { markdown } else { format!("{markdown}\n\n{links}") })
    });
    let node_context = parsed.locate(offset, ALLOWED_TARGETS);
    let Some(target) = refine_target(&parsed, node_context.node.as_ref(), node_context.parent.as_ref(), position) else {
        return Ok(markdown_response(type_markdown));
    };

    let sorbet_level = SorbetLevel::for_document(global_state, &parsed);
    let mut response_builder = HoverResponseBuilder::default();
    {
        let mut index = global_state.index();
        let mut listener =
            Hover::new(&mut response_builder, global_state, &mut index, document.uri(), &node_context, sorbet_level);
        listener.dispatch(&target);
    }

    let existing_markdown = (!response_builder.is_empty()).then(|| response_builder.response());
    Ok(markdown_response(type_checking::merge_hover(type_markdown, existing_markdown)))
}

fn location_links_markdown(
    global_state: &GlobalState,
    analysis: &Analysis,
    hover_entry: &HoverEntry,
    document_uri: &str,
    position: Position,
) -> String {
    let workspace_path = PathBuf::from(global_state.workspace_path());
    if let HoverKind::Type(shown_type) = &hover_entry.kind {
        return type_links_markdown(global_state, shown_type, &workspace_path, document_uri, position);
    }
    let mut sections = Vec::new();

    let signature_links: Vec<String> = analysis
        .definitions
        .iter()
        .filter(|definition| definition.start == hover_entry.start && definition.end == hover_entry.end)
        .flat_map(|definition| definition.targets.iter())
        .map(|location| type_checking::signature_markdown_link(location, &workspace_path))
        .collect();
    if !signature_links.is_empty() {
        sections.push(format!("**Signature**: {}", joined_links(signature_links)));
    }

    let mut index = global_state.index();
    let implementation_ids = implementation_lookup::implementation_entries(&mut index, hover_entry).unwrap_or_default();
    let implementation_links: Vec<String> = implementation_ids
        .into_iter()
        .map(|id| index.entry(id))
        .filter(|entry| !is_at_position(entry, document_uri, position))
        .map(|entry| implementation_lookup::implementation_markdown_link(entry, &workspace_path))
        .collect();
    if !implementation_links.is_empty() {
        sections.push(format!("**Implementation**: {}", joined_links(implementation_links)));
    }

    sections.join("\n\n")
}

fn type_links_markdown(
    global_state: &GlobalState,
    shown_type: &Type,
    workspace_path: &Path,
    document_uri: &str,
    position: Position,
) -> String {
    let named_types = type_checking::referenced_named_types(shown_type);
    let labelled = named_types.len() > 1;
    let label = |named_type: &NamedType, links: String| {
        if labelled { format!("`{}` {links}", named_type.name()) } else { links }
    };
    let environment = global_state.type_checking.environment.as_deref();
    let core_directory = global_state.ruby_environment.rbs_core_path.as_deref().map(Path::new);
    let index = global_state.index();
    let mut signature_groups = Vec::new();
    let mut implementation_groups = Vec::new();
    for named_type in &named_types {
        if let Some(location) = environment.and_then(|environment| type_checking::type_declaration(environment, named_type)) {
            signature_groups.push(label(named_type, type_checking::signature_markdown_link(location, workspace_path)));
        }
        if let NamedType::Class { name, .. } = named_type {
            let core_class = environment
                .is_some_and(|environment| type_checking::is_core_class(environment, name, core_directory));
            let only_within = core_class.then_some(workspace_path);
            let implementation_links: Vec<String> = implementation_lookup::class_implementations(&index, name, only_within)
                .into_iter()
                .map(|id| index.entry(id))
                .filter(|entry| !is_at_position(entry, document_uri, position))
                .map(|entry| implementation_lookup::implementation_markdown_link(entry, workspace_path))
                .collect();
            if !implementation_links.is_empty() {
                implementation_groups.push(label(named_type, joined_links(implementation_links)));
            }
        }
    }

    let mut sections = Vec::new();
    if !signature_groups.is_empty() {
        sections.push(format!("**Signature**: {}", signature_groups.join(TYPE_GROUP_SEPARATOR)));
    }
    if !implementation_groups.is_empty() {
        sections.push(format!("**Implementation**: {}", implementation_groups.join(TYPE_GROUP_SEPARATOR)));
    }
    sections.join("\n\n")
}

fn is_at_position(entry: &Entry, document_uri: &str, position: Position) -> bool {
    entry.uri.as_ref() == document_uri && range_covers(range_from_location(&entry.name_location()), position)
}

/// Namespaces reopened in many files would otherwise fill the hover with links
fn joined_links(links: Vec<String>) -> String {
    let other_count = links.len().saturating_sub(MAXIMUM_LINKS_PER_SECTION);
    let mut joined = links.into_iter().take(MAXIMUM_LINKS_PER_SECTION).collect::<Vec<_>>().join(" | ");
    if other_count > 0 {
        joined.push_str(&format!(" | {other_count} other{}", if other_count > 1 { "s" } else { "" }));
    }
    joined
}

fn range_covers(range: Range, position: Position) -> bool {
    range.start <= position && position <= range.end
}

fn markdown_response(markdown: Option<String>) -> Value {
    match markdown {
        Some(markdown) => json!({ "contents": { "kind": "markdown", "value": markdown } }),
        None => Value::Null,
    }
}

fn refine_target<'pr>(
    parsed: &ParsedDocument<'_>,
    target: Option<&Node<'pr>>,
    parent: Option<&Node<'pr>>,
    position: Position,
) -> Option<Node<'pr>> {
    let target = target?;

    if let Some(parent) = parent
        && should_refine_target(parent, target)
    {
        return Some(determine_target(parsed, target.clone_node(), parent.clone_node(), position));
    }

    if position_outside_target(parsed, target, position) {
        return None;
    }

    Some(target.clone_node())
}

fn should_refine_target(parent: &Node<'_>, target: &Node<'_>) -> bool {
    (ALLOWED_TARGETS.contains(&parent.type_name()) && !ALLOWED_TARGETS.contains(&target.type_name()))
        || (parent.as_constant_path_node().is_some() && target.as_constant_read_node().is_some())
}

fn position_outside_target(parsed: &ParsedDocument<'_>, target: &Node<'_>, position: Position) -> bool {
    let name_location = match target {
        Node::GlobalVariableAndWriteNode { .. } => target.as_global_variable_and_write_node().map(|node| node.name_loc()),
        Node::GlobalVariableOperatorWriteNode { .. } => {
            target.as_global_variable_operator_write_node().map(|node| node.name_loc())
        }
        Node::GlobalVariableOrWriteNode { .. } => target.as_global_variable_or_write_node().map(|node| node.name_loc()),
        Node::GlobalVariableWriteNode { .. } => target.as_global_variable_write_node().map(|node| node.name_loc()),
        Node::CallNode { .. } => target.as_call_node().and_then(|call| call.message_loc()),
        _ => return false,
    };
    !location_covers_position(parsed, name_location.as_ref(), position)
}
