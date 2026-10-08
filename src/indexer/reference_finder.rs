use ruby_prism::{Node, Visit};

use super::entry::EntryKind;
use super::index::{Index, actual_nesting};
use crate::position::{LineIndex, Location};
use crate::prism::{constant_id_name, constant_name, node_slice};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Constant { fully_qualified_name: String },
    Method { method_name: String },
    InstanceVariable { name: String, owner_ancestors: Vec<String> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reference {
    pub name: String,
    pub location: Location,
    pub declaration: bool,
}

/// Mirrors `RubyIndexer::ReferenceFinder`: collects references to `target` in one document
pub fn find_references(
    target: &Target,
    index: &mut Index,
    source: &[u8],
    uri: &str,
    include_declarations: bool,
) -> Vec<Reference> {
    let parse_result = ruby_prism::parse(source);
    let line_index = LineIndex::new(source, super::index::indexer_column_encoding(index.configuration.encoding));
    let mut finder = ReferenceFinder { target, index, uri, line_index, stack: Vec::new(), references: Vec::new() };
    finder.visit(&parse_result.node());

    let references = finder.references;
    if include_declarations {
        references
    } else {
        references.into_iter().filter(|reference| !reference.declaration).collect()
    }
}

struct ReferenceFinder<'target, 'index, 'source> {
    target: &'target Target,
    index: &'index mut Index,
    uri: &'target str,
    line_index: LineIndex<'source>,
    stack: Vec<String>,
    references: Vec<Reference>,
}

impl ReferenceFinder<'_, '_, '_> {
    fn location(&self, location: &ruby_prism::Location<'_>) -> Location {
        self.line_index.location_of(location.start_offset(), location.end_offset())
    }

    fn singleton_name(&self) -> String {
        format!("<Class:{}>", self.stack.last().map(String::as_str).unwrap_or(""))
    }

    fn collect_constant_references(&mut self, name: &str, location: &ruby_prism::Location<'_>) {
        let Target::Constant { fully_qualified_name } = self.target else { return };
        let Some(entries) = self.index.resolve(name, &self.stack) else { return };
        let location = self.location(location);

        let mut has_matching_entry = false;
        let mut declaration = false;
        for id in entries {
            let entry = self.index.entry(id);
            let is_constant_like = matches!(
                entry.kind,
                EntryKind::Namespace(_)
                    | EntryKind::Constant
                    | EntryKind::ConstantAlias { .. }
                    | EntryKind::UnresolvedConstantAlias { .. }
            );
            if !is_constant_like || entry.name != *fully_qualified_name {
                continue;
            }
            has_matching_entry = true;
            if &*entry.uri == self.uri && entry.name_location() == location {
                declaration = true;
            }
        }

        if has_matching_entry {
            self.references.push(Reference { name: name.to_string(), location, declaration });
        }
    }

    fn collect_instance_variable_references(&mut self, name: &str, location: &ruby_prism::Location<'_>, declaration: bool) {
        let Target::InstanceVariable { name: target_name, owner_ancestors } = self.target else { return };
        if name != target_name {
            return;
        }
        let receiver_type = actual_nesting(&self.stack, None).join("::");
        if owner_ancestors.contains(&receiver_type) {
            let location = self.location(location);
            self.references.push(Reference { name: name.to_string(), location, declaration });
        }
    }

    fn collect_constant_path_write(&mut self, target: &ruby_prism::ConstantPathNode<'_>) {
        if target.parent().is_some_and(|parent| parent.as_constant_read_node().is_none()) {
            return;
        }
        let target_node = target.as_node();
        let Some(name) = constant_name(&target_node) else { return };
        self.collect_constant_references(&name, &target.location());
    }
}

macro_rules! constant_write_visitors {
    ($($method:ident, $node:ident;)*) => {
        $(
            fn $method(&mut self, node: &ruby_prism::$node<'pr>) {
                self.collect_constant_references(&constant_id_name(&node.name()), &node.name_loc());
                ruby_prism::$method(self, node);
            }
        )*
    };
}

macro_rules! constant_path_write_visitors {
    ($($method:ident, $node:ident;)*) => {
        $(
            fn $method(&mut self, node: &ruby_prism::$node<'pr>) {
                self.collect_constant_path_write(&node.target());
                ruby_prism::$method(self, node);
            }
        )*
    };
}

macro_rules! instance_variable_write_visitors {
    ($($method:ident, $node:ident;)*) => {
        $(
            fn $method(&mut self, node: &ruby_prism::$node<'pr>) {
                self.collect_instance_variable_references(&constant_id_name(&node.name()), &node.name_loc(), true);
                ruby_prism::$method(self, node);
            }
        )*
    };
}

