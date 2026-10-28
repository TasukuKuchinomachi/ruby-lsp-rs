use std::path::{Path, PathBuf};

use rbs_environment::{MethodKind, NameKind, SignatureTarget};
use ruby_prism::Node;
use serde_json::Value;

use super::support::common::{delegate_request_if_needed, determine_target, location_covers_position};
use super::support::sorbet_level::SorbetLevel;
use super::{RequestContext, RequestResult, TextDocumentPositionParams, parse_params, to_result};
use crate::document::{Document, LanguageId, ParsedDocument};
use crate::global_state::GlobalState;
use crate::indexer::Index;
use crate::listeners::definition::{Definition, variable_name};
use crate::position::Position;
use crate::prism::NodeExt;
use crate::requests::support::implementation_lookup;
use crate::requests::support::local_variable_definition;
use crate::requests::support::interface::{LocationLink, LocationOrLink};
use crate::type_checking::{self, DefinitionTarget, NamedType};
use crate::type_inferrer::TypeInferrer;

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
    if document.language_id() == LanguageId::Rbs {
        return to_result(signature_definitions(context.global_state, document, params.position));
    }
    if !document.is_ruby_or_erb() {
        return Ok(Value::Null);
    }

    let analysis = context.global_state.type_checking.analysis_for(document);
    let global_state = &*context.global_state;
    let parsed = document.parse();
    let position = params.position;
    let offset = parsed.offset_of(position)?;
    delegate_request_if_needed(global_state, document, offset)?;

    if let Some((start, end)) = local_variable_definition::definition_range(&parsed.ast(), offset) {
        let range = parsed.line_index.range_of(start, end);
        let link = LocationLink { target_uri: document.uri().to_string(), target_range: range, target_selection_range: range };
        return to_result(vec![LocationOrLink::Link(link)]);
    }

    let signature_links = analysis
        .as_ref()
        .map(|analysis| type_checking::definition_links(analysis, offset, global_state.encoding))
        .unwrap_or_default();
    let definition_target = global_state.type_checking.settings.definition_target;
    if definition_target == DefinitionTarget::Signature && !signature_links.is_empty() {
        return to_result(merge_definitions(Vec::new(), signature_links));
    }

    let mut index = global_state.index();
    let typed_implementations = analysis.as_ref().and_then(|analysis| analysis.hover_at(offset)).and_then(|hover_entry| {
        implementation_lookup::implementation_entries(&mut index, hover_entry).map(|ids| {
            ids.into_iter().map(|id| implementation_lookup::implementation_link(index.entry(id))).collect::<Vec<_>>()
        })
    });

    let node_context = parsed.locate(offset, NODE_TYPES);
    let target = refine_target(&parsed, node_context.node.as_ref(), node_context.parent.as_ref(), position);
    let instance_variable_name =
        target.as_ref().filter(|target| target.type_name().starts_with("InstanceVariable")).and_then(variable_name);
    let name_based_definitions = match target {
        Some(target) => {
            let sorbet_level = SorbetLevel::for_document(global_state, &parsed);
            let mut listener =
                Definition::new(global_state, &mut index, document.language_id(), document.uri(), &node_context, sorbet_level);
            listener.dispatch(&target);
            listener.into_response()
        }
        None => Vec::new(),
    };

    let environment = global_state.type_checking.environment_for(document.uri());
    let instance_variable_declarations = match (instance_variable_name, environment) {
        (Some(variable_name), Some(environment)) => TypeInferrer::new(&mut index)
            .infer_receiver_type(&node_context)
            .map(|self_type| {
                let (class_name, singleton) = match self_type.name.split_once("::<Class:") {
                    Some((class_name, _)) => (class_name.to_string(), true),
                    None => (self_type.name, false),
                };
                type_checking::instance_variable_declaration_links(
                    environment,
                    &class_name,
                    &variable_name,
                    singleton,
                    global_state.encoding,
                )
            })
            .unwrap_or_default(),
        _ => Vec::new(),
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
            merge_definitions(merge_definitions(implementations, signature_links), instance_variable_declarations)
        }
    };
    to_result(definitions)
}

