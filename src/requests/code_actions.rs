use serde::Deserialize;
use serde_json::{Value, json};

use super::support::workspace_edit::CodeAction;
use super::{RequestContext, RequestResult, TextDocumentIdentifier, parse_params, to_result};
use crate::document::ParsedDocument;
use crate::position::{InvalidLocationError, Range};
use crate::prism::NodeExt;

pub const EXTRACT_TO_VARIABLE_TITLE: &str = "Refactor: Extract Variable";
pub const EXTRACT_TO_METHOD_TITLE: &str = "Refactor: Extract Method";
pub const TOGGLE_BLOCK_STYLE_TITLE: &str = "Refactor: Toggle block style";
pub const CREATE_ATTRIBUTE_READER: &str = "Create Attribute Reader";
pub const CREATE_ATTRIBUTE_WRITER: &str = "Create Attribute Writer";
pub const CREATE_ATTRIBUTE_ACCESSOR: &str = "Create Attribute Accessor";

pub const INSTANCE_VARIABLE_NODES: &[&str] = &[
    "InstanceVariableAndWriteNode",
    "InstanceVariableOperatorWriteNode",
    "InstanceVariableOrWriteNode",
    "InstanceVariableReadNode",
    "InstanceVariableTargetNode",
    "InstanceVariableWriteNode",
];

const REFACTOR_EXTRACT: &str = "refactor.extract";
const REFACTOR_REWRITE: &str = "refactor.rewrite";
const EMPTY_KIND: &str = "";

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodeActionParams {
    text_document: TextDocumentIdentifier,
    range: Value,
    #[serde(default)]
    context: Value,
}

pub fn handle(context: &mut RequestContext<'_>, params: Value) -> RequestResult {
    let params: CodeActionParams = parse_params(params)?;
    let range: Range = parse_params(params.range.clone())?;
    let document = context.store.get(&params.text_document.uri)?;
    if !document.is_ruby_or_erb() {
        return Ok(Value::Null);
    }

    let mut code_actions: Vec<Value> = Vec::new();
    if let Some(diagnostics) = params.context["diagnostics"].as_array() {
        for diagnostic in diagnostics {
            if let Some(actions) = diagnostic["data"]["code_actions"].as_array() {
                code_actions.extend(actions.iter().cloned());
            }
        }
    }

    let uri = document.uri().to_string();
    let data = json!({ "range": params.range, "uri": uri });
    let action = |title: &str, kind: &str| -> Value {
        serde_json::to_value(CodeAction {
            title: title.to_string(),
            kind: Some(kind.to_string()),
            edit: None,
            data: Some(data.clone()),
        })
        .expect("code actions serialize")
    };

    if range.start != range.end {
        code_actions.push(action(EXTRACT_TO_VARIABLE_TITLE, REFACTOR_EXTRACT));
        code_actions.push(action(EXTRACT_TO_METHOD_TITLE, REFACTOR_EXTRACT));
    }

    let parsed = document.parse();
    if toggle_block_style_available(&parsed, range)? {
        code_actions.push(action(TOGGLE_BLOCK_STYLE_TITLE, REFACTOR_REWRITE));
    }

    if document.is_ruby() && attribute_actions_available(&parsed, range)? {
        code_actions.push(action(CREATE_ATTRIBUTE_READER, EMPTY_KIND));
        code_actions.push(action(CREATE_ATTRIBUTE_WRITER, EMPTY_KIND));
        code_actions.push(action(CREATE_ATTRIBUTE_ACCESSOR, EMPTY_KIND));
    }

    to_result(code_actions)
}

fn toggle_block_style_available(parsed: &ParsedDocument<'_>, range: Range) -> Result<bool, InvalidLocationError> {
    if range.start != range.end {
        return Ok(true);
    }
    let node_context = parsed.locate_node(range.start, &["BlockNode"])?;
    Ok(node_context.node.is_some_and(|node| node.as_block_node().is_some()))
}

fn attribute_actions_available(parsed: &ParsedDocument<'_>, range: Range) -> Result<bool, InvalidLocationError> {
    if range.start != range.end && parsed.locate_first_within_range(range, INSTANCE_VARIABLE_NODES)?.is_some() {
        return Ok(true);
    }

    let node_context = parsed.locate_node(range.start, INSTANCE_VARIABLE_NODES)?;
    Ok(node_context.node.is_some_and(|node| INSTANCE_VARIABLE_NODES.contains(&node.type_name())))
}
