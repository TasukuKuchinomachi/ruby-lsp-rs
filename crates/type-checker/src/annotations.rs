use std::collections::HashMap;

use rbs_environment::{Type, parse_type};
use ruby_prism::ParseResult;

use crate::source::LineIndex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AnnotationTarget {
    LocalVariable(String),
    InstanceVariable(String),
    SelfType,
}

/// A Steep annotation such as `# @type var name: String`. The type is not resolved yet because resolution depends on
/// the nesting of the scope that the annotation ends up in
#[derive(Debug, Clone)]
pub(crate) struct TypeAnnotation {
    pub(crate) offset: usize,
    pub(crate) target: AnnotationTarget,
    pub(crate) annotated_type: Type,
}

#[derive(Debug, Default)]
pub(crate) struct Annotations {
    pub(crate) type_annotations: Vec<TypeAnnotation>,
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
            if let Some(annotation) = parse_type_annotation(body.trim(), start) {
                annotations.type_annotations.push(annotation);
            }
        }
        annotations
    }
}

fn parse_type_annotation(body: &str, offset: usize) -> Option<TypeAnnotation> {
    let rest = body.strip_prefix("@type")?;
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let rest = rest.trim_start();
    let (target, type_text) = if let Some(after_self) = rest.strip_prefix("self") {
        (
            AnnotationTarget::SelfType,
            after_self.trim_start().strip_prefix(':')?,
        )
    } else {
        let (keyword, rest) = rest.split_once(char::is_whitespace)?;
        let (name, type_text) = rest.split_once(':')?;
        let name = name.trim().to_string();
        match keyword {
            "var" => (AnnotationTarget::LocalVariable(name), type_text),
            "ivar" => (AnnotationTarget::InstanceVariable(name), type_text),
            _ => return None,
        }
    };
    let annotated_type = parse_type(type_text.trim()).ok()?;
    Some(TypeAnnotation {
        offset,
        target,
        annotated_type,
    })
}
