pub mod type_node;

use std::{ops::Range, rc::Rc};

use rbs_definition::collections::HashMap;

use steep_interface::steep_version::SteepVersion;

use crate::{
    annotation::{self, Annotation, AnnotationError},
    ast::node::{type_application::TypeApplication, type_assertion::TypeAssertion},
    ruby::{
        self,
        node::{Buffer, Comment, MapKind, Node, NodeId, SourceRange},
    },
    source::type_node::{TypeComment, TypeComments},
    strict::{Observation, StrictCheck},
};

pub struct Source {
    pub buffer: Buffer,
    pub node: Option<Rc<Node>>,
    pub mapping: HashMap<NodeId, Vec<Annotation>>,
    pub comments: Vec<Comment>,
    pub observations: Vec<Observation>,
}

pub enum SourceError {
    Syntax(ruby::SyntaxError),
    Annotation(AnnotationError),
}

impl Source {
    pub fn parse(
        path: &str,
        text: &str,
        steep_version: SteepVersion,
    ) -> Result<Source, SourceError> {
        let buffer = Buffer::new(path, text);
        let result = ruby::parse(&buffer, steep_version).map_err(SourceError::Syntax)?;
        let mut comments = result.comments;
        comments.sort_by_key(|comment| comment.location.begin_pos);
        let mut annotations = Vec::new();
        let mut type_comments = TypeComments::default();
        let mut observations = Vec::new();
        for comment in &comments {
            if !comment.is_inline() {
                continue;
            }
            let without_hash = &comment.text[1..];
            let content = without_hash.trim_start();
            let prefix = comment.text.len() - content.len();
            let content = content.trim_end();
            let suffix = comment.text.len() - content.len() - prefix;
            let location = SourceRange::new(
                comment.location.begin_pos + prefix,
                comment.location.end_pos - suffix,
            );
            let line = buffer.line(location.begin_pos);
            if let Some(annotation) =
                annotation::parse(content, location, line).map_err(SourceError::Annotation)?
            {
                annotations.push(annotation);
            } else if let Some(assertion) = TypeAssertion::parse(content, location, line) {
                type_comments.insert(line, TypeComment::Assertion(Rc::new(assertion)));
            } else if let Some(application) = TypeApplication::parse(content, location, line) {
                type_comments.insert(line, TypeComment::Application(Rc::new(application)));
            } else if annotation::looks_like_annotation(content) {
                observations.push(Observation::new(
                    StrictCheck::UnrecognizedAnnotation,
                    location,
                    "Steep ignores this comment because it matches none of the annotation syntaxes",
                ));
            } else if comment.text.starts_with("#:") && TypeAssertion::looks_like_assertion(content)
            {
                observations.push(Observation::new(
                    StrictCheck::InvalidTypeAssertion,
                    location,
                    "Steep ignores this type assertion because its type does not parse",
                ));
            }
        }
        let node = result
            .node
            .map(|node| Source::insert_type_node(&buffer, &node, &mut type_comments));
        let mut source = Source {
            buffer,
            node,
            mapping: HashMap::default(),
            comments,
            observations,
        };
        if let Some(node) = source.node.clone() {
            source.construct_mapping(&node, &mut annotations, None);
            for annotation in annotations {
                source.mapping.entry(node.id).or_default().push(annotation);
            }
        }
        Ok(source)
    }

    fn line(&self, position: usize) -> usize {
        self.buffer.line(position)
    }

    fn first_line(&self, node: &Node) -> usize {
        self.line(node.expression().begin_pos)
    }

    fn last_line(&self, node: &Node) -> usize {
        self.line(node.expression().end_pos)
    }

    fn keyword_source(&self, node: &Node) -> &str {
        node.location
            .keyword
            .map(|keyword| self.buffer.source_of(keyword))
            .unwrap_or("")
    }

