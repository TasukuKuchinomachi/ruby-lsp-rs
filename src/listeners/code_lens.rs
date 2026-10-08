use ruby_prism::{CallNode, Node};
use serde_json::{Value, json};

use crate::document::ParsedDocument;
use crate::global_state::GlobalState;
use crate::prism::{child_nodes, constant_id_name, constant_name, location_slice, node_slice};
use crate::requests::support::common::namespace_constant_name;
use crate::requests::support::file_glob::fnmatch;
use crate::requests::support::test_command::{base_command, shell_escape};

const ACCESS_MODIFIERS: &[&str] = &["public", "private", "protected"];
const SUPPORTED_TEST_LIBRARIES: &[&str] = &["minitest", "test-unit"];
pub const DYNAMIC_REFERENCE_MARKER: &str = "<dynamic_reference>";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Group,
    Example,
}

impl Kind {
    fn as_str(self) -> &'static str {
        match self {
            Kind::Group => "group",
            Kind::Example => "example",
        }
    }
}

pub struct CodeLens<'a, 'b> {
    response: Vec<Value>,
    parsed: &'a ParsedDocument<'b>,
    global_state: &'a GlobalState,
    path: Option<String>,
    visibility_stack: Vec<(Option<String>, Option<String>)>,
    group_stack: Vec<String>,
    group_id: u32,
    group_id_stack: Vec<u32>,
    def_depth: u32,
    spec_id: u32,
}

impl<'a, 'b> CodeLens<'a, 'b> {
    pub fn new(parsed: &'a ParsedDocument<'b>, global_state: &'a GlobalState, uri: &str) -> CodeLens<'a, 'b> {
        CodeLens {
            response: Vec::new(),
            parsed,
            global_state,
            path: crate::uri::to_path(uri),
            visibility_stack: vec![(Some("public".to_string()), Some("public".to_string()))],
            group_stack: Vec::new(),
            group_id: 1,
            group_id_stack: Vec::new(),
            def_depth: 0,
            spec_id: 0,
        }
    }

