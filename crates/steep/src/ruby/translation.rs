mod builder;
mod compiler;
mod numeric;

use std::{ffi::CStr, marker::PhantomData, mem::MaybeUninit, ptr::NonNull, rc::Rc};

use ruby_prism_sys::{
    PM_ERR_ARGUMENT_BLOCK_MULTI, PM_ERR_ARGUMENT_FORMAL_CLASS, PM_ERR_ARGUMENT_FORMAL_CONSTANT,
    PM_ERR_ARGUMENT_FORMAL_GLOBAL, PM_ERR_ARGUMENT_FORMAL_IVAR, PM_ERR_ARGUMENT_NO_FORWARDING_STAR,
    PM_ERR_ARGUMENT_NO_FORWARDING_STAR_STAR, PM_ERR_BEGIN_LONELY_ELSE, PM_ERR_CLASS_IN_METHOD,
    PM_ERR_CLASS_NAME, PM_ERR_DEF_ENDLESS_SETTER, PM_ERR_EMBDOC_TERM,
    PM_ERR_INCOMPLETE_VARIABLE_CLASS, PM_ERR_INCOMPLETE_VARIABLE_CLASS_3_3,
    PM_ERR_INCOMPLETE_VARIABLE_INSTANCE, PM_ERR_INCOMPLETE_VARIABLE_INSTANCE_3_3,
    PM_ERR_INVALID_VARIABLE_GLOBAL, PM_ERR_INVALID_VARIABLE_GLOBAL_3_3, PM_ERR_MODULE_IN_METHOD,
    PM_ERR_MODULE_NAME, PM_ERR_NUMBERED_PARAMETER_ORDINARY, PM_ERR_PARAMETER_CIRCULAR,
    PM_ERR_PARAMETER_NUMBERED_RESERVED, PM_ERR_REGEXP_UNKNOWN_OPTIONS,
    PM_ERR_SINGLETON_FOR_LITERALS, PM_ERR_STRING_LITERAL_EOF, PM_ERR_UNEXPECTED_TOKEN_IGNORE,
    PM_ERR_WRITE_TARGET_IN_METHOD, PM_OPTIONS_VERSION_CRUBY_3_3, PM_OPTIONS_VERSION_CRUBY_3_4,
    pm_comment_t, pm_diagnostic_t, pm_location_t, pm_node_destroy, pm_node_t, pm_options_t,
    pm_parse, pm_parser_free, pm_parser_init, pm_parser_t,
};

use crate::steep_version::SteepVersion;

use crate::ruby::node::{Buffer, Comment, Node, SourceRange};
use crate::ruby::translation::{builder::Builder, compiler::Compiler};

pub use crate::ruby::translation::numeric::inspect_float;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyntaxError {
    pub message: String,
    pub location: SourceRange,
}

pub struct ParseResult {
    pub node: Option<Rc<Node>>,
    pub comments: Vec<Comment>,
}

// Why not `ruby_prism::parse`: it parses without options, but the translation
// parses as the Ruby version of Steep's parser with `partial_script` and encoding
// magic comments ignored, which changes both the accepted syntax and the reported errors.
struct PrismParse<'source> {
    parser: NonNull<pm_parser_t>,
    node: NonNull<pm_node_t>,
    _options: Box<pm_options_t>,
    marker: PhantomData<&'source [u8]>,
}

impl<'source> PrismParse<'source> {
    fn new(source: &'source [u8], steep_version: SteepVersion) -> PrismParse<'source> {
        unsafe {
            let mut options: Box<pm_options_t> = Box::new(std::mem::zeroed());
            options.line = 1;
            options.version = match steep_version {
                SteepVersion::Version2_0_0 => PM_OPTIONS_VERSION_CRUBY_3_3,
                SteepVersion::Version2_1_0 => PM_OPTIONS_VERSION_CRUBY_3_4,
            };
            options.encoding_locked = true;
            options.partial_script = true;
            let parser = Box::into_raw(Box::new(MaybeUninit::<pm_parser_t>::uninit()));
            let parser = (*parser).as_mut_ptr();
            pm_parser_init(parser, source.as_ptr(), source.len(), &*options);
            let node = pm_parse(parser);
            PrismParse {
                parser: NonNull::new_unchecked(parser),
                node: NonNull::new_unchecked(node),
                _options: options,
                marker: PhantomData,
            }
        }
    }

    fn program(&self) -> ruby_prism::Node<'_> {
        ruby_prism::Node::ProgramNode {
            parser: self.parser,
            pointer: self.node.as_ptr().cast(),
            marker: PhantomData,
        }
    }

    fn start(&self) -> *const u8 {
        unsafe { (*self.parser.as_ptr()).start }
    }

    fn offsets(&self, location: &pm_location_t) -> (usize, usize) {
        let start = self.start();
        unsafe {
            (
                location.start.offset_from(start).max(0) as usize,
                location.end.offset_from(start).max(0) as usize,
            )
        }
    }

    fn first_error(&self) -> Option<(u32, String, (usize, usize))> {
        unsafe {
            let head = (*self.parser.as_ptr()).error_list.head;
            let diagnostic = head.cast::<pm_diagnostic_t>().as_ref()?;
            let message = CStr::from_ptr(diagnostic.message)
                .to_string_lossy()
                .into_owned();
            Some((
                diagnostic.diag_id,
                message,
                self.offsets(&diagnostic.location),
            ))
        }
    }

    fn comments(&self) -> Vec<(usize, usize)> {
        let mut comments = Vec::new();
        unsafe {
            let mut current = (*self.parser.as_ptr()).comment_list.head;
            while let Some(comment) = current.cast::<pm_comment_t>().as_ref() {
                comments.push(self.offsets(&comment.location));
                current = comment.node.next;
            }
        }
        comments
    }
}

