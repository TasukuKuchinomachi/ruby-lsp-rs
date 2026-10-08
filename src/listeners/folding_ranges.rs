use ruby_prism::{CallNode, CommentType, Location, Node};
use serde::Serialize;

use crate::document::ParsedDocument;
use crate::prism::{NodeExt, child_nodes, location_slice, node_slice};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FoldingRange {
    pub start_line: u32,
    pub end_line: u32,
    pub kind: &'static str,
}

struct CommentLines {
    start_line: usize,
    end_line: usize,
    trailing: bool,
}

pub struct FoldingRanges<'a, 'b> {
    parsed: &'a ParsedDocument<'b>,
    response: Vec<FoldingRange>,
    requires: Vec<(usize, usize)>,
}

impl<'a, 'b> FoldingRanges<'a, 'b> {
    pub fn new(parsed: &'a ParsedDocument<'b>) -> FoldingRanges<'a, 'b> {
        FoldingRanges { parsed, response: Vec::new(), requires: Vec::new() }
    }

    pub fn perform(mut self) -> Vec<FoldingRange> {
        let root = self.parsed.ast();
        let mut stack = vec![root];
        while let Some(node) = stack.pop() {
            self.on_node_enter(&node);
            let mut children = child_nodes(&node);
            children.reverse();
            stack.extend(children);
        }
        self.finalize_response();
        self.response
    }

    fn on_node_enter(&mut self, node: &Node<'_>) {
        match node {
            Node::IfNode { .. } => {
                let statements = node.as_if_node().expect("variant was matched").statements();
                self.add_statements_range(node, statements.map(|statements| statements.body().last()));
            }
            Node::InNode { .. } => {
                let statements = node.as_in_node().expect("variant was matched").statements();
                self.add_statements_range(node, statements.map(|statements| statements.body().last()));
            }
            Node::RescueNode { .. } => {
                let statements = node.as_rescue_node().expect("variant was matched").statements();
                self.add_statements_range(node, statements.map(|statements| statements.body().last()));
            }
            Node::WhenNode { .. } => {
                let statements = node.as_when_node().expect("variant was matched").statements();
                self.add_statements_range(node, statements.map(|statements| statements.body().last()));
            }
            Node::InterpolatedStringNode { .. } => self.on_interpolated_string_node_enter(node),
            Node::ArrayNode { .. }
            | Node::BlockNode { .. }
            | Node::CaseNode { .. }
            | Node::CaseMatchNode { .. }
            | Node::ClassNode { .. }
            | Node::ModuleNode { .. }
            | Node::ForNode { .. }
            | Node::HashNode { .. }
            | Node::SingletonClassNode { .. }
            | Node::UnlessNode { .. }
            | Node::UntilNode { .. }
            | Node::WhileNode { .. }
            | Node::ElseNode { .. }
            | Node::EnsureNode { .. }
            | Node::BeginNode { .. }
            | Node::LambdaNode { .. } => self.add_simple_range(node),
            Node::DefNode { .. } => self.on_def_node_enter(node),
            Node::CallNode { .. } => self.on_call_node_enter(node),
            _ => {}
        }
    }

    fn on_interpolated_string_node_enter(&mut self, node: &Node<'_>) {
        let string = node.as_interpolated_string_node().expect("variant was matched");
        let opening_location = string.opening_loc().unwrap_or_else(|| node.location());
        let closing_location = string
            .closing_loc()
            .or_else(|| string.parts().last().map(|part| part.location()))
            .unwrap_or_else(|| node.location());
        self.add_lines_range(self.start_line(&opening_location), self.start_line(&closing_location) - 1);
    }

    fn on_def_node_enter(&mut self, node: &Node<'_>) {
        let definition = node.as_def_node().expect("variant was matched");
        let location = node.location();
        match definition.parameters() {
            Some(parameters) if self.end_line(&parameters.location()) > self.start_line(&location) => {
                let parameters_end_line = self.end_line(&parameters.location());
                self.add_lines_range(self.start_line(&location), parameters_end_line);
                self.add_lines_range(parameters_end_line + 1, self.end_line(&location) - 1);
            }
            _ => self.add_lines_range(self.start_line(&location), self.end_line(&location) - 1),
        }
    }

