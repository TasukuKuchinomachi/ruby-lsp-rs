use std::sync::LazyLock;

use fancy_regex::Regex;
use ruby_prism::Node;
use serde::Deserialize;
use serde_json::Value;

use super::support::workspace_edit::TextEdit;
use super::{RequestContext, RequestResult, TextDocumentIdentifier, parse_params, to_result};
use crate::document::Document;
use crate::position::Position;
use crate::prism::NodeExt;

static END_REGEXES: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        r"(?m)\b(if|unless|for|while|until)\b($|\s|\()",
        r"(?m)\b(class|module|def|case)\b($|\s)",
        r"(?m).*\s\bdo\b($|\s)",
    ]
    .iter()
    .map(|pattern| Regex::new(pattern).expect("valid regex"))
    .collect()
});

static COMMENT_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^#(?!:)(\s*)").expect("valid regex"));
static HEREDOC_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(<<((-|~)?))(?<quote>['"`]?)(?<delimiter>\w+)\k<quote>"#).expect("valid regex")
});
static ENDLESS_METHOD_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bdef\s+[\w.]+[!?=]?(\([^)]*\))?\s*=[^=~>]").expect("valid regex"));
static PIPE_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"((?<=do)|(?<=\{))\s+\|").expect("valid regex"));
static CLOSING_PIPE_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)(do|\{)\s+\|[^|]*\|\|$").expect("valid regex"));
static INTERPOLATION_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"".*#\{"#).expect("valid regex"));
static SNIPPET_CLIENT_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"Visual Studio Code|Cursor|VSCodium|Windsurf").expect("valid regex"));

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OnTypeFormattingParams {
    text_document: TextDocumentIdentifier,
    position: Position,
    ch: String,
}

pub fn handle(context: &mut RequestContext<'_>, params: Value) -> RequestResult {
    let params: OnTypeFormattingParams = parse_params(params)?;
    let client_name = context.store.client_name.clone();
    let document = context.store.get(&params.text_document.uri)?;
    if !document.is_ruby() {
        return Ok(Value::Null);
    }

    to_result(OnTypeFormatting::new(document, params.position, &params.ch, &client_name).perform())
}

fn matches(regex: &Regex, text: &str) -> bool {
    regex.is_match(text).unwrap_or(false)
}

/// Mirrors Ruby's `String#strip`, which only removes ASCII whitespace and null characters
fn ruby_strip(text: &str) -> &str {
    text.trim_matches(|character: char| matches!(character, ' ' | '\t' | '\n' | '\u{0B}' | '\u{0C}' | '\r' | '\0'))
}

fn find_indentation(line: &str) -> usize {
    line.chars().take_while(|character| *character == ' ').count()
}

pub struct OnTypeFormatting<'a> {
    document: &'a Document,
    lines: Vec<&'a str>,
    indentation: usize,
    previous_line: String,
    position: Position,
    edits: Vec<TextEdit>,
    trigger_character: &'a str,
    client_name: &'a str,
}

