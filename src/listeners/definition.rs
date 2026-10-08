use std::path::{Component, Path, PathBuf};

use ruby_prism::{CallNode, Node};

use crate::document::LanguageId;
use crate::global_state::GlobalState;
use crate::indexer::Index;
use crate::node_context::NodeContext;
use crate::position::{Position, Range};
use crate::prism::{NodeExt, bytes_to_string, constant_id_name, constant_name, location_slice};
use crate::requests::support::common::{range_from_location, self_receiver};
use crate::requests::support::interface::{Location, LocationLink, LocationOrLink};
use crate::requests::support::sorbet_level::SorbetLevel;
use crate::type_inferrer::{Type, TypeInferrer};

pub const MAX_NUMBER_OF_DEFINITION_CANDIDATES_WITHOUT_RECEIVER: usize = 10;

pub struct Definition<'a, 'pr> {
    response: Vec<LocationOrLink>,
    global_state: &'a GlobalState,
    index: &'a mut Index,
    language_id: LanguageId,
    uri: &'a str,
    node_context: &'a NodeContext<'pr>,
    sorbet_level: SorbetLevel,
}

impl<'a, 'pr> Definition<'a, 'pr> {
    pub fn new(
        global_state: &'a GlobalState,
        index: &'a mut Index,
        language_id: LanguageId,
        uri: &'a str,
        node_context: &'a NodeContext<'pr>,
        sorbet_level: SorbetLevel,
    ) -> Definition<'a, 'pr> {
        Definition { response: Vec::new(), global_state, index, language_id, uri, node_context, sorbet_level }
    }

    pub fn into_response(self) -> Vec<LocationOrLink> {
        self.response
    }

    pub fn dispatch(&mut self, target: &Node<'_>) {
        match target.type_name() {
            "CallNode" => self.on_call_node_enter(&target.as_call_node().expect("type name was checked")),
            "StringNode" => self.on_string_node_enter(target),
            "SymbolNode" => self.on_symbol_node_enter(target),
            "BlockArgumentNode" => self.on_block_argument_node_enter(target),
            "ConstantPathNode" | "ConstantReadNode" => {
                if let Some(name) = constant_name(target) {
                    self.find_in_index(&name);
                }
            }
            "GlobalVariableAndWriteNode"
            | "GlobalVariableOperatorWriteNode"
            | "GlobalVariableOrWriteNode"
            | "GlobalVariableReadNode"
            | "GlobalVariableTargetNode"
            | "GlobalVariableWriteNode" => {
                if let Some(name) = variable_name(target) {
                    self.handle_global_variable_definition(&name);
                }
            }
            "InstanceVariableReadNode"
            | "InstanceVariableWriteNode"
            | "InstanceVariableAndWriteNode"
            | "InstanceVariableOperatorWriteNode"
            | "InstanceVariableOrWriteNode"
            | "InstanceVariableTargetNode" => {
                if let Some(name) = variable_name(target) {
                    self.handle_instance_variable_definition(&name);
                }
            }
            "SuperNode" | "ForwardingSuperNode" => self.handle_super_node_definition(),
            "ClassVariableAndWriteNode"
            | "ClassVariableOperatorWriteNode"
            | "ClassVariableOrWriteNode"
            | "ClassVariableTargetNode"
            | "ClassVariableReadNode"
            | "ClassVariableWriteNode" => {
                if let Some(name) = variable_name(target) {
                    self.handle_class_variable_definition(&name);
                }
            }
            _ => {}
        }
    }

    fn on_call_node_enter(&mut self, node: &CallNode<'_>) {
        if self.sorbet_level.is_true_or_higher() && self_receiver(node) {
            return;
        }

        let Some(message) = node.message_loc().map(|location| location_slice(&location)) else { return };

        let mut receiver_type = TypeInferrer::new(self.index).infer_receiver_type(self.node_context);
        if self.language_id == LanguageId::Erb && receiver_type.as_ref().is_some_and(|receiver| receiver.name == "Object") {
            receiver_type = None;
        }

        self.handle_method_definition(&message, receiver_type.as_ref(), false);
    }

    fn on_string_node_enter(&mut self, node: &Node<'_>) {
        let Some(enclosing_call) = self.node_context.call_node() else { return };
        let string = node.as_string_node().expect("type name was checked");
        match constant_id_name(&enclosing_call.name()).as_str() {
            "require" => self.handle_require_definition(&location_slice(&string.content_loc())),
            "require_relative" => self.handle_require_relative_definition(&location_slice(&string.content_loc())),
            "send" | "public_send" => {
                let method_name = location_slice(&string.content_loc());
                self.handle_send_or_public_send_definition(&enclosing_call, node, &method_name);
            }
            _ => {}
        }
    }

