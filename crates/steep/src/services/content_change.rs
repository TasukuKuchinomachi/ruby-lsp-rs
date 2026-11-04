#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Position {
    pub line: usize,
    pub column: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ContentChange {
    pub range: Option<(Position, Position)>,
    pub text: String,
}

fn byte_position(text: &str, position: Position) -> usize {
    let mut line_start = 0;
    for _ in 1..position.line {
        match text[line_start..].find('\n') {
            Some(index) => line_start += index + 1,
            None => return text.len(),
        }
    }
    let line = &text[line_start..];
    let line_end = line.find('\n').map_or(line.len(), |index| index + 1);
    line[..line_end]
        .char_indices()
        .nth(position.column)
        .map_or(line_start + line_end, |(index, _)| line_start + index)
}

impl ContentChange {
    pub fn string(text: impl Into<String>) -> ContentChange {
        ContentChange {
            range: None,
            text: text.into(),
        }
    }

    pub fn new(start: Position, end: Position, text: impl Into<String>) -> ContentChange {
        ContentChange {
            range: Some((start, end)),
            text: text.into(),
        }
    }

    pub fn apply_to(&self, text: &str) -> String {
        match self.range {
            Some((start, end)) => {
                let start = byte_position(text, start);
                let end = byte_position(text, end).max(start);
                let mut result = String::with_capacity(text.len() + self.text.len());
                result.push_str(&text[..start]);
                result.push_str(&self.text);
                result.push_str(&text[end..]);
                result
            }
            None => self.text.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_the_range_given_in_lines_and_characters() {
        let change = ContentChange::new(
            Position { line: 2, column: 4 },
            Position { line: 2, column: 7 },
            "baz",
        );
        assert_eq!(change.apply_to("a = 1\nfoo.bar\n"), "a = 1\nfoo.baz\n");
    }

    #[test]
    fn counts_columns_in_characters() {
        let change = ContentChange::new(
            Position { line: 1, column: 2 },
            Position { line: 1, column: 2 },
            "x",
        );
        assert_eq!(change.apply_to("あい"), "あいx");
    }

    #[test]
    fn replaces_the_whole_text_without_a_range() {
        assert_eq!(ContentChange::string("new").apply_to("old"), "new");
    }
}
