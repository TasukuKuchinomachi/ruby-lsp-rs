use std::path::{Path, PathBuf};

use serde_json::Value;

use super::prepare_rename::constant_target;
use super::support::common::range_from_location;
use super::support::interface::Location;
use super::{RequestContext, RequestError, RequestResult, TextDocumentPositionParams, parse_params, to_result};
use crate::indexer::Index;
use crate::indexer::reference_finder::{Target, find_references};
use crate::node_context::NodeContext;
use crate::prism::{NodeExt, constant_id_name, constant_name};
use crate::store::Store;
use crate::type_inferrer::TypeInferrer;

pub const NODE_TYPES: &[&str] = &[
    "ConstantReadNode",
    "ConstantPathNode",
    "ConstantPathTargetNode",
    "InstanceVariableAndWriteNode",
    "InstanceVariableOperatorWriteNode",
    "InstanceVariableOrWriteNode",
    "InstanceVariableReadNode",
    "InstanceVariableTargetNode",
    "InstanceVariableWriteNode",
    "CallNode",
    "DefNode",
];

pub fn handle(context: &mut RequestContext<'_>, params: Value) -> RequestResult {
    let params: TextDocumentPositionParams = parse_params(params)?;
    let global_state = &*context.global_state;
    let mut index = global_state.index();

    let reference_target = {
        let document = context.store.get(&params.text_document.uri)?;
        if !document.is_ruby() {
            return Ok(Value::Null);
        }

        let parsed = document.parse();
        let offset = parsed.offset_of(params.position)?;
        let node_context = parsed.locate(offset, NODE_TYPES);
        let Some(target) = constant_target(&parsed, &node_context, params.position) else {
            return to_result(Vec::<Location>::new());
        };
        create_reference_target(&mut index, &target, &node_context)?
    };
    let Some(reference_target) = reference_target else { return to_result(Vec::<Location>::new()) };

    let mut locations = Vec::new();
    for_each_workspace_document(&global_state.workspace_path(), context.store, |uri, source| {
        for reference in find_references(&reference_target, &mut index, source, uri, true) {
            locations.push(Location { uri: uri.to_string(), range: range_from_location(&reference.location) });
        }
    });
    to_result(locations)
}

fn create_reference_target(
    index: &mut Index,
    target: &ruby_prism::Node<'_>,
    node_context: &NodeContext<'_>,
) -> Result<Option<Target>, RequestError> {
    match target.type_name() {
        "ConstantReadNode" | "ConstantPathNode" | "ConstantPathTargetNode" => {
            Ok(resolve_constant_target(index, target, &node_context.nesting))
        }
        "InstanceVariableAndWriteNode"
        | "InstanceVariableOperatorWriteNode"
        | "InstanceVariableOrWriteNode"
        | "InstanceVariableReadNode"
        | "InstanceVariableTargetNode"
        | "InstanceVariableWriteNode" => {
            let Some(receiver_type) = TypeInferrer::new(index).infer_receiver_type(node_context) else { return Ok(None) };
            let owner_ancestors = index
                .linearized_ancestors_of(&receiver_type.name)
                .map_err(|_| RequestError::Internal(format!("No entry found for {}", receiver_type.name)))?;
            let Some(name) = variable_name(target) else { return Ok(None) };
            Ok(Some(Target::InstanceVariable { name, owner_ancestors }))
        }
        "CallNode" => Ok(target.as_call_node().map(|call| Target::Method { method_name: constant_id_name(&call.name()) })),
        "DefNode" => {
            Ok(target.as_def_node().map(|definition| Target::Method { method_name: constant_id_name(&definition.name()) }))
        }
        _ => Ok(None),
    }
}

pub fn resolve_constant_target(index: &mut Index, target: &ruby_prism::Node<'_>, nesting: &[String]) -> Option<Target> {
    let name = constant_name(target)?;
    let entries = index.resolve(&name, nesting)?;
    let fully_qualified_name = index.entry(*entries.first()?).name.clone();
    Some(Target::Constant { fully_qualified_name })
}

fn variable_name(target: &ruby_prism::Node<'_>) -> Option<String> {
    let name = match target {
        ruby_prism::Node::InstanceVariableAndWriteNode { .. } => target.as_instance_variable_and_write_node()?.name(),
        ruby_prism::Node::InstanceVariableOperatorWriteNode { .. } => {
            target.as_instance_variable_operator_write_node()?.name()
        }
        ruby_prism::Node::InstanceVariableOrWriteNode { .. } => target.as_instance_variable_or_write_node()?.name(),
        ruby_prism::Node::InstanceVariableReadNode { .. } => target.as_instance_variable_read_node()?.name(),
        ruby_prism::Node::InstanceVariableTargetNode { .. } => target.as_instance_variable_target_node()?.name(),
        ruby_prism::Node::InstanceVariableWriteNode { .. } => target.as_instance_variable_write_node()?.name(),
        _ => return None,
    };
    Some(constant_id_name(&name))
}

/// Mirrors `Dir.glob(File.join(workspace_path, "**/*.rb"))`: hidden entries are skipped, symbolic links to directories
/// are not followed and entries are visited depth first in sorted order
pub fn workspace_ruby_files(workspace_path: &str) -> Vec<PathBuf> {
    walkdir::WalkDir::new(workspace_path)
        .follow_links(false)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|entry| entry.depth() == 0 || !entry.file_name().to_string_lossy().starts_with('.'))
        .filter_map(Result::ok)
        .filter(|entry| entry.depth() > 0 && entry.file_name().to_string_lossy().ends_with(".rb"))
        .map(|entry| entry.into_path())
        .collect()
}

/// Visits the workspace files that the editor doesn't manage, read from disk, followed by the Ruby and ERB documents of
/// the store. Store documents are visited in URI order because the store, unlike Ruby LSP's, doesn't keep insertion
/// order
pub fn for_each_workspace_document(workspace_path: &str, store: &Store, mut visit: impl FnMut(&str, &[u8])) {
    for path in workspace_ruby_files(workspace_path) {
        let uri = crate::uri::from_path_buf(&path);
        if store.contains(&uri) {
            continue;
        }
        let Ok(source) = read_file(&path) else { continue };
        visit(&uri, &source);
    }

    let mut documents: Vec<_> = store.iter().filter(|(_, document)| document.is_ruby_or_erb()).collect();
    documents.sort_by_key(|(uri, _)| *uri);
    for (uri, document) in documents {
        visit(uri, document.ruby_source().as_bytes());
    }
}

fn read_file(path: &Path) -> std::io::Result<Vec<u8>> {
    if path.is_dir() {
        return Err(std::io::Error::other("is a directory"));
    }
    std::fs::read(path)
}
