use std::sync::Arc;

use rbs_definition::{
    ast::{self, TypeName},
    environment::map_type_names,
    location::Buffer,
    parse,
};
use ruby_rbs::node::{Node, NodeList, RBSLocationRange};

use crate::ruby::node::SourceRange;

#[derive(Clone, Debug, PartialEq)]
pub struct LocatedType {
    pub type_: ast::Type,
    pub location: SourceRange,
    pub name_location: Option<SourceRange>,
    pub children: Vec<LocatedType>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ParsingError {
    pub message: String,
}

struct Converter {
    buffer: Arc<Buffer>,
    start_pos: usize,
}

impl Converter {
    fn range(&self, range: RBSLocationRange) -> SourceRange {
        SourceRange::new(
            self.start_pos + range.start() as usize,
            self.start_pos + range.end() as usize,
        )
    }

    fn list(&self, list: NodeList, children: &mut Vec<LocatedType>) {
        for node in list.iter() {
            children.push(self.located(node));
        }
    }

    fn parameter(&self, node: Node, children: &mut Vec<LocatedType>) {
        if let Node::FunctionParam(parameter) = node {
            children.push(self.located(parameter.type_()));
        }
    }

    fn function(&self, node: Node, children: &mut Vec<LocatedType>) {
        match node {
            Node::FunctionType(function) => {
                for parameter in function.required_positionals().iter() {
                    self.parameter(parameter, children);
                }
                for parameter in function.optional_positionals().iter() {
                    self.parameter(parameter, children);
                }
                if let Some(parameter) = function.rest_positionals() {
                    self.parameter(parameter, children);
                }
                for parameter in function.trailing_positionals().iter() {
                    self.parameter(parameter, children);
                }
                for (_, parameter) in function.required_keywords().iter() {
                    self.parameter(parameter, children);
                }
                for (_, parameter) in function.optional_keywords().iter() {
                    self.parameter(parameter, children);
                }
                if let Some(parameter) = function.rest_keywords() {
                    self.parameter(parameter, children);
                }
                children.push(self.located(function.return_type()));
            }
            Node::UntypedFunctionType(function) => {
                children.push(self.located(function.return_type()));
            }
            _ => {}
        }
    }

    fn located(&self, node: Node) -> LocatedType {
        let location = self.range(node.location());
        let mut children = Vec::new();
        let mut name_location = None;
        match &node {
            Node::ClassInstanceType(instance) => {
                name_location = Some(self.range(instance.name_location()));
                self.list(instance.args(), &mut children);
            }
            Node::ClassSingletonType(singleton) => {
                name_location = Some(self.range(singleton.name_location()));
                self.list(singleton.args(), &mut children);
            }
            Node::InterfaceType(interface) => {
                name_location = Some(self.range(interface.name_location()));
                self.list(interface.args(), &mut children);
            }
            Node::AliasType(alias) => {
                name_location = Some(self.range(alias.name_location()));
                self.list(alias.args(), &mut children);
            }
            Node::TupleType(tuple) => self.list(tuple.types(), &mut children),
            Node::RecordType(record) => {
                for (_, field) in record.all_fields().iter() {
                    if let Node::RecordFieldType(field) = field {
                        children.push(self.located(field.type_()));
                    }
                }
            }
            Node::OptionalType(optional) => children.push(self.located(optional.type_())),
            Node::UnionType(union) => self.list(union.types(), &mut children),
            Node::IntersectionType(intersection) => self.list(intersection.types(), &mut children),
            Node::ProcType(proc_type) => {
                self.function(proc_type.type_(), &mut children);
                if let Some(self_type) = proc_type.self_type() {
                    children.push(self.located(self_type));
                }
                if let Some(block) = proc_type.block() {
                    self.function(block.type_(), &mut children);
                    if let Some(self_type) = block.self_type() {
                        children.push(self.located(self_type));
                    }
                }
            }
            _ => {}
        }
        LocatedType {
            type_: parse::located_type(node, &self.buffer, self.start_pos as isize),
            location,
            name_location,
            children,
        }
    }
}

fn parse_whole_type(source: &str, start_pos: usize) -> Result<LocatedType, ParsingError> {
    let converter = Converter {
        buffer: Buffer::new("", source),
        start_pos,
    };
    match parse::parse_whole_type_node(source, |node| converter.located(node)) {
        Ok(Some(located)) => Ok(located),
        Ok(None) => Err(ParsingError {
            message: "Failed to parse a type".to_string(),
        }),
        Err(error) => Err(ParsingError {
            message: error.message(),
        }),
    }
}

pub fn parse_type(source: &str, start_pos: usize) -> Result<Option<LocatedType>, ParsingError> {
    if source.trim().is_empty() {
        return Ok(None);
    }
    parse_whole_type(source, start_pos).map(Some)
}

pub fn parse_types(source: &str, start_pos: usize) -> Result<Vec<LocatedType>, ParsingError> {
    if source.trim().is_empty() {
        return Ok(Vec::new());
    }
    let tuple = parse_whole_type(&format!("[{source}]"), start_pos - 1)?;
    match tuple.type_ {
        ast::Type::Tuple(_) => Ok(tuple.children),
        _ => Err(ParsingError {
            message: "Failed to parse types".to_string(),
        }),
    }
}

impl LocatedType {
    pub fn map_type_name(&self, map: &mut dyn FnMut(&TypeName) -> TypeName) -> LocatedType {
        LocatedType {
            type_: map_type_names(&self.type_, map),
            location: self.location,
            name_location: self.name_location,
            children: self
                .children
                .iter()
                .map(|child| child.map_type_name(map))
                .collect(),
        }
    }

    pub fn type_name(&self) -> Option<&TypeName> {
        match &self.type_ {
            ast::Type::ClassInstance { name, .. }
            | ast::Type::ClassSingleton { name, .. }
            | ast::Type::Interface { name, .. }
            | ast::Type::Alias { name, .. } => Some(name),
            _ => None,
        }
    }

    pub fn type_args(&self) -> &[ast::Type] {
        match &self.type_ {
            ast::Type::ClassInstance { args, .. }
            | ast::Type::ClassSingleton { args, .. }
            | ast::Type::Interface { args, .. }
            | ast::Type::Alias { args, .. } => args,
            _ => &[],
        }
    }
}
