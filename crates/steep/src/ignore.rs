use std::sync::LazyLock;

use regex::Regex;

use crate::{
    diagnostic::ruby::{self as diagnostics, Diagnostic},
    ruby::node::{Buffer, Comment, SourceRange},
};

#[derive(Clone, Debug)]
pub enum Ignore {
    Start {
        comment: SourceRange,
        location: SourceRange,
        line: usize,
    },
    End {
        comment: SourceRange,
        location: SourceRange,
        line: usize,
    },
    Line {
        comment: SourceRange,
        diagnostics: Option<Vec<String>>,
        location: SourceRange,
        line: usize,
    },
}

impl Ignore {
    fn line(&self) -> usize {
        match self {
            Ignore::Start { line, .. } | Ignore::End { line, .. } | Ignore::Line { line, .. } => {
                *line
            }
        }
    }

    fn comment(&self) -> SourceRange {
        match self {
            Ignore::Start { comment, .. }
            | Ignore::End { comment, .. }
            | Ignore::Line { comment, .. } => *comment,
        }
    }

    fn location(&self) -> SourceRange {
        match self {
            Ignore::Start { location, .. }
            | Ignore::End { location, .. }
            | Ignore::Line { location, .. } => *location,
        }
    }
}

static START: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^steep:ignore:start\b").unwrap());
static END: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^steep:ignore:end\b").unwrap());
static LINE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^steep:ignore\b").unwrap());
static NAME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Z]\w*").unwrap());

fn skip_spaces(text: &str, position: &mut usize) {
    while text[*position..].starts_with(char::is_whitespace) {
        *position += text[*position..]
            .chars()
            .next()
            .map(char::len_utf8)
            .unwrap_or(1);
    }
}

pub fn parse(comment: &Comment, buffer: &Buffer) -> Option<Ignore> {
    if !comment.is_inline() {
        return None;
    }
    let text = &comment.text;
    let offset = comment.location.begin_pos;
    let line = buffer.line(offset);
    let mut position = 1;
    skip_spaces(text, &mut position);
    let range = |begin: usize, end: usize| SourceRange::new(offset + begin, offset + end);
    if let Some(matched) = START.find(&text[position..]) {
        let location = range(position, position + matched.end());
        position += matched.end();
        skip_spaces(text, &mut position);
        return (position == text.len()).then_some(Ignore::Start {
            comment: comment.location,
            location,
            line,
        });
    }
    if let Some(matched) = END.find(&text[position..]) {
        let location = range(position, position + matched.end());
        position += matched.end();
        skip_spaces(text, &mut position);
        return (position == text.len()).then_some(Ignore::End {
            comment: comment.location,
            location,
            line,
        });
    }
    let matched = LINE.find(&text[position..])?;
    let keyword_begin = position;
    position += matched.end();
    let mut end = position;
    let mut names = Vec::new();
    skip_spaces(text, &mut position);
    while let Some(name) = NAME.find(&text[position..]) {
        names.push(name.as_str().to_string());
        position += name.end();
        end = position;
        skip_spaces(text, &mut position);
        if text[position..].starts_with(',') {
            position += 1;
            end = position;
            skip_spaces(text, &mut position);
        } else {
            break;
        }
    }
    if position != text.len() {
        return None;
    }
    let diagnostics = if names.is_empty() || (names.len() == 1 && names[0] == "all") {
        None
    } else {
        Some(
            names
                .into_iter()
                .map(|name| name.trim_start_matches("Ruby::").to_string())
                .collect(),
        )
    };
    Some(Ignore::Line {
        comment: comment.location,
        diagnostics,
        location: range(keyword_begin, end),
        line,
    })
}

pub struct IgnoreRanges {
    error_ignores: Vec<Ignore>,
    ignored_lines: Vec<(usize, Ignore)>,
    ignored_ranges: Vec<(Ignore, Ignore)>,
}

