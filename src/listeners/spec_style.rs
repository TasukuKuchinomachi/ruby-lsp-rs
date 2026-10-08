use ruby_prism::{CallNode, ClassNode, Node};

use crate::listeners::test_discovery::{DiscoveryContext, TestDiscovery};
use crate::prism::{constant_id_name, constant_name, location_slice, node_slice};
use crate::requests::support::test_collection::TestCollection;
use crate::requests::support::test_item::TestItem;

#[derive(Debug, Clone)]
enum Group {
    Class(String),
    Describe(String),
}

impl Group {
    fn id(&self) -> &str {
        match self {
            Group::Class(id) | Group::Describe(id) => id,
        }
    }
}

pub struct SpecStyle {
    base: TestDiscovery,
    spec_group_id_stack: Vec<Option<Group>>,
}

impl SpecStyle {
    pub fn new() -> SpecStyle {
        SpecStyle { base: TestDiscovery::new(), spec_group_id_stack: Vec::new() }
    }

    pub fn on_enter(&mut self, node: &Node<'_>, context: &mut DiscoveryContext<'_, '_>) {
        match node {
            Node::ClassNode { .. } => self.on_class_node_enter(&node.as_class_node().expect("matched a class node"), context),
            Node::ModuleNode { .. } => {
                self.spec_group_id_stack.push(None);
                self.base.on_module_node_enter(&node.as_module_node().expect("matched a module node"));
            }
            Node::CallNode { .. } => {
                let call = node.as_call_node().expect("matched a call node");
                match constant_id_name(&call.name()).as_str() {
                    "describe" => self.handle_describe(&call, node, context),
                    "it" | "specify" if self.in_spec_context() => self.handle_example(&call, node, context),
                    _ => {}
                }
            }
            _ => {}
        }
    }

    pub fn on_leave(&mut self, node: &Node<'_>) {
        match node {
            Node::ClassNode { .. } => {
                self.spec_group_id_stack.pop();
                self.base.on_class_node_leave();
            }
            Node::ModuleNode { .. } => {
                self.spec_group_id_stack.pop();
                self.base.on_module_node_leave();
            }
            Node::CallNode { .. } => self.on_call_node_leave(&node.as_call_node().expect("matched a call node")),
            _ => {}
        }
    }

    fn on_class_node_enter(&mut self, class: &ClassNode<'_>, context: &mut DiscoveryContext<'_, '_>) {
        let tracked_class = self.base.start_test_ancestor_tracking(class, context.index);
        let group = tracked_class
            .attached_ancestors
            .iter()
            .any(|ancestor| ancestor == "Minitest::Spec")
            .then(|| Group::Class(tracked_class.fully_qualified_name.clone()));
        self.spec_group_id_stack.push(group);
        self.base.finish_test_ancestor_tracking(tracked_class);
    }

    fn on_call_node_leave(&mut self, call: &CallNode<'_>) {
        if constant_id_name(&call.name()) != "describe" || call.receiver().is_some() {
            return;
        }

        let Some(Some(Group::Describe(current_id))) = self.spec_group_id_stack.last() else { return };
        let Some(description) = extract_description(call) else { return };
        if current_id.ends_with(&description) {
            self.spec_group_id_stack.pop();
        }
    }

    fn handle_describe(&mut self, call: &CallNode<'_>, node: &Node<'_>, context: &mut DiscoveryContext<'_, '_>) {
        if call.block().is_none() {
            return;
        }
        let Some(description) = extract_description(call) else { return };
        let Some(parent_path) = self.latest_group(context.collection) else { return };

        let id = match context.collection.item_at(&parent_path) {
            Some(parent) => format!("{}::{description}", parent.id),
            None => description.clone(),
        };

        let test_item = TestItem::new(id.clone(), description, context.uri, context.parsed.range_from_node(node), "minitest");
        context.collection.add_at(&parent_path, test_item);
        self.spec_group_id_stack.push(Some(Group::Describe(id)));
    }

    fn handle_example(&mut self, call: &CallNode<'_>, node: &Node<'_>, context: &mut DiscoveryContext<'_, '_>) {
        let description = extract_it_description(call);
        let line = context.parsed.byte_position(node.location().start_offset()).line;
        let Some(parent_path) = self.latest_group(context.collection) else { return };
        let Some(parent) = context.collection.item_at(&parent_path) else { return };

        let id = format!("{}#test_{line:04}_{description}", parent.id);
        let test_item = TestItem::new(id, description, context.uri, context.parsed.range_from_node(node), "minitest");
        context.collection.add_at(&parent_path, test_item);
    }

    /// Ruby LSP raises when an intermediate group is missing. No group is returned instead, so that the request succeeds
    fn latest_group(&self, collection: &TestCollection) -> Option<Vec<String>> {
        let stack = &self.spec_group_id_stack;
        if stack.iter().all(Option::is_none) {
            return Some(Vec::new());
        }
        stack.last()?.as_ref()?;

        let is_class = |group: &Option<Group>| matches!(group, Some(Group::Class(_)));
        let is_describe = |group: &Option<Group>| matches!(group, Some(Group::Describe(_)));
        let mut path = Vec::new();

        if let Some(closest_class_index) = stack.iter().rposition(is_class) {
            let first_class_index = stack.iter().position(is_class).expect("a class group exists");
            path.push(stack[first_class_index].as_ref().expect("is a class group").id().to_string());
            for group in stack[first_class_index + 1..=closest_class_index].iter().flatten() {
                if let Group::Class(id) = group {
                    path.push(id.clone());
                }
            }
            for group in stack[closest_class_index + 1..].iter().flatten() {
                path.push(group.id().to_string());
            }
        } else {
            let first_group_index = stack.iter().position(is_describe)?;
            path.push(stack[first_group_index].as_ref().expect("is a describe group").id().to_string());
            for group in stack[first_group_index + 1..].iter().flatten() {
                if let Group::Describe(id) = group {
                    path.push(id.clone());
                }
            }
        }

        collection.item_at(&path).map(|_| path)
    }

    fn in_spec_context(&self) -> bool {
        self.base.nesting.is_empty() || self.spec_group_id_stack.iter().any(Option::is_some)
    }
}

impl Default for SpecStyle {
    fn default() -> Self {
        SpecStyle::new()
    }
}

fn extract_description(call: &CallNode<'_>) -> Option<String> {
    let arguments = call.arguments()?;
    let parts: Vec<String> =
        arguments.arguments().iter().map(|argument| extract_argument_content(&argument).unwrap_or_default()).collect();
    if parts.is_empty() {
        return None;
    }
    Some(parts.join("::"))
}

fn extract_it_description(call: &CallNode<'_>) -> String {
    call.arguments()
        .and_then(|arguments| arguments.arguments().iter().next())
        .and_then(|argument| extract_argument_content(&argument))
        .unwrap_or_else(|| "anonymous".to_string())
}

fn extract_argument_content(argument: &Node<'_>) -> Option<String> {
    if let Some(string) = argument.as_string_node() {
        Some(location_slice(&string.content_loc()))
    } else if let Some(symbol) = argument.as_symbol_node() {
        symbol.value_loc().map(|location| location_slice(&location))
    } else if argument.as_constant_read_node().is_some() || argument.as_constant_path_node().is_some() {
        constant_name(argument)
    } else {
        Some(node_slice(argument))
    }
}
