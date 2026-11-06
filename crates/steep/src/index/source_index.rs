use rbs_definition::{ast::TypeName, collections::IndexMap};

use crate::{
    method_name::MethodName,
    ruby::node::{Location, Node, NodeId, SourceRange},
};

#[derive(Clone, Debug)]
pub struct IndexedNode {
    pub id: NodeId,
    pub type_: &'static str,
    pub location: Location,
    pub scope_expression: Option<SourceRange>,
}

impl IndexedNode {
    pub fn new(node: &Node) -> IndexedNode {
        IndexedNode {
            id: node.id,
            type_: node.type_,
            location: node.location.clone(),
            scope_expression: node
                .child_node(0)
                .and_then(|scope| scope.location.expression),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ConstantEntry {
    pub definitions: Vec<IndexedNode>,
    pub references: Vec<IndexedNode>,
}

#[derive(Clone, Debug, Default)]
pub struct MethodEntry {
    pub definitions: Vec<IndexedNode>,
    pub references: Vec<IndexedNode>,
}

fn add(nodes: &mut Vec<IndexedNode>, node: IndexedNode) {
    if !nodes.iter().any(|existing| existing.id == node.id) {
        nodes.push(node);
    }
}

#[derive(Clone, Debug, Default)]
pub struct SourceIndex {
    pub constant_index: IndexMap<TypeName, ConstantEntry>,
    pub method_index: IndexMap<MethodName, MethodEntry>,
}

impl SourceIndex {
    pub fn merge(&mut self, child: SourceIndex) {
        for (name, entry) in child.constant_index {
            let existing = self.constant_index.entry(name).or_default();
            for node in entry.definitions {
                add(&mut existing.definitions, node);
            }
            for node in entry.references {
                add(&mut existing.references, node);
            }
        }
        for (name, entry) in child.method_index {
            let existing = self.method_index.entry(name).or_default();
            for node in entry.definitions {
                add(&mut existing.definitions, node);
            }
            for node in entry.references {
                add(&mut existing.references, node);
            }
        }
    }

    pub fn add_constant_definition(&mut self, constant: TypeName, node: &Node) {
        add(
            &mut self.constant_index.entry(constant).or_default().definitions,
            IndexedNode::new(node),
        );
    }

    pub fn add_constant_reference(&mut self, constant: TypeName, node: &Node) {
        add(
            &mut self.constant_index.entry(constant).or_default().references,
            IndexedNode::new(node),
        );
    }

    pub fn add_method_definition(&mut self, method: MethodName, node: &Node) {
        add(
            &mut self.method_index.entry(method).or_default().definitions,
            IndexedNode::new(node),
        );
    }

    pub fn constant_entry(&self, constant: &TypeName) -> Option<&ConstantEntry> {
        self.constant_index.get(constant)
    }

    pub fn method_entry(&self, method: &MethodName) -> Option<&MethodEntry> {
        self.method_index.get(method)
    }

    pub fn reference(&self, constant_node: NodeId) -> Option<&TypeName> {
        self.constant_index.iter().find_map(|(name, entry)| {
            entry
                .references
                .iter()
                .chain(&entry.definitions)
                .any(|node| node.id == constant_node)
                .then_some(name)
        })
    }
}