impl IgnoreRanges {
    pub fn new(mut ignores: Vec<Ignore>) -> IgnoreRanges {
        ignores.sort_by_key(Ignore::line);
        let mut ranges = IgnoreRanges {
            error_ignores: Vec::new(),
            ignored_lines: Vec::new(),
            ignored_ranges: Vec::new(),
        };
        let mut last_start: Option<Ignore> = None;
        for ignore in ignores {
            match &ignore {
                Ignore::Start { .. } => {
                    if let Some(start) = last_start.take() {
                        ranges.error_ignores.push(start);
                    }
                    last_start = Some(ignore);
                }
                Ignore::End { .. } => match last_start.take() {
                    Some(start) => ranges.ignored_ranges.push((start, ignore)),
                    None => ranges.error_ignores.push(ignore),
                },
                Ignore::Line { line, .. } => {
                    if last_start.is_some() {
                        ranges.error_ignores.push(ignore);
                    } else {
                        let line = *line;
                        ranges
                            .ignored_lines
                            .retain(|(existing, _)| *existing != line);
                        ranges.ignored_lines.push((line, ignore));
                    }
                }
            }
        }
        if let Some(start) = last_start {
            ranges.error_ignores.push(start);
        }
        ranges
    }

    fn ignore_code(ignore: &Ignore, code: &str) -> bool {
        match ignore {
            Ignore::Line {
                diagnostics: None, ..
            } => true,
            Ignore::Line {
                diagnostics: Some(names),
                ..
            } => names.iter().any(|name| code == format!("Ruby::{name}")),
            _ => false,
        }
    }

    fn line_ignore(&self, line: usize) -> Option<usize> {
        self.ignored_lines
            .iter()
            .position(|(ignored, _)| *ignored == line)
    }

    pub fn covers(&self, location: SourceRange, buffer: &Buffer) -> bool {
        let start_line = buffer.line(location.begin_pos);
        let end_line = buffer.line(location.end_pos);
        let ignores_every_code = |line: usize| {
            self.line_ignore(line).is_some_and(|index| {
                matches!(
                    self.ignored_lines[index].1,
                    Ignore::Line {
                        diagnostics: None,
                        ..
                    }
                )
            })
        };
        ignores_every_code(start_line)
            || ignores_every_code(end_line)
            || self.ignored_ranges.iter().any(|(start, end)| {
                let range = start.line()..=end.line();
                range.contains(&start_line) && range.contains(&end_line)
            })
    }

    pub fn filter(&self, errors: Vec<Diagnostic>, buffer: &Buffer) -> Vec<Diagnostic> {
        let mut used_lines: Vec<usize> = Vec::new();
        let mut used_ranges: Vec<usize> = Vec::new();
        let mut kept = Vec::new();
        for error in errors {
            let Some(location) = error.location else {
                kept.push(error);
                continue;
            };
            let start_line = buffer.line(location.begin_pos);
            let end_line = buffer.line(location.end_pos);
            let code = error.diagnostic_code();
            let mut ignored = false;
            if let Some(index) = self.line_ignore(start_line)
                && Self::ignore_code(&self.ignored_lines[index].1, &code)
            {
                used_lines.push(index);
                ignored = true;
            }
            if !ignored
                && start_line != end_line
                && let Some(index) = self.line_ignore(end_line)
                && Self::ignore_code(&self.ignored_lines[index].1, &code)
            {
                used_lines.push(index);
                ignored = true;
            }
            if !ignored
                && let Some(index) = self.ignored_ranges.iter().position(|(start, end)| {
                    let range = start.line()..=end.line();
                    range.contains(&start_line) && range.contains(&end_line)
                })
            {
                used_ranges.push(index);
                ignored = true;
            }
            if !ignored {
                kept.push(error);
            }
        }
        for (index, (start, end)) in self.ignored_ranges.iter().enumerate() {
            if !used_ranges.contains(&index) {
                kept.push(diagnostics::redundant_ignore_comment(SourceRange::new(
                    start.location().begin_pos,
                    end.location().end_pos,
                )));
            }
        }
        for (index, (_, ignore)) in self.ignored_lines.iter().enumerate() {
            if !used_lines.contains(&index) {
                kept.push(diagnostics::redundant_ignore_comment(ignore.location()));
            }
        }
        for ignore in &self.error_ignores {
            kept.push(diagnostics::invalid_ignore_comment(ignore.comment()));
        }
        kept
    }
}