    fn on_symbol_node_enter(&mut self, node: &Node<'_>) {
        let Some(enclosing_call) = self.node_context.call_node() else { return };
        let symbol = node.as_symbol_node().expect("type name was checked");
        match constant_id_name(&enclosing_call.name()).as_str() {
            "autoload" => self.handle_autoload_definition(&enclosing_call),
            "send" | "public_send" => {
                let method_name = bytes_to_string(symbol.unescaped());
                self.handle_send_or_public_send_definition(&enclosing_call, node, &method_name);
            }
            _ => {}
        }
    }

    fn on_block_argument_node_enter(&mut self, node: &Node<'_>) {
        let block_argument = node.as_block_argument_node().expect("type name was checked");
        let Some(expression) = block_argument.expression() else { return };
        let Some(symbol) = expression.as_symbol_node() else { return };
        let Some(value) = symbol.value_loc().map(|location| location_slice(&location)) else { return };
        self.handle_method_definition(&value, None, false);
    }

    fn handle_send_or_public_send_definition(&mut self, enclosing_call: &CallNode<'_>, node: &Node<'_>, method_name: &str) {
        let first_argument = enclosing_call.arguments().and_then(|arguments| arguments.arguments().iter().next());
        if !first_argument.is_some_and(|argument| argument.same_node(node)) {
            return;
        }
        self.handle_method_definition(method_name, None, false);
    }

    fn handle_super_node_definition(&mut self) {
        if self.sorbet_level.is_true_or_higher() {
            return;
        }
        let Some(surrounding_method) = self.node_context.surrounding_method.clone() else { return };
        let receiver_type = TypeInferrer::new(self.index).infer_receiver_type(self.node_context);
        self.handle_method_definition(&surrounding_method, receiver_type.as_ref(), true);
    }

    fn push_location(&mut self, uri: &str, range: Range) {
        self.response.push(LocationOrLink::Location(Location { uri: uri.to_string(), range }));
    }

    fn handle_global_variable_definition(&mut self, name: &str) {
        let Some(ids) = self.index.get_owned(name) else { return };
        for id in ids {
            let entry = self.index.entry(id);
            let (uri, range) = (entry.uri.to_string(), range_from_location(&entry.location));
            self.push_location(&uri, range);
        }
    }

    fn handle_class_variable_definition(&mut self, name: &str) {
        let Some(receiver_type) = TypeInferrer::new(self.index).infer_receiver_type(self.node_context) else { return };
        let Some(ids) = self.index.resolve_class_variable(name, &receiver_type.name) else { return };
        for id in ids {
            let entry = self.index.entry(id);
            let (uri, range) = (entry.uri.to_string(), range_from_location(&entry.location));
            self.push_location(&uri, range);
        }
    }

    fn handle_instance_variable_definition(&mut self, name: &str) {
        if self.sorbet_level.is_strict() {
            return;
        }
        let Some(receiver_type) = TypeInferrer::new(self.index).infer_receiver_type(self.node_context) else { return };
        let Some(ids) = self.index.resolve_instance_variable(name, &receiver_type.name) else { return };
        for id in ids {
            let entry = self.index.entry(id);
            let (uri, range) = (entry.uri.to_string(), range_from_location(&entry.location));
            self.push_location(&uri, range);
        }
    }

    fn handle_method_definition(&mut self, message: &str, receiver_type: Option<&Type>, inherited_only: bool) {
        let mut methods = receiver_type.and_then(|receiver| self.index.resolve_method(message, &receiver.name, inherited_only));

        if receiver_type.is_none_or(|receiver| receiver.guessed && methods.is_none()) {
            methods = self.index.get(message).map(|ids| {
                ids.iter().take(MAX_NUMBER_OF_DEFINITION_CANDIDATES_WITHOUT_RECEIVER).copied().collect()
            });
        }

        let Some(methods) = methods else { return };

        for id in methods {
            let entry = self.index.entry(id);
            let full_path = crate::uri::to_path(&entry.uri);
            if self.sorbet_level.is_true_or_higher()
                && full_path.as_deref().is_none_or(|path| self.global_state.not_in_dependencies(path))
            {
                continue;
            }

            self.response.push(LocationOrLink::Link(LocationLink {
                target_uri: entry.uri.to_string(),
                target_range: range_from_location(&entry.location),
                target_selection_range: range_from_location(&entry.name_location()),
            }));
        }
    }

    fn handle_require_definition(&mut self, content: &str) {
        let candidate = self
            .index
            .search_require_paths(content)
            .into_iter()
            .find(|(require_path, _)| require_path.as_ref() == content)
            .and_then(|(_, uri)| crate::uri::to_path(&uri));

        if let Some(candidate) = candidate {
            self.push_location(&crate::uri::from_path(&candidate), file_start_range());
        }
    }

