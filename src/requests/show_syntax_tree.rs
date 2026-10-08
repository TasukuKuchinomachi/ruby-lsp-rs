use std::collections::VecDeque;

use serde::Deserialize;
use serde_json::{Value, json};

use super::support::prism_inspect::Inspector;
use super::{RequestContext, RequestResult, TextDocumentIdentifier, parse_params};
use crate::document::ParsedDocument;
use crate::position::Range;
use crate::prism::{NodeExt, child_nodes};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShowSyntaxTreeParams {
    text_document: TextDocumentIdentifier,
    #[serde(default)]
    range: Option<Range>,
}

pub fn handle(context: &mut RequestContext<'_>, params: Value) -> RequestResult {
    let params: ShowSyntaxTreeParams = parse_params(params)?;
    let document = context.store.get(&params.text_document.uri)?;
    if !document.is_ruby() {
        return Ok(Value::Null);
    }

    let parsed = document.parse();
    let ast = match params.range {
        Some(range) => ast_for_range(&parsed, range)?,
        None => Inspector::new(&parsed.line_index).inspect(&parsed.ast()),
    };
    Ok(json!({ "ast": ast }))
}

fn ast_for_range(parsed: &ParsedDocument<'_>, range: Range) -> Result<String, super::RequestError> {
    let start_offset = parsed.offset_of(range.start)?;
    let end_offset = parsed.offset_of(range.end)?;

    let tree = parsed.ast();
    let mut queue: VecDeque<_> = tree
        .as_program_node()
        .map(|program| program.statements().body().iter().collect())
        .unwrap_or_default();
    let mut found_nodes = Vec::new();

    while let Some(node) = queue.pop_front() {
        if start_offset <= node.start_offset() && node.end_offset() <= end_offset {
            found_nodes.push(node);
        } else {
            for child in child_nodes(&node).into_iter().rev() {
                queue.push_front(child);
            }
        }
    }

    let inspector = Inspector::new(&parsed.line_index);
    Ok(found_nodes.iter().map(|node| inspector.inspect(node)).collect::<Vec<_>>().join("\n"))
}
