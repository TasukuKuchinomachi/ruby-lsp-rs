use serde::Deserialize;
use serde_json::Value;

use super::support::selection_range::{SelectionRange, covers};
use super::{RequestContext, RequestResult, TextDocumentIdentifier, parse_params, to_result};
use crate::document::ParsedDocument;
use crate::position::{Position, Range};
use crate::prism::child_nodes;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SelectionRangeParams {
    text_document: TextDocumentIdentifier,
    positions: Vec<Position>,
}

struct FlatSelectionRange {
    range: Range,
    parent: Option<usize>,
}

pub fn handle(context: &mut RequestContext<'_>, params: Value) -> RequestResult {
    let params: SelectionRangeParams = parse_params(params)?;
    let document = context.store.get(&params.text_document.uri)?;
    let ranges = if document.is_ruby_or_erb() { flat_selection_ranges(&document.parse()) } else { Vec::new() };

    let response: Vec<Option<SelectionRange>> = params
        .positions
        .iter()
        .map(|position| {
            ranges.iter().rposition(|range| covers(&range.range, *position)).map(|index| build_selection_range(&ranges, index))
        })
        .collect();
    to_result(response)
}

fn flat_selection_ranges(parsed: &ParsedDocument<'_>) -> Vec<FlatSelectionRange> {
    let mut ranges = Vec::new();
    let mut stack = vec![(parsed.ast(), None)];
    while let Some((node, parent)) = stack.pop() {
        let index = ranges.len();
        ranges.push(FlatSelectionRange { range: parsed.range_from_node(&node), parent });
        stack.extend(child_nodes(&node).into_iter().rev().map(|child| (child, Some(index))));
    }
    ranges
}

fn build_selection_range(ranges: &[FlatSelectionRange], index: usize) -> SelectionRange {
    let flat = &ranges[index];
    SelectionRange { range: flat.range, parent: flat.parent.map(|parent| Box::new(build_selection_range(ranges, parent))) }
}
