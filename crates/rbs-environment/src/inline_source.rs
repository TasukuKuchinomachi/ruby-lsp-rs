//! Inline RBS declarations (`check "lib", inline: true` in a `Steepfile`) read the way RBS 4.2.0's `RBS::InlineParser`
//! reads them, and written out as RBS text so that the declarations go through the same conversion and validation as
//! `.rbs` files. The text copies every name and type from the Ruby file and records where, so that locations in it
//! map back to the Ruby file.

use std::collections::HashMap;
use std::ops::Range;

use ruby_prism::{Node, Visit};
use ruby_rbs_sys::bindings::{
    rbs_encoding_t, rbs_encoding_type_t, rbs_encodings, rbs_parser_free, rbs_parser_new,
    rbs_parser_t, rbs_string_new,
};

pub(crate) struct InlineSignature {
    pub(crate) rbs: String,
    pub(crate) copied_spans: Vec<CopiedSpan>,
    pub(crate) methods: Vec<InlineMethod>,
    pub(crate) diagnostics: Vec<InlineDiagnostic>,
}

/// `length` bytes of the RBS text from `rbs_start` are the same bytes of the Ruby file from `source_start`
pub(crate) struct CopiedSpan {
    pub(crate) rbs_start: usize,
    pub(crate) source_start: usize,
    pub(crate) length: usize,
}

/// A method definition in the RBS text, which starts with the `def` keyword at `rbs_start`
pub(crate) struct InlineMethod {
    pub(crate) rbs_start: usize,
    /// The Ruby `def` node, which RBS uses as the location of the method
    pub(crate) source_range: Range<usize>,
    /// RBS gives a method without annotations the type of the method it overrides, or `(?) -> untyped`
    pub(crate) annotated: bool,
}

/// A diagnostic of `RBS::InlineParser`, which Steep reports as `RBS::InlineDiagnostic`
pub(crate) struct InlineDiagnostic {
    pub(crate) range: Range<usize>,
    pub(crate) message: String,
}

impl InlineSignature {
    /// An offset in a copied span maps to the same byte of the Ruby file, and an offset between spans to the end of
    /// the span before it
    pub(crate) fn source_offset(&self, rbs_offset: usize) -> usize {
        let index = self
            .copied_spans
            .partition_point(|span| span.rbs_start <= rbs_offset);
        let Some(span) = index
            .checked_sub(1)
            .and_then(|index| self.copied_spans.get(index))
        else {
            return 0;
        };
        span.source_start + (rbs_offset - span.rbs_start).min(span.length)
    }

    pub(crate) fn method_starting_at(&self, rbs_offset: usize) -> Option<&InlineMethod> {
        self.methods
            .iter()
            .find(|method| method.rbs_start == rbs_offset)
    }
}

pub(crate) fn translate(source: &str) -> InlineSignature {
    let parse_result = ruby_prism::parse(source.as_bytes());
    let line_starts = line_starts(source);
    let comments: Vec<Comment> = parse_result
        .comments()
        .filter(|comment| comment.type_() == ruby_prism::CommentType::InlineComment)
        .map(|comment| Comment::new(source, &line_starts, &comment.location()))
        .collect();
    let mut parser = InlineParser::new(source, line_starts, CommentBlock::build(comments));
    parser.visit(&parse_result.node());
    for index in 0..parser.blocks.len() {
        if !parser.associated[index] {
            parser.report_unused_block(index);
        }
    }
    let InlineParser {
        declarations,
        diagnostics,
        ..
    } = parser;
    let mut writer = RbsWriter {
        source,
        rbs: String::new(),
        copied_spans: Vec::new(),
        methods: Vec::new(),
    };
    for declaration in &declarations {
        writer.write_declaration(declaration);
    }
    InlineSignature {
        rbs: writer.rbs,
        copied_spans: writer.copied_spans,
        methods: writer.methods,
        diagnostics,
    }
}

fn line_starts(source: &str) -> Vec<usize> {
    let mut starts = vec![0];
    starts.extend(source.match_indices('\n').map(|(index, _)| index + 1));
    starts
}

/// 1-based, as Prism counts lines
fn line_of(line_starts: &[usize], offset: usize) -> usize {
    line_starts.partition_point(|start| *start <= offset)
}

/// Text cut out of the Ruby file, possibly across comment lines joined with newlines, with the offsets of its pieces
#[derive(Clone, Debug)]
struct Snippet {
    text: String,
    /// `(offset in text, offset in the Ruby file, length)`
    pieces: Vec<(usize, usize, usize)>,
}

impl Snippet {
    fn from_lines(lines: &[(usize, &str)]) -> Snippet {
        let mut snippet = Snippet {
            text: String::new(),
            pieces: Vec::new(),
        };
        for (index, (source_start, line)) in lines.iter().enumerate() {
            if index > 0 {
                snippet.text.push('\n');
            }
            snippet
                .pieces
                .push((snippet.text.len(), *source_start, line.len()));
            snippet.text.push_str(line);
        }
        snippet
    }

    fn slice(&self, range: Range<usize>) -> Snippet {
        let pieces = self
            .pieces
            .iter()
            .filter_map(|(text_start, source_start, length)| {
                let start = range.start.max(*text_start);
                let end = range.end.min(text_start + length);
                (start < end).then(|| {
                    (
                        start - range.start,
                        source_start + start - text_start,
                        end - start,
                    )
                })
            })
            .collect();
        Snippet {
            text: self.text[range].to_string(),
            pieces,
        }
    }

    fn source_range(&self) -> Range<usize> {
        let start = self.pieces.first().map(|piece| piece.1).unwrap_or(0);
        let end = self
            .pieces
            .last()
            .map(|piece| piece.1 + piece.2)
            .unwrap_or(start);
        start..end
    }

    /// The snippet without leading and trailing whitespace
    fn trimmed(&self) -> Snippet {
        let start = self.text.len() - self.text.trim_start().len();
        let end = self.text.trim_end().len().max(start);
        self.slice(start..end)
    }
}

struct Comment {
    start: usize,
    end: usize,
    /// `# ` or `#`, which RBS leaves out of the comment text
    prefix: usize,
    line: usize,
    column: usize,
    /// Nothing but spaces before the comment on its line
    leading: bool,
}

impl Comment {
    fn new(source: &str, line_starts: &[usize], location: &ruby_prism::Location) -> Comment {
        let start = location.start_offset();
        let mut end = location.end_offset();
        if source[..end].ends_with('\r') {
            end -= 1;
        }
        let line = line_of(line_starts, start);
        let line_start = line_starts[line - 1];
        let prefix = if source[start..end].starts_with("# ") {
            2
        } else {
            1
        };
        Comment {
            start,
            end,
            prefix,
            line,
            column: start - line_start,
            leading: source[line_start..start].trim().is_empty(),
        }
    }
}

/// `RBS::AST::Ruby::CommentBlock`: comments on consecutive lines at the same column, where only the first one may
/// follow code on its line
struct CommentBlock {
    comments: Vec<Comment>,
}

impl CommentBlock {
    fn build(comments: Vec<Comment>) -> Vec<CommentBlock> {
        let mut blocks: Vec<CommentBlock> = Vec::new();
        for comment in comments {
            if let Some(block) = blocks.last_mut() {
                let last = block.comments.last().expect("a block has a comment");
                if last.line + 1 == comment.line && last.column == comment.column && comment.leading
                {
                    block.comments.push(comment);
                    continue;
                }
            }
            blocks.push(CommentBlock {
                comments: vec![comment],
            });
        }
        blocks
    }

    fn leading(&self) -> bool {
        self.comments[0].leading
    }

