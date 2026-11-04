use ruby_prism::Node;
use serde_json::Value;

use super::support::common::{delegate_request_if_needed, determine_target, location_covers_position};
use super::support::sorbet_level::SorbetLevel;
use super::{RequestContext, RequestResult, TextDocumentPositionParams, parse_params, to_result};
use crate::document::{LanguageId, ParsedDocument};
use crate::listeners::definition::Definition;
use crate::position::Position;
use crate::prism::NodeExt;
use crate::requests::support::interface::{LocationLink, LocationOrLink};
use crate::requests::support::local_variable_definition;
use crate::type_checking::DefinitionTarget;
use crate::type_checking::worker::GotoKind;

pub const NODE_TYPES: &[&str] = &[
    "CallNode",
    "ConstantReadNode",
    "ConstantPathNode",
    "BlockArgumentNode",
    "GlobalVariableAndWriteNode",
    "GlobalVariableOperatorWriteNode",
    "GlobalVariableOrWriteNode",
    "GlobalVariableReadNode",
    "GlobalVariableTargetNode",
    "GlobalVariableWriteNode",
    "InstanceVariableReadNode",
    "InstanceVariableAndWriteNode",
    "InstanceVariableOperatorWriteNode",
    "InstanceVariableOrWriteNode",
    "InstanceVariableTargetNode",
    "InstanceVariableWriteNode",
    "SymbolNode",
    "StringNode",
    "SuperNode",
    "ForwardingSuperNode",
    "ClassVariableAndWriteNode",
    "ClassVariableOperatorWriteNode",
    "ClassVariableOrWriteNode",
    "ClassVariableReadNode",
    "ClassVariableTargetNode",
    "ClassVariableWriteNode",
];

pub fn handle(context: &mut RequestContext<'_>, params: Value) -> RequestResult {
    let params: TextDocumentPositionParams = parse_params(params)?;
    let document = context.store.get(&params.text_document.uri)?;
    let global_state = &*context.global_state;
    let position = params.position;
    if document.language_id() == LanguageId::Rbs {
        return match global_state
            .type_checking
            .goto(GotoKind::Definition, document.uri(), position)
        {
            Some(answer) => to_result(answer.links),
            None => Ok(Value::Null),
        };
    }
    if !document.is_ruby_or_erb() {
        return Ok(Value::Null);
    }

    let parsed = document.parse();
    let offset = parsed.offset_of(position)?;
    delegate_request_if_needed(global_state, document, offset)?;

    if let Some((start, end)) = local_variable_definition::definition_range(&parsed.ast(), offset) {
        let range = parsed.line_index.range_of(start, end);
        let link = LocationLink {
            target_uri: document.uri().to_string(),
            target_range: range,
            target_selection_range: range,
        };
        return to_result(vec![LocationOrLink::Link(link)]);
    }

    let type_checking = &global_state.type_checking;
    let (signature_links, instance_variable_declarations, typed_implementations) = if document.is_ruby() {
        let definition = type_checking
            .goto(GotoKind::Definition, document.uri(), position)
            .unwrap_or_default();
        let typed_implementations = type_checking
            .goto(GotoKind::Implementation, document.uri(), position)
            .map(|answer| answer.links);
        (
            definition.links,
            definition.instance_variable_links,
            typed_implementations,
        )
    } else {
        (Vec::new(), Vec::new(), None)
    };
    let definition_target = type_checking.settings.definition_target;
    if definition_target == DefinitionTarget::Signature && !signature_links.is_empty() {
        return to_result(merge_definitions(Vec::new(), signature_links));
    }

    let node_context = parsed.locate(offset, NODE_TYPES);
    let target = refine_target(
        &parsed,
        node_context.node.as_ref(),
        node_context.parent.as_ref(),
        position,
    );
    let name_based_definitions = match target {
        Some(target) => {
            let sorbet_level = SorbetLevel::for_document(global_state, &parsed);
            let mut index = global_state.index();
            let mut listener = Definition::new(
                global_state,
                &mut index,
                document.language_id(),
                document.uri(),
                &node_context,
                sorbet_level,
            );
            listener.dispatch(&target);
            listener.into_response()
        }
        None => Vec::new(),
    };

    let definitions = match definition_target {
        DefinitionTarget::Implementation => merge_definitions(
            implementations(typed_implementations, name_based_definitions, signature_links),
            instance_variable_declarations,
        ),
        DefinitionTarget::Signature if !instance_variable_declarations.is_empty() => {
            merge_definitions(Vec::new(), instance_variable_declarations)
        }
        DefinitionTarget::Signature => name_based_definitions,
        DefinitionTarget::Both => {
            let implementations = implementations(typed_implementations, name_based_definitions, Vec::new());
            merge_definitions(
                merge_definitions(implementations, signature_links),
                instance_variable_declarations,
            )
        }
    };
    to_result(definitions)
}

