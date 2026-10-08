use serde::Serialize;

use crate::position::{Encoding, LineIndex};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenType {
    Namespace = 0,
    Type = 1,
    Class = 2,
    Enum = 3,
    Interface = 4,
    Struct = 5,
    TypeParameter = 6,
    Parameter = 7,
    Variable = 8,
    Property = 9,
    EnumMember = 10,
    Event = 11,
    Function = 12,
    Method = 13,
    Macro = 14,
    Keyword = 15,
    Modifier = 16,
    Comment = 17,
    String = 18,
    Number = 19,
    Regexp = 20,
    Operator = 21,
    Decorator = 22,
}

pub const TOKEN_TYPES: &[&str] = &[
    "namespace",
    "type",
    "class",
    "enum",
    "interface",
    "struct",
    "typeParameter",
    "parameter",
    "variable",
    "property",
    "enumMember",
    "event",
    "function",
    "method",
    "macro",
    "keyword",
    "modifier",
    "comment",
    "string",
    "number",
    "regexp",
    "operator",
    "decorator",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenModifier {
    Declaration = 0,
    Definition = 1,
    Readonly = 2,
    Static = 3,
    Deprecated = 4,
    Abstract = 5,
    Async = 6,
    Modification = 7,
    Documentation = 8,
    DefaultLibrary = 9,
}

pub const TOKEN_MODIFIERS: &[&str] = &[
    "declaration",
    "definition",
    "readonly",
    "static",
    "deprecated",
    "abstract",
    "async",
    "modification",
    "documentation",
    "defaultLibrary",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticToken {
    pub start_line: usize,
    pub start_code_unit_column: usize,
    pub length: usize,
    pub token_type: u32,
    pub modifier: Vec<u32>,
}

impl SemanticToken {
    pub fn replace_type(&mut self, token_type: TokenType) {
        self.token_type = token_type as u32;
    }

    pub fn replace_modifier(&mut self, modifiers: &[TokenModifier]) {
        self.modifier = modifiers.iter().map(|modifier| *modifier as u32).collect();
    }
}

pub struct SemanticHighlightingResponseBuilder<'a> {
    source: &'a [u8],
    line_index: LineIndex<'a>,
    encoding: Encoding,
    stack: Vec<SemanticToken>,
}

impl<'a> SemanticHighlightingResponseBuilder<'a> {
    pub fn new(source: &'a [u8], encoding: Encoding) -> SemanticHighlightingResponseBuilder<'a> {
        SemanticHighlightingResponseBuilder { source, line_index: LineIndex::new(source, encoding), encoding, stack: Vec::new() }
    }

    pub fn add_token(&mut self, start_offset: usize, end_offset: usize, token_type: TokenType, modifiers: &[TokenModifier]) {
        let token = SemanticToken {
            start_line: self.line_index.line_of_offset(start_offset) + 1,
            start_code_unit_column: self.code_unit_column(start_offset),
            length: self.code_units_between(start_offset, end_offset),
            token_type: token_type as u32,
            modifier: modifiers.iter().map(|modifier| *modifier as u32).collect(),
        };
        self.stack.push(token);
    }

    pub fn last_token_matches(&self, start_offset: usize) -> bool {
        self.stack.last().is_some_and(|token| {
            token.start_line == self.line_index.line_of_offset(start_offset) + 1
                && token.start_code_unit_column == self.code_unit_column(start_offset)
        })
    }

    pub fn last(&mut self) -> Option<&mut SemanticToken> {
        self.stack.last_mut()
    }

    pub fn into_response(self) -> Vec<SemanticToken> {
        self.stack
    }

    fn code_unit_column(&self, offset: usize) -> usize {
        let line_start = self.line_index.line_start(self.line_index.line_of_offset(offset)).unwrap_or(0);
        self.code_units_between(line_start, offset)
    }

    /// Prism's code units cache counts characters for every encoding other than UTF-16, so UTF-8 lengths and columns are
    /// characters rather than bytes, exactly like Ruby LSP's responses
    fn code_units_between(&self, start_offset: usize, end_offset: usize) -> usize {
        let end_offset = end_offset.min(self.source.len());
        if start_offset >= end_offset {
            return 0;
        }
        let text = String::from_utf8_lossy(&self.source[start_offset..end_offset]);
        match self.encoding {
            Encoding::Utf16 => text.chars().map(char::len_utf16).sum(),
            Encoding::Utf8 | Encoding::Utf32 => text.chars().count(),
        }
    }
}

pub fn encode(tokens: &[SemanticToken]) -> Vec<u32> {
    let mut sorted_tokens: Vec<&SemanticToken> = tokens.iter().collect();
    sorted_tokens.sort_by_key(|token| (token.start_line, token.start_code_unit_column));

    let mut current_row = 0usize;
    let mut current_column = 0usize;
    let mut data = Vec::with_capacity(sorted_tokens.len() * 5);
    for token in sorted_tokens {
        let row = token.start_line - 1;
        let column = token.start_code_unit_column;
        let delta_line = row - current_row;
        let delta_column = if delta_line == 0 { column - current_column } else { column };
        data.extend([
            delta_line as u32,
            delta_column as u32,
            token.length as u32,
            token.token_type,
            encode_modifiers(&token.modifier),
        ]);
        current_row = row;
        current_column = column;
    }
    data
}

pub fn encode_modifiers(modifiers: &[u32]) -> u32 {
    modifiers.iter().fold(0, |encoded_modifiers, modifier| encoded_modifiers | (1 << modifier))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticTokens {
    pub result_id: String,
    pub data: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticTokensEdit {
    pub start: usize,
    pub delete_count: usize,
    pub data: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticTokensDelta {
    pub result_id: String,
    pub edits: Vec<SemanticTokensEdit>,
}
