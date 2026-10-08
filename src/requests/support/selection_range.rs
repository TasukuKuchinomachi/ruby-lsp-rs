use serde::Serialize;

use crate::position::{Position, Range};

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SelectionRange {
    pub range: Range,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<Box<SelectionRange>>,
}

impl SelectionRange {
    pub fn covers(&self, position: Position) -> bool {
        covers(&self.range, position)
    }
}

pub fn covers(range: &Range, position: Position) -> bool {
    let start_covered = range.start.line < position.line
        || (range.start.line == position.line && range.start.character <= position.character);
    let end_covered =
        range.end.line > position.line || (range.end.line == position.line && range.end.character >= position.character);
    start_covered && end_covered
}
