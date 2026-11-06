use std::collections::HashMap;
use std::rc::Rc;

use rbs_definition::ast::TypeName;
use steep::method_name::MethodName;
use steep::ruby::node::{Child, Node, NodeId};
use steep::source::Source;
use steep::type_construction::Checker;
use steep::typing::Typing;
use steep_interface::types::Type;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeHintKind {
    MethodReturnType,
    AttributeType,
    VariableType,
    BlockParameterType,
    MethodParameterType,
    KeywordParameterType,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NodeTypeHint {
    pub kind: TypeHintKind,
    pub offset: usize,
    pub hint_type: Type,
    pub is_obvious: bool,
}

const IGNORED_VARIABLE_NAMES: &[&str] = &["_", "__any__", "__skip__"];
const ATTRIBUTE_METHODS: &[&str] = &["attr_reader", "attr_writer", "attr_accessor"];

struct Collector<'a> {
    checker: &'a Checker,
    typing: &'a Typing,
    method_names: HashMap<NodeId, MethodName>,
    class_names: HashMap<NodeId, TypeName>,
    hints: Vec<NodeTypeHint>,
}

pub fn collect(checker: &Checker, source: &Source, typing: &Typing) -> Vec<NodeTypeHint> {
    let mut method_names = HashMap::new();
    let mut class_names = HashMap::new();
    {
        let source_index = typing.source_index();
        for (method_name, entry) in &source_index.method_index {
            for definition in &entry.definitions {
                method_names.insert(definition.id, method_name.clone());
            }
        }
        for (type_name, entry) in &source_index.constant_index {
            for definition in &entry.definitions {
                class_names.insert(definition.id, type_name.clone());
            }
        }
    }
    let mut collector = Collector {
        checker,
        typing,
        method_names,
        class_names,
        hints: Vec::new(),
    };
    if let Some(node) = &source.node {
        collector.visit(node, None);
    }
    collector.hints.sort_by_key(|hint| hint.offset);
    collector.hints
}

