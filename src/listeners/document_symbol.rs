use ruby_prism::{CallNode, Location, Node};

use crate::document::ParsedDocument;
use crate::prism::{child_nodes, constant_id_name, location_slice, node_slice};
use crate::requests::support::document_symbol_response_builder::{DocumentSymbol, DocumentSymbolResponseBuilder, SymbolId};

pub mod symbol_kind {
    pub const MODULE: u32 = 2;
    pub const NAMESPACE: u32 = 3;
    pub const CLASS: u32 = 5;
    pub const METHOD: u32 = 6;
    pub const FIELD: u32 = 8;
    pub const CONSTRUCTOR: u32 = 9;
    pub const FUNCTION: u32 = 12;
    pub const VARIABLE: u32 = 13;
    pub const CONSTANT: u32 = 14;
}

const ATTR_ACCESSORS: &[&str] = &["attr_reader", "attr_writer", "attr_accessor"];

pub struct DocumentSymbolListener<'a> {
    response_builder: DocumentSymbolResponseBuilder,
    parsed: &'a ParsedDocument<'a>,
    rake: bool,
}

impl<'a> DocumentSymbolListener<'a> {
    pub fn new(parsed: &'a ParsedDocument<'a>, uri: &str) -> DocumentSymbolListener<'a> {
        let rake = crate::uri::to_path(uri).is_some_and(|path| path.ends_with("Rakefile") || path.ends_with(".rake"));
        DocumentSymbolListener { response_builder: DocumentSymbolResponseBuilder::new(), parsed, rake }
    }

    pub fn into_response(self) -> Vec<DocumentSymbol> {
        self.response_builder.response()
    }

    pub fn visit(&mut self, node: &Node<'_>) {
        self.on_enter(node);
        for child in child_nodes(node) {
            self.visit(&child);
        }
        self.on_leave(node);
    }

    fn on_enter(&mut self, node: &Node<'_>) {
        match node {
            Node::ClassNode { .. } => {
                let class = node.as_class_node().expect("variant was matched");
                let constant_path = class.constant_path();
                let symbol = self.create_document_symbol(
                    node_slice(&constant_path),
                    symbol_kind::CLASS,
                    &node.location(),
                    &constant_path.location(),
                );
                self.response_builder.push(symbol);
            }
            Node::SingletonClassNode { .. } => {
                let singleton_class = node.as_singleton_class_node().expect("variant was matched");
                let expression = singleton_class.expression();
                let symbol = self.create_document_symbol(
                    format!("<< {}", node_slice(&expression)),
                    symbol_kind::NAMESPACE,
                    &node.location(),
                    &expression.location(),
                );
                self.response_builder.push(symbol);
            }
            Node::CallNode { .. } => self.on_call_node_enter(&node.as_call_node().expect("variant was matched")),
            Node::ConstantPathWriteNode { .. } => {
                let target = node.as_constant_path_write_node().expect("variant was matched").target();
                self.create_constant_path_symbol(node, &target.location());
            }
            Node::ConstantPathAndWriteNode { .. } => {
                let target = node.as_constant_path_and_write_node().expect("variant was matched").target();
                self.create_constant_path_symbol(node, &target.location());
            }
            Node::ConstantPathOrWriteNode { .. } => {
                let target = node.as_constant_path_or_write_node().expect("variant was matched").target();
                self.create_constant_path_symbol(node, &target.location());
            }
            Node::ConstantPathOperatorWriteNode { .. } => {
                let target = node.as_constant_path_operator_write_node().expect("variant was matched").target();
                self.create_constant_path_symbol(node, &target.location());
            }
            Node::ConstantWriteNode { .. } => {
                let write = node.as_constant_write_node().expect("variant was matched");
                self.create_named_symbol(constant_id_name(&write.name()), symbol_kind::CONSTANT, node, &write.name_loc());
            }
            Node::ConstantOrWriteNode { .. } => {
                let write = node.as_constant_or_write_node().expect("variant was matched");
                self.create_named_symbol(constant_id_name(&write.name()), symbol_kind::CONSTANT, node, &write.name_loc());
            }
            Node::ConstantAndWriteNode { .. } => {
                let write = node.as_constant_and_write_node().expect("variant was matched");
                self.create_named_symbol(constant_id_name(&write.name()), symbol_kind::CONSTANT, node, &write.name_loc());
            }
            Node::ConstantOperatorWriteNode { .. } => {
                let write = node.as_constant_operator_write_node().expect("variant was matched");
                self.create_named_symbol(constant_id_name(&write.name()), symbol_kind::CONSTANT, node, &write.name_loc());
            }
            Node::ConstantTargetNode { .. } => {
                let target = node.as_constant_target_node().expect("variant was matched");
                self.create_named_symbol(constant_id_name(&target.name()), symbol_kind::CONSTANT, node, &node.location());
            }
            Node::ConstantPathTargetNode { .. } => {
                self.create_named_symbol(node_slice(node), symbol_kind::CONSTANT, node, &node.location());
            }
            Node::DefNode { .. } => self.on_def_node_enter(node),
            Node::ModuleNode { .. } => {
                let module = node.as_module_node().expect("variant was matched");
                let constant_path = module.constant_path();
                let symbol = self.create_document_symbol(
                    node_slice(&constant_path),
                    symbol_kind::MODULE,
                    &node.location(),
                    &constant_path.location(),
                );
                self.response_builder.push(symbol);
            }
            Node::InstanceVariableWriteNode { .. } => {
                let write = node.as_instance_variable_write_node().expect("variant was matched");
                self.create_name_location_symbol(constant_id_name(&write.name()), symbol_kind::FIELD, &write.name_loc());
            }
            Node::InstanceVariableTargetNode { .. } => {
                let target = node.as_instance_variable_target_node().expect("variant was matched");
                self.create_named_symbol(constant_id_name(&target.name()), symbol_kind::FIELD, node, &node.location());
            }
            Node::InstanceVariableOperatorWriteNode { .. } => {
                let write = node.as_instance_variable_operator_write_node().expect("variant was matched");
                self.create_name_location_symbol(constant_id_name(&write.name()), symbol_kind::FIELD, &write.name_loc());
            }
            Node::InstanceVariableOrWriteNode { .. } => {
                let write = node.as_instance_variable_or_write_node().expect("variant was matched");
                self.create_name_location_symbol(constant_id_name(&write.name()), symbol_kind::FIELD, &write.name_loc());
            }
            Node::InstanceVariableAndWriteNode { .. } => {
                let write = node.as_instance_variable_and_write_node().expect("variant was matched");
                self.create_name_location_symbol(constant_id_name(&write.name()), symbol_kind::FIELD, &write.name_loc());
            }
            Node::ClassVariableWriteNode { .. } => {
                let write = node.as_class_variable_write_node().expect("variant was matched");
                self.create_name_location_symbol(constant_id_name(&write.name()), symbol_kind::VARIABLE, &write.name_loc());
            }
            Node::AliasMethodNode { .. } => {
                let new_name = node.as_alias_method_node().expect("variant was matched").new_name();
                let Some(symbol) = new_name.as_symbol_node() else { return };
                let Some(value_location) = symbol.value_loc() else { return };
                self.create_document_symbol(
                    location_slice(&value_location),
                    symbol_kind::METHOD,
                    &new_name.location(),
                    &value_location,
                );
            }
            _ => {}
        }
    }

    fn on_leave(&mut self, node: &Node<'_>) {
        match node {
            Node::ClassNode { .. } | Node::SingletonClassNode { .. } | Node::DefNode { .. } | Node::ModuleNode { .. } => {
                self.response_builder.pop();
            }
            Node::CallNode { .. } => {
                if !self.rake {
                    return;
                }
                let call = node.as_call_node().expect("variant was matched");
                if constant_id_name(&call.name()) == "namespace" && call.receiver().is_none() {
                    self.response_builder.pop();
                }
            }
            _ => {}
        }
    }

    fn on_call_node_enter(&mut self, node: &CallNode<'_>) {
        let node_name = constant_id_name(&node.name());
        if ATTR_ACCESSORS.contains(&node_name.as_str()) {
            self.handle_attr_accessor(node);
        } else if node_name == "alias_method" {
            self.handle_alias_method(node);
        } else if node_name == "namespace" {
            self.handle_rake_namespace(node);
        } else if node_name == "task" {
            self.handle_rake_task(node);
        }
    }

    fn on_def_node_enter(&mut self, node: &Node<'_>) {
        let definition = node.as_def_node().expect("variant was matched");
        let method_name = constant_id_name(&definition.name());
        let previous_symbol_is_singleton = self.response_builder.last_name().is_some_and(|name| name.starts_with("<<"));

        let (name, kind) = if definition.receiver().is_some_and(|receiver| receiver.as_self_node().is_some()) {
            (format!("self.{method_name}"), symbol_kind::FUNCTION)
        } else if previous_symbol_is_singleton {
            (method_name, symbol_kind::FUNCTION)
        } else {
            let kind = if method_name == "initialize" { symbol_kind::CONSTRUCTOR } else { symbol_kind::METHOD };
            (method_name, kind)
        };

        let symbol = self.create_document_symbol(name, kind, &node.location(), &definition.name_loc());
        self.response_builder.push(symbol);
    }

    fn create_constant_path_symbol(&mut self, node: &Node<'_>, target_location: &Location<'_>) {
        self.create_document_symbol(location_slice(target_location), symbol_kind::CONSTANT, &node.location(), target_location);
    }

    fn create_named_symbol(&mut self, name: String, kind: u32, node: &Node<'_>, selection_range_location: &Location<'_>) {
        self.create_document_symbol(name, kind, &node.location(), selection_range_location);
    }

    fn create_name_location_symbol(&mut self, name: String, kind: u32, name_location: &Location<'_>) {
        self.create_document_symbol(name, kind, name_location, name_location);
    }

    fn create_document_symbol(
        &mut self,
        name: String,
        kind: u32,
        range_location: &Location<'_>,
        selection_range_location: &Location<'_>,
    ) -> SymbolId {
        let name = if name.trim_matches(|character: char| character.is_whitespace() || character == '\0').is_empty() {
            "<blank>".to_string()
        } else {
            name
        };
        self.response_builder.add_to_last(
            name,
            kind,
            self.parsed.range_from_location(range_location),
            self.parsed.range_from_location(selection_range_location),
        )
    }

    fn handle_attr_accessor(&mut self, node: &CallNode<'_>) {
        if node.receiver().is_some_and(|receiver| receiver.as_self_node().is_none()) {
            return;
        }
        let Some(arguments) = node.arguments() else { return };

        for argument in arguments.arguments().iter() {
            if let Some(symbol) = argument.as_symbol_node() {
                let Some(value_location) = symbol.value_loc() else { continue };
                self.create_document_symbol(
                    location_slice(&value_location),
                    symbol_kind::FIELD,
                    &argument.location(),
                    &value_location,
                );
            } else if let Some(string) = argument.as_string_node() {
                let content_location = string.content_loc();
                let name = location_slice(&content_location);
                if name.is_empty() {
                    continue;
                }
                self.create_document_symbol(name, symbol_kind::FIELD, &argument.location(), &content_location);
            }
        }
    }

    fn handle_alias_method(&mut self, node: &CallNode<'_>) {
        if node.receiver().is_some_and(|receiver| receiver.as_self_node().is_none()) {
            return;
        }
        let Some(arguments) = node.arguments() else { return };
        let Some(new_name_argument) = arguments.arguments().iter().next() else { return };

        if let Some(symbol) = new_name_argument.as_symbol_node() {
            let Some(value_location) = symbol.value_loc() else { return };
            self.create_document_symbol(
                location_slice(&value_location),
                symbol_kind::METHOD,
                &new_name_argument.location(),
                &value_location,
            );
        } else if let Some(string) = new_name_argument.as_string_node() {
            let content_location = string.content_loc();
            let name = location_slice(&content_location);
            if name.is_empty() {
                return;
            }
            self.create_document_symbol(name, symbol_kind::METHOD, &new_name_argument.location(), &content_location);
        }
    }

    fn handle_rake_namespace(&mut self, node: &CallNode<'_>) {
        if !self.rake || node.receiver().is_some() {
            return;
        }
        let Some(arguments) = node.arguments() else { return };
        let Some(name_argument) = arguments.arguments().iter().next() else { return };

        let Some(name) = string_or_symbol_value(&name_argument) else { return };
        if name.is_empty() {
            return;
        }

        let location = name_argument.location();
        let symbol = self.create_document_symbol(name, symbol_kind::MODULE, &location, &location);
        self.response_builder.push(symbol);
    }

    fn handle_rake_task(&mut self, node: &CallNode<'_>) {
        if !self.rake || node.receiver().is_some() {
            return;
        }
        let Some(arguments) = node.arguments() else { return };
        let Some(name_argument) = arguments.arguments().iter().next() else { return };

        let name = if let Some(keyword_hash) = name_argument.as_keyword_hash_node() {
            keyword_hash
                .elements()
                .iter()
                .next()
                .and_then(|element| element.as_assoc_node().and_then(|assoc| string_or_symbol_value(&assoc.key())))
        } else {
            string_or_symbol_value(&name_argument)
        };
        let Some(name) = name else { return };
        if name.is_empty() {
            return;
        }

        let location = name_argument.location();
        self.create_document_symbol(name, symbol_kind::METHOD, &location, &location);
    }
}

fn string_or_symbol_value(node: &Node<'_>) -> Option<String> {
    if let Some(string) = node.as_string_node() {
        return Some(location_slice(&string.content_loc()));
    }
    node.as_symbol_node().and_then(|symbol| symbol.value_loc()).map(|location| location_slice(&location))
}