    pub fn into_response(self) -> Vec<Value> {
        self.response
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
            Node::ClassNode { .. } => self.on_class_node_enter(node),
            Node::ModuleNode { .. } => self.on_module_node_enter(node),
            Node::DefNode { .. } => self.on_def_node_enter(node),
            Node::CallNode { .. } => self.on_call_node_enter(&node.as_call_node().expect("matched a call node"), node),
            _ => {}
        }
    }

    fn on_leave(&mut self, node: &Node<'_>) {
        match node {
            Node::ClassNode { .. } => self.on_class_node_leave(node),
            Node::ModuleNode { .. } => {
                self.group_stack.pop();
            }
            Node::DefNode { .. } => self.def_depth -= 1,
            Node::CallNode { .. } => self.on_call_node_leave(&node.as_call_node().expect("matched a call node")),
            _ => {}
        }
    }

    fn on_class_node_enter(&mut self, node: &Node<'_>) {
        let class = node.as_class_node().expect("matched a class node");
        self.visibility_stack.push((Some("public".to_string()), Some("public".to_string())));
        let class_name = node_slice(&class.constant_path());
        self.group_stack.push(class_name.clone());

        if self.path.is_some() && class_name.ends_with("Test") {
            let command = self.generate_test_command(None, None);
            let id = self.generate_fully_qualified_id(None);
            self.add_test_code_lens(node, &class_name, &command, Kind::Group, &id);
            self.group_id_stack.push(self.group_id);
            self.group_id += 1;
        }
    }

    fn on_class_node_leave(&mut self, node: &Node<'_>) {
        self.visibility_stack.pop();
        self.group_stack.pop();
        let class = node.as_class_node().expect("matched a class node");
        if self.path.is_some() && node_slice(&class.constant_path()).ends_with("Test") {
            self.group_id_stack.pop();
        }
    }

    fn on_def_node_enter(&mut self, node: &Node<'_>) {
        self.def_depth += 1;
        if self.def_depth > 1 {
            return;
        }

        if !self.group_stack.last().is_some_and(|class_name| class_name.ends_with("Test")) {
            return;
        }

        let visibility = self.visibility_stack.last().and_then(|(visibility, _)| visibility.clone());
        if visibility.as_deref() != Some("public") {
            return;
        }

        let method_name = constant_id_name(&node.as_def_node().expect("matched a def node").name());
        if self.path.is_some() && method_name.starts_with("test_") {
            let command = self.generate_test_command(None, Some(&method_name));
            let id = self.generate_fully_qualified_id(Some(&method_name));
            self.add_test_code_lens(node, &method_name, &command, Kind::Example, &id);
        }
    }

    fn on_module_node_enter(&mut self, node: &Node<'_>) {
        let name = namespace_constant_name(node).unwrap_or_else(|| DYNAMIC_REFERENCE_MARKER.to_string());
        self.group_stack.push(name);
    }

    fn on_call_node_enter(&mut self, call: &CallNode<'_>, node: &Node<'_>) {
        let name = constant_id_name(&call.name());

        if ACCESS_MODIFIERS.contains(&name.as_str()) {
            match call.arguments() {
                None => {
                    self.visibility_stack.pop();
                    self.visibility_stack.push((Some(name.clone()), Some(name)));
                }
                Some(arguments) => {
                    if arguments.arguments().iter().next().is_some_and(|argument| argument.as_def_node().is_some()) {
                        let visibility = self.visibility_stack.pop().and_then(|(visibility, _)| visibility);
                        self.visibility_stack.push((Some(name), visibility));
                    }
                }
            }
            return;
        }

        match name.as_str() {
            "describe" => {
                self.add_spec_code_lens(call, node, Kind::Group);
                self.group_id_stack.push(self.group_id);
                self.group_id += 1;
            }
            "it" | "specify" => self.add_spec_code_lens(call, node, Kind::Example),
            _ => {}
        }
    }

    fn on_call_node_leave(&mut self, call: &CallNode<'_>) {
        let previous_visibility = self.visibility_stack.pop().and_then(|(_, previous_visibility)| previous_visibility);
        self.visibility_stack.push((previous_visibility.clone(), previous_visibility));
        if constant_id_name(&call.name()) == "describe" {
            self.group_id_stack.pop();
            self.group_stack.pop();
        }
    }

    fn add_test_code_lens(&mut self, node: &Node<'_>, name: &str, command: &str, kind: Kind, id: &str) {
        if !SUPPORTED_TEST_LIBRARIES.contains(&self.global_state.test_library.as_str()) {
            return;
        }
        let Some(path) = &self.path else { return };

        let range = self.parsed.range_from_node(node);
        let arguments = json!([
            path,
            id,
            command,
            {
                "start_line": range.start.line,
                "start_column": range.start.character,
                "end_line": range.end.line,
                "end_column": range.end.character,
            },
            name,
        ]);

        let mut grouping_data = json!({ "group_id": self.group_id_stack.last(), "kind": kind.as_str() });
        if kind == Kind::Group {
            grouping_data["id"] = json!(self.group_id);
        }

        for (title, command_name, data_type) in [
            ("▶ Run", "rubyLsp.runTest", "test"),
            ("▶ Run In Terminal", "rubyLsp.runTestInTerminal", "test_in_terminal"),
            ("Debug", "rubyLsp.debugTest", "debug"),
        ] {
            let mut data = json!({ "type": data_type });
            if let (Some(data), Some(grouping_data)) = (data.as_object_mut(), grouping_data.as_object()) {
                data.extend(grouping_data.clone());
            }
            self.response.push(json!({
                "range": range,
                "command": { "title": title, "command": command_name, "arguments": arguments },
                "data": data,
            }));
        }
    }

    fn generate_test_command(&self, spec_name: Option<&str>, method_name: Option<&str>) -> String {
        let path = self.path.as_deref().unwrap_or_default();
        let mut command = base_command(&self.global_state.workspace_path()).to_string();
        if fnmatch("**/test/**/*", path) {
            command.push_str(" -Itest");
        }
        if fnmatch("**/spec/**/*", path) {
            command.push_str(" -Ispec");
        }
        command.push_str(&format!(" {path}"));

        match self.global_state.test_library.as_str() {
            "minitest" => command.push_str(&self.generate_minitest_command(method_name, spec_name)),
            "test-unit" => command.push_str(&self.generate_test_unit_command(method_name)),
            _ => {}
        }
        command
    }

    fn generate_minitest_command(&self, method_name: Option<&str>, spec_name: Option<&str>) -> String {
        let last_dynamic_reference_index = self.group_stack.iter().rposition(|group| group == DYNAMIC_REFERENCE_MARKER);

        if let Some(last_dynamic_reference_index) = last_dynamic_reference_index {
            let dynamic_stack = self.group_stack[last_dynamic_reference_index + 1..].join("::");
            return match method_name {
                Some(method_name) => {
                    format!(" --name /::{}#{}$/", shell_escape(&dynamic_stack), shell_escape(method_name))
                }
                None => format!(" --name \"/::{}(#|::)/\"", shell_escape(&dynamic_stack)),
            };
        }

        let group_name = self.group_stack.join("::");
        if let Some(method_name) = method_name {
            format!(" --name {}#{}", shell_escape(&group_name), shell_escape(method_name))
        } else if let Some(spec_name) = spec_name {
            format!(" --name \"/^{}#{}$/\"", shell_escape(&group_name), shell_escape(spec_name))
        } else {
            format!(" --name \"/^{}(#|::)/\"", shell_escape(&group_name))
        }
    }

    fn generate_test_unit_command(&self, method_name: Option<&str>) -> String {
        let group_name = self.group_stack.last().cloned().unwrap_or_default();
        let mut command = format!(" --testcase /{}/", shell_escape(&group_name));
        if let Some(method_name) = method_name {
            command.push_str(&format!(" --name {}", shell_escape(method_name)));
        }
        command
    }

    fn add_spec_code_lens(&mut self, call: &CallNode<'_>, node: &Node<'_>, kind: Kind) {
        let Some(arguments) = call.arguments() else { return };
        let Some(first_argument) = arguments.arguments().iter().next() else { return };

        let name = if let Some(string) = first_argument.as_string_node() {
            Some(location_slice(&string.content_loc()))
        } else if first_argument.as_constant_read_node().is_some() || first_argument.as_constant_path_node().is_some() {
            constant_name(&first_argument)
        } else {
            None
        };
        let Some(name) = name else { return };

        if kind == Kind::Example {
            self.spec_id += 1;
        } else {
            self.spec_id = 0;
            self.group_stack.push(name.clone());
        }

        if self.path.is_some() {
            let method_name = (kind == Kind::Example).then(|| format!("test_{:04}_{name}", self.spec_id));
            let command = self.generate_test_command(method_name.as_deref(), None);
            let id = self.generate_fully_qualified_id(method_name.as_deref());
            self.add_test_code_lens(node, &name, &command, kind, &id);
        }
    }

    fn generate_fully_qualified_id(&self, method_name: Option<&str>) -> String {
        let group_name = self.group_stack.join("::");
        match method_name {
            Some(method_name) => format!("{group_name}#{method_name}"),
            None => group_name,
        }
    }
}
