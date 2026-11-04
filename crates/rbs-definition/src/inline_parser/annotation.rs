use std::{ffi::CStr, ptr::NonNull, sync::Arc};

use ruby_rbs::node::{Node, NodeList, RBSLocationRange};
use ruby_rbs_sys::bindings::{
    RBSTokenType, RBSTokenType_pEOF, RBSTokenType_tINLINECOMMENT, rbs_allocator_t,
    rbs_ast_ruby_annotations_class_alias_annotation,
    rbs_ast_ruby_annotations_module_alias_annotation, rbs_encoding_type_t::RBS_ENCODING_UTF_8,
    rbs_encodings, rbs_node_list_t, rbs_node_t, rbs_parser_free, rbs_parser_new, rbs_parser_t,
    rbs_string_new, rbs_token_t,
};

use crate::{
    ast::{Block, MethodType, Overload, Type, TypeName},
    location::{Buffer, Location},
    parse,
};

unsafe extern "C" {
    fn rbs_parse_inline_leading_annotation(
        parser: *mut rbs_parser_t,
        annotation: *mut *mut rbs_node_t,
    ) -> bool;
    fn rbs_parse_inline_trailing_annotation(
        parser: *mut rbs_parser_t,
        annotation: *mut *mut rbs_node_t,
    ) -> bool;
    fn rbs_parse_type(
        parser: *mut rbs_parser_t,
        type_: *mut *mut rbs_node_t,
        void_allowed: bool,
        self_allowed: bool,
        classish_allowed: bool,
    ) -> bool;
    fn rbs_parser_push_typevar_table(parser: *mut rbs_parser_t, reset: bool);
    fn rbs_parser_advance(parser: *mut rbs_parser_t);
    fn rbs_node_list_new(allocator: *mut rbs_allocator_t) -> *mut rbs_node_list_t;
    fn rbs_node_list_append(list: *mut rbs_node_list_t, node: *mut rbs_node_t);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ByteRange {
    pub start: usize,
    pub end: usize,
}

#[derive(Clone, Debug)]
pub struct ParsingError {
    pub location: ByteRange,
    pub message: String,
}

#[derive(Clone, Debug)]
pub enum Annotation {
    NodeTypeAssertion {
        location: ByteRange,
        type_: Type,
        type_location: Option<Location>,
    },
    ClassAlias {
        location: ByteRange,
        type_name: Option<TypeName>,
    },
    ModuleAlias {
        location: ByteRange,
        type_name: Option<TypeName>,
    },
    TypeApplication {
        location: ByteRange,
        type_arguments: Vec<Type>,
    },
    ColonMethodType {
        location: ByteRange,
        annotations: Vec<String>,
        method_type: Box<MethodType>,
    },
    MethodTypes {
        location: ByteRange,
        overloads: Vec<Overload>,
        overloading: bool,
    },
    Skip {
        location: ByteRange,
    },
    ReturnType {
        location: ByteRange,
        return_type: Type,
    },
    InstanceVariable {
        location: ByteRange,
        name: String,
        type_: Type,
        type_location: Option<Location>,
    },
    ParameterType {
        location: ByteRange,
        name: String,
        parameter_type: Type,
    },
    SplatParameterType {
        location: ByteRange,
        name: Option<String>,
        parameter_type: Type,
    },
    DoubleSplatParameterType {
        location: ByteRange,
        name: Option<String>,
        parameter_type: Type,
    },
    BlockParameterType {
        location: ByteRange,
        name: Option<String>,
        block: Box<Block>,
    },
    ModuleSelf {
        location: ByteRange,
        name: TypeName,
        arguments: Vec<Type>,
    },
}

impl Annotation {
    pub fn location(&self) -> ByteRange {
        match self {
            Annotation::NodeTypeAssertion { location, .. }
            | Annotation::ClassAlias { location, .. }
            | Annotation::ModuleAlias { location, .. }
            | Annotation::TypeApplication { location, .. }
            | Annotation::ColonMethodType { location, .. }
            | Annotation::MethodTypes { location, .. }
            | Annotation::Skip { location }
            | Annotation::ReturnType { location, .. }
            | Annotation::InstanceVariable { location, .. }
            | Annotation::ParameterType { location, .. }
            | Annotation::SplatParameterType { location, .. }
            | Annotation::DoubleSplatParameterType { location, .. }
            | Annotation::BlockParameterType { location, .. }
            | Annotation::ModuleSelf { location, .. } => *location,
        }
    }
}

pub(crate) struct Parser<'content> {
    pointer: NonNull<rbs_parser_t>,
    content: &'content str,
}

