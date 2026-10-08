use indexmap::{IndexMap, IndexSet};
use ruby_prism::{CallNode, ClassNode, Node};
use serde_json::Value;

use crate::indexer::Index;
use crate::listeners::test_discovery::{DYNAMIC_REFERENCE_MARKER, DiscoveryContext, TestDiscovery};
use crate::prism::constant_id_name;
use crate::requests::support::file_glob::{fnmatch, glob};
use crate::requests::support::test_command::{shell_escape, test_command};
use crate::requests::support::test_item::TestItem;

const ACCESS_MODIFIERS: &[&str] = &["public", "private", "protected"];

pub struct TestStyle {
    base: TestDiscovery,
    framework: &'static str,
    parent_stack: Vec<Option<Vec<String>>>,
}

impl TestStyle {
    pub fn new() -> TestStyle {
        TestStyle { base: TestDiscovery::new(), framework: "minitest", parent_stack: vec![Some(Vec::new())] }
    }

    pub fn on_enter(&mut self, node: &Node<'_>, context: &mut DiscoveryContext<'_, '_>) {
        match node {
            Node::ClassNode { .. } => self.on_class_node_enter(&node.as_class_node().expect("matched a class node"), node, context),
            Node::ModuleNode { .. } => {
                self.parent_stack.push(None);
                self.base.on_module_node_enter(&node.as_module_node().expect("matched a module node"));
            }
            Node::DefNode { .. } => self.on_def_node_enter(node, context),
            Node::CallNode { .. } => {
                let name = constant_id_name(&node.as_call_node().expect("matched a call node").name());
                if ACCESS_MODIFIERS.contains(&name.as_str()) {
                    self.base.visibility_stack.push(name);
                }
            }
            _ => {}
        }
    }

    pub fn on_leave(&mut self, node: &Node<'_>) {
        match node {
            Node::ClassNode { .. } => {
                self.parent_stack.pop();
                self.base.on_class_node_leave();
            }
            Node::ModuleNode { .. } => {
                self.parent_stack.pop();
                self.base.on_module_node_leave();
            }
            Node::CallNode { .. } => self.on_call_node_leave(&node.as_call_node().expect("matched a call node")),
            _ => {}
        }
    }

    fn on_class_node_enter(&mut self, class: &ClassNode<'_>, node: &Node<'_>, context: &mut DiscoveryContext<'_, '_>) {
        let tracked_class = self.base.start_test_ancestor_tracking(class, context.index);
        let name = &tracked_class.fully_qualified_name;
        let ancestors = &tracked_class.attached_ancestors;

        if ancestors.iter().any(|ancestor| ancestor == "Test::Unit::TestCase") {
            self.framework = "test_unit";
        }

        if self.framework == "test_unit" || non_declarative_minitest(context.index, ancestors, name) {
            let test_item =
                TestItem::new(name.clone(), name.clone(), context.uri, context.parsed.range_from_node(node), self.framework);
            let parent_path = self.last_test_group();
            let item_path = context.collection.add_at(&parent_path, test_item);
            self.parent_stack.push(item_path);
        } else {
            self.parent_stack.push(None);
        }

        self.base.finish_test_ancestor_tracking(tracked_class);
    }

    fn on_def_node_enter(&mut self, node: &Node<'_>, context: &mut DiscoveryContext<'_, '_>) {
        if self.base.visibility_stack.last().map(String::as_str) != Some("public") {
            return;
        }

        let name = constant_id_name(&node.as_def_node().expect("matched a def node").name());
        if !name.starts_with("test_") {
            return;
        }

        let current_group_name = self.base.calc_fully_qualified_name(None);
        let Some(Some(parent_path)) = self.parent_stack.last() else { return };
        if parent_path.is_empty() {
            return;
        }

        let example_item = TestItem::new(
            format!("{current_group_name}#{name}"),
            name,
            context.uri,
            context.parsed.range_from_node(node),
            self.framework,
        );
        context.collection.add_at(parent_path, example_item);
    }

    fn on_call_node_leave(&mut self, node: &CallNode<'_>) {
        let name = constant_id_name(&node.name());
        if !ACCESS_MODIFIERS.contains(&name.as_str()) || node.arguments().is_none() {
            return;
        }
        self.base.visibility_stack.pop();
    }

    fn last_test_group(&self) -> Vec<String> {
        self.parent_stack.iter().rev().find_map(Clone::clone).unwrap_or_default()
    }
}

impl Default for TestStyle {
    fn default() -> Self {
        TestStyle::new()
    }
}

fn non_declarative_minitest(index: &mut Index, attached_ancestors: &[String], fully_qualified_name: &str) -> bool {
    if !attached_ancestors.iter().any(|ancestor| ancestor == "Minitest::Test") {
        return false;
    }

    let mut name_parts: Vec<&str> = fully_qualified_name.split("::").collect();
    while name_parts.last() == Some(&"") {
        name_parts.pop();
    }
    let singleton_name = format!("{}::<Class:{}>", name_parts.join("::"), name_parts.last().unwrap_or(&""));
    match index.linearized_ancestors_of(&singleton_name) {
        Ok(ancestors) => !ancestors.iter().any(|ancestor| ancestor == "ActiveSupport::Testing::Declarative"),
        Err(_) => true,
    }
}

#[derive(Default)]
struct AggregatedGroup {
    tags: IndexSet<String>,
    examples: Vec<String>,
}

