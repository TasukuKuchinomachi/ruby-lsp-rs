use ruby_prism::{Location, Node};
use serde::Serialize;

use crate::document::ParsedDocument;
use crate::position::{Position, Range};
use crate::prism::{NodeExt, child_nodes, constant_id_name, location_slice, node_slice};
use crate::requests::support::common::covers_position;

pub mod document_highlight_kind {
    pub const TEXT: u32 = 1;
    pub const READ: u32 = 2;
    pub const WRITE: u32 = 3;
}

const GLOBAL_VARIABLE_NODES: &[&str] = &[
    "GlobalVariableAndWriteNode",
    "GlobalVariableOperatorWriteNode",
    "GlobalVariableOrWriteNode",
    "GlobalVariableReadNode",
    "GlobalVariableTargetNode",
    "GlobalVariableWriteNode",
];

const INSTANCE_VARIABLE_NODES: &[&str] = &[
    "InstanceVariableAndWriteNode",
    "InstanceVariableOperatorWriteNode",
    "InstanceVariableOrWriteNode",
    "InstanceVariableReadNode",
    "InstanceVariableTargetNode",
    "InstanceVariableWriteNode",
];

const CONSTANT_NODES: &[&str] = &[
    "ConstantAndWriteNode",
    "ConstantOperatorWriteNode",
    "ConstantOrWriteNode",
    "ConstantReadNode",
    "ConstantTargetNode",
    "ConstantWriteNode",
];

const CONSTANT_PATH_NODES: &[&str] = &[
    "ConstantPathAndWriteNode",
    "ConstantPathNode",
    "ConstantPathOperatorWriteNode",
    "ConstantPathOrWriteNode",
    "ConstantPathTargetNode",
    "ConstantPathWriteNode",
];

const CLASS_VARIABLE_NODES: &[&str] = &[
    "ClassVariableAndWriteNode",
    "ClassVariableOperatorWriteNode",
    "ClassVariableOrWriteNode",
    "ClassVariableReadNode",
    "ClassVariableTargetNode",
    "ClassVariableWriteNode",
];

const LOCAL_NODES: &[&str] = &[
    "LocalVariableAndWriteNode",
    "LocalVariableOperatorWriteNode",
    "LocalVariableOrWriteNode",
    "LocalVariableReadNode",
    "LocalVariableTargetNode",
    "LocalVariableWriteNode",
    "BlockParameterNode",
    "RequiredParameterNode",
    "RequiredKeywordParameterNode",
    "OptionalKeywordParameterNode",
    "RestParameterNode",
    "OptionalParameterNode",
    "KeywordRestParameterNode",
];

const CALL_OR_DEFINITION_NODES: &[&str] = &["CallNode", "DefNode"];

const VALUE_TARGET_NODES: &[&str] = &[
    "GlobalVariableReadNode",
    "GlobalVariableAndWriteNode",
    "GlobalVariableOperatorWriteNode",
    "GlobalVariableOrWriteNode",
    "GlobalVariableTargetNode",
    "GlobalVariableWriteNode",
    "InstanceVariableAndWriteNode",
    "InstanceVariableOperatorWriteNode",
    "InstanceVariableOrWriteNode",
    "InstanceVariableReadNode",
    "InstanceVariableTargetNode",
    "InstanceVariableWriteNode",
    "ConstantAndWriteNode",
    "ConstantOperatorWriteNode",
    "ConstantOrWriteNode",
    "ConstantPathAndWriteNode",
    "ConstantPathNode",
    "ConstantPathOperatorWriteNode",
    "ConstantPathOrWriteNode",
    "ConstantPathTargetNode",
    "ConstantPathWriteNode",
    "ConstantReadNode",
    "ConstantTargetNode",
    "ConstantWriteNode",
    "ClassVariableAndWriteNode",
    "ClassVariableOperatorWriteNode",
    "ClassVariableOrWriteNode",
    "ClassVariableReadNode",
    "ClassVariableTargetNode",
    "ClassVariableWriteNode",
    "LocalVariableAndWriteNode",
    "LocalVariableOperatorWriteNode",
    "LocalVariableOrWriteNode",
    "LocalVariableReadNode",
    "LocalVariableTargetNode",
    "LocalVariableWriteNode",
    "CallNode",
    "BlockParameterNode",
    "RequiredKeywordParameterNode",
    "KeywordRestParameterNode",
    "OptionalParameterNode",
    "RequiredParameterNode",
    "RestParameterNode",
];