impl<'content> Parser<'content> {
    pub(crate) fn new(content: &'content str, start: usize, end: usize) -> Parser<'content> {
        unsafe {
            let start_pointer = content.as_ptr().cast::<std::os::raw::c_char>();
            let string = rbs_string_new(start_pointer, start_pointer.add(content.len()));
            let encoding = &rbs_encodings[RBS_ENCODING_UTF_8 as usize] as *const _;
            let pointer = rbs_parser_new(string, encoding, start as i32, end as i32);
            let pointer = NonNull::new(pointer).expect("the RBS parser is allocated");
            rbs_parser_push_typevar_table(pointer.as_ptr(), true);
            Parser { pointer, content }
        }
    }

    pub(crate) fn pointer(&self) -> *mut rbs_parser_t {
        self.pointer.as_ptr()
    }

    pub(crate) fn raw(&self) -> &rbs_parser_t {
        unsafe { self.pointer.as_ref() }
    }

    fn error(&self) -> Option<ParsingError> {
        let error = unsafe { self.raw().error.as_ref()? };
        let message = if error.message.is_null() {
            String::new()
        } else {
            unsafe { CStr::from_ptr(error.message) }
                .to_string_lossy()
                .into_owned()
        };
        Some(ParsingError {
            location: token_range(&error.token),
            message,
        })
    }

    pub(crate) fn node<'parser>(&'parser self, node: *mut rbs_node_t) -> Node<'parser> {
        unsafe {
            let list = rbs_node_list_new(self.raw().allocator);
            rbs_node_list_append(list, node);
            NodeList::new(self.pointer, list)
                .iter()
                .next()
                .expect("the list has the node")
        }
    }

    fn source(&self, location: RBSLocationRange) -> String {
        self.content[location.start() as usize..location.end() as usize].to_string()
    }
}

impl Drop for Parser<'_> {
    fn drop(&mut self) {
        unsafe { rbs_parser_free(self.pointer.as_ptr()) }
    }
}

fn token_range(token: &rbs_token_t) -> ByteRange {
    ByteRange {
        start: token.range.start.byte_pos.max(0) as usize,
        end: token.range.end.byte_pos.max(0) as usize,
    }
}

fn byte_range(location: RBSLocationRange) -> ByteRange {
    ByteRange {
        start: location.start() as usize,
        end: location.end() as usize,
    }
}

pub struct CommentText<'text> {
    pub content: &'text str,
    pub buffer: &'text Arc<Buffer>,
    pub position: &'text dyn Fn(usize) -> usize,
}

impl CommentText<'_> {
    fn located_parser(&self) -> parse::Parser<'_> {
        parse::Parser {
            buffer: self.buffer,
            source: self.content,
            position: self.position,
        }
    }
}

pub fn parse_leading(
    text: &CommentText,
    start: usize,
    end: usize,
) -> Result<Option<Annotation>, ParsingError> {
    // The C parser vendored in ruby-rbs-sys predates rbs 4.2.0 and has no `module-self` keyword.
    if let Some(keyword_end) = module_self_keyword_end(text.content, start, end) {
        return parse_module_self(text, start, keyword_end, end).map(Some);
    }
    parse_with(text, start, end, rbs_parse_inline_leading_annotation)
}

