use std::collections::HashMap;

use ruby_prism::{CommentType, ParseResult};

use super::configuration::Configuration;
use super::entry::Entry;
use crate::position::LineIndex;

pub struct CommentLine {
    pub text: String,
    pub trailing: bool,
    pub valid_encoding: bool,
}

/// Comments of a parsed file keyed by their 1-based start line
pub fn comments_by_line(parse_result: &ParseResult<'_>, line_index: &LineIndex<'_>) -> HashMap<usize, CommentLine> {
    let source = parse_result.source();
    parse_result
        .comments()
        .map(|comment| {
            let location = comment.location();
            let start = location.start_offset();
            let line = line_index.line_of_offset(start);
            let line_start = line_index.line_start(line).unwrap_or(0);
            let trailing = comment.type_() == CommentType::InlineComment
                && source[line_start..start].iter().any(|byte| !byte.is_ascii_whitespace() && *byte != 0);
            let bytes = location.as_slice();
            let valid_encoding = std::str::from_utf8(bytes).is_ok();
            (line + 1, CommentLine { text: String::from_utf8_lossy(bytes).into_owned(), trailing, valid_encoding })
        })
        .collect()
}

fn chomp(text: &str) -> String {
    text.strip_suffix("\r\n").or_else(|| text.strip_suffix('\n')).or_else(|| text.strip_suffix('\r')).unwrap_or(text).to_string()
}

fn strip_comment_prefix(content: &str) -> String {
    let content = content.strip_prefix('#').unwrap_or(content);
    content.strip_prefix(' ').unwrap_or(content).to_string()
}

/// Mirrors `DeclarationListener#collect_comments`
pub fn collect_comments(
    comments: &HashMap<usize, CommentLine>,
    source_lines: &[&str],
    node_start_line: usize,
    configuration: &Configuration,
) -> String {
    let comment_exists_at = |line: usize| -> bool {
        comments.contains_key(&line)
            || line.checked_sub(1).and_then(|index| source_lines.get(index)).is_some_and(|text| !text.trim().is_empty())
    };

    let mut start_line = node_start_line as isize - 1;
    if !comment_exists_at(start_line.max(0) as usize) || start_line <= 0 {
        start_line -= 1;
    }

    let mut collected: Vec<String> = Vec::new();
    let mut line = start_line;
    while line >= 1 {
        let Some(comment) = comments.get(&(line as usize)) else { break };
        if comment.trailing {
            break;
        }
        if comment.valid_encoding && !configuration.matches_magic_comment(&comment.text) {
            collected.push(strip_comment_prefix(&comment.text));
        }
        line -= 1;
    }

    collected.reverse();
    collected.join("\n")
}

/// Mirrors `Entry#comments`, which lazily reads comments from the file when they were not collected during indexing
pub fn comments_from_file(entry: &Entry, configuration: &Configuration) -> String {
    let Some(path) = entry.file_path() else { return String::new() };
    let Ok(source) = std::fs::read(&path) else { return String::new() };
    let parse_result = ruby_prism::parse(&source);
    let line_index = LineIndex::new(&source, configuration.encoding);

    let mut comments: Vec<(usize, Option<String>)> = parse_result
        .comments()
        .map(|comment| {
            let location = comment.location();
            let text = std::str::from_utf8(location.as_slice()).ok().map(str::to_string);
            (line_index.line_of_offset(location.start_offset()) + 1, text)
        })
        .collect();
    comments.sort_by_key(|(line, _)| *line);

    let mut groups: Vec<Vec<(usize, Option<String>)>> = Vec::new();
    for comment in comments {
        match groups.last_mut() {
            Some(group) if group.last().is_some_and(|(line, _)| line + 1 == comment.0) => group.push(comment),
            _ => groups.push(vec![comment]),
        }
    }

    let entry_line = entry.location.start_line as usize;
    let correct_group = groups.into_iter().find(|group| {
        let comment_end_line = group.last().map(|(line, _)| *line).unwrap_or(0);
        let target = entry_line as isize - 1;
        target >= comment_end_line as isize && target <= comment_end_line as isize + 1
    });

    match correct_group {
        Some(group) => group
            .into_iter()
            .filter_map(|(_, text)| {
                let content = chomp(&text?);
                if configuration.matches_magic_comment(&content) { None } else { Some(strip_comment_prefix(&content)) }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        None => String::new(),
    }
}