const KEYWORD_TARGET_NODES: &[&str] = &[
    "ModuleNode",
    "ClassNode",
    "SingletonClassNode",
    "DefNode",
    "CaseNode",
    "WhileNode",
    "UntilNode",
    "ForNode",
    "IfNode",
    "UnlessNode",
    "BlockNode",
    "LambdaNode",
    "BeginNode",
];

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DocumentHighlight {
    pub range: Range,
    pub kind: u32,
}

pub struct DocumentHighlightListener<'a, 'document> {
    response: Vec<DocumentHighlight>,
    parsed: &'a ParsedDocument<'document>,
    target_type: Option<&'static str>,
    target_value: Option<String>,
    target_position: Position,
}

impl<'a, 'document> DocumentHighlightListener<'a, 'document> {
    pub fn new(
        parsed: &'a ParsedDocument<'document>,
        target: Option<&Node<'_>>,
        parent: Option<&Node<'_>>,
        position: Position,
    ) -> DocumentHighlightListener<'a, 'document> {
        let mut listener =
            DocumentHighlightListener { response: Vec::new(), parsed, target_type: None, target_value: None, target_position: position };

        let (Some(target), Some(_)) = (target, parent) else { return listener };
        let type_name = target.type_name();
        if VALUE_TARGET_NODES.contains(&type_name) {
            listener.target_type = Some(type_name);
            listener.target_value = node_value(target);
        } else if KEYWORD_TARGET_NODES.contains(&type_name) {
            listener.target_type = Some(type_name);
        }
        listener
    }

    pub fn into_response(self) -> Vec<DocumentHighlight> {
        self.response
    }

    pub fn visit(&mut self, node: &Node<'_>) {
        if self.target_type.is_none() {
            return;
        }
        self.walk(node);
    }

    fn walk(&mut self, node: &Node<'_>) {
        self.on_enter(node);
        for child in child_nodes(node) {
            self.walk(&child);
        }
    }

    fn target_is(&self, type_name: &str) -> bool {
        self.target_type == Some(type_name)
    }

    fn matches(&self, node: &Node<'_>, classes: &[&str]) -> bool {
        self.target_type.is_some_and(|target_type| classes.contains(&target_type)) && self.target_value == node_value(node)
    }

    fn highlight_if_matches(&mut self, node: &Node<'_>, classes: &[&str], kind: u32, location: Option<Location<'_>>) {
        if !self.matches(node, classes) {
            return;
        }
        if let Some(location) = location {
            self.add_highlight(kind, &location);
        }
    }