    fn start_line(&self) -> usize {
        self.comments[0].line
    }

    fn end_line(&self) -> usize {
        self.comments.last().expect("a block has a comment").line
    }

    fn text<'source>(&self, source: &'source str, index: usize) -> &'source str {
        let comment = &self.comments[index];
        &source[(comment.start + comment.prefix).min(comment.end)..comment.end]
    }

    fn starts_annotation(&self, source: &str, index: usize) -> bool {
        let Some(comment) = self.comments.get(index) else {
            return false;
        };
        let text = self.text(source, index);
        let rbs_keyword = text.strip_prefix("@rbs").is_some_and(|rest| {
            !rest.starts_with(|character: char| character.is_alphanumeric() || character == '_')
        });
        rbs_keyword || source[comment.start..comment.end].starts_with("#:")
    }

    fn snippet(&self, source: &str, lines: Range<usize>) -> Snippet {
        let pieces: Vec<(usize, &str)> = lines
            .map(|index| {
                let comment = &self.comments[index];
                (
                    (comment.start + comment.prefix).min(comment.end),
                    self.text(source, index),
                )
            })
            .collect();
        Snippet::from_lines(&pieces)
    }

    /// `CommentBlock#each_paragraph`: an annotation runs while the following lines are indented or blank, and the
    /// other lines are documentation
    fn paragraphs(&self, source: &str) -> Vec<Paragraph> {
        let line_count = self.comments.len();
        let mut paragraphs = Vec::new();
        let mut annotation_start = self.starts_annotation(source, 0).then_some(0);
        let mut start = 0;
        let mut end = 0;
        let mut current = 0;
        loop {
            let next = current + 1;
            match annotation_start {
                None => {
                    if next >= line_count {
                        paragraphs
                            .push(Paragraph::Text(self.text_lines(source, start..current + 1)));
                        return paragraphs;
                    }
                    if self.starts_annotation(source, next) {
                        paragraphs
                            .push(Paragraph::Text(self.text_lines(source, start..current + 1)));
                        annotation_start = Some(next);
                        start = next;
                        end = next;
                    }
                    current = next;
                }
                Some(annotation) => {
                    if next >= line_count {
                        paragraphs.push(Paragraph::Annotation(
                            self.leading_annotation(source, annotation..end + 1),
                        ));
                        return paragraphs;
                    }
                    let line_text = self.text(source, next);
                    match line_text.find(|character: char| !character.is_whitespace()) {
                        Some(0) => {
                            paragraphs.push(Paragraph::Annotation(
                                self.leading_annotation(source, annotation..end + 1),
                            ));
                            start = end + 1;
                            annotation_start =
                                self.starts_annotation(source, start).then_some(start);
                            end = start;
                            current = start;
                        }
                        Some(_) => {
                            current = next;
                            end = next;
                        }
                        None => current = next,
                    }
                }
            }
        }
    }

    fn text_lines(&self, source: &str, lines: Range<usize>) -> Vec<String> {
        lines
            .map(|index| self.text(source, index).to_string())
            .collect()
    }

    fn leading_annotation(&self, source: &str, lines: Range<usize>) -> ParsedAnnotation {
        let snippet = self.snippet(source, lines);
        ParsedAnnotation {
            range: snippet.source_range(),
            annotation: parse_leading_annotation(&snippet),
        }
    }

    /// `CommentBlock#trailing_annotation`: a block after code whose first comment starts with `#:` or `#[`
    fn trailing_annotation(&self, source: &str) -> Option<ParsedAnnotation> {
        if self.leading() {
            return None;
        }
        let first = &self.comments[0];
        let comment_text = &source[first.start..first.end];
        if !(comment_text.starts_with("#:") || comment_text.starts_with("#[")) {
            return None;
        }
        let snippet = self.snippet(source, 0..self.comments.len());
        Some(ParsedAnnotation {
            range: snippet.source_range(),
            annotation: parse_trailing_annotation(&snippet),
        })
    }
}

enum Paragraph {
    Text(Vec<String>),
    Annotation(ParsedAnnotation),
}

#[derive(Clone)]
struct ParsedAnnotation {
    annotation: Result<Annotation, String>,
    range: Range<usize>,
}

#[derive(Clone)]
enum Annotation {
    Skip,
    ReturnType(Snippet),
    Parameter {
        name: String,
        parameter_type: Snippet,
    },
    Splat {
        name: Option<String>,
        parameter_type: Snippet,
    },
    DoubleSplat {
        name: Option<String>,
        parameter_type: Snippet,
    },
    BlockParameter {
        name: Option<String>,
        optional: bool,
        block_type: Snippet,
    },
    InstanceVariable {
        name: Snippet,
        variable_type: Snippet,
    },
    ModuleSelf(Snippet),
    /// `@rbs METHOD-TYPE`, which may have overloads joined with `|` and end with `...`
    MethodTypes(Snippet),
    /// `#: METHOD-TYPE`
    ColonMethodType(Snippet),
    TypeAssertion(Snippet),
    /// `#[String]`, brackets included
    TypeApplication(Snippet),
    ClassAlias(Option<Snippet>),
    ModuleAlias(Option<Snippet>),
}

fn is_identifier_start(character: char) -> bool {
    character.is_lowercase()
        || character == '_'
        || !character.is_ascii() && character.is_alphabetic()
}