    fn handle_require_relative_definition(&mut self, content: &str) {
        let required_file = format!("{content}.rb");
        let current_folder = match crate::uri::to_path(self.uri) {
            Some(path) => Path::new(&path).parent().map(Path::to_path_buf).unwrap_or_default(),
            None => PathBuf::from(self.global_state.workspace_path()),
        };
        let candidate = expand_path(&current_folder.join(required_file));
        self.push_location(&crate::uri::from_path_buf(&candidate), file_start_range());
    }

    fn handle_autoload_definition(&mut self, node: &CallNode<'_>) {
        let Some(argument) = node.arguments().and_then(|arguments| arguments.arguments().iter().next()) else { return };
        let Some(symbol) = argument.as_symbol_node() else { return };
        let Some(constant_name) = symbol.value_loc().map(|location| location_slice(&location)) else { return };
        self.find_in_index(&constant_name);
    }

    fn find_in_index(&mut self, value: &str) {
        let Some(ids) = self.index.resolve(value, &self.node_context.nesting) else { return };
        let Some(first) = ids.first() else { return };

        let first_entry = self.index.entry(*first);
        if first_entry.is_private() && first_entry.name != format!("{}::{value}", self.node_context.fully_qualified_name()) {
            return;
        }

        for id in ids {
            let entry = self.index.entry(id);
            let full_path = crate::uri::to_path(&entry.uri);
            if !self.sorbet_level.is_ignore()
                && full_path.as_deref().is_none_or(|path| self.global_state.not_in_dependencies(path))
            {
                continue;
            }

            self.response.push(LocationOrLink::Link(LocationLink {
                target_uri: entry.uri.to_string(),
                target_range: range_from_location(&entry.location),
                target_selection_range: range_from_location(&entry.name_location()),
            }));
        }
    }
}

fn file_start_range() -> Range {
    Range::new(Position::new(0, 0), Position::new(0, 0))
}

pub fn expand_path(path: &Path) -> PathBuf {
    let mut expanded = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                if !expanded.pop() {
                    expanded.push(Component::RootDir);
                }
            }
            Component::CurDir => {}
            other => expanded.push(other),
        }
    }
    if expanded.as_os_str().is_empty() {
        expanded.push(Component::RootDir);
    }
    expanded
}

pub fn variable_name(node: &Node<'_>) -> Option<String> {
    let name = match node {
        Node::GlobalVariableAndWriteNode { .. } => node.as_global_variable_and_write_node()?.name(),
        Node::GlobalVariableOperatorWriteNode { .. } => node.as_global_variable_operator_write_node()?.name(),
        Node::GlobalVariableOrWriteNode { .. } => node.as_global_variable_or_write_node()?.name(),
        Node::GlobalVariableReadNode { .. } => node.as_global_variable_read_node()?.name(),
        Node::GlobalVariableTargetNode { .. } => node.as_global_variable_target_node()?.name(),
        Node::GlobalVariableWriteNode { .. } => node.as_global_variable_write_node()?.name(),
        Node::InstanceVariableAndWriteNode { .. } => node.as_instance_variable_and_write_node()?.name(),
        Node::InstanceVariableOperatorWriteNode { .. } => node.as_instance_variable_operator_write_node()?.name(),
        Node::InstanceVariableOrWriteNode { .. } => node.as_instance_variable_or_write_node()?.name(),
        Node::InstanceVariableReadNode { .. } => node.as_instance_variable_read_node()?.name(),
        Node::InstanceVariableTargetNode { .. } => node.as_instance_variable_target_node()?.name(),
        Node::InstanceVariableWriteNode { .. } => node.as_instance_variable_write_node()?.name(),
        Node::ClassVariableAndWriteNode { .. } => node.as_class_variable_and_write_node()?.name(),
        Node::ClassVariableOperatorWriteNode { .. } => node.as_class_variable_operator_write_node()?.name(),
        Node::ClassVariableOrWriteNode { .. } => node.as_class_variable_or_write_node()?.name(),
        Node::ClassVariableReadNode { .. } => node.as_class_variable_read_node()?.name(),
        Node::ClassVariableTargetNode { .. } => node.as_class_variable_target_node()?.name(),
        Node::ClassVariableWriteNode { .. } => node.as_class_variable_write_node()?.name(),
        _ => return None,
    };
    Some(constant_id_name(&name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_paths_lexically() {
        assert_eq!(expand_path(Path::new("/../../lib/ruby_lsp/utils.rb")), PathBuf::from("/lib/ruby_lsp/utils.rb"));
        assert_eq!(expand_path(Path::new("/a/b/./../c.rb")), PathBuf::from("/a/c.rb"));
    }
}
