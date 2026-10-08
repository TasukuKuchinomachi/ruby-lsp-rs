use std::collections::HashSet;

use ruby_prism::{CallNode, Location, MatchWriteNode, Node};

use crate::prism::{child_nodes, constant_id_name, location_slice};
use crate::requests::support::common::each_constant_path_part;
use crate::requests::support::semantic_highlighting_response_builder::{
    SemanticHighlightingResponseBuilder, TokenModifier, TokenType,
};
use crate::requests::support::sorbet;

/// `Module.instance_methods(false)`, `Kernel.instance_methods(false)`, `Kernel.methods(false)`,
/// `Bundler::Dsl.instance_methods(false)` and `Module.private_instance_methods(false)`, computed with Ruby 3.2, because
/// Ruby LSP computes them from the Ruby running the server and there is none here
pub const SPECIAL_RUBY_METHODS: &[&str] = &[
    "!~", "<", "<=", "<=>", "==", "===", ">", ">=", "Array", "Complex", "Float", "Hash", "Integer", "Rational", "String",
    "__callee__", "__dir__", "__method__", "`", "abort", "alias_method", "ancestors", "append_features", "at_exit", "attr",
    "attr_accessor", "attr_reader", "attr_writer", "autoload", "autoload?", "binding", "block_given?", "caller",
    "caller_locations", "catch", "check_primary_source_safety", "class", "class_eval", "class_exec",
    "class_variable_defined?", "class_variable_get", "class_variable_set", "class_variables", "clone", "const_added",
    "const_defined?", "const_get", "const_missing", "const_set", "const_source_location", "constants", "define_method",
    "define_singleton_method", "dependencies", "dependencies=", "deprecate_constant", "display", "dup", "enum_for", "env",
    "eql?", "eval", "eval_gemfile", "exec", "exit", "exit!", "extend", "extend_object", "extended", "fail", "fork",
    "format", "freeze", "frozen?", "gem", "gemfile", "gemspec", "gemspecs", "gets", "git", "git_source", "github",
    "global_variables", "group", "hash", "include", "include?", "included", "included_modules", "initialize",
    "initialize_clone", "initialize_copy", "inspect", "install_if", "instance_method", "instance_methods", "instance_of?",
    "instance_variable_defined?", "instance_variable_get", "instance_variable_set", "instance_variables", "is_a?",
    "iterator?", "itself", "kind_of?", "lambda", "load", "local_variables", "lockfile", "lockfile_path", "loop", "method",
    "method_added", "method_defined?", "method_missing", "method_removed", "method_undefined", "methods", "module_eval",
    "module_exec", "module_function", "name", "nil?", "object_id", "open", "p", "path", "platform", "platforms", "plugin",
    "prepend", "prepend_features", "prepended", "print", "printf", "private", "private_class_method", "private_constant",
    "private_instance_methods", "private_method_defined?", "private_methods", "proc", "protected",
    "protected_instance_methods", "protected_method_defined?", "protected_methods", "public", "public_class_method",
    "public_constant", "public_instance_method", "public_instance_methods", "public_method", "public_method_defined?",
    "public_methods", "public_send", "putc", "puts", "raise", "rand", "readline", "readlines", "refine", "refinements",
    "remove_class_variable", "remove_const", "remove_instance_variable", "remove_method", "require", "require_relative",
    "respond_to?", "ruby2_keywords", "select", "send", "set_trace_func", "singleton_class", "singleton_class?",
    "singleton_method", "singleton_methods", "sleep", "source", "spawn", "sprintf", "srand", "syscall", "system", "tap",
    "test", "then", "throw", "to_definition", "to_enum", "to_s", "trace_var", "trap", "undef_method",
    "undefined_instance_methods", "untrace_var", "using", "warn", "yield_self",
];

