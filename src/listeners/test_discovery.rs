use ruby_prism::{ClassNode, ModuleNode, Node};

use crate::document::ParsedDocument;
use crate::indexer::Index;
use crate::indexer::index::actual_nesting;
use crate::listeners::spec_style::SpecStyle;
use crate::listeners::test_style::TestStyle;
use crate::prism::{child_nodes, constant_name, node_slice};
use crate::requests::support::test_collection::TestCollection;

pub const DYNAMIC_REFERENCE_MARKER: &str = "<dynamic_reference>";

pub struct DiscoveryContext<'a, 'b> {
    pub parsed: &'a ParsedDocument<'b>,
    pub index: &'a mut Index,
    pub collection: &'a mut TestCollection,
    pub uri: &'a str,
}

pub struct TestDiscovery {
    pub visibility_stack: Vec<String>,
    pub nesting: Vec<String>,
}

pub struct TrackedClass {
    pub name: String,
    pub fully_qualified_name: String,
    pub attached_ancestors: Vec<String>,
}

impl TestDiscovery {
    pub fn new() -> TestDiscovery {
        TestDiscovery { visibility_stack: vec!["public".to_string()], nesting: Vec::new() }
    }

    pub fn on_module_node_enter(&mut self, node: &ModuleNode<'_>) {
        self.visibility_stack.push("public".to_string());
        let constant_path = node.constant_path();
        let name = constant_name(&constant_path).unwrap_or_else(|| name_with_dynamic_reference(&node_slice(&constant_path)));
        self.nesting.push(name);
    }

    pub fn on_module_node_leave(&mut self) {
        self.visibility_stack.pop();
        self.nesting.pop();
    }

    pub fn on_class_node_leave(&mut self) {
        self.visibility_stack.pop();
        self.nesting.pop();
    }

    pub fn calc_fully_qualified_name(&self, name: Option<&str>) -> String {
        actual_nesting(&self.nesting, name).join("::")
    }

    fn calc_attached_ancestors(node: &ClassNode<'_>, fully_qualified_name: &str, index: &mut Index) -> Vec<String> {
        let superclass = node.superclass().map(|superclass| node_slice(&superclass));
        match index.linearized_ancestors_of(fully_qualified_name) {
            Ok(ancestors) => {
                if ancestors.len() > 1 {
                    return ancestors;
                }
                if ancestors.first().map(String::as_str) == Some(fully_qualified_name)
                    && let Some(superclass) = superclass
                {
                    let mut ancestors = ancestors;
                    ancestors.push(superclass);
                    return ancestors;
                }
                ancestors
            }
            Err(_) => superclass.into_iter().collect(),
        }
    }

    /// Ruby LSP yields to a block between computing the ancestors and pushing the nesting. The caller does its work
    /// between this call and `finish_test_ancestor_tracking`
    pub fn start_test_ancestor_tracking(&mut self, node: &ClassNode<'_>, index: &mut Index) -> TrackedClass {
        self.visibility_stack.push("public".to_string());
        let constant_path = node.constant_path();
        let name = constant_name(&constant_path).unwrap_or_else(|| name_with_dynamic_reference(&node_slice(&constant_path)));
        let fully_qualified_name = self.calc_fully_qualified_name(Some(&name));
        let attached_ancestors = TestDiscovery::calc_attached_ancestors(node, &fully_qualified_name, index);
        TrackedClass { name, fully_qualified_name, attached_ancestors }
    }

    pub fn finish_test_ancestor_tracking(&mut self, tracked_class: TrackedClass) {
        self.nesting.push(tracked_class.name);
    }
}

impl Default for TestDiscovery {
    fn default() -> Self {
        TestDiscovery::new()
    }
}

/// Written by hand because the `regex` crate does not support the lookbehind of `/((?<=::)|^)[a-z]\w*/`
pub fn name_with_dynamic_reference(slice: &str) -> String {
    let characters: Vec<char> = slice.chars().collect();
    let mut result = String::with_capacity(slice.len());
    let mut index = 0;
    while index < characters.len() {
        let at_segment_start = index == 0
            || characters[index - 1] == '\n'
            || (index >= 2 && characters[index - 1] == ':' && characters[index - 2] == ':');
        if at_segment_start && characters[index].is_ascii_lowercase() {
            index += 1;
            while index < characters.len() && (characters[index].is_ascii_alphanumeric() || characters[index] == '_') {
                index += 1;
            }
            result.push_str(DYNAMIC_REFERENCE_MARKER);
            continue;
        }
        result.push(characters[index]);
        index += 1;
    }
    result
}

pub struct TestDiscoveryVisitor {
    test_style: TestStyle,
    spec_style: SpecStyle,
}

impl TestDiscoveryVisitor {
    pub fn new() -> TestDiscoveryVisitor {
        TestDiscoveryVisitor { test_style: TestStyle::new(), spec_style: SpecStyle::new() }
    }

    pub fn visit(&mut self, node: &Node<'_>, context: &mut DiscoveryContext<'_, '_>) {
        self.test_style.on_enter(node, context);
        self.spec_style.on_enter(node, context);
        for child in child_nodes(node) {
            self.visit(&child, context);
        }
        self.test_style.on_leave(node);
        self.spec_style.on_leave(node);
    }
}

impl Default for TestDiscoveryVisitor {
    fn default() -> Self {
        TestDiscoveryVisitor::new()
    }
}

pub fn discover(parsed: &ParsedDocument<'_>, index: &mut Index, uri: &str) -> TestCollection {
    let mut collection = TestCollection::new();
    let mut context = DiscoveryContext { parsed, index, collection: &mut collection, uri };
    TestDiscoveryVisitor::new().visit(&parsed.ast(), &mut context);
    collection
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_dynamic_references() {
        assert_eq!(name_with_dynamic_reference("var::Namespace"), "<dynamic_reference>::Namespace");
        assert_eq!(name_with_dynamic_reference("Foo::bar::Baz"), "Foo::<dynamic_reference>::Baz");
        assert_eq!(name_with_dynamic_reference("Foo::Bar"), "Foo::Bar");
    }
}