    fn construct_mapping(
        &mut self,
        node: &Rc<Node>,
        annotations: &mut Vec<Annotation>,
        line_range: Option<Range<usize>>,
    ) {
        match node.type_ {
            "if" => {
                if node.location.kind == MapKind::Ternary {
                    for child in node.child_nodes().cloned().collect::<Vec<_>>() {
                        self.construct_mapping(&child, annotations, None);
                    }
                } else if node.location.keyword.map(|keyword| keyword.begin_pos)
                    == Some(node.expression().begin_pos)
                {
                    let condition = node.child_node(0).cloned();
                    let truthy = node.child_node(1).cloned();
                    let falsy = node.child_node(2).cloned();
                    let keyword = self.keyword_source(node).to_string();
                    let is_if = keyword == "if" || keyword == "elsif";
                    let condition_last_line = condition
                        .as_ref()
                        .map(|condition| self.last_line(condition))
                        .unwrap_or(0);
                    if let Some(condition) = &condition {
                        self.construct_mapping(condition, annotations, None);
                    }
                    let begin_last_line = node.location.begin.map(|begin| self.line(begin.end_pos));
                    let else_line = node.location.else_.map(|else_| self.line(else_.begin_pos));
                    let else_last_line = node.location.else_.map(|else_| self.line(else_.end_pos));
                    let last_line = self.last_line(node);
                    if let Some(truthy) = truthy {
                        let range = if is_if {
                            begin_last_line.unwrap_or(condition_last_line)
                                ..else_line.unwrap_or(last_line)
                        } else {
                            else_last_line.unwrap_or(last_line)..last_line
                        };
                        self.construct_mapping(&truthy, annotations, Some(range));
                    }
                    if let Some(falsy) = falsy {
                        let range = if is_if {
                            else_last_line.unwrap_or(last_line)..last_line
                        } else {
                            begin_last_line.unwrap_or(condition_last_line)
                                ..else_line.unwrap_or(last_line)
                        };
                        self.construct_mapping(&falsy, annotations, Some(range));
                    }
                } else {
                    for child in node.child_nodes().cloned().collect::<Vec<_>>() {
                        self.construct_mapping(&child, annotations, None);
                    }
                }
            }
            "while" | "until" => {
                let condition = node.child_node(0).cloned();
                let body = node.child_node(1).cloned();
                if node.location.keyword.map(|keyword| keyword.begin_pos)
                    == Some(node.expression().begin_pos)
                {
                    if let Some(condition) = &condition {
                        self.construct_mapping(condition, annotations, None);
                        if let (Some(body), Some(end)) = (body, node.location.end) {
                            let range = self.last_line(condition)..self.line(end.begin_pos);
                            self.construct_mapping(&body, annotations, Some(range));
                        }
                    }
                } else {
                    for child in node.child_nodes().cloned().collect::<Vec<_>>() {
                        self.construct_mapping(&child, annotations, None);
                    }
                }
            }
            "while_post" | "until_post" => {
                if let Some(condition) = node.child_node(0).cloned() {
                    self.construct_mapping(&condition, annotations, None);
                }
                if let (Some(body), Some(keyword)) =
                    (node.child_node(1).cloned(), node.location.keyword)
                {
                    let range = self.first_line(node)..self.line(keyword.begin_pos);
                    self.construct_mapping(&body, annotations, Some(range));
                }
            }
            "case" => {
                if let Some(condition) = node.child_node(0).cloned() {
                    self.construct_mapping(&condition, annotations, None);
                }
                let last = node.children.len().saturating_sub(1);
                if let Some(else_node) = node.child_node(last).cloned()
                    && else_node.type_ != "when"
                    && let (Some(else_), Some(end)) = (node.location.else_, node.location.end)
                {
                    let range = self.line(else_.end_pos)..self.line(end.begin_pos);
                    self.construct_mapping(&else_node, annotations, Some(range));
                }
                for child in node.children[1..]
                    .iter()
                    .filter_map(|child| child.node())
                    .cloned()
                    .collect::<Vec<_>>()
                {
                    if child.type_ == "when" {
                        self.construct_mapping(&child, annotations, None);
                    }
                }
            }
            "when" => {
                let count = node.children.len();
                let operands: Vec<Rc<Node>> = node.children[..count - 1]
                    .iter()
                    .filter_map(|child| child.node())
                    .cloned()
                    .collect();
                for operand in &operands {
                    self.construct_mapping(operand, annotations, None);
                }
                if let Some(body) = node.child_node(count - 1).cloned()
                    && let Some(last_operand) = operands.last()
                {
                    let condition_end = node
                        .location
                        .begin
                        .map(|begin| self.line(begin.end_pos))
                        .unwrap_or(self.last_line(last_operand) + 1);
                    let range = condition_end..self.last_line(&body);
                    self.construct_mapping(&body, annotations, Some(range));
                }
            }
            "rescue" => {
                let last = node.children.len().saturating_sub(1);
                if let Some(else_node) = node.child_node(last).cloned()
                    && let Some(else_) = node.location.else_
                {
                    let range = self.line(else_.end_pos)..self.last_line(node);
                    self.construct_mapping(&else_node, annotations, Some(range));
                }
                for child in node.child_nodes().cloned().collect::<Vec<_>>() {
                    self.construct_mapping(&child, annotations, None);
                }
            }
            _ => {
                for child in node.child_nodes().cloned().collect::<Vec<_>>() {
                    self.construct_mapping(&child, annotations, None);
                }
            }
        }
        let (associated, others): (Vec<Annotation>, Vec<Annotation>) = annotations
            .drain(..)
            .partition(|annotation| match node.type_ {
                "def" | "module" | "class" | "block" | "numblock" | "itblock" | "ensure"
                | "defs" | "resbody" => {
                    self.first_line(node) <= annotation.line
                        && annotation.line < self.last_line(node)
                }
                _ => line_range.as_ref().is_some_and(|range| {
                    range.start <= annotation.line && annotation.line < range.end
                }),
            });
        if !associated.is_empty() {
            self.mapping.entry(node.id).or_default().extend(associated);
        }
        *annotations = others;
    }

    pub fn annotations_of(&self, node: Option<&Node>) -> Vec<Annotation> {
        node.and_then(|node| self.mapping.get(&node.id).cloned())
            .unwrap_or_default()
    }
}