impl<'a> OnTypeFormatting<'a> {
    pub fn new(
        document: &'a Document,
        position: Position,
        trigger_character: &'a str,
        client_name: &'a str,
    ) -> OnTypeFormatting<'a> {
        let lines: Vec<&str> = document.source().split_inclusive('\n').collect();
        let line = lines.get(position.line.saturating_sub(1) as usize).copied();
        let indentation = line.map(find_indentation).unwrap_or(0);
        let previous_line = line.map(|line| ruby_strip(line).to_string()).unwrap_or_default();
        OnTypeFormatting {
            document,
            lines,
            indentation,
            previous_line,
            position,
            edits: Vec::new(),
            trigger_character,
            client_name,
        }
    }

    pub fn perform(mut self) -> Vec<TextEdit> {
        match self.trigger_character {
            "{" => {
                if self.document.syntax_error() {
                    self.handle_curly_brace();
                }
            }
            "|" => {
                if self.document.syntax_error() {
                    self.handle_pipe();
                }
            }
            "\n" => {
                let comment_spaces = COMMENT_REGEX
                    .captures(&self.previous_line)
                    .ok()
                    .flatten()
                    .map(|captures| captures.get(1).map(|spaces| spaces.as_str().to_string()).unwrap_or_default());
                if let Some(spaces) = comment_spaces {
                    self.handle_comment_line(&spaces);
                } else if self.document.syntax_error() {
                    let heredoc_delimiter = HEREDOC_REGEX
                        .captures(&self.previous_line)
                        .ok()
                        .flatten()
                        .and_then(|captures| captures.name("delimiter").map(|delimiter| delimiter.as_str().to_string()));
                    match heredoc_delimiter {
                        Some(delimiter) => self.handle_heredoc_end(&delimiter),
                        None => self.handle_statement_end(),
                    }
                }
            }
            "d" => self.auto_indent_after_end_keyword(),
            _ => {}
        }
        self.edits
    }

    fn line(&self, line: u32) -> Option<&'a str> {
        self.lines.get(line as usize).copied()
    }

    fn handle_pipe(&mut self) {
        let Some(current_line) = self.line(self.position.line) else { return };
        if !matches(&PIPE_REGEX, current_line) {
            return;
        }

        let line_until_cursor: String = current_line.chars().take(self.position.character as usize + 1).collect();
        if matches(&CLOSING_PIPE_REGEX, &line_until_cursor) {
            let end = Position::new(self.position.line, self.position.character + 1);
            self.edits.push(TextEdit::new(crate::position::Range::new(self.position, end), ""));
        } else {
            self.add_edit_with_text("|", self.position);
        }

        self.move_cursor_to(self.position.line, self.position.character);
    }

    fn handle_curly_brace(&mut self) {
        if !matches(&INTERPOLATION_REGEX, &self.previous_line) {
            return;
        }

        self.add_edit_with_text("}", self.position);
        self.move_cursor_to(self.position.line, self.position.character);
    }

    fn handle_statement_end(&mut self) {
        if self.previous_line.starts_with(['"', '\'', '#']) {
            return;
        }

        if !END_REGEXES.iter().any(|regex| matches(regex, &self.previous_line)) {
            return;
        }

        if matches(&ENDLESS_METHOD_REGEX, &self.previous_line) {
            return;
        }

        let indents = " ".repeat(self.indentation);
        let current_line = self.line(self.position.line);
        let next_line = self.line(self.position.line + 1);

        if current_line.is_none_or(|line| ruby_strip(line).is_empty() || line.contains(')') || line.contains(']')) {
            self.add_edit_with_text("\n", self.position);
            self.add_edit_with_text(&format!("{indents}end"), self.position);
            self.move_cursor_to(self.position.line, (self.indentation + 2) as u32);
        } else if next_line.is_none_or(|line| ruby_strip(line).is_empty()) {
            let position = Position::new(self.position.line + 1, self.position.character);
            self.add_edit_with_text(&format!("{indents}end\n"), position);
            let character = self.indentation + self.previous_line.chars().count() + 1;
            self.move_cursor_to(self.position.line.saturating_sub(1), character as u32);
        }
    }

    fn handle_heredoc_end(&mut self, delimiter: &str) {
        let indents = " ".repeat(self.indentation);
        self.add_edit_with_text("\n", self.position);
        self.add_edit_with_text(&format!("{indents}{delimiter}"), self.position);
        self.move_cursor_to(self.position.line, (self.indentation + 2) as u32);
    }

    fn handle_comment_line(&mut self, spaces: &str) {
        self.add_edit_with_text(&format!("#{spaces}"), self.position);
    }

    fn add_edit_with_text(&mut self, text: &str, position: Position) {
        self.edits.push(TextEdit::insert(position, text));
    }

    fn move_cursor_to(&mut self, line: u32, character: u32) {
        if !matches(&SNIPPET_CLIENT_REGEX, self.client_name) {
            return;
        }

        self.edits.push(TextEdit::insert(Position::new(line, character), "$0"));
    }

    fn auto_indent_after_end_keyword(&mut self) {
        let Some(current_line) = self.line(self.position.line) else { return };
        if ruby_strip(current_line) != "end" {
            return;
        }

        let parsed = self.document.parse();
        let position = Position::new(self.position.line, self.position.character.saturating_sub(1));
        let Ok(node_context) = parsed.locate_node(position, &[]) else { return };
        let Some(target) = node_context.node else { return };

        let Some(statements) = statements_of(&target) else { return };

        let current_indentation = find_indentation(current_line);
        let mut lines_to_indent = Vec::new();
        for node in &statements.body() {
            let start = parsed.byte_position(node.start_offset());
            if start.character as usize != current_indentation {
                continue;
            }
            let end = parsed.byte_position(node.end_offset());
            lines_to_indent.extend(start.line..=end.line);
        }

        for line in lines_to_indent {
            self.add_edit_with_text("  ", Position::new(line, 0));
        }

        self.move_cursor_to(self.position.line, self.position.character);
    }
}

fn statements_of<'pr>(target: &Node<'pr>) -> Option<ruby_prism::StatementsNode<'pr>> {
    if let Some(node) = target.as_if_node() {
        return node.statements();
    }
    if let Some(node) = target.as_unless_node() {
        return node.statements();
    }
    if let Some(node) = target.as_for_node() {
        return node.statements();
    }
    if let Some(node) = target.as_while_node() {
        return node.statements();
    }
    if let Some(node) = target.as_until_node() {
        return node.statements();
    }
    None
}
