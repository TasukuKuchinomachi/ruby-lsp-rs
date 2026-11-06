use std::rc::Rc;

use rbs_definition::{ast::TypeName, environment::TypeNameResolver};

use crate::{
    annotation::Annotation,
    ast::{
        located_type::{self, LocatedType},
        node::{type_application::TypeApplication, type_assertion::TypeAssertion},
    },
    ruby::node::{Child, Comment, Node, SourceRange},
    source::Source,
};

#[derive(Clone)]
pub struct NodeResult {
    pub node: Rc<Node>,
    pub parents: Vec<Rc<Node>>,
}

#[derive(Clone)]
pub enum RubyResult {
    Node(NodeResult),
    TypeAssertion {
        assertion: Rc<TypeAssertion>,
        node: NodeResult,
    },
    TypeApplication {
        application: Rc<TypeApplication>,
        node: NodeResult,
    },
    Annotation {
        annotation: Box<Annotation>,
        node: NodeResult,
        block: Rc<Node>,
    },
    Comment {
        comment: Comment,
        node: Option<NodeResult>,
    },
}

fn includes(range: SourceRange, position: usize) -> bool {
    range.begin_pos <= position && position <= range.end_pos
}

pub fn type_name_at(position: usize, type_: &LocatedType) -> Option<(TypeName, SourceRange)> {
    if !includes(type_.location, position) {
        return None;
    }
    for child in &type_.children {
        if includes(child.location, position) {
            return type_name_at(position, child);
        }
    }
    let name_location = type_.name_location?;
    let in_args = type_.location.end_pos > name_location.end_pos
        && name_location.end_pos <= position
        && position < type_.location.end_pos;
    if in_args {
        return None;
    }
    if name_location.begin_pos <= position && position < name_location.end_pos {
        return type_.type_name().map(|name| (name.clone(), name_location));
    }
    None
}

pub fn inline_type_name_at(
    path: &str,
    content: &str,
    position: usize,
    resolver: &TypeNameResolver,
) -> Option<(TypeName, SourceRange)> {
    let buffer = rbs_definition::location::Buffer::new(path, content);
    let regions = rbs_definition::inline_parser::parse(&buffer).type_regions;
    let region = regions
        .iter()
        .find(|region| region.annotation.start <= position && position <= region.annotation.end)?;
    if let Some((type_name, location)) = &region.type_name
        && location.start() <= position
        && position <= location.end()
    {
        return Some((
            type_name.clone(),
            SourceRange::new(location.start(), location.end()),
        ));
    }
    let location = region
        .type_locations
        .iter()
        .find(|location| location.start() <= position && position <= location.end())?;
    let located = located_type::parse_type(location.source(), location.start())
        .ok()
        .flatten()?;
    let (name, range) = type_name_at(position, &located)?;
    let name = resolver.resolve(&name, &region.context).unwrap_or(name);
    Some((name, range))
}

pub struct Ruby<'source> {
    pub source: &'source Source,
}

impl<'source> Ruby<'source> {
    pub fn new(source: &'source Source) -> Ruby<'source> {
        Ruby { source }
    }

    pub fn find(&self, position: usize) -> Option<RubyResult> {
        if let Some(nodes) = self.source.find_heredoc_nodes(position) {
            let heredoc_node = nodes[0].clone();
            for child in heredoc_node.child_nodes() {
                if let Some(result) = find_ruby_node_in(position, child, nodes.clone()) {
                    return Some(self.ruby_result_from_node(result, position));
                }
            }
            return Some(RubyResult::Node(NodeResult {
                node: heredoc_node,
                parents: nodes[1..].to_vec(),
            }));
        }
        if let Some(node) = &self.source.node
            && let Some(result) = find_ruby_node_in(position, node, Vec::new())
        {
            return Some(self.ruby_result_from_node(result, position));
        }
        self.comment_at(position)
            .map(|comment| RubyResult::Comment {
                comment: comment.clone(),
                node: None,
            })
    }

    fn comment_at(&self, position: usize) -> Option<&Comment> {
        self.source
            .comments
            .iter()
            .find(|comment| includes(comment.location, position))
    }

    fn ruby_result_from_node(&self, result: NodeResult, position: usize) -> RubyResult {
        match result.node.type_ {
            "assertion" => match result.node.child(1) {
                Child::TypeAssertion(assertion) => {
                    return RubyResult::TypeAssertion {
                        assertion: assertion.clone(),
                        node: result,
                    };
                }
                _ => unreachable!("an assertion node has a type assertion"),
            },
            "tapp" => match result.node.child(1) {
                Child::TypeApplication(application) => {
                    return RubyResult::TypeApplication {
                        application: application.clone(),
                        node: result,
                    };
                }
                _ => unreachable!("a tapp node has a type application"),
            },
            _ => {}
        }
        let comment = self.comment_at(position).cloned();
        let nodes: Vec<&Rc<Node>> = std::iter::once(&result.node)
            .chain(result.parents.iter())
            .collect();
        if let Some(block_node) = self.source.find_block_node(&nodes) {
            for annotation in self.source.each_block_annotation(block_node) {
                if includes(annotation.location, position) {
                    return RubyResult::Annotation {
                        annotation: Box::new(annotation.clone()),
                        block: block_node.clone(),
                        node: result,
                    };
                }
            }
        }
        match comment {
            Some(comment) => RubyResult::Comment {
                comment,
                node: Some(result),
            },
            None => RubyResult::Node(result),
        }
    }
}

fn find_ruby_node_in(
    position: usize,
    node: &Rc<Node>,
    mut parents: Vec<Rc<Node>>,
) -> Option<NodeResult> {
    let range = node.location.expression?;
    if !includes(range, position) {
        return None;
    }
    parents.insert(0, node.clone());
    for child in node.child_nodes() {
        if let Some(result) = find_ruby_node_in(position, child, parents.clone()) {
            return Some(result);
        }
    }
    parents.remove(0);
    Some(NodeResult {
        node: node.clone(),
        parents,
    })
}