    fn on_enter(&mut self, node: &Node<'_>) {
        use document_highlight_kind::{READ, WRITE};

        match node {
            Node::CallNode { .. } => {
                let message_location = node.as_call_node().expect("variant was matched").message_loc();
                self.highlight_if_matches(node, CALL_OR_DEFINITION_NODES, READ, message_location);
            }
            Node::DefNode { .. } => {
                let definition = node.as_def_node().expect("variant was matched");
                if self.target_is("DefNode") {
                    self.add_matching_end_highlights(Some(definition.def_keyword_loc()), definition.end_keyword_loc());
                }
                self.highlight_if_matches(node, CALL_OR_DEFINITION_NODES, WRITE, Some(definition.name_loc()));
            }
            Node::GlobalVariableTargetNode { .. } => {
                self.highlight_if_matches(node, GLOBAL_VARIABLE_NODES, WRITE, Some(node.location()));
            }
            Node::InstanceVariableTargetNode { .. } => {
                self.highlight_if_matches(node, INSTANCE_VARIABLE_NODES, WRITE, Some(node.location()));
            }
            Node::ConstantPathTargetNode { .. } => {
                self.highlight_if_matches(node, CONSTANT_PATH_NODES, WRITE, Some(node.location()));
            }
            Node::ConstantTargetNode { .. } => {
                self.highlight_if_matches(node, CONSTANT_NODES, WRITE, Some(node.location()));
            }
            Node::ClassVariableTargetNode { .. } => {
                self.highlight_if_matches(node, CLASS_VARIABLE_NODES, WRITE, Some(node.location()));
            }
            Node::LocalVariableTargetNode { .. } | Node::BlockParameterNode { .. } | Node::RequiredParameterNode { .. } => {
                self.highlight_if_matches(node, LOCAL_NODES, WRITE, Some(node.location()));
            }
            Node::ClassNode { .. } => {
                let class = node.as_class_node().expect("variant was matched");
                if self.target_is("ClassNode") {
                    self.add_matching_end_highlights(Some(class.class_keyword_loc()), Some(class.end_keyword_loc()));
                }
                let classes = [CONSTANT_NODES, CONSTANT_PATH_NODES, &["ClassNode"]].concat();
                self.highlight_if_matches(node, &classes, WRITE, Some(class.constant_path().location()));
            }
            Node::ModuleNode { .. } => {
                let module = node.as_module_node().expect("variant was matched");
                if self.target_is("ModuleNode") {
                    self.add_matching_end_highlights(Some(module.module_keyword_loc()), Some(module.end_keyword_loc()));
                }
                let classes = [CONSTANT_NODES, CONSTANT_PATH_NODES, &["ModuleNode"]].concat();
                self.highlight_if_matches(node, &classes, WRITE, Some(module.constant_path().location()));
            }
            Node::LocalVariableReadNode { .. } => {
                self.highlight_if_matches(node, LOCAL_NODES, READ, Some(node.location()));
            }
            Node::ConstantPathNode { .. } => {
                let name_location = node.as_constant_path_node().expect("variant was matched").name_loc();
                self.highlight_if_matches(node, CONSTANT_PATH_NODES, READ, Some(name_location));
            }
            Node::ConstantReadNode { .. } => {
                self.highlight_if_matches(node, CONSTANT_NODES, READ, Some(node.location()));
            }
            Node::InstanceVariableReadNode { .. } => {
                self.highlight_if_matches(node, INSTANCE_VARIABLE_NODES, READ, Some(node.location()));
            }
            Node::ClassVariableReadNode { .. } => {
                self.highlight_if_matches(node, CLASS_VARIABLE_NODES, READ, Some(node.location()));
            }
            Node::GlobalVariableReadNode { .. } => {
                self.highlight_if_matches(node, GLOBAL_VARIABLE_NODES, READ, Some(node.location()));
            }
            Node::ConstantPathWriteNode { .. } => {
                let target = node.as_constant_path_write_node().expect("variant was matched").target();
                self.highlight_if_matches(node, CONSTANT_PATH_NODES, WRITE, Some(target.location()));
            }
            Node::ConstantPathOrWriteNode { .. } => {
                let target = node.as_constant_path_or_write_node().expect("variant was matched").target();
                self.highlight_if_matches(node, CONSTANT_PATH_NODES, WRITE, Some(target.location()));
            }
            Node::ConstantPathAndWriteNode { .. } => {
                let target = node.as_constant_path_and_write_node().expect("variant was matched").target();
                self.highlight_if_matches(node, CONSTANT_PATH_NODES, WRITE, Some(target.location()));
            }
            Node::ConstantPathOperatorWriteNode { .. } => {
                let target = node.as_constant_path_operator_write_node().expect("variant was matched").target();
                self.highlight_if_matches(node, CONSTANT_PATH_NODES, WRITE, Some(target.location()));
            }
            Node::LocalVariableWriteNode { .. } => {
                let name_location = node.as_local_variable_write_node().expect("variant was matched").name_loc();
                self.highlight_if_matches(node, LOCAL_NODES, WRITE, Some(name_location));
            }
            Node::RequiredKeywordParameterNode { .. } => {
                let name_location = node.as_required_keyword_parameter_node().expect("variant was matched").name_loc();
                self.highlight_if_matches(node, LOCAL_NODES, WRITE, Some(name_location));
            }
            Node::OptionalKeywordParameterNode { .. } => {
                let name_location = node.as_optional_keyword_parameter_node().expect("variant was matched").name_loc();
                self.highlight_if_matches(node, LOCAL_NODES, WRITE, Some(name_location));
            }
            Node::RestParameterNode { .. } => {
                let name_location = node.as_rest_parameter_node().expect("variant was matched").name_loc();
                self.highlight_if_matches(node, LOCAL_NODES, WRITE, name_location);
            }
            Node::OptionalParameterNode { .. } => {
                let name_location = node.as_optional_parameter_node().expect("variant was matched").name_loc();
                self.highlight_if_matches(node, LOCAL_NODES, WRITE, Some(name_location));
            }
            Node::KeywordRestParameterNode { .. } => {
                let name_location = node.as_keyword_rest_parameter_node().expect("variant was matched").name_loc();
                self.highlight_if_matches(node, LOCAL_NODES, WRITE, name_location);
            }
            Node::LocalVariableAndWriteNode { .. } => {
                let name_location = node.as_local_variable_and_write_node().expect("variant was matched").name_loc();
                self.highlight_if_matches(node, LOCAL_NODES, WRITE, Some(name_location));
            }
            Node::LocalVariableOperatorWriteNode { .. } => {
                let name_location = node.as_local_variable_operator_write_node().expect("variant was matched").name_loc();
                self.highlight_if_matches(node, LOCAL_NODES, WRITE, Some(name_location));
            }
            Node::LocalVariableOrWriteNode { .. } => {
                let name_location = node.as_local_variable_or_write_node().expect("variant was matched").name_loc();
                self.highlight_if_matches(node, LOCAL_NODES, WRITE, Some(name_location));
            }
            Node::ClassVariableWriteNode { .. } => {
                let name_location = node.as_class_variable_write_node().expect("variant was matched").name_loc();
                self.highlight_if_matches(node, CLASS_VARIABLE_NODES, WRITE, Some(name_location));
            }
            Node::ClassVariableOrWriteNode { .. } => {
                let name_location = node.as_class_variable_or_write_node().expect("variant was matched").name_loc();
                self.highlight_if_matches(node, CLASS_VARIABLE_NODES, WRITE, Some(name_location));
            }
            Node::ClassVariableOperatorWriteNode { .. } => {
                let name_location = node.as_class_variable_operator_write_node().expect("variant was matched").name_loc();
                self.highlight_if_matches(node, CLASS_VARIABLE_NODES, WRITE, Some(name_location));
            }
            Node::ClassVariableAndWriteNode { .. } => {
                let name_location = node.as_class_variable_and_write_node().expect("variant was matched").name_loc();
                self.highlight_if_matches(node, CLASS_VARIABLE_NODES, WRITE, Some(name_location));
            }
            Node::ConstantWriteNode { .. } => {
                let name_location = node.as_constant_write_node().expect("variant was matched").name_loc();
                self.highlight_if_matches(node, CONSTANT_NODES, WRITE, Some(name_location));
            }
            Node::ConstantOrWriteNode { .. } => {
                let name_location = node.as_constant_or_write_node().expect("variant was matched").name_loc();
                self.highlight_if_matches(node, CONSTANT_NODES, WRITE, Some(name_location));
            }
            Node::ConstantOperatorWriteNode { .. } => {
                let name_location = node.as_constant_operator_write_node().expect("variant was matched").name_loc();
                self.highlight_if_matches(node, CONSTANT_NODES, WRITE, Some(name_location));
            }
            Node::ConstantAndWriteNode { .. } => {
                let name_location = node.as_constant_and_write_node().expect("variant was matched").name_loc();
                self.highlight_if_matches(node, CONSTANT_NODES, WRITE, Some(name_location));
            }
            Node::InstanceVariableWriteNode { .. } => {
                let name_location = node.as_instance_variable_write_node().expect("variant was matched").name_loc();
                self.highlight_if_matches(node, INSTANCE_VARIABLE_NODES, WRITE, Some(name_location));
            }
            Node::InstanceVariableOrWriteNode { .. } => {
                let name_location = node.as_instance_variable_or_write_node().expect("variant was matched").name_loc();
                self.highlight_if_matches(node, INSTANCE_VARIABLE_NODES, WRITE, Some(name_location));
            }
            Node::InstanceVariableAndWriteNode { .. } => {
                let name_location = node.as_instance_variable_and_write_node().expect("variant was matched").name_loc();
                self.highlight_if_matches(node, INSTANCE_VARIABLE_NODES, WRITE, Some(name_location));
            }
            Node::InstanceVariableOperatorWriteNode { .. } => {
                let name_location = node.as_instance_variable_operator_write_node().expect("variant was matched").name_loc();
                self.highlight_if_matches(node, INSTANCE_VARIABLE_NODES, WRITE, Some(name_location));
            }
            Node::GlobalVariableWriteNode { .. } => {
                let name_location = node.as_global_variable_write_node().expect("variant was matched").name_loc();
                self.highlight_if_matches(node, GLOBAL_VARIABLE_NODES, WRITE, Some(name_location));
            }
            Node::GlobalVariableOrWriteNode { .. } => {
                let name_location = node.as_global_variable_or_write_node().expect("variant was matched").name_loc();
                self.highlight_if_matches(node, GLOBAL_VARIABLE_NODES, WRITE, Some(name_location));
            }
            Node::GlobalVariableAndWriteNode { .. } => {
                let name_location = node.as_global_variable_and_write_node().expect("variant was matched").name_loc();
                self.highlight_if_matches(node, GLOBAL_VARIABLE_NODES, WRITE, Some(name_location));
            }
            Node::GlobalVariableOperatorWriteNode { .. } => {
                let name_location = node.as_global_variable_operator_write_node().expect("variant was matched").name_loc();
                self.highlight_if_matches(node, GLOBAL_VARIABLE_NODES, WRITE, Some(name_location));
            }
            Node::SingletonClassNode { .. } if self.target_is("SingletonClassNode") => {
                let singleton_class = node.as_singleton_class_node().expect("variant was matched");
                self.add_matching_end_highlights(Some(singleton_class.class_keyword_loc()), Some(singleton_class.end_keyword_loc()));
            }
            Node::CaseNode { .. } if self.target_is("CaseNode") => {
                let case = node.as_case_node().expect("variant was matched");
                self.add_matching_end_highlights(Some(case.case_keyword_loc()), Some(case.end_keyword_loc()));
            }
            Node::WhileNode { .. } if self.target_is("WhileNode") => {
                let while_node = node.as_while_node().expect("variant was matched");
                self.add_matching_end_highlights(Some(while_node.keyword_loc()), while_node.closing_loc());
            }
            Node::UntilNode { .. } if self.target_is("UntilNode") => {
                let until = node.as_until_node().expect("variant was matched");
                self.add_matching_end_highlights(Some(until.keyword_loc()), until.closing_loc());
            }
            Node::ForNode { .. } if self.target_is("ForNode") => {
                let for_node = node.as_for_node().expect("variant was matched");
                self.add_matching_end_highlights(Some(for_node.for_keyword_loc()), Some(for_node.end_keyword_loc()));
            }
            Node::IfNode { .. } if self.target_is("IfNode") => {
                let if_node = node.as_if_node().expect("variant was matched");
                self.add_matching_end_highlights(if_node.if_keyword_loc(), if_node.end_keyword_loc());
            }
            Node::UnlessNode { .. } if self.target_is("UnlessNode") => {
                let unless = node.as_unless_node().expect("variant was matched");
                self.add_matching_end_highlights(Some(unless.keyword_loc()), unless.end_keyword_loc());
            }
            Node::BlockNode { .. } if self.target_is("BlockNode") => {
                let block = node.as_block_node().expect("variant was matched");
                self.add_matching_end_highlights(Some(block.opening_loc()), Some(block.closing_loc()));
            }
            Node::LambdaNode { .. } if self.target_is("LambdaNode") => {
                let lambda = node.as_lambda_node().expect("variant was matched");
                self.add_matching_end_highlights(Some(lambda.opening_loc()), Some(lambda.closing_loc()));
            }
            Node::BeginNode { .. } if self.target_is("BeginNode") => {
                let begin = node.as_begin_node().expect("variant was matched");
                self.add_matching_end_highlights(begin.begin_keyword_loc(), begin.end_keyword_loc());
            }
            _ => {}
        }
    }

