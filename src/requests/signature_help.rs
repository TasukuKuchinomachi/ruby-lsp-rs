use ruby_prism::{CallNode, Node};
use serde_json::Value;
use type_checker::{Analysis, HoverKind};

use super::support::common::delegate_request_if_needed;
use super::support::sorbet_level::SorbetLevel;
use super::{RequestContext, RequestResult, TextDocumentPositionParams, parse_params, to_result};
use crate::document::{Document, ParsedDocument};
use crate::listeners::signature_help::{SignatureHelp, SignatureHelpListener, overload_signature_help};
use crate::node_context::NodeContext;
use crate::position::{Position, range_covers};
use crate::prism::NodeExt;

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

    let node_context = locate_call(&parsed, document.ruby_source(), offset);
    let Some(target) = adjust_for_nested_target(&parsed, node_context.node.as_ref(), node_context.parent.as_ref(), position)
    else {
        return Ok(Value::Null);
    };

    let sorbet_level = SorbetLevel::for_document(global_state, &parsed);
    if !sorbet_level.is_true_or_higher()
        && let Some(analysis) = analysis.as_deref()
        && let Some(call) = target.as_call_node()
        && let Some(response) = type_signature_help(analysis, &call, document, offset)
    {
        return to_result(response);
    }

    let mut index = global_state.index();
    let mut listener = SignatureHelpListener::new(&mut index, document.ruby_source().as_bytes(), &node_context, sorbet_level);
    listener.dispatch(&target);
    to_result(listener.into_response())
}

/// Nodes never cover their end offset, so a call missing its closing parenthesis, like `foo.bar(1, ` at the end of a
/// line, is looked up from the last character before the cursor
fn locate_call<'a>(parsed: &'a ParsedDocument<'_>, source: &str, offset: usize) -> NodeContext<'a> {
    let node_context = parsed.locate(offset, &["CallNode"]);
    if node_context.node.as_ref().is_some_and(|node| node.as_call_node().is_some()) {
        return node_context;
    }
    let Some(last_character_offset) = source[..offset].trim_end().len().checked_sub(1) else { return node_context };
    let previous_context = parsed.locate(last_character_offset, &["CallNode"]);
    let is_unclosed_call = previous_context.node.as_ref().and_then(|node| node.as_call_node()).is_some_and(|call| {
        call.as_node().end_offset() == last_character_offset + 1
            && call.closing_loc().is_some_and(|closing| closing.start_offset() == closing.end_offset())
    });
    if is_unclosed_call { previous_context } else { node_context }
}

fn adjust_for_nested_target<'pr>(
    parsed: &ParsedDocument<'_>,
    target: Option<&Node<'pr>>,
    parent: Option<&Node<'pr>>,
    position: Position,
) -> Option<Node<'pr>> {
    let Some(parent) = parent.filter(|parent| parent.as_call_node().is_some()) else {
        return target.map(NodeExt::clone_node);
    };
    let Some(target_call) = target.and_then(|target| target.as_call_node()) else {
        return Some(parent.clone_node());
    };

    match target_call.arguments() {
        Some(arguments) if range_covers(&parsed.range_from_node(&arguments.as_node()), position) => {
            target.map(NodeExt::clone_node)
        }
        _ => Some(parent.clone_node()),
    }
}

fn type_signature_help(analysis: &Analysis, call: &CallNode<'_>, document: &Document, offset: usize) -> Option<SignatureHelp> {
    let message_location = call.message_loc()?;
    let hover_entry = analysis.hover_at(message_location.start_offset())?;
    if hover_entry.start != message_location.start_offset() || hover_entry.end != message_location.end_offset() {
        return None;
    }
    let HoverKind::Method { owner, method_name, overloads, comment, .. } = &hover_entry.kind else { return None };
    let mut documentation = vec![format!("Defined in `{owner}`")];
    documentation.extend(comment.as_ref().map(|comment| comment.trim().to_string()).filter(|comment| !comment.is_empty()));
    overload_signature_help(
        call,
        method_name,
        overloads,
        &documentation.join("\n\n"),
        document.ruby_source().as_bytes(),
        offset,
        document.encoding(),
    )
}