pub fn parse_trailing(
    text: &CommentText,
    start: usize,
    end: usize,
) -> Result<Option<Annotation>, ParsingError> {
    parse_with(text, start, end, rbs_parse_inline_trailing_annotation)
}

fn parse_with(
    text: &CommentText,
    start: usize,
    end: usize,
    function: unsafe extern "C" fn(*mut rbs_parser_t, *mut *mut rbs_node_t) -> bool,
) -> Result<Option<Annotation>, ParsingError> {
    let parser = Parser::new(text.content, start, end);
    let mut annotation: *mut rbs_node_t = std::ptr::null_mut();
    let success = unsafe { function(parser.pointer.as_ptr(), &mut annotation) };
    if let Some(error) = parser.error() {
        return Err(error);
    }
    if !success || annotation.is_null() {
        return Ok(None);
    }
    Ok(Some(convert(&parser, &text.located_parser(), annotation)))
}

fn convert(parser: &Parser, located: &parse::Parser, pointer: *mut rbs_node_t) -> Annotation {
    match parser.node(pointer) {
        Node::NodeTypeAssertion(node) => Annotation::NodeTypeAssertion {
            location: byte_range(node.location()),
            type_location: located.type_node_location(&node.type_()),
            type_: located.type_(node.type_()),
        },
        Node::ClassAliasAnnotation(node) => {
            let raw = pointer.cast::<rbs_ast_ruby_annotations_class_alias_annotation>();
            let has_type_name = unsafe { !(*raw).type_name.is_null() };
            Annotation::ClassAlias {
                location: byte_range(node.location()),
                type_name: has_type_name.then(|| parse::type_name(node.type_name())),
            }
        }
        Node::ModuleAliasAnnotation(node) => {
            let raw = pointer.cast::<rbs_ast_ruby_annotations_module_alias_annotation>();
            let has_type_name = unsafe { !(*raw).type_name.is_null() };
            Annotation::ModuleAlias {
                location: byte_range(node.location()),
                type_name: has_type_name.then(|| parse::type_name(node.type_name())),
            }
        }
        Node::TypeApplicationAnnotation(node) => Annotation::TypeApplication {
            location: byte_range(node.location()),
            type_arguments: located.types(node.type_args()),
        },
        Node::ColonMethodTypeAnnotation(node) => Annotation::ColonMethodType {
            location: byte_range(node.location()),
            annotations: parse::annotations(node.annotations()),
            method_type: Box::new(located.method_type(node.method_type())),
        },
        Node::MethodTypesAnnotation(node) => Annotation::MethodTypes {
            location: byte_range(node.location()),
            overloads: node
                .overloads()
                .iter()
                .map(|overload| match overload {
                    Node::MethodDefinitionOverload(overload) => Overload {
                        method_type: Arc::new(located.method_type(overload.method_type())),
                        annotations: parse::annotations(overload.annotations()),
                    },
                    _ => unreachable!("method types annotation has only overloads"),
                })
                .collect(),
            overloading: node.dot3_location().is_some(),
        },
        Node::SkipAnnotation(node) => Annotation::Skip {
            location: byte_range(node.location()),
        },
        Node::ReturnTypeAnnotation(node) => Annotation::ReturnType {
            location: byte_range(node.location()),
            return_type: located.type_(node.return_type()),
        },
        Node::InstanceVariableAnnotation(node) => Annotation::InstanceVariable {
            location: byte_range(node.location()),
            name: parse::symbol(node.ivar_name()),
            type_location: located.type_node_location(&node.type_()),
            type_: located.type_(node.type_()),
        },
        Node::ParamTypeAnnotation(node) => Annotation::ParameterType {
            location: byte_range(node.location()),
            name: parser.source(node.name_location()),
            parameter_type: located.type_(node.param_type()),
        },
        Node::SplatParamTypeAnnotation(node) => Annotation::SplatParameterType {
            location: byte_range(node.location()),
            name: node.name_location().map(|location| parser.source(location)),
            parameter_type: located.type_(node.param_type()),
        },
        Node::DoubleSplatParamTypeAnnotation(node) => Annotation::DoubleSplatParameterType {
            location: byte_range(node.location()),
            name: node.name_location().map(|location| parser.source(location)),
            parameter_type: located.type_(node.param_type()),
        },
        Node::BlockParamTypeAnnotation(node) => Annotation::BlockParameterType {
            location: byte_range(node.location()),
            name: node.name_location().map(|location| parser.source(location)),
            block: Box::new(Block {
                type_: located.callable(node.type_()),
                required: node.question_location().is_none(),
                self_type: None,
            }),
        },
        _ => unreachable!("not an inline annotation"),
    }
}