    fn on_call_node_enter(&mut self, node: &Node<'_>) {
        let call = node.as_call_node().expect("variant was matched");
        let location = node.location();
        if is_require(&call) {
            self.requires.push((self.start_line(&location), self.end_line(&location)));
            return;
        }
        self.add_lines_range(self.start_line(&location), self.end_line(&location) - 1);
    }

    fn finalize_response(&mut self) {
        self.push_comment_ranges();
        self.emit_requires_range();
    }

    fn push_comment_ranges(&mut self) {
        let comments: Vec<CommentLines> = self
            .parsed
            .parse_result
            .comments()
            .map(|comment| {
                let location = comment.location();
                CommentLines {
                    start_line: self.start_line(&location),
                    end_line: self.end_line(&location),
                    trailing: comment.type_() == CommentType::InlineComment && self.has_code_before(location.start_offset()),
                }
            })
            .collect();

        let mut chunks: Vec<Vec<&CommentLines>> = Vec::new();
        for comment in &comments {
            match chunks.last_mut() {
                Some(chunk)
                    if chunk.last().is_some_and(|previous| {
                        previous.end_line + 1 == comment.start_line && !previous.trailing && !comment.trailing
                    }) =>
                {
                    chunk.push(comment)
                }
                _ => chunks.push(vec![comment]),
            }
        }

        for chunk in chunks {
            if chunk.len() == 1 {
                continue;
            }
            self.response.push(FoldingRange {
                start_line: (chunk[0].start_line - 1) as u32,
                end_line: (chunk[chunk.len() - 1].end_line - 1) as u32,
                kind: "comment",
            });
        }
    }

    fn emit_requires_range(&mut self) {
        if self.requires.len() > 1 {
            self.response.push(FoldingRange {
                start_line: (self.requires[0].0 - 1) as u32,
                end_line: (self.requires[self.requires.len() - 1].1 - 1) as u32,
                kind: "imports",
            });
        }
        self.requires.clear();
    }

    fn add_statements_range(&mut self, node: &Node<'_>, last_statement: Option<Option<Node<'_>>>) {
        let Some(Some(statement)) = last_statement else { return };
        self.add_lines_range(self.start_line(&node.location()), self.end_line(&statement.location()));
    }

    fn add_simple_range(&mut self, node: &Node<'_>) {
        let location = node.location();
        self.add_lines_range(self.start_line(&location), self.end_line(&location) - 1);
    }

    fn add_lines_range(&mut self, start_line: usize, end_line: usize) {
        self.emit_requires_range();
        if start_line >= end_line {
            return;
        }
        self.response.push(FoldingRange { start_line: (start_line - 1) as u32, end_line: (end_line - 1) as u32, kind: "region" });
    }

    fn start_line(&self, location: &Location<'_>) -> usize {
        self.parsed.line_index.line_of_offset(location.start_offset()) + 1
    }

    fn end_line(&self, location: &Location<'_>) -> usize {
        self.parsed.line_index.line_of_offset(location.end_offset()) + 1
    }

    fn has_code_before(&self, offset: usize) -> bool {
        let line = self.parsed.line_index.line_of_offset(offset);
        let line_start = self.parsed.line_index.line_start(line).unwrap_or(0);
        let source = self.parsed.document.ruby_source().as_bytes();
        source[line_start..offset].iter().any(|byte| !matches!(byte, b' ' | b'\t' | b'\n' | b'\x0b' | b'\x0c' | b'\r' | b'\0'))
    }
}

fn is_require(call: &CallNode<'_>) -> bool {
    let Some(message) = call.message_loc().map(|location| location_slice(&location)) else { return false };
    if message != "require" && message != "require_relative" {
        return false;
    }
    if call.receiver().is_some_and(|receiver| node_slice(&receiver) != "Kernel") {
        return false;
    }
    let Some(arguments) = call.arguments() else { return false };
    let arguments: Vec<Node<'_>> = arguments.arguments().iter().collect();
    arguments.len() == 1 && arguments[0].type_name() == "StringNode"
}