/// Implementations found from the inferred type are exact, while the name based lookup may list methods of unrelated
/// classes, so it's only used when the type checker doesn't know what is at the position. Signatures stand in for
/// methods without a Ruby implementation, like the core methods written in C
fn implementations(
    typed_implementations: Option<Vec<LocationLink>>,
    name_based_definitions: Vec<LocationOrLink>,
    signature_links: Vec<LocationLink>,
) -> Vec<LocationOrLink> {
    match typed_implementations {
        Some(typed_implementations) if !typed_implementations.is_empty() => {
            typed_implementations.into_iter().map(LocationOrLink::Link).collect()
        }
        Some(_) if !signature_links.is_empty() => merge_definitions(Vec::new(), signature_links),
        _ if !name_based_definitions.is_empty() => name_based_definitions,
        _ => merge_definitions(Vec::new(), signature_links),
    }
}

fn merge_definitions(mut definitions: Vec<LocationOrLink>, type_definitions: Vec<LocationLink>) -> Vec<LocationOrLink> {
    for type_definition in type_definitions {
        let already_present = definitions.iter().any(|definition| match definition {
            LocationOrLink::Link(link) => {
                link.target_uri == type_definition.target_uri
                    && link.target_range.start.line == type_definition.target_range.start.line
            }
            LocationOrLink::Location(location) => {
                location.uri == type_definition.target_uri
                    && location.range.start.line == type_definition.target_range.start.line
            }
        });
        if !already_present {
            definitions.push(LocationOrLink::Link(type_definition));
        }
    }
    definitions
}

fn refine_target<'pr>(
    parsed: &ParsedDocument<'_>,
    target: Option<&Node<'pr>>,
    parent: Option<&Node<'pr>>,
    position: Position,
) -> Option<Node<'pr>> {
    let target = target?;

    if target.as_constant_read_node().is_some()
        && let Some(parent) = parent.filter(|parent| parent.as_constant_path_node().is_some())
    {
        return Some(determine_target(
            parsed,
            target.clone_node(),
            parent.clone_node(),
            position,
        ));
    }

    if position_outside_target(parsed, target, position) {
        return None;
    }

    if target.as_symbol_node().is_some()
        && let Some(parent) = parent.filter(|parent| parent.as_block_argument_node().is_some())
    {
        return Some(parent.clone_node());
    }

    Some(target.clone_node())
}

fn position_outside_target(parsed: &ParsedDocument<'_>, target: &Node<'_>, position: Position) -> bool {
    let name_location = match target {
        Node::GlobalVariableAndWriteNode { .. } => {
            target.as_global_variable_and_write_node().map(|node| node.name_loc())
        }
        Node::GlobalVariableOperatorWriteNode { .. } => target
            .as_global_variable_operator_write_node()
            .map(|node| node.name_loc()),
        Node::GlobalVariableOrWriteNode { .. } => target.as_global_variable_or_write_node().map(|node| node.name_loc()),
        Node::GlobalVariableWriteNode { .. } => target.as_global_variable_write_node().map(|node| node.name_loc()),
        Node::InstanceVariableAndWriteNode { .. } => {
            target.as_instance_variable_and_write_node().map(|node| node.name_loc())
        }
        Node::InstanceVariableOperatorWriteNode { .. } => target
            .as_instance_variable_operator_write_node()
            .map(|node| node.name_loc()),
        Node::InstanceVariableOrWriteNode { .. } => {
            target.as_instance_variable_or_write_node().map(|node| node.name_loc())
        }
        Node::InstanceVariableWriteNode { .. } => target.as_instance_variable_write_node().map(|node| node.name_loc()),
        Node::CallNode { .. } => {
            let message_location = target.as_call_node().and_then(|call| call.message_loc());
            return !location_covers_position(parsed, message_location.as_ref(), position);
        }
        _ => return false,
    };
    !location_covers_position(parsed, name_location.as_ref(), position)
}