fn skip_whitespace(content: &str, mut position: usize, end: usize) -> usize {
    let bytes = content.as_bytes();
    while position < end && bytes[position].is_ascii_whitespace() {
        position += 1;
    }
    position
}

fn is_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || !byte.is_ascii()
}

fn module_self_keyword_end(content: &str, start: usize, end: usize) -> Option<usize> {
    let bytes = content.as_bytes();
    let position = skip_whitespace(content, start, end);
    let after_rbs = position + "@rbs".len();
    if !content[position..end].starts_with("@rbs")
        || (after_rbs < end && is_identifier_byte(bytes[after_rbs]))
    {
        return None;
    }
    let keyword_start = skip_whitespace(content, after_rbs, end);
    content[keyword_start..end]
        .starts_with("module-self")
        .then_some(keyword_start + "module-self".len())
}

fn next_token_range(content: &str, position: usize, end: usize) -> ByteRange {
    let bytes = content.as_bytes();
    let start = skip_whitespace(content, position, end);
    let mut token_end = start;
    while token_end < end && !bytes[token_end].is_ascii_whitespace() {
        token_end += 1;
    }
    ByteRange {
        start,
        end: token_end,
    }
}

fn token_type(token: &rbs_token_t) -> RBSTokenType {
    token.type_
}

fn parse_module_self(
    text: &CommentText,
    start: usize,
    keyword_end: usize,
    end: usize,
) -> Result<Annotation, ParsingError> {
    let content = text.content;
    let annotation_start = skip_whitespace(content, start, end);
    let colon = skip_whitespace(content, keyword_end, end);
    if !content[colon..end].starts_with(':') {
        return Err(ParsingError {
            location: next_token_range(content, colon, end),
            message: "expected a token `pCOLON`".to_string(),
        });
    }
    let parser = Parser::new(content, colon + 1, end);
    let mut type_: *mut rbs_node_t = std::ptr::null_mut();
    let success = unsafe { rbs_parse_type(parser.pointer.as_ptr(), &mut type_, false, true, true) };
    if let Some(error) = parser.error() {
        return Err(error);
    }
    if !success || type_.is_null() {
        return Err(ParsingError {
            location: next_token_range(content, colon + 1, end),
            message: "expected one of class/module/constant name".to_string(),
        });
    }
    let (name, arguments) = match text.located_parser().type_(parser.node(type_)) {
        Type::ClassInstance { name, args, .. } | Type::Interface { name, args, .. } => (name, args),
        _ => {
            return Err(ParsingError {
                location: next_token_range(content, colon + 1, end),
                message: "expected one of class/module/constant name".to_string(),
            });
        }
    };
    if token_type(&parser.raw().next_token) == RBSTokenType_tINLINECOMMENT {
        unsafe { rbs_parser_advance(parser.pointer.as_ptr()) };
    }
    let annotation_end = token_range(&parser.raw().current_token).end;
    let next_token = parser.raw().next_token;
    if token_type(&next_token) != RBSTokenType_pEOF {
        return Err(ParsingError {
            location: token_range(&next_token),
            message: "expected a token `pEOF`".to_string(),
        });
    }
    Ok(Annotation::ModuleSelf {
        location: ByteRange {
            start: annotation_start,
            end: annotation_end,
        },
        name,
        arguments,
    })
}