    fn add_highlight(&mut self, kind: u32, location: &Location<'_>) {
        self.response.push(DocumentHighlight { range: self.parsed.range_from_location(location), kind });
    }

    fn add_matching_end_highlights(&mut self, keyword_location: Option<Location<'_>>, end_location: Option<Location<'_>>) {
        let (Some(keyword_location), Some(end_location)) = (keyword_location, end_location) else { return };
        if end_location.end_offset() <= end_location.start_offset() {
            return;
        }
        if !self.covers_target_position(&keyword_location) && !self.covers_target_position(&end_location) {
            return;
        }
        self.add_highlight(document_highlight_kind::TEXT, &keyword_location);
        self.add_highlight(document_highlight_kind::TEXT, &end_location);
    }

    fn covers_target_position(&self, location: &Location<'_>) -> bool {
        covers_position(self.parsed, location.start_offset(), location.end_offset(), self.target_position)
    }
}

fn node_value(node: &Node<'_>) -> Option<String> {
    let name = match node {
        Node::ConstantReadNode { .. }
        | Node::ConstantPathNode { .. }
        | Node::BlockArgumentNode { .. }
        | Node::ConstantTargetNode { .. }
        | Node::ConstantPathWriteNode { .. }
        | Node::ConstantPathTargetNode { .. }
        | Node::ConstantPathOrWriteNode { .. }
        | Node::ConstantPathOperatorWriteNode { .. }
        | Node::ConstantPathAndWriteNode { .. } => return Some(node_slice(node)),
        Node::CallNode { .. } => {
            return node.as_call_node()?.message_loc().map(|location| location_slice(&location));
        }
        Node::ClassNode { .. } => return Some(node_slice(&node.as_class_node()?.constant_path())),
        Node::ModuleNode { .. } => return Some(node_slice(&node.as_module_node()?.constant_path())),
        Node::GlobalVariableReadNode { .. } => node.as_global_variable_read_node()?.name(),
        Node::GlobalVariableAndWriteNode { .. } => node.as_global_variable_and_write_node()?.name(),
        Node::GlobalVariableOperatorWriteNode { .. } => node.as_global_variable_operator_write_node()?.name(),
        Node::GlobalVariableOrWriteNode { .. } => node.as_global_variable_or_write_node()?.name(),
        Node::GlobalVariableTargetNode { .. } => node.as_global_variable_target_node()?.name(),
        Node::GlobalVariableWriteNode { .. } => node.as_global_variable_write_node()?.name(),
        Node::InstanceVariableAndWriteNode { .. } => node.as_instance_variable_and_write_node()?.name(),
        Node::InstanceVariableOperatorWriteNode { .. } => node.as_instance_variable_operator_write_node()?.name(),
        Node::InstanceVariableOrWriteNode { .. } => node.as_instance_variable_or_write_node()?.name(),
        Node::InstanceVariableReadNode { .. } => node.as_instance_variable_read_node()?.name(),
        Node::InstanceVariableTargetNode { .. } => node.as_instance_variable_target_node()?.name(),
        Node::InstanceVariableWriteNode { .. } => node.as_instance_variable_write_node()?.name(),
        Node::ConstantAndWriteNode { .. } => node.as_constant_and_write_node()?.name(),
        Node::ConstantOperatorWriteNode { .. } => node.as_constant_operator_write_node()?.name(),
        Node::ConstantOrWriteNode { .. } => node.as_constant_or_write_node()?.name(),
        Node::ConstantWriteNode { .. } => node.as_constant_write_node()?.name(),
        Node::ClassVariableAndWriteNode { .. } => node.as_class_variable_and_write_node()?.name(),
        Node::ClassVariableOperatorWriteNode { .. } => node.as_class_variable_operator_write_node()?.name(),
        Node::ClassVariableOrWriteNode { .. } => node.as_class_variable_or_write_node()?.name(),
        Node::ClassVariableReadNode { .. } => node.as_class_variable_read_node()?.name(),
        Node::ClassVariableTargetNode { .. } => node.as_class_variable_target_node()?.name(),
        Node::ClassVariableWriteNode { .. } => node.as_class_variable_write_node()?.name(),
        Node::LocalVariableAndWriteNode { .. } => node.as_local_variable_and_write_node()?.name(),
        Node::LocalVariableOperatorWriteNode { .. } => node.as_local_variable_operator_write_node()?.name(),
        Node::LocalVariableOrWriteNode { .. } => node.as_local_variable_or_write_node()?.name(),
        Node::LocalVariableReadNode { .. } => node.as_local_variable_read_node()?.name(),
        Node::LocalVariableTargetNode { .. } => node.as_local_variable_target_node()?.name(),
        Node::LocalVariableWriteNode { .. } => node.as_local_variable_write_node()?.name(),
        Node::DefNode { .. } => node.as_def_node()?.name(),
        Node::RequiredKeywordParameterNode { .. } => node.as_required_keyword_parameter_node()?.name(),
        Node::OptionalKeywordParameterNode { .. } => node.as_optional_keyword_parameter_node()?.name(),
        Node::OptionalParameterNode { .. } => node.as_optional_parameter_node()?.name(),
        Node::RequiredParameterNode { .. } => node.as_required_parameter_node()?.name(),
        Node::BlockParameterNode { .. } => return Some(optional_name(node.as_block_parameter_node()?.name())),
        Node::KeywordRestParameterNode { .. } => {
            return Some(optional_name(node.as_keyword_rest_parameter_node()?.name()));
        }
        Node::RestParameterNode { .. } => return Some(optional_name(node.as_rest_parameter_node()?.name())),
        _ => return None,
    };
    Some(constant_id_name(&name))
}

fn optional_name(name: Option<ruby_prism::ConstantId<'_>>) -> String {
    name.map(|name| constant_id_name(&name)).unwrap_or_default()
}
