use std::{
    fmt,
    rc::Rc,
    sync::atomic::{AtomicUsize, Ordering},
};

use crate::ast::node::{type_application::TypeApplication, type_assertion::TypeAssertion};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SourceRange {
    pub begin_pos: usize,
    pub end_pos: usize,
}

impl SourceRange {
    pub fn new(begin_pos: usize, end_pos: usize) -> SourceRange {
        SourceRange { begin_pos, end_pos }
    }

    pub fn join(self, other: SourceRange) -> SourceRange {
        SourceRange {
            begin_pos: self.begin_pos.min(other.begin_pos),
            end_pos: self.end_pos.max(other.end_pos),
        }
    }

    pub fn with_begin_pos(self, begin_pos: usize) -> SourceRange {
        SourceRange { begin_pos, ..self }
    }

    pub fn with_end_pos(self, end_pos: usize) -> SourceRange {
        SourceRange { end_pos, ..self }
    }

    pub fn size(self) -> usize {
        self.end_pos - self.begin_pos
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MapKind {
    Map,
    Collection,
    Condition,
    Constant,
    Definition,
    For,
    Heredoc,
    Index,
    Keyword,
    MethodDefinition,
    ObjcKwarg,
    Operator,
    RescueBody,
    Send,
    Ternary,
    Variable,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Location {
    pub kind: MapKind,
    pub expression: Option<SourceRange>,
    pub keyword: Option<SourceRange>,
    pub name: Option<SourceRange>,
    pub selector: Option<SourceRange>,
    pub operator: Option<SourceRange>,
    pub begin: Option<SourceRange>,
    pub end: Option<SourceRange>,
    pub dot: Option<SourceRange>,
    pub else_: Option<SourceRange>,
    pub assignment: Option<SourceRange>,
    pub question: Option<SourceRange>,
    pub colon: Option<SourceRange>,
    pub double_colon: Option<SourceRange>,
    pub in_: Option<SourceRange>,
    pub assoc: Option<SourceRange>,
    pub argument: Option<SourceRange>,
    pub heredoc_body: Option<SourceRange>,
    pub heredoc_end: Option<SourceRange>,
}

impl Location {
    pub fn new(kind: MapKind, expression: Option<SourceRange>) -> Location {
        Location {
            kind,
            expression,
            keyword: None,
            name: None,
            selector: None,
            operator: None,
            begin: None,
            end: None,
            dot: None,
            else_: None,
            assignment: None,
            question: None,
            colon: None,
            double_colon: None,
            in_: None,
            assoc: None,
            argument: None,
            heredoc_body: None,
            heredoc_end: None,
        }
    }

    pub fn expression(&self) -> SourceRange {
        self.expression
            .expect("the node has a source range in its expression")
    }

    pub fn responds_to(&self, field: &str) -> bool {
        use MapKind::*;
        let fields: &[&str] = match self.kind {
            Map => &[],
            Collection => &["begin", "end"],
            Condition => &["keyword", "begin", "else", "end"],
            Constant => &["double_colon", "name", "operator"],
            Definition => &["keyword", "operator", "name", "end"],
            For => &["keyword", "in", "begin", "end"],
            Heredoc => &["heredoc_body", "heredoc_end"],
            Index => &["begin", "end", "operator"],
            Keyword => &["keyword", "begin", "end"],
            MethodDefinition => &["keyword", "operator", "name", "end", "assignment"],
            ObjcKwarg => &["keyword", "operator", "argument"],
            Operator => &["operator"],
            RescueBody => &["keyword", "assoc", "begin"],
            Send => &["dot", "selector", "operator", "begin", "end"],
            Ternary => &["question", "colon"],
            Variable => &["name", "operator"],
        };
        field == "expression" || fields.contains(&field)
    }

    pub fn with_expression(&self, expression: SourceRange) -> Location {
        Location {
            expression: Some(expression),
            ..self.clone()
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Child {
    Node(Rc<Node>),
    Nil,
    Symbol(String),
    String(String),
    Integer(String),
    Float(f64),
    Rational(String),
    Complex(String),
    TypeAssertion(Rc<TypeAssertion>),
    TypeApplication(Rc<TypeApplication>),
}

impl Child {
    pub fn node(&self) -> Option<&Rc<Node>> {
        match self {
            Child::Node(node) => Some(node),
            _ => None,
        }
    }

    pub fn symbol(&self) -> Option<&str> {
        match self {
            Child::Symbol(symbol) => Some(symbol),
            _ => None,
        }
    }

    pub fn string(&self) -> Option<&str> {
        match self {
            Child::String(string) => Some(string),
            _ => None,
        }
    }

    pub fn is_nil(&self) -> bool {
        matches!(self, Child::Nil)
    }
}

static NODE_COUNT: AtomicUsize = AtomicUsize::new(0);

pub type NodeId = usize;

#[derive(Debug)]
pub struct Node {
    pub id: NodeId,
    pub type_: &'static str,
    pub children: Vec<Child>,
    pub location: Location,
}

impl PartialEq for Node {
    fn eq(&self, other: &Node) -> bool {
        self.type_ == other.type_ && self.children == other.children
    }
}

impl Node {
    pub fn new(type_: &'static str, children: Vec<Child>, location: Location) -> Rc<Node> {
        Rc::new(Node {
            id: NODE_COUNT.fetch_add(1, Ordering::Relaxed),
            type_,
            children,
            location,
        })
    }

    pub fn updated(
        &self,
        type_: Option<&'static str>,
        children: Option<Vec<Child>>,
        location: Option<Location>,
    ) -> Rc<Node> {
        Node::new(
            type_.unwrap_or(self.type_),
            children.unwrap_or_else(|| self.children.clone()),
            location.unwrap_or_else(|| self.location.clone()),
        )
    }

    pub fn expression(&self) -> SourceRange {
        self.location.expression()
    }

    pub fn child(&self, index: usize) -> &Child {
        self.children.get(index).unwrap_or(&Child::Nil)
    }

    pub fn child_node(&self, index: usize) -> Option<&Rc<Node>> {
        self.child(index).node()
    }

    pub fn child_symbol(&self, index: usize) -> Option<&str> {
        self.child(index).symbol()
    }

    pub fn child_nodes(&self) -> impl Iterator<Item = &Rc<Node>> {
        self.children.iter().filter_map(Child::node)
    }
}

#[derive(Clone)]
pub struct Comment {
    pub text: String,
    pub location: SourceRange,
}

impl Comment {
    pub fn is_inline(&self) -> bool {
        self.text.starts_with('#')
    }
}

#[derive(Clone)]
pub struct Buffer {
    pub name: String,
    pub source: String,
    line_starts: Vec<usize>,
}

impl Buffer {
    pub fn new(name: impl Into<String>, source: impl Into<String>) -> Buffer {
        let source = source.into().replace("\r\n", "\n");
        let mut line_starts = vec![0];
        for (index, byte) in source.bytes().enumerate() {
            if byte == b'\n' {
                line_starts.push(index + 1);
            }
        }
        Buffer {
            name: name.into(),
            source,
            line_starts,
        }
    }

    pub fn line(&self, position: usize) -> usize {
        match self.line_starts.binary_search(&position) {
            Ok(index) => index + 1,
            Err(index) => index,
        }
    }

    pub fn column(&self, position: usize) -> usize {
        let line_start = self.line_starts[self.line(position) - 1];
        self.source[line_start..position].chars().count()
    }

    pub fn line_start(&self, line: usize) -> usize {
        self.line_starts[line - 1]
    }

    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    pub fn first_line(&self, range: SourceRange) -> usize {
        self.line(range.begin_pos)
    }

    pub fn last_line(&self, range: SourceRange) -> usize {
        self.line(range.end_pos)
    }

    pub fn source_of(&self, range: SourceRange) -> &str {
        &self.source[range.begin_pos..range.end_pos]
    }
}

impl fmt::Display for Node {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "({}", self.type_)?;
        for child in &self.children {
            match child {
                Child::Node(node) => write!(formatter, " {node}")?,
                Child::Nil => write!(formatter, " nil")?,
                Child::Symbol(symbol) => write!(formatter, " :{symbol}")?,
                Child::String(string) => write!(formatter, " {string:?}")?,
                Child::Integer(value) | Child::Rational(value) | Child::Complex(value) => {
                    write!(formatter, " {value}")?
                }
                Child::Float(value) => write!(formatter, " {value:?}")?,
                Child::TypeAssertion(assertion) => write!(formatter, " {:?}", assertion.source)?,
                Child::TypeApplication(application) => {
                    write!(formatter, " {:?}", application.source)?
                }
            }
        }
        write!(formatter, ")")
    }
}