impl Collector<'_> {
    fn type_of(&self, node: &Node) -> Option<Type> {
        self.typing.type_of(node.id)
    }

    fn push(&mut self, kind: TypeHintKind, offset: usize, hint_type: Type, is_obvious: bool) {
        self.hints.push(NodeTypeHint {
            kind,
            offset,
            hint_type,
            is_obvious,
        });
    }

    fn visit_children(&mut self, node: &Node, class_name: Option<&TypeName>) {
        for child in node.child_nodes() {
            self.visit(child, class_name);
        }
    }

    fn visit(&mut self, node: &Rc<Node>, class_name: Option<&TypeName>) {
        match node.type_ {
            "lvasgn" | "ivasgn" => {
                self.variable_hint(node, node.child_node(1).map(|value| is_obvious_value(value)));
                self.visit_children(node, class_name);
            }
            "or_asgn" => {
                if let (Some(target), Some(value)) = (node.child_node(0), node.child_node(1))
                    && matches!(target.type_, "lvasgn" | "ivasgn")
                    && let Some(name) = target.child_symbol(0)
                    && !IGNORED_VARIABLE_NAMES.contains(&name)
                    && let Some(hint_type) = self.type_of(node)
                {
                    let offset = target.location.name.unwrap_or(target.expression()).end_pos;
                    self.push(TypeHintKind::VariableType, offset, hint_type, is_obvious_value(value));
                }
                if let Some(value) = node.child_node(1) {
                    self.visit(value, class_name);
                }
            }
            "def" => self.method_definition(node, 1, class_name),
            "defs" => {
                if let Some(receiver) = node.child_node(0) {
                    self.visit(receiver, class_name);
                }
                self.method_definition(node, 2, class_name);
            }
            "block" | "numblock" | "itblock" => {
                if let Some(call) = node.child_node(0) {
                    self.visit(call, class_name);
                }
                if node.type_ == "block"
                    && let Some(parameters) = node.child_node(1)
                {
                    self.parameters(parameters, TypeHintKind::BlockParameterType, class_name);
                }
                if let Some(body) = node.child_node(2) {
                    self.visit(body, class_name);
                }
            }
            "class" => {
                let own_name = self.declared_class_name(node);
                for (index, child) in node.children.iter().enumerate() {
                    if let Child::Node(child) = child {
                        let scope = if index == 2 { own_name.as_ref() } else { class_name };
                        self.visit(child, scope);
                    }
                }
            }
            "module" => {
                let own_name = self.declared_class_name(node);
                for (index, child) in node.children.iter().enumerate() {
                    if let Child::Node(child) = child {
                        let scope = if index == 1 { own_name.as_ref() } else { class_name };
                        self.visit(child, scope);
                    }
                }
            }
            "sclass" => self.visit_children(node, None),
            "send" => {
                if let Some(class_name) = class_name {
                    self.attribute_hints(node, class_name);
                }
                self.visit_children(node, class_name);
            }
            _ => self.visit_children(node, class_name),
        }
    }

    fn declared_class_name(&self, node: &Node) -> Option<TypeName> {
        self.class_names.get(&node.child_node(0)?.id).cloned()
    }

    fn variable_hint(&mut self, node: &Node, is_obvious: Option<bool>) {
        let Some(name) = node.child_symbol(0) else { return };
        if IGNORED_VARIABLE_NAMES.contains(&name) {
            return;
        }
        let Some(hint_type) = self.type_of(node) else { return };
        let offset = node.location.name.unwrap_or(node.expression()).end_pos;
        self.push(
            TypeHintKind::VariableType,
            offset,
            hint_type,
            is_obvious.unwrap_or(false),
        );
    }

    fn method_definition(&mut self, node: &Rc<Node>, parameters_index: usize, class_name: Option<&TypeName>) {
        let parameters = node.child_node(parameters_index);
        let body = node.child_node(parameters_index + 1);
        let return_type = match self.method_names.get(&node.id) {
            Some(method_name) => self.declared_return_type(method_name),
            None => None,
        }
        .or_else(|| match body {
            Some(body) => self.type_of(body),
            None => Some(Type::Nil),
        });
        if let Some(return_type) = return_type {
            let name_end = node.location.name.map(|name| name.end_pos);
            let offset = match parameters {
                Some(parameters) => match parameters.location.end {
                    Some(right_parenthesis) => Some(right_parenthesis.end_pos),
                    None if parameters
                        .location
                        .expression
                        .is_some_and(|expression| expression.size() > 0) =>
                    {
                        Some(parameters.expression().end_pos)
                    }
                    None => name_end,
                },
                None => name_end,
            };
            if let Some(offset) = offset {
                self.push(TypeHintKind::MethodReturnType, offset, return_type, false);
            }
        }
        if let Some(parameters) = parameters {
            self.parameters(parameters, TypeHintKind::MethodParameterType, class_name);
        }
        if let Some(body) = body {
            self.visit(body, class_name);
        }
    }

    fn declared_return_type(&self, method_name: &MethodName) -> Option<Type> {
        let definition_builder = &self.checker.builder.definition_builder;
        let definition = if method_name.is_singleton() {
            definition_builder.build_singleton(method_name.type_name()).ok()?
        } else {
            definition_builder.build_instance(method_name.type_name()).ok()?
        };
        let method = definition.methods.get(method_name.method_name())?;
        let return_types: Vec<Type> = method
            .method_types()
            .map(|method_type| self.checker.builder.factory.method_type(method_type).type_.return_type)
            .collect();
        if return_types.is_empty() {
            None
        } else {
            Some(Type::union(return_types))
        }
    }

    fn parameters(&mut self, parameters: &Rc<Node>, positional_kind: TypeHintKind, class_name: Option<&TypeName>) {
        for parameter in parameters.child_nodes() {
            match parameter.type_ {
                "arg" | "restarg" | "kwrestarg" | "blockarg" | "shadowarg" => {
                    self.parameter_hint(parameter, positional_kind, None);
                }
                "optarg" => {
                    let default = parameter.child_node(1).cloned();
                    self.parameter_hint(parameter, positional_kind, default.as_ref());
                    if let Some(default) = default {
                        self.visit(&default, class_name);
                    }
                }
                "kwarg" => self.keyword_parameter_hint(parameter, None),
                "kwoptarg" => {
                    let default = parameter.child_node(1).cloned();
                    self.keyword_parameter_hint(parameter, default.as_ref());
                    if let Some(default) = default {
                        self.visit(&default, class_name);
                    }
                }
                "procarg0" if parameter.child_symbol(0).is_some() => {
                    self.parameter_hint(parameter, positional_kind, None);
                }
                "procarg0" | "mlhs" => self.parameters(parameter, positional_kind, class_name),
                _ => {}
            }
        }
    }

    fn parameter_hint(&mut self, parameter: &Node, kind: TypeHintKind, default: Option<&Rc<Node>>) {
        let Some(name) = parameter.child_symbol(0) else { return };
        if IGNORED_VARIABLE_NAMES.contains(&name) {
            return;
        }
        let Some(location) = parameter.location.name.or(parameter.location.expression) else {
            return;
        };
        let Some(hint_type) = self.type_of(parameter) else {
            return;
        };
        let is_obvious = default.is_some_and(|default| {
            is_obvious_value(default)
                && self
                    .type_of(default)
                    .is_some_and(|default_type| default_type == hint_type)
        });
        self.push(kind, location.end_pos, hint_type, is_obvious);
    }

    fn keyword_parameter_hint(&mut self, parameter: &Node, default: Option<&Rc<Node>>) {
        let Some(name_location) = parameter.location.name else {
            return;
        };
        let Some(hint_type) = self.type_of(parameter) else {
            return;
        };
        let is_obvious = default.is_some_and(|default| {
            is_obvious_value(default)
                && self
                    .type_of(default)
                    .is_some_and(|default_type| default_type == hint_type)
        });
        self.push(
            TypeHintKind::KeywordParameterType,
            name_location.end_pos + 1,
            hint_type,
            is_obvious,
        );
    }

    fn attribute_hints(&mut self, node: &Node, class_name: &TypeName) {
        if node.child_node(0).is_some() {
            return;
        }
        let Some(method) = node.child_symbol(1) else { return };
        if !ATTRIBUTE_METHODS.contains(&method) {
            return;
        }
        let Ok(definition) = self.checker.builder.definition_builder.build_instance(class_name) else {
            return;
        };
        for argument in node.children.iter().skip(2).filter_map(Child::node) {
            if argument.type_ != "sym" {
                continue;
            }
            let Some(attribute_name) = argument.child_symbol(0) else {
                continue;
            };
            let method_name = if method == "attr_writer" {
                format!("{attribute_name}=")
            } else {
                attribute_name.to_string()
            };
            let Some(method) = definition.methods.get(&method_name) else {
                continue;
            };
            let Some(method_type) = method.method_types().next() else {
                continue;
            };
            let attribute_type = self.checker.builder.factory.method_type(method_type).type_.return_type;
            self.push(
                TypeHintKind::AttributeType,
                argument.expression().begin_pos,
                attribute_type,
                false,
            );
        }
    }
}

fn is_obvious_value(value: &Node) -> bool {
    match value.type_ {
        "int" | "float" | "rational" | "complex" | "str" | "dstr" | "xstr" | "sym" | "dsym" | "regexp" | "nil"
        | "true" | "false" | "array" | "hash" | "irange" | "erange" | "lambda" => true,
        "block" => value.child_node(0).is_some_and(|call| call.type_ == "lambda"),
        "send" => {
            let receiver_is_constant = value.child_node(0).is_some_and(|receiver| receiver.type_ == "const");
            receiver_is_constant && matches!(value.child_symbol(1), Some("new" | "[]"))
        }
        _ => false,
    }
}