fn string_tags(item: &Value) -> IndexSet<String> {
    item["tags"].as_array().map(|tags| tags.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default()
}

fn is_spec(path: &str) -> bool {
    fnmatch("**/spec/**/*_spec.rb", path)
}

pub fn resolve_test_commands(items: &[Value], workspace_path: &str) -> Vec<String> {
    let command = test_command(workspace_path);
    let mut aggregated_tests: IndexMap<String, IndexMap<String, AggregatedGroup>> = IndexMap::new();
    let mut full_files: Vec<String> = Vec::new();
    let mut queue: std::collections::VecDeque<Value> = items.iter().cloned().collect();

    while let Some(item) = queue.pop_front() {
        let tags = string_tags(&item);
        if !tags.contains("framework:minitest") && !tags.contains("framework:test_unit") {
            continue;
        }

        let children: Vec<Value> = item["children"].as_array().cloned().unwrap_or_default();
        let Some(path) = item["uri"].as_str().and_then(crate::uri::to_path) else { continue };
        let id = item["id"].as_str().unwrap_or_default().to_string();

        if tags.contains("test_dir") {
            if children.is_empty() {
                let pattern = format!("{path}/**/{{*_test,test_*,*_spec}}.rb");
                full_files.extend(glob(&pattern, std::path::Path::new("/")).iter().map(|file| shell_escape(file)));
            }
        } else if tags.contains("test_file") {
            if children.is_empty() {
                full_files.push(shell_escape(&path));
            }
        } else if tags.contains("test_group") {
            let all_children_are_groups =
                !children.is_empty() && children.iter().all(|child| string_tags(child).contains("test_group"));
            if !all_children_are_groups {
                aggregated_tests
                    .entry(path.clone())
                    .or_default()
                    .insert(id.clone(), AggregatedGroup { tags: tags.clone(), examples: Vec::new() });
            }
        } else {
            let mut parts = id.split('#');
            let class_name = parts.next().unwrap_or_default().to_string();
            let method_name = parts.next().unwrap_or_default().to_string();
            let group = aggregated_tests.entry(path.clone()).or_default().entry(class_name).or_default();
            group.examples.push(method_name);
            group.tags.extend(tags.iter().cloned());
        }

        queue.extend(children);
    }

    let mut commands = Vec::new();

    for (file_path, groups_and_examples) in &aggregated_tests {
        let (minitest_groups, test_unit_groups): (Vec<_>, Vec<_>) =
            groups_and_examples.iter().partition(|(_, info)| info.tags.contains("framework:minitest"));

        if !minitest_groups.is_empty() {
            commands.push(handle_minitest_groups(&command, file_path, &minitest_groups));
        }
        if !test_unit_groups.is_empty() {
            commands.extend(handle_test_unit_groups(&command, file_path, &test_unit_groups));
        }
    }

    if !full_files.is_empty() {
        let (specs, tests): (Vec<String>, Vec<String>) = full_files.into_iter().partition(|path| is_spec(path));
        if !tests.is_empty() {
            commands.push(format!("{command} -Itest -e \"ARGV.each {{ |f| require f }}\" {}", tests.join(" ")));
        }
        if !specs.is_empty() {
            commands.push(format!("{command} -Ispec -e \"ARGV.each {{ |f| require f }}\" {}", specs.join(" ")));
        }
    }

    commands
}

fn group_regex(group: &str) -> String {
    shell_escape(group).replace(&shell_escape(DYNAMIC_REFERENCE_MARKER), ".*")
}

fn replace_example_numbers(example: &str) -> String {
    let characters: Vec<char> = example.chars().collect();
    let mut result = String::with_capacity(example.len());
    let mut index = 0;
    while index < characters.len() {
        let is_numbered_example = characters[index..].starts_with(&['t', 'e', 's', 't', '_'])
            && characters.len() >= index + 9
            && characters[index + 5..index + 9].iter().all(char::is_ascii_digit);
        if is_numbered_example {
            result.push_str("test_\\d{4}");
            index += 9;
        } else {
            result.push(characters[index]);
            index += 1;
        }
    }
    result
}

fn handle_minitest_groups(command: &str, file_path: &str, groups_and_examples: &[(&String, &AggregatedGroup)]) -> String {
    let regexes: Vec<String> = groups_and_examples
        .iter()
        .map(|(group, info)| {
            let examples: Vec<String> =
                info.examples.iter().map(|example| replace_example_numbers(&shell_escape(example))).collect();
            let group_regex = group_regex(group);
            match examples.len() {
                0 => format!("^{group_regex}(#|::)"),
                1 => format!("^{group_regex}#{}\\$", examples[0]),
                _ => format!("^{group_regex}#({})\\$", examples.join("|")),
            }
        })
        .collect();

    let regex = if regexes.len() == 1 { regexes[0].clone() } else { format!("({})", regexes.join("|")) };
    let load_path = if is_spec(file_path) { "-Ispec" } else { "-Itest" };
    format!("{command} {load_path} {} --name \"/{regex}/\"", shell_escape(file_path))
}

fn handle_test_unit_groups(
    command: &str,
    file_path: &str,
    groups_and_examples: &[(&String, &AggregatedGroup)],
) -> Vec<String> {
    groups_and_examples
        .iter()
        .map(|(group, info)| {
            let mut group_command =
                format!("{command} -Itest {} --testcase \"/^{}\\$/\"", shell_escape(file_path), group_regex(group));
            match info.examples.len() {
                0 => {}
                1 => group_command.push_str(&format!(" --name \"/{}\\$/\"", info.examples[0])),
                _ => group_command.push_str(&format!(" --name \"/({})\\$/\"", info.examples.join("|"))),
            }
            group_command
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_example_numbers() {
        assert_eq!(replace_example_numbers("test_0001_something"), "test_\\d{4}_something");
        assert_eq!(replace_example_numbers("test_something"), "test_something");
    }
}
