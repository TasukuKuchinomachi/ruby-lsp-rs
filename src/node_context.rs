use std::collections::VecDeque;

use ruby_prism::{CallNode, Node};

use crate::prism::{NodeExt, child_nodes, constant_id_name, node_slice};

const NESTING_NODE_TYPES: &[&str] =
    &["ClassNode", "ModuleNode", "SingletonClassNode", "DefNode", "BlockNode", "LambdaNode"];

pub struct NodeContext<'pr> {
    pub node: Option<Node<'pr>>,
    pub parent: Option<Node<'pr>>,
    pub nesting_nodes: Vec<Node<'pr>>,
    call_node: Option<Node<'pr>>,
    pub nesting: Vec<String>,
    pub surrounding_method: Option<String>,
}

impl<'pr> NodeContext<'pr> {
    pub fn new(
        node: Option<Node<'pr>>,
        parent: Option<Node<'pr>>,
        nesting_nodes: Vec<Node<'pr>>,
        call_node: Option<Node<'pr>>,
    ) -> NodeContext<'pr> {
        let (nesting, surrounding_method) = handle_nesting_nodes(&nesting_nodes);
        NodeContext { node, parent, nesting_nodes, call_node, nesting, surrounding_method }
    }

    pub fn call_node(&self) -> Option<CallNode<'pr>> {
        self.call_node.as_ref().and_then(|node| node.as_call_node())
    }

    pub fn call_node_as_node(&self) -> Option<&Node<'pr>> {
        self.call_node.as_ref()
    }

    pub fn fully_qualified_name(&self) -> String {
        self.nesting.join("::")
    }

    pub fn locals_for_scope(&self) -> Vec<String> {
        let mut locals = Vec::new();
        for node in &self.nesting_nodes {
            let node_locals = match node {
                Node::ClassNode { .. } => {
                    locals.clear();
                    node.as_class_node().map(|class| class.locals())
                }
                Node::ModuleNode { .. } => {
                    locals.clear();
                    node.as_module_node().map(|module| module.locals())
                }
                Node::SingletonClassNode { .. } => {
                    locals.clear();
                    node.as_singleton_class_node().map(|singleton| singleton.locals())
                }
                Node::DefNode { .. } => {
                    locals.clear();
                    node.as_def_node().map(|definition| definition.locals())
                }
                Node::BlockNode { .. } => node.as_block_node().map(|block| block.locals()),
                Node::LambdaNode { .. } => node.as_lambda_node().map(|lambda| lambda.locals()),
                Node::ProgramNode { .. } => node.as_program_node().map(|program| program.locals()),
                _ => None,
            };
            if let Some(node_locals) = node_locals {
                locals.extend(node_locals.iter().map(|local| constant_id_name(&local)));
            }
        }
        locals
    }
}

fn last_nesting_part(nesting: &[String]) -> String {
    nesting.iter().flat_map(|part| part.split("::")).last().unwrap_or("").to_string()
}