/// A type name leads to both its implementation and its declaration, while the name of a method, class or constant being
/// declared leads to its implementation, since its declaration is where the cursor already is
fn signature_definitions(global_state: &GlobalState, document: &Document, position: Position) -> Vec<LocationLink> {
    let (Some(environment), Some(path)) = (
        global_state.type_checking.environment_for(document.uri()),
        crate::uri::to_path_buf(document.uri()),
    ) else {
        return Vec::new();
    };
    let encoding = global_state.encoding;
    let targets = environment.signature_targets_at(&path, document.source(), |location| {
        type_checking::signature_location_covers(location, position, encoding)
    });
    let workspace_path = PathBuf::from(global_state.workspace_path());
    let core_directory = global_state.ruby_environment.rbs_core_path.as_deref().map(Path::new);
    let class_implementations = |index: &Index, class_name: &str| {
        let core_class = type_checking::is_core_class(environment, class_name, core_directory);
        implementation_lookup::preferred_class_implementations(index, class_name, core_class.then_some(&workspace_path))
    };
    let mut index = global_state.index();
    let mut links: Vec<LocationLink> = Vec::new();
    for target in targets {
        let (implementations, signature) = match target {
            SignatureTarget::TypeReference { kind, name } => {
                let named_type = match kind {
                    NameKind::Class => NamedType::Class { name, singleton: false },
                    NameKind::Interface => NamedType::Interface(name),
                    NameKind::Alias => NamedType::Alias(name),
                };
                let implementations = match &named_type {
                    NamedType::Class { name, .. } => class_implementations(&index, name),
                    NamedType::Interface(_) | NamedType::Alias(_) => Vec::new(),
                };
                let signature = type_checking::type_declaration(environment, &named_type)
                    .map(|location| type_checking::signature_location_link(location, encoding));
                (implementations, signature)
            }
            SignatureTarget::ClassDeclaration(name) => (class_implementations(&index, &name), None),
            SignatureTarget::ConstantDeclaration(name) => (implementation_lookup::constant_implementations(&index, &name), None),
            SignatureTarget::Method { owner, kind, name } => {
                (implementation_lookup::resolve_ruby_method(&mut index, &name, &owner, kind == MethodKind::Singleton), None)
            }
        };
        let implementation_links = implementations.into_iter().map(|id| implementation_lookup::implementation_link(index.entry(id)));
        for link in implementation_links.chain(signature) {
            if !links.contains(&link) {
                links.push(link);
            }
        }
    }
    links
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
                location.uri == type_definition.target_uri && location.range.start.line == type_definition.target_range.start.line
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
        return Some(determine_target(parsed, target.clone_node(), parent.clone_node(), position));
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
        Node::GlobalVariableAndWriteNode { .. } => target.as_global_variable_and_write_node().map(|node| node.name_loc()),
        Node::GlobalVariableOperatorWriteNode { .. } => {
            target.as_global_variable_operator_write_node().map(|node| node.name_loc())
        }
        Node::GlobalVariableOrWriteNode { .. } => target.as_global_variable_or_write_node().map(|node| node.name_loc()),
        Node::GlobalVariableWriteNode { .. } => target.as_global_variable_write_node().map(|node| node.name_loc()),
        Node::InstanceVariableAndWriteNode { .. } => target.as_instance_variable_and_write_node().map(|node| node.name_loc()),
        Node::InstanceVariableOperatorWriteNode { .. } => {
            target.as_instance_variable_operator_write_node().map(|node| node.name_loc())
        }
        Node::InstanceVariableOrWriteNode { .. } => target.as_instance_variable_or_write_node().map(|node| node.name_loc()),
        Node::InstanceVariableWriteNode { .. } => target.as_instance_variable_write_node().map(|node| node.name_loc()),
        Node::CallNode { .. } => {
            let message_location = target.as_call_node().and_then(|call| call.message_loc());
            return !location_covers_position(parsed, message_location.as_ref(), position);
        }
        _ => return false,
    };
    !location_covers_position(parsed, name_location.as_ref(), position)
}