impl Drop for PrismParse<'_> {
    fn drop(&mut self) {
        unsafe {
            pm_node_destroy(self.parser.as_ptr(), self.node.as_ptr());
            pm_parser_free(self.parser.as_ptr());
            drop(Box::from_raw(
                self.parser.as_ptr().cast::<MaybeUninit<pm_parser_t>>(),
            ));
        }
    }
}

fn floor_to_character(source: &str, offset: usize) -> usize {
    let mut offset = offset.min(source.len());
    while !source.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

fn error_diagnostic(
    buffer: &Buffer,
    diagnostic_id: u32,
    message: String,
    offsets: (usize, usize),
) -> SyntaxError {
    let source = buffer.source.as_bytes();
    let (start, mut end) = offsets;
    let slice = |start: usize, end: usize| {
        String::from_utf8_lossy(&source[start.min(source.len())..end.min(source.len())])
            .into_owned()
    };
    let message = match diagnostic_id {
        PM_ERR_ARGUMENT_BLOCK_MULTI => {
            "both block argument and literal block are passed".to_string()
        }
        PM_ERR_ARGUMENT_FORMAL_CONSTANT => "formal argument cannot be a constant".to_string(),
        PM_ERR_ARGUMENT_FORMAL_CLASS => "formal argument cannot be a class variable".to_string(),
        PM_ERR_ARGUMENT_FORMAL_GLOBAL => "formal argument cannot be a global variable".to_string(),
        PM_ERR_ARGUMENT_FORMAL_IVAR => "formal argument cannot be an instance variable".to_string(),
        PM_ERR_ARGUMENT_NO_FORWARDING_STAR => "no anonymous rest parameter".to_string(),
        PM_ERR_ARGUMENT_NO_FORWARDING_STAR_STAR => {
            "no anonymous keyword rest parameter".to_string()
        }
        PM_ERR_BEGIN_LONELY_ELSE => {
            end = start + 4;
            "else without rescue is useless".to_string()
        }
        PM_ERR_CLASS_NAME | PM_ERR_MODULE_NAME => {
            "class or module name must be a constant literal".to_string()
        }
        PM_ERR_CLASS_IN_METHOD => "class definition in method body".to_string(),
        PM_ERR_DEF_ENDLESS_SETTER => {
            "setter method cannot be defined in an endless method definition".to_string()
        }
        PM_ERR_EMBDOC_TERM => {
            "embedded document meets end of file (and they embark on a romantic journey)"
                .to_string()
        }
        PM_ERR_INCOMPLETE_VARIABLE_CLASS | PM_ERR_INCOMPLETE_VARIABLE_CLASS_3_3 => {
            end += 1;
            format!(
                "`{}' is not allowed as a class variable name",
                slice(start, end)
            )
        }
        PM_ERR_INCOMPLETE_VARIABLE_INSTANCE | PM_ERR_INCOMPLETE_VARIABLE_INSTANCE_3_3 => {
            end += 1;
            format!(
                "`{}' is not allowed as an instance variable name",
                slice(start, end)
            )
        }
        PM_ERR_INVALID_VARIABLE_GLOBAL | PM_ERR_INVALID_VARIABLE_GLOBAL_3_3 => format!(
            "`{}' is not allowed as a global variable name",
            slice(start, end)
        ),
        PM_ERR_MODULE_IN_METHOD => "module definition in method body".to_string(),
        PM_ERR_NUMBERED_PARAMETER_ORDINARY => "ordinary parameter is defined".to_string(),
        PM_ERR_PARAMETER_CIRCULAR => {
            format!("circular argument reference {}", slice(start, end))
        }
        PM_ERR_PARAMETER_NUMBERED_RESERVED => {
            format!("{} is reserved for numbered parameter", slice(start, end))
        }
        PM_ERR_REGEXP_UNKNOWN_OPTIONS => {
            let options: String = slice(start, end).chars().skip(1).collect();
            format!("unknown regexp options: {options}")
        }
        PM_ERR_SINGLETON_FOR_LITERALS => {
            "cannot define a singleton method for a literal".to_string()
        }
        PM_ERR_STRING_LITERAL_EOF => "unterminated string meets end of file".to_string(),
        PM_ERR_UNEXPECTED_TOKEN_IGNORE => format!("unexpected token {}", slice(start, end)),
        PM_ERR_WRITE_TARGET_IN_METHOD => "dynamic constant assignment".to_string(),
        _ => message,
    };
    SyntaxError {
        message,
        location: SourceRange::new(
            floor_to_character(&buffer.source, start),
            floor_to_character(&buffer.source, end),
        ),
    }
}

pub fn parse(buffer: &Buffer, steep_version: SteepVersion) -> Result<ParseResult, SyntaxError> {
    let parsed = PrismParse::new(buffer.source.as_bytes(), steep_version);
    if let Some((diagnostic_id, message, offsets)) = parsed.first_error() {
        return Err(error_diagnostic(buffer, diagnostic_id, message, offsets));
    }

    let mut compiler = Compiler::new(Builder::new(buffer), &buffer.source);
    let node = compiler.visit(&parsed.program());
    if let Some(error) = compiler.builder.error.take() {
        return Err(error);
    }

    let comments = parsed
        .comments()
        .into_iter()
        .map(|(start, end)| {
            let location = SourceRange::new(
                floor_to_character(&buffer.source, start),
                floor_to_character(&buffer.source, end),
            );
            Comment {
                text: buffer.source_of(location).to_string(),
                location,
            }
        })
        .collect();

    Ok(ParseResult { node, comments })
}