enum Visit<'pr> {
    Enter(Node<'pr>),
    Leave(Node<'pr>),
}

pub struct SemanticHighlighting<'a, 'b> {
    response_builder: &'a mut SemanticHighlightingResponseBuilder<'b>,
    scopes: Vec<HashSet<String>>,
    inside_regex_capture: bool,
    inside_implicit_node: bool,
}

impl<'a, 'b> SemanticHighlighting<'a, 'b> {
    pub fn new(response_builder: &'a mut SemanticHighlightingResponseBuilder<'b>) -> SemanticHighlighting<'a, 'b> {
        SemanticHighlighting { response_builder, scopes: vec![HashSet::new()], inside_regex_capture: false, inside_implicit_node: false }
    }

    /// An explicit stack instead of recursion, so that deeply nested code can't overflow the request thread's stack
    pub fn visit(&mut self, root: Node<'_>) {
        let mut stack = vec![Visit::Enter(root)];
        while let Some(visit) = stack.pop() {
            match visit {
                Visit::Enter(node) => {
                    self.on_enter(&node);
                    let children = child_nodes(&node);
                    stack.push(Visit::Leave(node));
                    stack.extend(children.into_iter().rev().map(Visit::Enter));
                }
                Visit::Leave(node) => self.on_leave(&node),
            }
        }
    }

    fn on_enter(&mut self, node: &Node<'_>) {
        match node {
            Node::CallNode { .. } => self.on_call_node_enter(&node.as_call_node().expect("matched variant")),
            Node::ClassNode { .. } => self.on_class_node_enter(node),
            Node::DefNode { .. } | Node::BlockNode { .. } => self.scopes.push(HashSet::new()),
            Node::SelfNode { .. } => {
                self.add_location_token(&node.location(), TokenType::Variable, &[TokenModifier::DefaultLibrary]);
            }
            Node::ModuleNode { .. } => self.on_module_node_enter(node),
            Node::LocalVariableWriteNode { .. } => {
                let write = node.as_local_variable_write_node().expect("matched variant");
                self.add_parameter_token_if_parameter(&constant_id_name(&write.name()), &write.name_loc());
            }
            Node::LocalVariableReadNode { .. } => self.on_local_variable_read_node_enter(node),
            Node::BlockParameterNode { .. } => {
                if let Some(name) = node.as_block_parameter_node().expect("matched variant").name() {
                    self.add_parameter(constant_id_name(&name));
                }
            }
            Node::RequiredKeywordParameterNode { .. } => {
                let parameter = node.as_required_keyword_parameter_node().expect("matched variant");
                self.add_parameter(constant_id_name(&parameter.name()));
            }
            Node::OptionalKeywordParameterNode { .. } => {
                let parameter = node.as_optional_keyword_parameter_node().expect("matched variant");
                self.add_parameter(constant_id_name(&parameter.name()));
            }
            Node::KeywordRestParameterNode { .. } => {
                if let Some(name) = node.as_keyword_rest_parameter_node().expect("matched variant").name() {
                    self.add_parameter(constant_id_name(&name));
                }
            }
            Node::OptionalParameterNode { .. } => {
                let parameter = node.as_optional_parameter_node().expect("matched variant");
                self.add_parameter(constant_id_name(&parameter.name()));
            }
            Node::RequiredParameterNode { .. } => {
                let parameter = node.as_required_parameter_node().expect("matched variant");
                self.add_parameter(constant_id_name(&parameter.name()));
            }
            Node::RestParameterNode { .. } => {
                if let Some(name) = node.as_rest_parameter_node().expect("matched variant").name() {
                    self.add_parameter(constant_id_name(&name));
                }
            }
            Node::LocalVariableAndWriteNode { .. } => {
                let write = node.as_local_variable_and_write_node().expect("matched variant");
                self.add_parameter_token_if_parameter(&constant_id_name(&write.name()), &write.name_loc());
            }
            Node::LocalVariableOperatorWriteNode { .. } => {
                let write = node.as_local_variable_operator_write_node().expect("matched variant");
                self.add_parameter_token_if_parameter(&constant_id_name(&write.name()), &write.name_loc());
            }
            Node::LocalVariableOrWriteNode { .. } => {
                let write = node.as_local_variable_or_write_node().expect("matched variant");
                self.add_parameter_token_if_parameter(&constant_id_name(&write.name()), &write.name_loc());
            }
            Node::LocalVariableTargetNode { .. } => {
                if self.inside_regex_capture {
                    return;
                }
                let target = node.as_local_variable_target_node().expect("matched variant");
                let token_type = self.local_token_type(&constant_id_name(&target.name()));
                self.add_location_token(&node.location(), token_type, &[]);
            }
            Node::BlockLocalVariableNode { .. } => self.add_location_token(&node.location(), TokenType::Variable, &[]),
            Node::MatchWriteNode { .. } => {
                let match_write = node.as_match_write_node().expect("matched variant");
                if call_message(&match_write.call()).as_deref() == Some("=~") {
                    self.inside_regex_capture = true;
                    self.process_regexp_locals(&match_write);
                }
            }
            Node::ImplicitNode { .. } => self.inside_implicit_node = true,
            _ => {}
        }
    }

    fn on_leave(&mut self, node: &Node<'_>) {
        match node {
            Node::DefNode { .. } | Node::BlockNode { .. } => {
                self.scopes.pop();
            }
            Node::MatchWriteNode { .. } => {
                let match_write = node.as_match_write_node().expect("matched variant");
                if call_message(&match_write.call()).as_deref() == Some("=~") {
                    self.inside_regex_capture = false;
                }
            }
            Node::ImplicitNode { .. } => self.inside_implicit_node = false,
            _ => {}
        }
    }

    fn on_call_node_enter(&mut self, node: &CallNode<'_>) {
        if self.inside_implicit_node {
            return;
        }
        let Some(message_location) = node.message_loc() else { return };
        let message = location_slice(&message_location);

        if message.starts_with('[') && (message.ends_with(']') || message.ends_with("]=")) {
            return;
        }
        if message == "=~" || SPECIAL_RUBY_METHODS.contains(&message.as_str()) {
            return;
        }

        if sorbet::annotation(node) {
            self.add_location_token(&message_location, TokenType::Type, &[]);
        } else if node.receiver().is_none() && node.opening_loc().is_none() {
            self.add_location_token(&message_location, TokenType::Method, &[]);
        }
    }

    fn on_local_variable_read_node_enter(&mut self, node: &Node<'_>) {
        if self.inside_implicit_node {
            return;
        }
        let read = node.as_local_variable_read_node().expect("matched variant");
        let name = constant_id_name(&read.name());

        if numbered_parameter_like(&name) {
            self.add_location_token(&node.location(), TokenType::Parameter, &[]);
            return;
        }

        let token_type = self.local_token_type(&name);
        self.add_location_token(&node.location(), token_type, &[]);
    }

    fn on_class_node_enter(&mut self, node: &Node<'_>) {
        let class = node.as_class_node().expect("matched variant");
        self.add_constant_path_tokens(&class.constant_path(), TokenType::Class, &[TokenModifier::Declaration]);
        if let Some(superclass) = class.superclass() {
            self.add_constant_path_tokens(&superclass, TokenType::Class, &[]);
        }
    }

    fn on_module_node_enter(&mut self, node: &Node<'_>) {
        let module = node.as_module_node().expect("matched variant");
        self.add_constant_path_tokens(&module.constant_path(), TokenType::Namespace, &[TokenModifier::Declaration]);
    }

    fn add_constant_path_tokens(&mut self, constant_path: &Node<'_>, token_type: TokenType, modifiers: &[TokenModifier]) {
        if constant_path.as_constant_read_node().is_some() {
            self.add_location_token(&constant_path.location(), token_type, modifiers);
            return;
        }

        let mut name_locations = Vec::new();
        each_constant_path_part(constant_path, |part| {
            if let Some(path) = part.as_constant_path_node() {
                let name_location = path.name_loc();
                name_locations.push((name_location.start_offset(), name_location.end_offset()));
            }
        });
        for (start_offset, end_offset) in name_locations {
            self.response_builder.add_token(start_offset, end_offset, token_type, modifiers);
        }
    }

    fn process_regexp_locals(&mut self, match_write: &MatchWriteNode<'_>) {
        let Some(receiver) = match_write.call().receiver() else { return };
        let Some(regular_expression) = receiver.as_regular_expression_node() else { return };

        let content_location = regular_expression.content_loc();
        let content = location_slice(&content_location);

        for name in capture_names(match_write) {
            let Some(byte_index) = content.find(&format!("(?<{name}>")) else { continue };
            let capture_name_index = content[..byte_index].chars().count();
            let start_offset = content_location.start_offset() + capture_name_index + 3;
            let end_offset = start_offset + name.chars().count();

            let token_type = self.local_token_type(&name);
            self.response_builder.add_token(start_offset, end_offset, token_type, &[]);
        }
    }

    fn add_parameter(&mut self, name: String) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name);
        }
    }

    fn is_parameter(&self, name: &str) -> bool {
        self.scopes.iter().rev().any(|scope| scope.contains(name))
    }

    fn local_token_type(&self, name: &str) -> TokenType {
        if self.is_parameter(name) { TokenType::Parameter } else { TokenType::Variable }
    }

    fn add_parameter_token_if_parameter(&mut self, name: &str, name_location: &Location<'_>) {
        if self.is_parameter(name) {
            self.add_location_token(name_location, TokenType::Parameter, &[]);
        }
    }

    fn add_location_token(&mut self, location: &Location<'_>, token_type: TokenType, modifiers: &[TokenModifier]) {
        self.response_builder.add_token(location.start_offset(), location.end_offset(), token_type, modifiers);
    }
}

fn call_message(call: &CallNode<'_>) -> Option<String> {
    call.message_loc().map(|location| location_slice(&location))
}

fn numbered_parameter_like(name: &str) -> bool {
    name.as_bytes().windows(2).any(|pair| pair[0] == b'_' && pair[1].is_ascii_digit())
}

/// Ruby LSP reads the names with `Regexp#names`. Prism already extracted the same named captures into the match write
/// targets, which avoids depending on a Ruby compatible regular expression engine
fn capture_names(match_write: &MatchWriteNode<'_>) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for target in &match_write.targets() {
        let Some(target) = target.as_local_variable_target_node() else { continue };
        let name = constant_id_name(&target.name());
        if !names.contains(&name) {
            names.push(name);
        }
    }
    names
}