fn handle_nesting_nodes(nodes: &[Node<'_>]) -> (Vec<String>, Option<String>) {
    let mut nesting: Vec<String> = Vec::new();
    let mut surrounding_method = None;

    for node in nodes {
        if let Some(class) = node.as_class_node() {
            nesting.push(node_slice(&class.constant_path()));
        } else if let Some(module) = node.as_module_node() {
            nesting.push(node_slice(&module.constant_path()));
        } else if node.as_singleton_class_node().is_some() {
            nesting.push(format!("<Class:{}>", last_nesting_part(&nesting)));
        } else if let Some(definition) = node.as_def_node() {
            surrounding_method = Some(constant_id_name(&definition.name()));
            if definition.receiver().is_some_and(|receiver| receiver.as_self_node().is_some()) {
                nesting.push(format!("<Class:{}>", last_nesting_part(&nesting)));
            }
        }
    }

    (nesting, surrounding_method)
}

fn covers(start: usize, end: usize, offset: usize) -> bool {
    start <= offset && offset < end
}

pub fn locate<'pr>(root: &Node<'pr>, offset: usize, node_types: &[&str]) -> NodeContext<'pr> {
    let mut queue: VecDeque<Node<'pr>> = child_nodes(root).into();
    let mut closest = root.clone_node();
    let mut parent: Option<Node<'pr>> = None;
    let mut nesting_nodes: Vec<Node<'pr>> = Vec::new();
    if root.as_program_node().is_some() {
        nesting_nodes.push(root.clone_node());
    }
    let mut call_node: Option<Node<'pr>> = None;

    while let Some(candidate) = queue.pop_front() {
        for child in child_nodes(&candidate).into_iter().rev() {
            queue.push_front(child);
        }

        let start = candidate.start_offset();
        let end = candidate.end_offset();
        if !covers(start, end, offset) {
            continue;
        }

        if nesting_nodes.last().is_some_and(|previous_level| start > previous_level.end_offset()) {
            nesting_nodes.pop();
        }

        let type_name = candidate.type_name();
        if NESTING_NODE_TYPES.contains(&type_name) {
            nesting_nodes.push(candidate.clone_node());
        }

        if let Some(call) = candidate.as_call_node() {
            let in_arguments =
                call.arguments().is_some_and(|arguments| covers(arguments.location().start_offset(), arguments.location().end_offset(), offset));
            let in_block = call.block().is_some_and(|block| covers(block.start_offset(), block.end_offset(), offset));
            if in_arguments || in_block {
                call_node = Some(candidate.clone_node());
            }
        }

        if !node_types.is_empty() && !node_types.contains(&type_name) {
            continue;
        }

        if end - start <= closest.end_offset() - closest.start_offset() {
            parent = Some(closest);
            closest = candidate;
        }
    }

    if closest.as_constant_read_node().is_some() || closest.as_constant_path_node().is_some() {
        let constant_path = nesting_nodes.last().and_then(|last_level| {
            last_level
                .as_module_node()
                .map(|module| module.constant_path())
                .or_else(|| last_level.as_class_node().map(|class| class.constant_path()))
        });
        if constant_path.is_some_and(|path| path.same_node(&closest)) {
            nesting_nodes.pop();
        }
    }

    NodeContext::new(Some(closest), parent, nesting_nodes, call_node)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn locate_in(source: &str, offset: usize, node_types: &[&str], check: impl FnOnce(NodeContext<'_>)) {
        let parse_result = ruby_prism::parse(source.as_bytes());
        let root = parse_result.node();
        check(locate(&root, offset, node_types));
    }

    fn offset_of(source: &str, line: usize, character: usize) -> usize {
        source.split_inclusive('\n').take(line).map(str::len).sum::<usize>() + character
    }

    #[test]
    fn locates_narrowest_node() {
        let source = "class Post < ActiveRecord::Base\n  scope :published do\n    true\n  end\nend\n";
        locate_in(source, offset_of(source, 0, 22), &[], |context| {
            let node = context.node.unwrap();
            assert_eq!(node.type_name(), "ConstantReadNode");
            assert_eq!(node_slice(&node), "ActiveRecord");
            assert_eq!(context.parent.unwrap().type_name(), "ConstantPathNode");
        });
        locate_in(source, offset_of(source, 1, 3), &[], |context| {
            assert_eq!(context.node.unwrap().type_name(), "CallNode");
        });
        locate_in(source, offset_of(source, 2, 5), &[], |context| {
            assert_eq!(context.node.unwrap().type_name(), "TrueNode");
            assert_eq!(context.nesting, vec!["Post".to_string()]);
        });
    }

    #[test]
    fn returns_nesting() {
        let source = "module Foo\n  class Other\n    def do_it\n      Hello::Qux\n    end\n  end\nend\n";
        locate_in(source, offset_of(source, 3, 7), &["ConstantReadNode", "ConstantPathNode"], |context| {
            assert_eq!(node_slice(context.node.as_ref().unwrap()), "Hello");
            assert_eq!(context.nesting, vec!["Foo".to_string(), "Other".to_string()]);
            assert_eq!(context.surrounding_method.as_deref(), Some("do_it"));
        });
    }

    #[test]
    fn returns_call_node() {
        let source = "ProfileHelper.find_profile(name: \"foo\")\n";
        locate_in(source, offset_of(source, 0, 34), &[], |context| {
            assert_eq!(node_slice(context.call_node_as_node().unwrap()), "ProfileHelper.find_profile(name: \"foo\")");
        });
    }

    #[test]
    fn locating_compact_namespace_declaration() {
        let source = "class Foo::Bar\nend\n";
        locate_in(source, offset_of(source, 0, 11), &[], |context| {
            assert_eq!(node_slice(context.node.as_ref().unwrap()), "Foo::Bar");
            assert!(context.nesting.is_empty());
        });
    }

    #[test]
    fn locating_singleton_contexts() {
        let source = "class Foo\n  hello1\n\n  def self.bar\n    hello2\n  end\n\n  class << self\n    hello3\n\n    def baz\n      hello4\n    end\n  end\n\n  def qux\n    hello5\n  end\nend\n";
        locate_in(source, offset_of(source, 1, 4), &[], |context| {
            assert_eq!(context.nesting, vec!["Foo".to_string()]);
            assert_eq!(context.surrounding_method, None);
        });
        locate_in(source, offset_of(source, 4, 4), &[], |context| {
            assert_eq!(context.nesting, vec!["Foo".to_string(), "<Class:Foo>".to_string()]);
            assert_eq!(context.surrounding_method.as_deref(), Some("bar"));
        });
        locate_in(source, offset_of(source, 8, 4), &[], |context| {
            assert_eq!(context.nesting, vec!["Foo".to_string(), "<Class:Foo>".to_string()]);
            assert_eq!(context.surrounding_method, None);
        });
        locate_in(source, offset_of(source, 11, 6), &[], |context| {
            assert_eq!(context.nesting, vec!["Foo".to_string(), "<Class:Foo>".to_string()]);
            assert_eq!(context.surrounding_method.as_deref(), Some("baz"));
        });
        locate_in(source, offset_of(source, 16, 4), &[], |context| {
            assert_eq!(context.nesting, vec!["Foo".to_string()]);
            assert_eq!(context.surrounding_method.as_deref(), Some("qux"));
        });
    }

    #[test]
    fn locals_for_scope() {
        let source = "a = 1\nclass Foo\n  b = 2\n  def bar(c)\n    [1].each { |d| d }\n  end\nend\n";
        locate_in(source, offset_of(source, 4, 20), &[], |context| {
            assert_eq!(context.locals_for_scope(), vec!["c".to_string(), "d".to_string()]);
        });
    }
}