fn is_identifier_character(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

/// The length of the identifier at the start of `text`
fn identifier_length(text: &str) -> usize {
    let mut characters = text.char_indices();
    match characters.next() {
        Some((_, first)) if is_identifier_start(first) => {}
        _ => return 0,
    }
    characters
        .find(|(_, character)| !is_identifier_character(*character))
        .map_or(text.len(), |(index, _)| index)
}

fn skip_spaces(text: &str, offset: usize) -> usize {
    offset + (text[offset..].len() - text[offset..].trim_start().len())
}

/// The range of the type after `:` at `colon`, up to the `--` that starts the description
fn type_after_colon(text: &str, colon: usize) -> Range<usize> {
    let start = colon + 1;
    let end = text[start..]
        .find("--")
        .map_or(text.len(), |index| start + index);
    start..end
}

unsafe extern "C" {
    // The bindings of ruby-rbs-sys list only the functions ruby-rbs uses, but its library has the whole parser
    fn rbs_parse_inline_leading_annotation(
        parser: *mut rbs_parser_t,
        annotation: *mut *mut std::ffi::c_void,
    ) -> bool;
    fn rbs_parse_inline_trailing_annotation(
        parser: *mut rbs_parser_t,
        annotation: *mut *mut std::ffi::c_void,
    ) -> bool;
}

/// The message of the syntax error RBS's parser reports for an annotation, as `RBS::Parser.parse_inline_leading_annotation`
/// and `parse_inline_trailing_annotation` raise it
fn annotation_syntax_error(text: &str, trailing: bool) -> Option<String> {
    let Ok(length) = i32::try_from(text.len()) else {
        return Some("the annotation is too long".to_string());
    };
    unsafe {
        let start_pointer = text.as_ptr().cast::<std::os::raw::c_char>();
        let string = rbs_string_new(start_pointer, start_pointer.add(text.len()));
        let encoding = &rbs_encodings[rbs_encoding_type_t::RBS_ENCODING_UTF_8 as usize]
            as *const rbs_encoding_t;
        let parser = rbs_parser_new(string, encoding, 0, length);
        let mut annotation = std::ptr::null_mut();
        let succeeded = if trailing {
            rbs_parse_inline_trailing_annotation(parser, &mut annotation)
        } else {
            rbs_parse_inline_leading_annotation(parser, &mut annotation)
        };
        let message = (!succeeded).then(|| {
            (*parser)
                .error
                .as_ref()
                .filter(|error| !error.message.is_null())
                .map(|error| {
                    std::ffi::CStr::from_ptr(error.message)
                        .to_string_lossy()
                        .into_owned()
                })
                .unwrap_or_else(|| "unexpected token".to_string())
        });
        rbs_parser_free(parser);
        message
    }
}

fn parse_leading_annotation(snippet: &Snippet) -> Result<Annotation, String> {
    let text = snippet.text.as_str();
    let Some(rest) = text.strip_prefix("@rbs") else {
        if let Some(message) = annotation_syntax_error(text, false) {
            return Err(message);
        }
        return Ok(Annotation::ColonMethodType(
            snippet.slice(1..text.len()).trimmed(),
        ));
    };
    let start = skip_spaces(text, text.len() - rest.len());
    let rest = &text[start..];
    let colon_after = |offset: usize| -> Option<usize> {
        let colon = skip_spaces(text, offset);
        (text[colon..].starts_with(':') && !text[colon..].starts_with("::")).then_some(colon)
    };
    let typed = |colon: usize| snippet.slice(type_after_colon(text, colon)).trimmed();
    if let Some(after) = rest.strip_prefix("module-self")
        && let Some(colon) = colon_after(text.len() - after.len())
    {
        let self_type = typed(colon);
        validate_module_self(&self_type.text)?;
        return Ok(Annotation::ModuleSelf(self_type));
    }
    if let Some(message) = annotation_syntax_error(text, false) {
        return Err(message);
    }
    if let Some(after) = rest.strip_prefix("skip")
        && !after.starts_with(is_identifier_character)
        && colon_after(text.len() - after.len()).is_none()
    {
        return Ok(Annotation::Skip);
    }
    if let Some(after) = rest.strip_prefix("return")
        && let Some(colon) = colon_after(text.len() - after.len())
    {
        return Ok(Annotation::ReturnType(typed(colon)));
    }
    if let Some(after) = rest.strip_prefix('@') {
        let name_length = after
            .find(|character: char| !is_identifier_character(character))
            .unwrap_or(after.len());
        let name_end = start + 1 + name_length;
        if name_length > 0
            && let Some(colon) = colon_after(name_end)
        {
            return Ok(Annotation::InstanceVariable {
                name: snippet.slice(start..name_end),
                variable_type: typed(colon),
            });
        }
    }
    for marker in ["**", "*", "&"] {
        let Some(after) = rest.strip_prefix(marker) else {
            continue;
        };
        let name_start = start + marker.len();
        let name_end = name_start + identifier_length(after);
        let Some(colon) = colon_after(name_end) else {
            continue;
        };
        let name = (name_end > name_start).then(|| text[name_start..name_end].to_string());
        let parameter_type = typed(colon);
        return Ok(match marker {
            "**" => Annotation::DoubleSplat {
                name,
                parameter_type,
            },
            "*" => Annotation::Splat {
                name,
                parameter_type,
            },
            _ => {
                let (optional, block_type) = match parameter_type.text.strip_prefix('?') {
                    Some(_) => (
                        true,
                        parameter_type.slice(1..parameter_type.text.len()).trimmed(),
                    ),
                    None => (false, parameter_type),
                };
                Annotation::BlockParameter {
                    name,
                    optional,
                    block_type,
                }
            }
        });
    }
    let name_length = identifier_length(rest);
    if name_length > 0
        && let Some(colon) = colon_after(start + name_length)
    {
        return Ok(Annotation::Parameter {
            name: rest[..name_length].to_string(),
            parameter_type: typed(colon),
        });
    }
    Ok(Annotation::MethodTypes(
        snippet.slice(start..text.len()).trimmed(),
    ))
}

fn parse_trailing_annotation(snippet: &Snippet) -> Result<Annotation, String> {
    let text = snippet.text.as_str();
    if let Some(message) = annotation_syntax_error(text, true) {
        return Err(message);
    }
    if text.starts_with('[') {
        return Ok(Annotation::TypeApplication(snippet.trimmed()));
    }
    let body = snippet.slice(1..text.len()).trimmed();
    for (keyword, module) in [("class-alias", false), ("module-alias", true)] {
        let Some(after) = body.text.strip_prefix(keyword) else {
            continue;
        };
        if !(after.is_empty() || after.starts_with(char::is_whitespace)) {
            continue;
        }
        let name = body.slice(keyword.len()..body.text.len()).trimmed();
        let name = (!name.text.is_empty()).then_some(name);
        return Ok(if module {
            Annotation::ModuleAlias(name)
        } else {
            Annotation::ClassAlias(name)
        });
    }
    Ok(Annotation::TypeAssertion(body))
}

/// RBS 4.0.2's parser, which ruby-rbs 0.3.0 has, does not know `module-self` annotations of RBS 4.1, so the type is
/// checked as the self type of a module declaration
fn validate_module_self(text: &str) -> Result<(), String> {
    let wrapped = format!("module InlineAnnotation : {text}\nend\n");
    let signature = ruby_rbs::node::parse(&wrapped)?;
    if signature.declarations().iter().count() == 1 {
        Ok(())
    } else {
        Err("unexpected token".to_string())
    }
}

/// `RBS::AST::Ruby::Helpers::ConstantHelper#constant_as_type_name`: `None` for a path through something other than
/// constants, such as `object::Name`
fn is_constant_name(node: &Node) -> bool {
    match node {
        Node::ConstantReadNode { .. } => true,
        Node::ConstantPathNode { .. } => node
            .as_constant_path_node()
            .is_some_and(|path| path.parent().is_none_or(|parent| is_constant_name(&parent))),
        _ => false,
    }
}

fn range_of(location: &ruby_prism::Location) -> Range<usize> {
    location.start_offset()..location.end_offset()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ContainerKind {
    Class,
    Module,
}

struct Container {
    kind: ContainerKind,
    keyword: Range<usize>,
    name: Range<usize>,
    end_keyword: Range<usize>,
    super_class: Option<SuperClass>,
    members: Vec<(usize, Member)>,
}

struct SuperClass {
    name: Range<usize>,
    arguments: Option<Snippet>,
}

enum Declaration {
    Container(Container),
    Constant {
        name: Range<usize>,
        constant_type: ConstantType,
        comment: Vec<String>,
    },
    ClassAlias {
        module: bool,
        new_name: Range<usize>,
        old_name: AliasedName,
    },
}

enum ConstantType {
    Annotated(Snippet),
    /// `RBS::AST::Ruby::Declarations::ConstantDecl#type` for a literal, anchored at the value
    Inferred(&'static str),
}

enum AliasedName {
    Source(Range<usize>),
    Annotation(Snippet),
}

enum Member {
    Declaration(Declaration),
    Method(Box<MethodMember>),
    Mixin {
        keyword: Range<usize>,
        name: Range<usize>,
        arguments: Option<Snippet>,
    },
    Attribute {
        keyword: Range<usize>,
        names: Vec<(Range<usize>, String)>,
        attribute_type: Option<Snippet>,
        comment: Vec<String>,
    },
    InstanceVariable {
        name: Snippet,
        variable_type: Snippet,
    },
    ModuleSelf(Snippet),
}

struct MethodMember {
    keyword: Range<usize>,
    name: Range<usize>,
    singleton: bool,
    range: Range<usize>,
    method_type: MethodTypeSource,
    comment: Vec<String>,
}

enum MethodTypeSource {
    Overloads(Vec<Snippet>),
    DocStyle(Box<DocStyle>),
    Unannotated,
}

/// `RBS::AST::Ruby::Members::MethodTypeAnnotation::DocStyle`
#[derive(Default)]
struct DocStyle {
    return_type: Option<Snippet>,
    required_positionals: Vec<DocParameter>,
    optional_positionals: Vec<DocParameter>,
    rest_positionals: Option<DocParameter>,
    trailing_positionals: Vec<DocParameter>,
    required_keywords: Vec<(String, Option<Snippet>)>,
    optional_keywords: Vec<(String, Option<Snippet>)>,
    rest_keywords: Option<DocParameter>,
    block: Option<DocBlock>,
}

struct DocParameter {
    name: Option<String>,
    parameter_type: Option<Snippet>,
}

enum DocBlock {
    Annotated { optional: bool, block_type: Snippet },
    Unannotated,
}

struct InlineParser<'source> {
    source: &'source str,
    line_starts: Vec<usize>,
    blocks: Vec<CommentBlock>,
    associated: Vec<bool>,
    paragraphs: Vec<Option<std::rc::Rc<Vec<Paragraph>>>>,
    leading_by_end_line: HashMap<usize, usize>,
    trailing_by_start_line: HashMap<usize, usize>,
    nesting: Vec<Container>,
    declarations: Vec<Declaration>,
    diagnostics: Vec<InlineDiagnostic>,
}

impl<'source> InlineParser<'source> {
    fn new(
        source: &'source str,
        line_starts: Vec<usize>,
        blocks: Vec<CommentBlock>,
    ) -> InlineParser<'source> {
        let mut leading_by_end_line = HashMap::new();
        let mut trailing_by_start_line = HashMap::new();
        for (index, block) in blocks.iter().enumerate() {
            if block.leading() {
                leading_by_end_line.insert(block.end_line(), index);
            } else {
                trailing_by_start_line.insert(block.start_line(), index);
            }
        }
        InlineParser {
            source,
            line_starts,
            associated: vec![false; blocks.len()],
            paragraphs: (0..blocks.len()).map(|_| None).collect(),
            blocks,
            leading_by_end_line,
            trailing_by_start_line,
            nesting: Vec::new(),
            declarations: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    fn start_line(&self, location: &ruby_prism::Location) -> usize {
        line_of(&self.line_starts, location.start_offset())
    }

    fn end_line(&self, location: &ruby_prism::Location) -> usize {
        line_of(&self.line_starts, location.end_offset())
    }

    fn paragraphs_of(&mut self, index: usize) -> std::rc::Rc<Vec<Paragraph>> {
        if let Some(paragraphs) = &self.paragraphs[index] {
            return paragraphs.clone();
        }
        let paragraphs = std::rc::Rc::new(self.blocks[index].paragraphs(self.source));
        self.paragraphs[index] = Some(paragraphs.clone());
        paragraphs
    }

    fn diagnose(&mut self, range: Range<usize>, message: &str) {
        self.diagnostics.push(InlineDiagnostic {
            range,
            message: message.to_string(),
        });
    }

    /// `CommentAssociation#leading_block`, which does not care whether the block is taken
    fn leading_block(&self, start_line: usize) -> Option<usize> {
        self.leading_by_end_line.get(&(start_line - 1)).copied()
    }

    /// `CommentAssociation#leading_block!`: the block unless it is taken, taking it
    fn take_leading_block(&mut self, start_line: usize) -> Option<usize> {
        let index = self.leading_block(start_line)?;
        if self.associated[index] {
            return None;
        }
        self.associated[index] = true;
        Some(index)
    }

    /// `CommentAssociation#trailing_block!`
    fn take_trailing_block(&mut self, end_line: usize) -> Option<usize> {
        let index = *self.trailing_by_start_line.get(&end_line)?;
        if self.associated[index] {
            return None;
        }
        self.associated[index] = true;
        Some(index)
    }

    /// `CommentAssociation#each_enclosed_block`: the untaken leading blocks between the first and the last line
    fn take_enclosed_blocks(&mut self, start_line: usize, end_line: usize) -> Vec<usize> {
        let mut indexes = Vec::new();
        for line in start_line + 1..end_line {
            if let Some(&index) = self.leading_by_end_line.get(&line)
                && !self.associated[index]
            {
                self.associated[index] = true;
                indexes.push(index);
            }
        }
        indexes
    }

    fn report_unused_annotation(&mut self, annotation: &ParsedAnnotation) {
        let message = match &annotation.annotation {
            Ok(_) => "Unused inline rbs annotation".to_string(),
            Err(error) => format!("Syntax error: {error}"),
        };
        self.diagnostics.push(InlineDiagnostic {
            range: annotation.range.clone(),
            message,
        });
    }

    fn report_unused_block(&mut self, index: usize) {
        if !self.blocks[index].leading() {
            return;
        }
        let paragraphs = self.paragraphs_of(index);
        for paragraph in paragraphs.iter() {
            if let Paragraph::Annotation(annotation) = paragraph {
                self.report_unused_annotation(annotation);
            }
        }
    }

    fn report_enclosed_blocks(&mut self, start_line: usize, end_line: usize) {
        for index in self.take_enclosed_blocks(start_line, end_line) {
            self.report_unused_block(index);
        }
    }

    fn comment_of(&mut self, index: Option<usize>) -> Vec<String> {
        let Some(index) = index else {
            return Vec::new();
        };
        self.paragraphs_of(index)
            .iter()
            .flat_map(|paragraph| match paragraph {
                Paragraph::Text(lines) => lines.clone(),
                Paragraph::Annotation(_) => Vec::new(),
            })
            .collect()
    }

    fn is_skipped(&mut self, start_line: usize) -> bool {
        let Some(index) = self.leading_block(start_line) else {
            return false;
        };
        let skipped = self.paragraphs_of(index).iter().any(|paragraph| {
            matches!(
                paragraph,
                Paragraph::Annotation(ParsedAnnotation {
                    annotation: Ok(Annotation::Skip),
                    ..
                })
            )
        });
        if skipped {
            self.associated[index] = true;
        }
        skipped
    }

    fn insert_declaration(&mut self, line: usize, declaration: Declaration) {
        match self.nesting.last_mut() {
            Some(container) => container
                .members
                .push((line, Member::Declaration(declaration))),
            None => self.declarations.push(declaration),
        }
    }

    fn visit_container(
        &mut self,
        container: Container,
        node: &Node<'_>,
        child_nodes: Vec<Option<Node<'_>>>,
        visit_children: impl FnOnce(&mut Self),
    ) {
        let location = node.location();
        let (start_line, end_line) = (self.start_line(&location), self.end_line(&location));
        self.nesting.push(container);
        visit_children(self);
        for child in child_nodes.into_iter().flatten() {
            let child_location = child.location();
            let (child_start, child_end) = (
                self.start_line(&child_location),
                self.end_line(&child_location),
            );
            self.report_enclosed_blocks(child_start, child_end);
        }
        let mut container = self.nesting.pop().expect("the container was pushed");
        for index in self.take_enclosed_blocks(start_line, end_line) {
            let paragraphs = self.paragraphs_of(index);
            for paragraph in paragraphs.iter() {
                match paragraph {
                    Paragraph::Text(_) => {}
                    Paragraph::Annotation(ParsedAnnotation {
                        annotation:
                            Ok(Annotation::InstanceVariable {
                                name,
                                variable_type,
                            }),
                        range,
                    }) => {
                        let line = line_of(&self.line_starts, range.start);
                        container.members.push((
                            line,
                            Member::InstanceVariable {
                                name: name.clone(),
                                variable_type: variable_type.clone(),
                            },
                        ));
                    }
                    Paragraph::Annotation(annotation) => self.report_unused_annotation(annotation),
                }
            }
        }
        container.members.sort_by_key(|(line, _)| *line);
        self.insert_declaration(start_line, Declaration::Container(container));
    }

    fn parse_super_class(&mut self, super_class: &Node<'_>) -> Option<SuperClass> {
        let location = super_class.location();
        if !is_constant_name(super_class) {
            self.diagnose(range_of(&location), "Super class name must be a constant");
            return None;
        }
        let end_line = self.end_line(&location);
        let mut arguments = None;
        if let Some(index) = self.take_trailing_block(end_line) {
            match self.blocks[index].trailing_annotation(self.source) {
                Some(ParsedAnnotation {
                    annotation: Ok(Annotation::TypeApplication(application)),
                    ..
                }) => arguments = Some(application),
                Some(annotation) => self.report_unused_annotation(&annotation),
                None => {}
            }
        }
        Some(SuperClass {
            name: range_of(&location),
            arguments,
        })
    }

    fn build_method_type(
        &mut self,
        leading: Option<usize>,
        trailing: Option<usize>,
        node: &ruby_prism::DefNode<'_>,
    ) -> MethodTypeSource {
        let mut overloads: Option<Vec<Snippet>> = None;
        let mut return_type: Option<Snippet> = None;
        let mut parameter_annotations: Vec<ParsedAnnotation> = Vec::new();
        let mut unused: Vec<ParsedAnnotation> = Vec::new();
        let mut unused_trailing = None;
        if let Some(index) = trailing {
            match self.blocks[index].trailing_annotation(self.source) {
                Some(ParsedAnnotation {
                    annotation: Ok(Annotation::TypeAssertion(assertion)),
                    ..
                }) => return_type = Some(assertion),
                other => unused_trailing = other,
            }
        }
        if let Some(index) = leading {
            let paragraphs = self.paragraphs_of(index);
            for paragraph in paragraphs.iter() {
                let Paragraph::Annotation(annotation) = paragraph else {
                    continue;
                };
                match &annotation.annotation {
                    Ok(
                        Annotation::MethodTypes(method_type)
                        | Annotation::ColonMethodType(method_type),
                    ) => {
                        overloads
                            .get_or_insert_with(Vec::new)
                            .push(method_type.clone());
                        continue;
                    }
                    Ok(Annotation::ReturnType(annotated))
                        if overloads.is_none() && return_type.is_none() =>
                    {
                        return_type = Some(annotated.clone());
                        continue;
                    }
                    Ok(
                        Annotation::Parameter { .. }
                        | Annotation::Splat { .. }
                        | Annotation::DoubleSplat { .. }
                        | Annotation::BlockParameter { .. },
                    ) if overloads.is_none() => {
                        parameter_annotations.push(annotation.clone());
                        continue;
                    }
                    _ => {}
                }
                unused.push(annotation.clone());
            }
        }
        let method_type = match overloads {
            Some(overloads) => MethodTypeSource::Overloads(overloads),
            None if return_type.is_some() || !parameter_annotations.is_empty() => {
                let (doc_style, unused_parameters) =
                    build_doc_style(parameter_annotations, return_type, node);
                unused.extend(unused_parameters);
                MethodTypeSource::DocStyle(Box::new(doc_style))
            }
            None => MethodTypeSource::Unannotated,
        };
        if let Some(annotation) = unused_trailing {
            self.report_unused_annotation(&annotation);
        }
        for annotation in &unused {
            self.report_unused_annotation(annotation);
        }
        method_type
    }

    fn parse_mixin_call(&mut self, node: &ruby_prism::CallNode<'_>) {
        let arguments: Vec<Node<'_>> = node
            .arguments()
            .map(|arguments| arguments.arguments().iter().collect())
            .unwrap_or_default();
        if arguments.len() > 1 {
            self.diagnose(
                range_of(&node.location()),
                "Mixing multiple modules with one call is not supported",
            );
            return;
        }
        let [argument] = arguments.as_slice() else {
            return;
        };
        if !is_constant_name(argument) {
            self.diagnose(
                range_of(&argument.location()),
                "Module name must be a constant",
            );
            return;
        }
        let end_line = self.end_line(&node.location());
        let mut application = None;
        if let Some(index) = self.take_trailing_block(end_line) {
            match self.blocks[index].trailing_annotation(self.source) {
                Some(ParsedAnnotation {
                    annotation: Ok(Annotation::TypeApplication(arguments)),
                    ..
                }) => application = Some(arguments),
                Some(annotation) => self.report_unused_annotation(&annotation),
                None => {}
            }
        }
        let line = self.start_line(&node.location());
        let keyword = node.message_loc().map_or_else(
            || range_of(&node.location()),
            |location| range_of(&location),
        );
        if let Some(container) = self.nesting.last_mut() {
            container.members.push((
                line,
                Member::Mixin {
                    keyword,
                    name: range_of(&argument.location()),
                    arguments: application,
                },
            ));
        }
    }

    fn parse_attribute_call(&mut self, node: &ruby_prism::CallNode<'_>) {
        let Some(arguments) = node.arguments() else {
            return;
        };
        let mut names = Vec::new();
        for argument in arguments.arguments().iter() {
            match argument.as_symbol_node() {
                Some(symbol) => {
                    let name = String::from_utf8_lossy(symbol.unescaped()).into_owned();
                    let range = symbol
                        .value_loc()
                        .map(|location| range_of(&location))
                        .filter(|range| self.source[range.clone()] == name)
                        .unwrap_or_else(|| range_of(&symbol.location()));
                    names.push((range, name));
                }
                None => self.diagnose(
                    range_of(&argument.location()),
                    "Attribute name must be a symbol",
                ),
            }
        }
        if names.is_empty() {
            return;
        }
        let location = node.location();
        let leading = self.take_leading_block(self.start_line(&location));
        let mut attribute_type = None;
        if let Some(index) = self.take_trailing_block(self.end_line(&location)) {
            match self.blocks[index].trailing_annotation(self.source) {
                Some(ParsedAnnotation {
                    annotation: Ok(Annotation::TypeAssertion(assertion)),
                    ..
                }) => attribute_type = Some(assertion),
                Some(ParsedAnnotation {
                    annotation: Err(error),
                    range,
                }) => self.diagnostics.push(InlineDiagnostic {
                    range,
                    message: format!("Syntax error: {error}"),
                }),
                _ => {}
            }
        }
        let comment = self.comment_of(leading);
        if let Some(index) = leading {
            self.report_unused_block(index);
        }
        let line = self.start_line(&location);
        let keyword = node
            .message_loc()
            .map_or_else(|| range_of(&location), |location| range_of(&location));
        if let Some(container) = self.nesting.last_mut() {
            container.members.push((
                line,
                Member::Attribute {
                    keyword,
                    names,
                    attribute_type,
                    comment,
                },
            ));
        }
    }

    fn parse_constant_declaration(
        &mut self,
        node: &Node<'_>,
        name: Range<usize>,
        target_is_constant: bool,
        value: &Node<'_>,
    ) {
        if !target_is_constant {
            self.diagnose(name, "Constant name must be a constant");
            return;
        }
        let location = node.location();
        let leading = self.take_leading_block(self.start_line(&location));
        let comment = self.comment_of(leading);
        if let Some(index) = leading {
            self.report_unused_block(index);
        }
        let mut type_annotation = None;
        let mut alias_annotation = None;
        if let Some(index) = self.take_trailing_block(self.end_line(&location)) {
            match self.blocks[index].trailing_annotation(self.source) {
                Some(ParsedAnnotation {
                    annotation: Ok(Annotation::TypeAssertion(assertion)),
                    ..
                }) => type_annotation = Some(assertion),
                Some(
                    annotation @ ParsedAnnotation {
                        annotation: Ok(Annotation::ClassAlias(_) | Annotation::ModuleAlias(_)),
                        ..
                    },
                ) => alias_annotation = Some(annotation),
                Some(ParsedAnnotation {
                    annotation: Err(error),
                    range,
                }) => self.diagnostics.push(InlineDiagnostic {
                    range,
                    message: format!("Syntax error: {error}"),
                }),
                _ => {}
            }
        }
        let line = self.start_line(&location);
        if let Some(alias_annotation) = alias_annotation {
            let (module, annotated_name) = match alias_annotation.annotation {
                Ok(Annotation::ClassAlias(annotated_name)) => (false, annotated_name),
                Ok(Annotation::ModuleAlias(annotated_name)) => (true, annotated_name),
                _ => return,
            };
            let old_name = match annotated_name {
                Some(annotated_name) => AliasedName::Annotation(annotated_name),
                None if is_constant_name(value) => AliasedName::Source(range_of(&value.location())),
                None => {
                    let message = if module {
                        "Module name is missing in module alias declaration"
                    } else {
                        "Class name is missing in class alias declaration"
                    };
                    self.diagnose(alias_annotation.range, message);
                    return;
                }
            };
            self.insert_declaration(
                line,
                Declaration::ClassAlias {
                    module,
                    new_name: name,
                    old_name,
                },
            );
            return;
        }
        let constant_type = match type_annotation {
            Some(annotated) => ConstantType::Annotated(annotated),
            None => ConstantType::Inferred(match value {
                Node::IntegerNode { .. } => "Integer",
                Node::FloatNode { .. } => "Float",
                Node::StringNode { .. } => "String",
                Node::TrueNode { .. } | Node::FalseNode { .. } => "bool",
                Node::SymbolNode { .. } => "Symbol",
                Node::NilNode { .. } => "nil",
                _ => "untyped",
            }),
        };
        self.insert_declaration(
            line,
            Declaration::Constant {
                name,
                constant_type,
                comment,
            },
        );
    }
}

fn build_doc_style(
    annotations: Vec<ParsedAnnotation>,
    return_type: Option<Snippet>,
    node: &ruby_prism::DefNode<'_>,
) -> (DocStyle, Vec<ParsedAnnotation>) {
    let mut doc = DocStyle {
        return_type,
        ..DocStyle::default()
    };
    let mut splat: Option<ParsedAnnotation> = None;
    let mut double_splat: Option<ParsedAnnotation> = None;
    let mut block: Option<ParsedAnnotation> = None;
    let mut parameters: Vec<(String, ParsedAnnotation)> = Vec::new();
    let mut unused = Vec::new();
    for annotation in annotations {
        let slot = match &annotation.annotation {
            Ok(Annotation::Splat { .. }) => &mut splat,
            Ok(Annotation::DoubleSplat { .. }) => &mut double_splat,
            Ok(Annotation::BlockParameter { .. }) => &mut block,
            Ok(Annotation::Parameter { name, .. }) => {
                if parameters.iter().any(|(existing, _)| existing == name) {
                    unused.push(annotation);
                } else {
                    parameters.push((name.clone(), annotation));
                }
                continue;
            }
            _ => continue,
        };
        if slot.is_some() {
            unused.push(annotation);
        } else {
            *slot = Some(annotation);
        }
    }
    let mut take_parameter = |name: &str| -> Option<Snippet> {
        let position = parameters
            .iter()
            .position(|(existing, _)| existing == name)?;
        match parameters.remove(position).1.annotation {
            Ok(Annotation::Parameter { parameter_type, .. }) => Some(parameter_type),
            _ => None,
        }
    };
    let constant_text =
        |name: ruby_prism::ConstantId| String::from_utf8_lossy(name.as_slice()).into_owned();
    let parameters_node = node.parameters();
    if let Some(parameters_node) = &parameters_node {
        for parameter in parameters_node.requireds().iter() {
            if let Some(required) = parameter.as_required_parameter_node() {
                let name = constant_text(required.name());
                doc.required_positionals.push(DocParameter {
                    parameter_type: take_parameter(&name),
                    name: Some(name),
                });
            }
        }
        for parameter in parameters_node.optionals().iter() {
            if let Some(optional) = parameter.as_optional_parameter_node() {
                let name = constant_text(optional.name());
                doc.optional_positionals.push(DocParameter {
                    parameter_type: take_parameter(&name),
                    name: Some(name),
                });
            }
        }
        if let Some(rest) = parameters_node
            .rest()
            .and_then(|rest| rest.as_rest_parameter_node())
        {
            let rest_name = rest.name().map(constant_text);
            let matches = |annotated_name: &Option<String>| {
                annotated_name.is_none() || rest_name.is_none() || annotated_name == &rest_name
            };
            doc.rest_positionals = Some(match splat.take() {
                Some(ParsedAnnotation {
                    annotation:
                        Ok(Annotation::Splat {
                            name,
                            parameter_type,
                        }),
                    ..
                }) if matches(&name) => DocParameter {
                    name,
                    parameter_type: Some(parameter_type),
                },
                other => {
                    splat = other;
                    DocParameter {
                        name: rest_name,
                        parameter_type: None,
                    }
                }
            });
        }
        for parameter in parameters_node.posts().iter() {
            if let Some(required) = parameter.as_required_parameter_node() {
                let name = constant_text(required.name());
                doc.trailing_positionals.push(DocParameter {
                    parameter_type: take_parameter(&name),
                    name: Some(name),
                });
            }
        }
        for parameter in parameters_node.keywords().iter() {
            if let Some(required) = parameter.as_required_keyword_parameter_node() {
                let name = constant_text(required.name());
                let parameter_type = take_parameter(&name);
                doc.required_keywords.push((name, parameter_type));
            } else if let Some(optional) = parameter.as_optional_keyword_parameter_node() {
                let name = constant_text(optional.name());
                let parameter_type = take_parameter(&name);
                doc.optional_keywords.push((name, parameter_type));
            }
        }
        if let Some(keyword_rest) = parameters_node
            .keyword_rest()
            .and_then(|keyword_rest| keyword_rest.as_keyword_rest_parameter_node())
        {
            let rest_name = keyword_rest.name().map(constant_text);
            let matches = |annotated_name: &Option<String>| {
                annotated_name.is_none() || rest_name.is_none() || annotated_name == &rest_name
            };
            doc.rest_keywords = Some(match double_splat.take() {
                Some(ParsedAnnotation {
                    annotation:
                        Ok(Annotation::DoubleSplat {
                            name,
                            parameter_type,
                        }),
                    ..
                }) if matches(&name) => DocParameter {
                    name,
                    parameter_type: Some(parameter_type),
                },
                other => {
                    double_splat = other;
                    DocParameter {
                        name: rest_name,
                        parameter_type: None,
                    }
                }
            });
        }
        if let Some(block_parameter) = parameters_node.block() {
            let block_name = block_parameter.name().map(constant_text);
            doc.block = Some(match block.take() {
                Some(ParsedAnnotation {
                    annotation:
                        Ok(Annotation::BlockParameter {
                            name,
                            optional,
                            block_type,
                        }),
                    ..
                }) if name.is_none() || block_name.is_none() || name == block_name => {
                    DocBlock::Annotated {
                        optional,
                        block_type,
                    }
                }
                other => {
                    block = other;
                    DocBlock::Unannotated
                }
            });
        }
    }
    let has_block_parameter = parameters_node
        .as_ref()
        .is_some_and(|parameters_node| parameters_node.block().is_some());
    if !has_block_parameter
        && let Some(ParsedAnnotation {
            annotation:
                Ok(Annotation::BlockParameter {
                    optional,
                    block_type,
                    ..
                }),
            ..
        }) = block.take()
    {
        doc.block = Some(DocBlock::Annotated {
            optional,
            block_type,
        });
    }
    unused.extend(parameters.into_iter().map(|(_, annotation)| annotation));
    unused.extend(splat);
    unused.extend(double_splat);
    unused.extend(block);
    (doc, unused)
}

impl<'pr> Visit<'pr> for InlineParser<'_> {
    fn visit_class_node(&mut self, node: &ruby_prism::ClassNode<'pr>) {
        let location = node.location();
        if self.is_skipped(self.start_line(&location)) {
            return;
        }
        let constant_path = node.constant_path();
        if !is_constant_name(&constant_path) {
            self.diagnose(
                range_of(&constant_path.location()),
                "Class name must be a constant",
            );
            return;
        }
        let super_class = node
            .superclass()
            .and_then(|super_class| self.parse_super_class(&super_class));
        let container = Container {
            kind: ContainerKind::Class,
            keyword: range_of(&node.class_keyword_loc()),
            name: range_of(&constant_path.location()),
            end_keyword: range_of(&node.end_keyword_loc()),
            super_class,
            members: Vec::new(),
        };
        let child_nodes = vec![Some(node.constant_path()), node.superclass(), node.body()];
        self.visit_container(container, &node.as_node(), child_nodes, |parser| {
            ruby_prism::visit_class_node(parser, node)
        });
    }

    fn visit_module_node(&mut self, node: &ruby_prism::ModuleNode<'pr>) {
        let location = node.location();
        let start_line = self.start_line(&location);
        if self.is_skipped(start_line) {
            return;
        }
        let constant_path = node.constant_path();
        if !is_constant_name(&constant_path) {
            self.diagnose(
                range_of(&constant_path.location()),
                "Module name must be a constant",
            );
            return;
        }
        let mut container = Container {
            kind: ContainerKind::Module,
            keyword: range_of(&node.module_keyword_loc()),
            name: range_of(&constant_path.location()),
            end_keyword: range_of(&node.end_keyword_loc()),
            super_class: None,
            members: Vec::new(),
        };
        if let Some(index) = self.leading_block(start_line) {
            let paragraphs = self.paragraphs_of(index);
            let mut unused = Vec::new();
            for paragraph in paragraphs.iter() {
                match paragraph {
                    Paragraph::Text(_) => {}
                    Paragraph::Annotation(ParsedAnnotation {
                        annotation: Ok(Annotation::ModuleSelf(self_type)),
                        range,
                    }) => {
                        let line = line_of(&self.line_starts, range.start);
                        container
                            .members
                            .push((line, Member::ModuleSelf(self_type.clone())));
                    }
                    Paragraph::Annotation(ParsedAnnotation {
                        annotation: Ok(Annotation::Skip),
                        ..
                    }) => {}
                    Paragraph::Annotation(annotation) => unused.push(annotation.clone()),
                }
            }
            for annotation in &unused {
                self.report_unused_annotation(annotation);
            }
            self.associated[index] = true;
        }
        let child_nodes = vec![Some(node.constant_path()), node.body()];
        self.visit_container(container, &node.as_node(), child_nodes, |parser| {
            ruby_prism::visit_module_node(parser, node)
        });
    }

    fn visit_def_node(&mut self, node: &ruby_prism::DefNode<'pr>) {
        let location = node.location();
        let start_line = self.start_line(&location);
        if self.is_skipped(start_line) {
            return;
        }
        let receiver = node.receiver();
        if let Some(receiver) = &receiver
            && !matches!(receiver, Node::SelfNode { .. })
        {
            self.diagnose(
                range_of(&receiver.location()),
                "Method definition with non-self receiver is not supported",
            );
            return;
        }
        if self.nesting.is_empty() {
            self.diagnose(
                range_of(&node.name_loc()),
                "Top-level method definition is not supported",
            );
            return;
        }
        let leading = self.take_leading_block(start_line);
        let trailing = match node.end_keyword_loc() {
            Some(_) => {
                let end_location = node
                    .rparen_loc()
                    .or_else(|| node.parameters().map(|parameters| parameters.location()))
                    .unwrap_or_else(|| node.name_loc());
                let end_line = self.end_line(&end_location);
                self.take_trailing_block(end_line)
            }
            None => None,
        };
        let method_type = self.build_method_type(leading, trailing, node);
        let comment = self.comment_of(leading);
        let member = MethodMember {
            keyword: range_of(&node.def_keyword_loc()),
            name: range_of(&node.name_loc()),
            singleton: receiver.is_some(),
            range: range_of(&location),
            method_type,
            comment,
        };
        if let Some(container) = self.nesting.last_mut() {
            container
                .members
                .push((start_line, Member::Method(Box::new(member))));
        }
        let end_line = self.end_line(&location);
        self.report_enclosed_blocks(start_line, end_line);
    }

    fn visit_call_node(&mut self, node: &ruby_prism::CallNode<'pr>) {
        if node.receiver().is_some() {
            return;
        }
        let location = node.location();
        match node.name().as_slice() {
            b"include" | b"extend" | b"prepend" => {
                if self.is_skipped(self.start_line(&location)) {
                    return;
                }
                if !self.nesting.is_empty() {
                    self.parse_mixin_call(node);
                }
            }
            b"attr_reader" | b"attr_writer" | b"attr_accessor" => {
                if self.is_skipped(self.start_line(&location)) {
                    return;
                }
                if self.nesting.is_empty() {
                    let range = node
                        .message_loc()
                        .map_or_else(|| range_of(&location), |location| range_of(&location));
                    self.diagnose(range, "Top-level attribute definition is not supported");
                } else {
                    self.parse_attribute_call(node);
                }
            }
            _ => ruby_prism::visit_call_node(self, node),
        }
    }

    fn visit_constant_write_node(&mut self, node: &ruby_prism::ConstantWriteNode<'pr>) {
        if self.is_skipped(self.start_line(&node.location())) {
            return;
        }
        let name = range_of(&node.name_loc());
        self.parse_constant_declaration(&node.as_node(), name, true, &node.value());
    }

    fn visit_constant_path_write_node(&mut self, node: &ruby_prism::ConstantPathWriteNode<'pr>) {
        if self.is_skipped(self.start_line(&node.location())) {
            return;
        }
        let target = node.target();
        let name = range_of(&target.location());
        let target_is_constant = is_constant_name(&target.as_node());
        self.parse_constant_declaration(&node.as_node(), name, target_is_constant, &node.value());
    }
}

/// Keywords of RBS that cannot be written as a method or attribute name without backquotes
const RBS_KEYWORDS: [&str; 30] = [
    "alias",
    "attr_accessor",
    "attr_reader",
    "attr_writer",
    "bool",
    "bot",
    "class",
    "def",
    "end",
    "extend",
    "false",
    "include",
    "instance",
    "interface",
    "module",
    "nil",
    "out",
    "prepend",
    "private",
    "public",
    "self",
    "singleton",
    "super",
    "top",
    "true",
    "type",
    "unchecked",
    "untyped",
    "use",
    "void",
];

/// Operators RBS reads as method names without backquotes
const RBS_OPERATORS: [&str; 28] = [
    "+", "-", "*", "/", "%", "**", "==", "===", "!=", "=~", "!~", "<", "<=", ">", ">=", "<=>", "<<", ">>", "[]", "[]=",
    "!", "~", "+@", "-@", "&", "|", "^", "`",
];

/// A name RBS cannot read bare, as a keyword or a symbol such as `:"two words"`
fn needs_backquotes(name: &str) -> bool {
    let identifier = name.strip_suffix(['?', '!', '=']).unwrap_or(name);
    let is_identifier = identifier
        .chars()
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && identifier
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_');
    RBS_KEYWORDS.contains(&name) || !(is_identifier || RBS_OPERATORS.contains(&name))
}

struct RbsWriter<'source> {
    source: &'source str,
    rbs: String,
    copied_spans: Vec<CopiedSpan>,
    methods: Vec<InlineMethod>,
}

impl RbsWriter<'_> {
    fn push(&mut self, text: &str) {
        self.rbs.push_str(text);
    }

    fn copy(&mut self, range: Range<usize>) {
        self.copied_spans.push(CopiedSpan {
            rbs_start: self.rbs.len(),
            source_start: range.start,
            length: range.len(),
        });
        self.rbs.push_str(&self.source[range]);
    }

    fn copy_snippet(&mut self, snippet: &Snippet) {
        let start = self.rbs.len();
        for (text_start, source_start, length) in &snippet.pieces {
            self.copied_spans.push(CopiedSpan {
                rbs_start: start + text_start,
                source_start: *source_start,
                length: *length,
            });
        }
        self.rbs.push_str(&snippet.text);
    }

    fn copy_name(&mut self, range: Range<usize>) {
        if needs_backquotes(&self.source[range.clone()]) {
            self.push("`");
            self.copy(range);
            self.push("`");
        } else {
            self.copy(range);
        }
    }

    fn write_comment(&mut self, comment: &[String]) {
        for line in comment {
            self.push("# ");
            self.push(line);
            self.push("\n");
        }
    }

    fn write_declaration(&mut self, declaration: &Declaration) {
        match declaration {
            Declaration::Container(container) => self.write_container(container),
            Declaration::Constant {
                name,
                constant_type,
                comment,
            } => {
                self.write_comment(comment);
                self.copy(name.clone());
                self.push(": ");
                match constant_type {
                    ConstantType::Annotated(annotated) => self.copy_snippet(annotated),
                    ConstantType::Inferred(inferred) => self.push(inferred),
                }
                self.push("\n");
            }
            Declaration::ClassAlias {
                module,
                new_name,
                old_name,
            } => {
                self.push(if *module { "module " } else { "class " });
                self.copy(new_name.clone());
                self.push(" = ");
                match old_name {
                    AliasedName::Source(range) => self.copy(range.clone()),
                    AliasedName::Annotation(annotated) => self.copy_snippet(annotated),
                }
                self.push("\n");
            }
        }
    }

    fn write_container(&mut self, container: &Container) {
        self.copy(container.keyword.clone());
        self.push(" ");
        self.copy(container.name.clone());
        if let Some(super_class) = &container.super_class {
            self.push(" < ");
            self.copy(super_class.name.clone());
            if let Some(arguments) = &super_class.arguments {
                self.copy_snippet(arguments);
            }
        }
        if container.kind == ContainerKind::Module {
            let self_types: Vec<&Snippet> = container
                .members
                .iter()
                .filter_map(|(_, member)| match member {
                    Member::ModuleSelf(self_type) => Some(self_type),
                    _ => None,
                })
                .collect();
            for (index, self_type) in self_types.into_iter().enumerate() {
                self.push(if index == 0 { " : " } else { ", " });
                self.copy_snippet(self_type);
            }
        }
        self.push("\n");
        for (_, member) in &container.members {
            self.write_member(member);
        }
        self.copy(container.end_keyword.clone());
        self.push("\n");
    }

    fn write_member(&mut self, member: &Member) {
        match member {
            Member::Declaration(declaration) => self.write_declaration(declaration),
            Member::Method(method) => self.write_method(method),
            Member::Mixin {
                keyword,
                name,
                arguments,
            } => {
                self.copy(keyword.clone());
                self.push(" ");
                self.copy(name.clone());
                if let Some(arguments) = arguments {
                    self.copy_snippet(arguments);
                }
                self.push("\n");
            }
            Member::Attribute {
                keyword,
                names,
                attribute_type,
                comment,
            } => {
                for (range, name) in names {
                    self.write_comment(comment);
                    self.copy(keyword.clone());
                    self.push(" ");
                    if self.source[range.clone()] == *name {
                        self.copy_name(range.clone());
                    } else {
                        self.push("`");
                        self.push(name);
                        self.push("`");
                    }
                    self.push(": ");
                    match attribute_type {
                        Some(attribute_type) => self.copy_snippet(attribute_type),
                        None => self.push("untyped"),
                    }
                    self.push("\n");
                }
            }
            Member::InstanceVariable {
                name,
                variable_type,
            } => {
                self.copy_snippet(name);
                self.push(": ");
                self.copy_snippet(variable_type);
                self.push("\n");
            }
            Member::ModuleSelf(_) => {}
        }
    }

    fn write_method(&mut self, method: &MethodMember) {
        self.write_comment(&method.comment);
        self.methods.push(InlineMethod {
            rbs_start: self.rbs.len(),
            source_range: method.range.clone(),
            annotated: !matches!(method.method_type, MethodTypeSource::Unannotated),
        });
        self.copy(method.keyword.clone());
        self.push(" ");
        if method.singleton {
            self.push("self.");
        }
        self.copy_name(method.name.clone());
        self.push(": ");
        match &method.method_type {
            MethodTypeSource::Overloads(overloads) => {
                for (index, overload) in overloads.iter().enumerate() {
                    if index > 0 {
                        self.push("\n    | ");
                    }
                    self.copy_snippet(overload);
                }
            }
            MethodTypeSource::DocStyle(doc) => self.write_doc_style(doc),
            MethodTypeSource::Unannotated => self.push("(?) -> untyped"),
        }
        self.push("\n");
    }

    fn write_parameter_type(&mut self, parameter_type: &Option<Snippet>) {
        match parameter_type {
            Some(parameter_type) => self.copy_snippet(parameter_type),
            None => self.push("untyped"),
        }
    }

    fn write_doc_style(&mut self, doc: &DocStyle) {
        let mut first = true;
        let mut separator = |writer: &mut Self| {
            if !first {
                writer.push(", ");
            }
            first = false;
        };
        self.push("(");
        let positional = |writer: &mut Self, prefix: &str, parameter: &DocParameter| {
            writer.push(prefix);
            writer.write_parameter_type(&parameter.parameter_type);
            if let Some(name) = &parameter.name {
                writer.push(" ");
                writer.push(name);
            }
        };
        for parameter in &doc.required_positionals {
            separator(self);
            positional(self, "", parameter);
        }
        for parameter in &doc.optional_positionals {
            separator(self);
            positional(self, "?", parameter);
        }
        if let Some(parameter) = &doc.rest_positionals {
            separator(self);
            positional(self, "*", parameter);
        }
        for parameter in &doc.trailing_positionals {
            separator(self);
            positional(self, "", parameter);
        }
        for (prefix, keywords) in [("", &doc.required_keywords), ("?", &doc.optional_keywords)] {
            for (name, parameter_type) in keywords {
                separator(self);
                self.push(prefix);
                self.push(name);
                self.push(": ");
                self.write_parameter_type(parameter_type);
            }
        }
        if let Some(parameter) = &doc.rest_keywords {
            separator(self);
            positional(self, "**", parameter);
        }
        self.push(")");
        match &doc.block {
            Some(DocBlock::Annotated {
                optional,
                block_type,
            }) => {
                self.push(if *optional { " ?{ " } else { " { " });
                self.copy_snippet(block_type);
                self.push(" }");
            }
            Some(DocBlock::Unannotated) => self.push(" ?{ (?) -> untyped }"),
            None => {}
        }
        self.push(" -> ");
        self.write_parameter_type(&doc.return_type);
    }
}
