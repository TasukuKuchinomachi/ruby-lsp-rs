use std::collections::HashMap;

use ruby_prism::{Node, ParseResult};
use serde_json::Value;

use crate::node_context::{self, NodeContext};
use crate::position::{Encoding, InvalidLocationError, LineIndex, Position, Range};
use crate::prism::{NodeExt, child_nodes, constant_id_name};

pub const MAXIMUM_CHARACTERS_FOR_EXPENSIVE_FEATURES: usize = 100_000;

const METHODS_THAT_CHANGE_DECLARATIONS: &[&str] = &[
    "private_constant",
    "attr_reader",
    "attr_writer",
    "attr_accessor",
    "alias_method",
    "include",
    "prepend",
    "extend",
    "public",
    "protected",
    "private",
    "module_function",
    "private_class_method",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LanguageId {
    Ruby,
    Erb,
    Rbs,
}

impl LanguageId {
    pub fn from_client_language_id(language_id: &str) -> LanguageId {
        match language_id {
            "erb" | "eruby" => LanguageId::Erb,
            "rbs" => LanguageId::Rbs,
            _ => LanguageId::Ruby,
        }
    }

    pub fn from_path(path: &str) -> LanguageId {
        let extension = std::path::Path::new(path).extension().map(|extension| extension.to_string_lossy().into_owned());
        match extension.as_deref() {
            Some("erb") | Some("rhtml") => LanguageId::Erb,
            Some("rbs") => LanguageId::Rbs,
            _ => LanguageId::Ruby,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            LanguageId::Ruby => "ruby",
            LanguageId::Erb => "erb",
            LanguageId::Rbs => "rbs",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edit {
    Insert(Range),
    Replace(Range),
    Delete(Range),
}

impl Edit {
    pub fn range(&self) -> Range {
        match self {
            Edit::Insert(range) | Edit::Replace(range) | Edit::Delete(range) => *range,
        }
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct ContentChange {
    #[serde(default)]
    pub range: Option<Range>,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct Document {
    uri: String,
    source: String,
    version: i32,
    language_id: LanguageId,
    encoding: Encoding,
    cache: HashMap<String, Value>,
    last_edit: Option<Edit>,
    indexing_pending: bool,
    parse_pending: bool,
    erb_sources: Option<ErbSources>,
    semantic_tokens: Option<Value>,
}

#[derive(Debug, Clone)]
struct ErbSources {
    ruby: String,
    host_language: String,
}

impl Document {
    pub fn new(uri: &str, source: String, version: i32, language_id: LanguageId, encoding: Encoding) -> Document {
        let mut document = Document {
            uri: uri.to_string(),
            source,
            version,
            language_id,
            encoding,
            cache: HashMap::new(),
            last_edit: None,
            indexing_pending: true,
            parse_pending: false,
            erb_sources: None,
            semantic_tokens: None,
        };
        document.refresh_derived_sources();
        document
    }

    pub fn uri(&self) -> &str {
        &self.uri
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn version(&self) -> i32 {
        self.version
    }

    pub fn language_id(&self) -> LanguageId {
        self.language_id
    }

    pub fn encoding(&self) -> Encoding {
        self.encoding
    }

    pub fn last_edit(&self) -> Option<Edit> {
        self.last_edit
    }

    pub fn is_ruby(&self) -> bool {
        self.language_id == LanguageId::Ruby
    }

    pub fn is_ruby_or_erb(&self) -> bool {
        matches!(self.language_id, LanguageId::Ruby | LanguageId::Erb)
    }

    pub fn host_language_source(&self) -> Option<&str> {
        self.erb_sources.as_ref().map(|sources| sources.host_language.as_str())
    }

    pub fn ruby_source(&self) -> &str {
        match &self.erb_sources {
            Some(sources) => &sources.ruby,
            None => &self.source,
        }
    }

    pub fn past_expensive_limit(&self) -> bool {
        self.source.len() > MAXIMUM_CHARACTERS_FOR_EXPENSIVE_FEATURES
            && self.source.chars().count() > MAXIMUM_CHARACTERS_FOR_EXPENSIVE_FEATURES
    }

    /// Unlike the request cache, the last semantic tokens survive edits because full/delta requests diff against them
    pub fn semantic_tokens(&self) -> Option<&Value> {
        self.semantic_tokens.as_ref()
    }

    pub fn set_semantic_tokens(&mut self, semantic_tokens: Value) {
        self.semantic_tokens = Some(semantic_tokens);
    }

    pub fn cache_get(&self, request_name: &str) -> Option<&Value> {
        self.cache.get(request_name)
    }

    pub fn cache_set(&mut self, request_name: &str, value: Value) -> Value {
        self.cache.insert(request_name.to_string(), value.clone());
        value
    }

    pub fn cache_fetch(&mut self, request_name: &str, compute: impl FnOnce(&Document) -> Value) -> Value {
        if let Some(cached) = self.cache.get(request_name) {
            return cached.clone();
        }
        let value = compute(self);
        self.cache_set(request_name, value)
    }

    pub fn clear_cache(&mut self, request_name: &str) {
        self.cache.remove(request_name);
    }

    pub fn line_index(&self) -> LineIndex<'_> {
        LineIndex::new(self.source.as_bytes(), self.encoding)
    }

    pub fn offset_of(&self, position: Position) -> Result<usize, InvalidLocationError> {
        self.line_index().offset_of(position)
    }

    pub fn push_edits(&mut self, edits: &[ContentChange], version: i32) -> Result<(), InvalidLocationError> {
        for edit in edits {
            match edit.range {
                Some(range) => {
                    let line_index = LineIndex::new(self.source.as_bytes(), self.encoding);
                    let start = line_index.offset_of(range.start)?;
                    let end = line_index.offset_of(range.end)?;
                    if start > end {
                        return Err(InvalidLocationError);
                    }
                    self.source.replace_range(start..end, &edit.text);
                }
                None => self.source = edit.text.clone(),
            }
        }

        self.version = version;
        self.cache.clear();
        self.indexing_pending = true;
        self.parse_pending = true;
        self.refresh_derived_sources();

        if let Some(last_edit) = edits.last() {
            self.last_edit = last_edit.range.map(|range| {
                if range.start == range.end {
                    Edit::Insert(range)
                } else if last_edit.text.is_empty() {
                    Edit::Delete(range)
                } else {
                    Edit::Replace(range)
                }
            });
        }
        Ok(())
    }

    fn refresh_derived_sources(&mut self) {
        if self.language_id == LanguageId::Erb {
            let (ruby, host_language) = scan_erb(&self.source);
            self.erb_sources = Some(ErbSources { ruby, host_language });
        } else {
            self.erb_sources = None;
        }
    }

    pub fn parse(&self) -> ParsedDocument<'_> {
        ParsedDocument {
            document: self,
            parse_result: ruby_prism::parse(self.ruby_source().as_bytes()),
            line_index: self.line_index(),
        }
    }

    pub fn syntax_error(&self) -> bool {
        match self.language_id {
            LanguageId::Rbs => false,
            _ => self.parse().syntax_error(),
        }
    }

    pub fn should_index(&self) -> bool {
        match self.last_edit {
            None => true,
            Some(Edit::Delete(_)) => true,
            Some(Edit::Insert(range)) | Some(Edit::Replace(range)) => {
                self.parse().position_may_impact_declarations(range.start)
            }
        }
    }

    /// A flag instead of Ruby LSP's check on the cached automatic requests (document symbol, folding ranges...), so that
    /// re-indexing doesn't depend on those requests caching their results
    pub fn indexing_pending(&self) -> bool {
        self.indexing_pending
    }

    pub fn mark_indexing_done(&mut self) {
        self.indexing_pending = false;
    }

    pub fn take_parse_pending(&mut self) -> bool {
        std::mem::replace(&mut self.parse_pending, false)
    }

    pub fn inside_host_language(&self, offset: usize) -> bool {
        match self.host_language_source() {
            Some(host_language) => host_language.as_bytes().get(offset).is_some_and(|byte| *byte != b' '),
            None => false,
        }
    }
}

impl PartialEq for Document {
    fn eq(&self, other: &Self) -> bool {
        self.language_id == other.language_id && self.uri == other.uri && self.source == other.source
    }
}

pub struct ParsedDocument<'a> {
    pub document: &'a Document,
    pub parse_result: ParseResult<'a>,
    pub line_index: LineIndex<'a>,
}

impl<'a> ParsedDocument<'a> {
    pub fn ast(&self) -> Node<'_> {
        self.parse_result.node()
    }

    pub fn syntax_error(&self) -> bool {
        self.parse_result.errors().next().is_some()
    }

    pub fn offset_of(&self, position: Position) -> Result<usize, InvalidLocationError> {
        self.line_index.offset_of(position)
    }

    pub fn locate_node(&self, position: Position, node_types: &[&str]) -> Result<NodeContext<'_>, InvalidLocationError> {
        let offset = self.offset_of(position)?;
        Ok(node_context::locate(&self.ast(), offset, node_types))
    }

    pub fn locate(&self, offset: usize, node_types: &[&str]) -> NodeContext<'_> {
        node_context::locate(&self.ast(), offset, node_types)
    }

    pub fn locate_first_within_range(&self, range: Range, node_types: &[&str]) -> Result<Option<Node<'_>>, InvalidLocationError> {
        let start = self.offset_of(range.start)?;
        let end = self.offset_of(range.end)?;
        let mut queue: std::collections::VecDeque<Node<'_>> = child_nodes(&self.ast()).into();

        while let Some(candidate) = queue.pop_front() {
            for child in child_nodes(&candidate).into_iter().rev() {
                queue.push_front(child);
            }

            let (node_start, node_end) = (candidate.start_offset(), candidate.end_offset());
            let covered = node_start < node_end && start <= node_start && node_end <= end;
            if covered && (node_types.is_empty() || node_types.contains(&candidate.type_name())) {
                return Ok(Some(candidate));
            }
        }
        Ok(None)
    }

    /// Columns are bytes rather than code units because Ruby LSP's `range_from_node` uses Prism's byte based columns
    pub fn range_from_node(&self, node: &Node<'_>) -> Range {
        self.byte_range(node.start_offset(), node.end_offset())
    }

    pub fn range_from_location(&self, location: &ruby_prism::Location<'_>) -> Range {
        self.byte_range(location.start_offset(), location.end_offset())
    }

    pub fn byte_range(&self, start_offset: usize, end_offset: usize) -> Range {
        Range::new(self.byte_position(start_offset), self.byte_position(end_offset))
    }

    pub fn byte_position(&self, offset: usize) -> Position {
        let line = self.line_index.line_of_offset(offset);
        let line_start = self.line_index.line_start(line).unwrap_or(0);
        Position::new(line as u32, offset.saturating_sub(line_start) as u32)
    }

    pub fn code_units_range(&self, start_offset: usize, end_offset: usize) -> Range {
        self.line_index.range_of(start_offset, end_offset)
    }

    pub fn magic_comment_value(&self, key: &str) -> Option<String> {
        self.parse_result
            .magic_comments()
            .find(|comment| comment.key() == key.as_bytes())
            .map(|comment| String::from_utf8_lossy(comment.value()).into_owned())
    }

    fn position_may_impact_declarations(&self, position: Position) -> bool {
        let Ok(node_context) = self.locate_node(position, &[]) else { return false };
        let mut node_at_edit = node_context.node;

        if node_at_edit
            .as_ref()
            .is_some_and(|node| node.as_constant_read_node().is_some() || node.as_constant_path_node().is_some())
        {
            node_at_edit = node_context.parent;
        }

        let Some(node) = node_at_edit else { return false };
        match node.type_name() {
            "ClassNode"
            | "ModuleNode"
            | "SingletonClassNode"
            | "DefNode"
            | "ConstantPathWriteNode"
            | "ConstantPathOrWriteNode"
            | "ConstantPathOperatorWriteNode"
            | "ConstantPathAndWriteNode"
            | "ConstantOrWriteNode"
            | "ConstantWriteNode"
            | "ConstantAndWriteNode"
            | "ConstantOperatorWriteNode"
            | "GlobalVariableAndWriteNode"
            | "GlobalVariableOperatorWriteNode"
            | "GlobalVariableOrWriteNode"
            | "GlobalVariableTargetNode"
            | "GlobalVariableWriteNode"
            | "InstanceVariableWriteNode"
            | "InstanceVariableAndWriteNode"
            | "InstanceVariableOperatorWriteNode"
            | "InstanceVariableOrWriteNode"
            | "InstanceVariableTargetNode"
            | "AliasMethodNode" => true,
            "MultiWriteNode" => {
                let multi_write = node.as_multi_write_node().expect("type name was checked");
                let mut targets: Vec<Node<'_>> = multi_write.lefts().iter().collect();
                targets.extend(multi_write.rest());
                targets.extend(multi_write.rights().iter());
                targets
                    .iter()
                    .any(|target| target.as_constant_target_node().is_some() || target.as_constant_path_target_node().is_some())
            }
            "CallNode" => {
                let call = node.as_call_node().expect("type name was checked");
                let self_receiver = call.receiver().is_none_or(|receiver| receiver.as_self_node().is_some());
                self_receiver && METHODS_THAT_CHANGE_DECLARATIONS.contains(&constant_id_name(&call.name()).as_str())
            }
            _ => false,
        }
    }
}

/// Splits an ERB template into the Ruby code and the host language code. Unlike Ruby LSP, which replaces each character
/// of the other language by a single space, every byte is replaced so that byte offsets are identical in all three
/// sources and editor positions can always be computed from the original template
pub fn scan_erb(source: &str) -> (String, String) {
    let bytes = source.as_bytes();
    let mut ruby: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut host_language: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut inside_ruby = false;
    let mut position = 0;

    let push = |ruby: &mut Vec<u8>, host_language: &mut Vec<u8>, inside_ruby: bool, text: &[u8]| {
        if inside_ruby {
            ruby.extend_from_slice(text);
            host_language.extend(std::iter::repeat_n(b' ', text.len()));
        } else {
            ruby.extend(std::iter::repeat_n(b' ', text.len()));
            host_language.extend_from_slice(text);
        }
    };

    while position < bytes.len() {
        let byte = bytes[position];
        let next = bytes.get(position + 1).copied();
        match byte {
            b'<' if next == Some(b'%') => {
                inside_ruby = true;
                ruby.extend_from_slice(b"  ");
                host_language.extend_from_slice(b"  ");
                position += 2;
                let after = bytes.get(position).copied();
                if after == Some(b'=') && bytes.get(position + 1) == Some(&b'=') {
                    ruby.extend_from_slice(b"  ");
                    host_language.extend_from_slice(b"  ");
                    position += 2;
                } else if after == Some(b'=') || after == Some(b'-') {
                    ruby.push(b' ');
                    host_language.push(b' ');
                    position += 1;
                }
                continue;
            }
            b'-' if inside_ruby && next == Some(b'%') && bytes.get(position + 2) == Some(&b'>') => {
                ruby.extend_from_slice(b"   ");
                host_language.extend_from_slice(b"   ");
                inside_ruby = false;
                position += 3;
                continue;
            }
            b'%' if inside_ruby && next == Some(b'>') => {
                ruby.extend_from_slice(b"  ");
                host_language.extend_from_slice(b"  ");
                inside_ruby = false;
                position += 2;
                continue;
            }
            b'\r' | b'\n' => {
                ruby.push(byte);
                host_language.push(byte);
            }
            _ => push(&mut ruby, &mut host_language, inside_ruby, &[byte]),
        }
        position += 1;
    }

    let to_string = |bytes: Vec<u8>| String::from_utf8(bytes).unwrap_or_else(|error| String::from_utf8_lossy(error.as_bytes()).into_owned());
    (to_string(ruby), to_string(host_language))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(start: (u32, u32), end: (u32, u32), text: &str) -> ContentChange {
        ContentChange {
            range: Some(Range::new(Position::new(start.0, start.1), Position::new(end.0, end.1))),
            text: text.to_string(),
        }
    }

    fn ruby_document(source: &str, encoding: Encoding) -> Document {
        Document::new("file:///fake.rb", source.to_string(), 1, LanguageId::Ruby, encoding)
    }

    #[test]
    fn valid_incremental_edits() {
        let mut document = ruby_document("def foo\nend\n", Encoding::Utf16);
        document.push_edits(&[change((0, 7), (0, 7), "(")], 2).unwrap();
        document.push_edits(&[change((0, 8), (0, 8), ")")], 3).unwrap();
        document.push_edits(&[change((0, 9), (0, 9), "\n  ")], 4).unwrap();
        document.push_edits(&[change((1, 2), (1, 2), "puts 'a'")], 5).unwrap();
        assert_eq!(document.source(), "def foo()\n  puts 'a'\nend\n");
        assert_eq!(document.version(), 5);
    }

    #[test]
    fn pushing_edit_on_empty_file() {
        for encoding in [Encoding::Utf8, Encoding::Utf16, Encoding::Utf32] {
            let mut document = ruby_document("", encoding);
            document.push_edits(&[change((0, 0), (0, 0), "def foo\nend\n")], 2).unwrap();
            assert_eq!(document.source(), "def foo\nend\n");
        }
    }

    #[test]
    fn pushing_edit_on_non_existing_location_fails() {
        let mut document = ruby_document("", Encoding::Utf16);
        assert!(document.push_edits(&[change((1, 0), (1, 0), "a")], 2).is_err());
    }

    #[test]
    fn multibyte_character_offsets() {
        let mut document = ruby_document("class Foo\n  'ˆ'\nend\n", Encoding::Utf8);
        document.push_edits(&[change((1, 6), (1, 6), "a")], 2).unwrap();
        assert_eq!(document.source(), "class Foo\n  'ˆ'a\nend\n");

        let mut document = ruby_document("class Foo\n  '😀'\nend\n", Encoding::Utf16);
        document.push_edits(&[change((1, 6), (1, 6), "a")], 2).unwrap();
        assert_eq!(document.source(), "class Foo\n  '😀'a\nend\n");

        let mut document = ruby_document("class Foo\n  '😀'\nend\n", Encoding::Utf32);
        document.push_edits(&[change((1, 5), (1, 5), "a")], 2).unwrap();
        assert_eq!(document.source(), "class Foo\n  '😀'a\nend\n");
    }

    #[test]
    fn tracks_latest_edit_kind() {
        let mut document = ruby_document("class Foo\nend\n", Encoding::Utf16);
        assert_eq!(document.last_edit(), None);
        document.push_edits(&[change((0, 9), (0, 9), "B")], 2).unwrap();
        assert!(matches!(document.last_edit(), Some(Edit::Insert(_))));
        document.push_edits(&[change((0, 9), (0, 10), "C")], 3).unwrap();
        assert!(matches!(document.last_edit(), Some(Edit::Replace(_))));
        document.push_edits(&[change((0, 9), (0, 10), "")], 4).unwrap();
        assert!(matches!(document.last_edit(), Some(Edit::Delete(_))));
    }

    #[test]
    fn should_index_for_edits_on_declarations_only() {
        let mut document = ruby_document("class Foo\n  def bar\n    x = 1\n  end\nend\n", Encoding::Utf16);
        assert!(document.should_index());
        document.push_edits(&[change((0, 9), (0, 9), "o")], 2).unwrap();
        assert!(document.should_index());
        document.push_edits(&[change((2, 8), (2, 9), "2")], 3).unwrap();
        assert!(!document.should_index());
        document.push_edits(&[change((2, 9), (2, 9), "")], 4).unwrap();
        assert!(document.should_index());
    }

    #[test]
    fn cache_set_and_get() {
        let mut document = ruby_document("", Encoding::Utf16);
        assert_eq!(document.cache_get("textDocument/semanticHighlighting"), None);
        document.cache_set("textDocument/semanticHighlighting", serde_json::json!([1, 2]));
        assert_eq!(document.cache_get("textDocument/semanticHighlighting"), Some(&serde_json::json!([1, 2])));
        document.push_edits(&[change((0, 0), (0, 0), "a")], 2).unwrap();
        assert_eq!(document.cache_get("textDocument/semanticHighlighting"), None);
    }

    #[test]
    fn locate_first_within_range() {
        let document = ruby_document("class Foo\n  def foo\n    bar\n  end\nend\n", Encoding::Utf16);
        let parsed = document.parse();
        let range = Range::new(Position::new(2, 4), Position::new(2, 7));
        let node = parsed.locate_first_within_range(range, &[]).unwrap().unwrap();
        assert_eq!(node.type_name(), "StatementsNode");
        let node = parsed.locate_first_within_range(range, &["CallNode"]).unwrap().unwrap();
        assert_eq!(node.type_name(), "CallNode");
        let node = parsed.locate_first_within_range(range, &["ConstantReadNode"]).unwrap();
        assert!(node.is_none());
    }

    #[test]
    fn scans_erb_preserving_offsets() {
        let source = "<ul>\n<% @items.each do |ö| %>\n  <li><%= ö %></li>\n<% end -%>\n</ul>";
        let (ruby, host_language) = scan_erb(source);
        assert_eq!(ruby.len(), source.len());
        assert_eq!(host_language.len(), source.len());
        assert_eq!(ruby, "    \n   @items.each do |ö|   \n          ö        \n   end    \n     ");
        assert!(host_language.starts_with("<ul>\n"));
    }
}
