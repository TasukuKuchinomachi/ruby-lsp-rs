pub use lsp_types::{Position, Range};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Encoding {
    Utf8,
    #[default]
    Utf16,
    Utf32,
}

impl Encoding {
    pub fn kind_name(self) -> &'static str {
        match self {
            Encoding::Utf8 => "utf-8",
            Encoding::Utf16 => "utf-16",
            Encoding::Utf32 => "utf-32",
        }
    }

    pub fn code_units_of(self, text: &str) -> usize {
        match self {
            Encoding::Utf8 => text.len(),
            Encoding::Utf16 => text.chars().map(char::len_utf16).sum(),
            Encoding::Utf32 => text.chars().count(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub struct Location {
    pub start_line: u32,
    pub end_line: u32,
    pub start_column: u32,
    pub end_column: u32,
}

impl Location {
    pub fn to_range(&self) -> Range {
        Range::new(
            Position::new(self.start_line.saturating_sub(1), self.start_column),
            Position::new(self.end_line.saturating_sub(1), self.end_column),
        )
    }
}

#[derive(Debug, thiserror::Error)]
#[error("Invalid location")]
pub struct InvalidLocationError;

pub struct LineIndex<'a> {
    source: &'a [u8],
    line_starts: Vec<usize>,
    encoding: Encoding,
}

impl<'a> LineIndex<'a> {
    pub fn new(source: &'a [u8], encoding: Encoding) -> Self {
        let mut line_starts = vec![0];
        for (offset, byte) in source.iter().enumerate() {
            if *byte == b'\n' {
                line_starts.push(offset + 1);
            }
        }
        LineIndex { source, line_starts, encoding }
    }

    pub fn encoding(&self) -> Encoding {
        self.encoding
    }

    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    pub fn line_start(&self, line: usize) -> Option<usize> {
        self.line_starts.get(line).copied()
    }

    pub fn line_end(&self, line: usize) -> usize {
        match self.line_starts.get(line + 1) {
            Some(next_start) => next_start - 1,
            None => self.source.len(),
        }
    }

    pub fn line_of_offset(&self, offset: usize) -> usize {
        match self.line_starts.binary_search(&offset) {
            Ok(line) => line,
            Err(next_line) => next_line - 1,
        }
    }

    pub fn column_of_offset(&self, offset: usize) -> u32 {
        let line = self.line_of_offset(offset);
        let line_start = self.line_starts[line];
        self.code_units_between(line_start, offset) as u32
    }

    fn code_units_between(&self, start: usize, end: usize) -> usize {
        let end = end.min(self.source.len());
        if start >= end {
            return 0;
        }
        let slice = &self.source[start..end];
        match self.encoding {
            Encoding::Utf8 => slice.len(),
            Encoding::Utf16 => String::from_utf8_lossy(slice).chars().map(char::len_utf16).sum(),
            Encoding::Utf32 => String::from_utf8_lossy(slice).chars().count(),
        }
    }

    pub fn position_of_offset(&self, offset: usize) -> Position {
        let line = self.line_of_offset(offset);
        Position::new(line as u32, self.column_of_offset(offset))
    }

    /// 1-based line numbers, like Prism's `start_line`.
    pub fn location_of(&self, start_offset: usize, end_offset: usize) -> Location {
        let start_line = self.line_of_offset(start_offset);
        let end_line = self.line_of_offset(end_offset);
        Location {
            start_line: start_line as u32 + 1,
            end_line: end_line as u32 + 1,
            start_column: self.code_units_between(self.line_starts[start_line], start_offset) as u32,
            end_column: self.code_units_between(self.line_starts[end_line], end_offset) as u32,
        }
    }

    pub fn range_of(&self, start_offset: usize, end_offset: usize) -> Range {
        self.location_of(start_offset, end_offset).to_range()
    }

    pub fn offset_of(&self, position: Position) -> Result<usize, InvalidLocationError> {
        let line = position.line as usize;
        let line_start = *self.line_starts.get(line).ok_or(InvalidLocationError)?;
        let mut offset = line_start;
        let mut code_units = 0usize;
        let wanted = position.character as usize;
        while code_units < wanted {
            if offset >= self.source.len() {
                return Err(InvalidLocationError);
            }
            let character_length = utf8_character_length(self.source[offset]);
            code_units += match self.encoding {
                Encoding::Utf8 => character_length,
                Encoding::Utf16 => {
                    if character_length == 4 {
                        2
                    } else {
                        1
                    }
                }
                Encoding::Utf32 => 1,
            };
            offset += character_length;
        }
        Ok(offset.min(self.source.len()))
    }
}

fn utf8_character_length(byte: u8) -> usize {
    if byte < 0x80 {
        1
    } else if byte < 0xE0 {
        2
    } else if byte < 0xF0 {
        3
    } else {
        4
    }
}

pub fn range_covers(range: &Range, position: Position) -> bool {
    let start_covered = range.start.line < position.line
        || (range.start.line == position.line && range.start.character <= position.character);
    let end_covered = range.end.line > position.line
        || (range.end.line == position.line && range.end.character >= position.character);
    start_covered && end_covered
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_positions_for_each_encoding() {
        let source = "a😀b\nc";
        let utf16 = LineIndex::new(source.as_bytes(), Encoding::Utf16);
        assert_eq!(utf16.offset_of(Position::new(0, 3)).unwrap(), 5);
        assert_eq!(utf16.position_of_offset(5), Position::new(0, 3));
        let utf8 = LineIndex::new(source.as_bytes(), Encoding::Utf8);
        assert_eq!(utf8.offset_of(Position::new(0, 5)).unwrap(), 5);
        let utf32 = LineIndex::new(source.as_bytes(), Encoding::Utf32);
        assert_eq!(utf32.offset_of(Position::new(0, 2)).unwrap(), 5);
        assert_eq!(utf16.offset_of(Position::new(1, 1)).unwrap(), 8);
    }
}
