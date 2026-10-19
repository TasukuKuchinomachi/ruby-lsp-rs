use std::collections::HashMap;

use rbs_environment::{AnnotationSyntax, MethodType, Type, annotation_syntax_error_message, parse_method_type, parse_type};
use ruby_prism::ParseResult;

use crate::source::LineIndex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AnnotationTarget {
    LocalVariable(String),
    InstanceVariable(String),
    SelfType,
    /// `@type const Name: T`, with the name as written
    Constant(String),
    /// `@type return: T`
    Return,
    /// `@type break: T`
    Break,
}

/// A Steep annotation such as `# @type var name: String`. The type is not resolved yet because resolution depends on
/// the nesting of the scope that the annotation ends up in
#[derive(Debug, Clone)]
pub(crate) struct TypeAnnotation {
    pub(crate) offset: usize,
    pub(crate) target: AnnotationTarget,
    pub(crate) annotated_type: Type,
}

/// `# @type method name: (Integer) -> String`
#[derive(Debug, Clone)]
pub(crate) struct MethodTypeAnnotation {
    pub(crate) offset: usize,
    pub(crate) name: String,
    pub(crate) method_type: MethodType,
}

/// `# @implements Name[T]`, with the name as written
#[derive(Debug, Clone)]
pub(crate) struct ImplementsAnnotation {
    pub(crate) offset: usize,
    pub(crate) name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DynamicKind {
    Instance,
    /// `self.name`
    Module,
    /// `self?.name`
    ModuleInstance,
}

/// `# @dynamic name, self.name`
#[derive(Debug, Clone)]
pub(crate) struct DynamicAnnotation {
    pub(crate) offset: usize,
    pub(crate) names: Vec<(String, DynamicKind)>,
}

/// A type in an annotation RBS cannot parse, which makes Steep report only `AnnotationSyntaxError` for the file
#[derive(Debug, Clone)]
pub(crate) struct AnnotationSyntaxError {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) message: String,
}

#[derive(Debug, Default)]
pub(crate) struct Annotations {
    /// The first annotation with a syntax error, in source order
    pub(crate) syntax_error: Option<AnnotationSyntaxError>,
    pub(crate) type_annotations: Vec<TypeAnnotation>,
    pub(crate) method_type_annotations: Vec<MethodTypeAnnotation>,
    pub(crate) implements_annotations: Vec<ImplementsAnnotation>,
    pub(crate) dynamic_annotations: Vec<DynamicAnnotation>,
    pub(crate) inline_assertions: HashMap<usize, InlineAssertion>,
}

#[derive(Debug, Clone)]
pub(crate) struct InlineAssertion {
    pub(crate) asserted_type: Type,
}

impl Annotations {
    pub(crate) fn collect(parse_result: &ParseResult<'_>, line_index: &LineIndex) -> Annotations {
        let mut annotations = Annotations::default();
        for comment in parse_result.comments() {
            let location = comment.location();
            let start = location.start_offset();
            if location.end_offset() <= start {
                continue;
            }
            let text = String::from_utf8_lossy(location.as_slice());
            let Some(body) = text.strip_prefix('#') else {
                continue;
            };
            if let Some(type_text) = body.strip_prefix(':') {
                if let Ok(asserted_type) = parse_type(type_text.trim()) {
                    annotations
                        .inline_assertions
                        .insert(line_index.line_of(start), InlineAssertion { asserted_type });
                }
                continue;
            }
            let content = body.trim();
            let content_start = start + 1 + (body.len() - body.trim_start().len());
            annotations.add(content, start, content_start);
        }
        annotations
    }

    fn record_syntax_error(&mut self, content: &str, content_start: usize, text: &str, syntax: AnnotationSyntax, whole: bool) {
        if self.syntax_error.is_some() {
            return;
        }
        let Some(message) = annotation_syntax_error_message(text, syntax) else {
            return;
        };
        let (start, end) = if whole {
            (content_start, content_start + content.len())
        } else {
            let text_start = content_start + (text.as_ptr() as usize - content.as_ptr() as usize);
            (text_start, text_start + text.len())
        };
        self.syntax_error = Some(AnnotationSyntaxError {
            start,
            end,
            message: format!("Type annotation has a syntax error: {message}"),
        });
    }

    /// Follows the order of the cases of Steep's `AnnotationParser#parse`
    fn add(&mut self, content: &str, offset: usize, content_start: usize) {
        if let Some(rest) = keyword_rest(content, "@type") {
            if let Some((keyword, rest)) = rest.split_once(char::is_whitespace) {
                let target: Option<fn(String) -> AnnotationTarget> = match keyword {
                    "var" => Some(AnnotationTarget::LocalVariable),
                    "ivar" => Some(AnnotationTarget::InstanceVariable),
                    "const" => Some(AnnotationTarget::Constant),
                    _ => None,
                };
                if let Some(target) = target
                    && let Some((name, type_text)) = rest.split_once(':')
                {
                    let type_text = type_text.trim();
                    match parse_type(type_text) {
                        Ok(annotated_type) => self.type_annotations.push(TypeAnnotation {
                            offset,
                            target: target(name.trim().to_string()),
                            annotated_type,
                        }),
                        Err(_) => self.record_syntax_error(content, content_start, type_text, AnnotationSyntax::Type, false),
                    }
                    return;
                }
                if keyword == "method"
                    && let Some((name, type_text)) = rest.split_once(':')
                {
                    let type_text = type_text.trim();
                    match parse_method_type(type_text) {
                        Ok(method_type) => self.method_type_annotations.push(MethodTypeAnnotation {
                            offset,
                            name: name.trim().to_string(),
                            method_type,
                        }),
                        Err(_) => self.record_syntax_error(content, content_start, type_text, AnnotationSyntax::MethodType, true),
                    }
                    return;
                }
            }
            let keyword_and_type = [
                ("self", AnnotationTarget::SelfType),
                ("return", AnnotationTarget::Return),
                ("break", AnnotationTarget::Break),
            ];
            for (keyword, target) in keyword_and_type {
                if let Some(type_text) = rest.strip_prefix(keyword).and_then(|after| after.trim_start().strip_prefix(':')) {
                    let type_text = type_text.trim();
                    match parse_type(type_text) {
                        Ok(annotated_type) => self.type_annotations.push(TypeAnnotation { offset, target, annotated_type }),
                        Err(_) => self.record_syntax_error(content, content_start, type_text, AnnotationSyntax::Type, false),
                    }
                    return;
                }
            }
            return;
        }
        if let Some(rest) = keyword_rest(content, "@dynamic") {
            let names = rest
                .split(',')
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(|name| {
                    if let Some(name) = name.strip_prefix("self.") {
                        (name.to_string(), DynamicKind::Module)
                    } else if let Some(name) = name.strip_prefix("self?.") {
                        (name.to_string(), DynamicKind::ModuleInstance)
                    } else {
                        (name.to_string(), DynamicKind::Instance)
                    }
                })
                .collect();
            self.dynamic_annotations.push(DynamicAnnotation { offset, names });
            return;
        }
        if let Some(rest) = keyword_rest(content, "@implements") {
            let name = rest.split('[').next().unwrap_or_default().trim();
            if !name.is_empty() {
                self.implements_annotations.push(ImplementsAnnotation { offset, name: name.to_string() });
            }
        }
    }
}

/// The text after `keyword` and the whitespace that must follow it
fn keyword_rest<'a>(body: &'a str, keyword: &str) -> Option<&'a str> {
    let rest = body.strip_prefix(keyword)?;
    rest.starts_with(char::is_whitespace).then(|| rest.trim_start())
}