impl<'pr> Visit<'pr> for ReferenceFinder<'_, '_, '_> {
    fn visit_class_node(&mut self, node: &ruby_prism::ClassNode<'pr>) {
        self.stack.push(node_slice(&node.constant_path()));
        ruby_prism::visit_class_node(self, node);
        self.stack.pop();
    }

    fn visit_module_node(&mut self, node: &ruby_prism::ModuleNode<'pr>) {
        self.stack.push(node_slice(&node.constant_path()));
        ruby_prism::visit_module_node(self, node);
        self.stack.pop();
    }

    fn visit_singleton_class_node(&mut self, node: &ruby_prism::SingletonClassNode<'pr>) {
        if node.expression().as_self_node().is_some() {
            let name = self.singleton_name();
            self.stack.push(name);
        }
        ruby_prism::visit_singleton_class_node(self, node);
        self.stack.pop();
    }

    fn visit_constant_path_node(&mut self, node: &ruby_prism::ConstantPathNode<'pr>) {
        if let Some(name) = constant_name(&node.as_node()) {
            self.collect_constant_references(&name, &node.location());
        }
        ruby_prism::visit_constant_path_node(self, node);
    }

    fn visit_constant_read_node(&mut self, node: &ruby_prism::ConstantReadNode<'pr>) {
        if let Some(name) = constant_name(&node.as_node()) {
            self.collect_constant_references(&name, &node.location());
        }
    }

    fn visit_multi_write_node(&mut self, node: &ruby_prism::MultiWriteNode<'pr>) {
        let mut targets: Vec<Node<'pr>> = node.lefts().iter().collect();
        targets.extend(node.rest());
        targets.extend(node.rights().iter());
        for target in targets {
            if let Some(constant) = target.as_constant_target_node() {
                self.collect_constant_references(&constant_id_name(&constant.name()), &target.location());
            } else if let Some(constant) = target.as_constant_path_target_node() {
                let name = constant.name().map(|name| constant_id_name(&name)).unwrap_or_default();
                self.collect_constant_references(&name, &target.location());
            }
        }
        ruby_prism::visit_multi_write_node(self, node);
    }

    constant_path_write_visitors! {
        visit_constant_path_write_node, ConstantPathWriteNode;
        visit_constant_path_or_write_node, ConstantPathOrWriteNode;
        visit_constant_path_operator_write_node, ConstantPathOperatorWriteNode;
        visit_constant_path_and_write_node, ConstantPathAndWriteNode;
    }

    constant_write_visitors! {
        visit_constant_write_node, ConstantWriteNode;
        visit_constant_or_write_node, ConstantOrWriteNode;
        visit_constant_and_write_node, ConstantAndWriteNode;
        visit_constant_operator_write_node, ConstantOperatorWriteNode;
    }

    fn visit_def_node(&mut self, node: &ruby_prism::DefNode<'pr>) {
        if let Target::Method { method_name } = self.target {
            let name = constant_id_name(&node.name());
            if name == *method_name {
                let location = self.location(&node.name_loc());
                self.references.push(Reference { name, location, declaration: true });
            }
        }
        let singleton = node.receiver().is_some_and(|receiver| receiver.as_self_node().is_some());
        if singleton {
            let name = self.singleton_name();
            self.stack.push(name);
        }
        ruby_prism::visit_def_node(self, node);
        if singleton {
            self.stack.pop();
        }
    }

    fn visit_instance_variable_read_node(&mut self, node: &ruby_prism::InstanceVariableReadNode<'pr>) {
        self.collect_instance_variable_references(&constant_id_name(&node.name()), &node.location(), false);
    }

    instance_variable_write_visitors! {
        visit_instance_variable_write_node, InstanceVariableWriteNode;
        visit_instance_variable_and_write_node, InstanceVariableAndWriteNode;
        visit_instance_variable_operator_write_node, InstanceVariableOperatorWriteNode;
        visit_instance_variable_or_write_node, InstanceVariableOrWriteNode;
    }

    fn visit_instance_variable_target_node(&mut self, node: &ruby_prism::InstanceVariableTargetNode<'pr>) {
        self.collect_instance_variable_references(&constant_id_name(&node.name()), &node.location(), true);
    }

    fn visit_call_node(&mut self, node: &ruby_prism::CallNode<'pr>) {
        if let Target::Method { method_name } = self.target {
            let name = constant_id_name(&node.name());
            if name == *method_name
                && let Some(message_location) = node.message_loc()
            {
                let location = self.location(&message_location);
                self.references.push(Reference { name, location, declaration: false });
            }
        }
        ruby_prism::visit_call_node(self, node);
    }
}
