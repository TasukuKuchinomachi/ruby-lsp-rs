use serde::Deserialize;
use serde_json::Value;

use super::support::common::delegate_request_if_needed;
use super::support::sorbet_level::SorbetLevel;
use super::{RequestContext, RequestResult, TextDocumentIdentifier, parse_params, to_result};
use crate::listeners::completion::{Completion, CompletionItem};
use crate::position::Position;

pub const NODE_TYPES: &[&str] = &[
    "CallNode",
    "ConstantReadNode",
    "ConstantPathNode",
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
    "ClassVariableAndWriteNode",
    "ClassVariableOperatorWriteNode",
    "ClassVariableOrWriteNode",
    "ClassVariableReadNode",
    "ClassVariableTargetNode",
    "ClassVariableWriteNode",
];

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CompletionContext {
    trigger_character: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CompletionParams {
    text_document: TextDocumentIdentifier,
    position: Position,
    #[serde(default)]
    context: Option<CompletionContext>,
}

pub fn handle(context: &mut RequestContext<'_>, params: Value) -> RequestResult {
    let params: CompletionParams = parse_params(params)?;
    let document = context.store.get(&params.text_document.uri)?;
    if !document.is_ruby_or_erb() {
        return Ok(Value::Null);
    }

    let global_state = &*context.global_state;
    let parsed = document.parse();
    let offset = parsed.offset_of(params.position)?;

    // Ruby LSP moves back by one character, and a position at the start of the file becomes -1, which Ruby reads as the
    // last character of the document when checking for delegation and which never matches any node
    let Some(character_position) = previous_character_offset(document.ruby_source(), offset) else {
        let last_offset = document.source().len().saturating_sub(1);
        delegate_request_if_needed(global_state, document, last_offset)?;
        return to_result(Vec::<CompletionItem>::new());
    };
    delegate_request_if_needed(global_state, document, character_position)?;

    let node_context = parsed.locate(character_position, NODE_TYPES);
    let (Some(matched), Some(parent)) = (node_context.node.as_ref(), node_context.parent.as_ref()) else {
        return to_result(Vec::<CompletionItem>::new());
    };
    let target = if parent.as_constant_path_node().is_some() && matched.as_constant_read_node().is_some() {
        parent
    } else {
        matched
    };

    let sorbet_level = SorbetLevel::for_document(global_state, &parsed);
    let trigger_character = params.context.as_ref().and_then(|context| context.trigger_character.as_deref());
    let mut index = global_state.index();
    let mut listener = Completion::new(&mut index, &parsed, &node_context, sorbet_level, document.uri(), trigger_character);
    listener.dispatch(target);
    to_result(listener.into_response())
}

fn previous_character_offset(source: &str, offset: usize) -> Option<usize> {
    if offset == 0 {
        return None;
    }
    let mut previous = offset - 1;
    while previous > 0 && !source.is_char_boundary(previous) {
        previous -= 1;
    }
    Some(previous)
}
