use std::rc::Rc;

use steep_interface::steep_version::SteepVersion;

use crate::{
    ruby::node::{Child, Location, MapKind, Node, NodeId},
    source::Source,
    type_construction::Checker,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum InferenceRule {
    CollectionLiteralWithHintOfSelfReturningCall,
    ProcTypeFromBlock,
    NumberedReferenceAsString,
    ItAsBlockParameter,
}

impl InferenceRule {
    pub const ALL: [InferenceRule; 4] = [
        InferenceRule::CollectionLiteralWithHintOfSelfReturningCall,
        InferenceRule::ProcTypeFromBlock,
        InferenceRule::NumberedReferenceAsString,
        InferenceRule::ItAsBlockParameter,
    ];

    fn bit(self) -> u8 {
        1 << self as u8
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InferenceRules {
    enabled: u8,
}

impl InferenceRules {
    pub const STEEP: InferenceRules = InferenceRules { enabled: 0 };

    pub fn more_than_steep() -> InferenceRules {
        InferenceRule::ALL
            .into_iter()
            .fold(InferenceRules::STEEP, InferenceRules::with)
    }

    pub fn only(rule: InferenceRule) -> InferenceRules {
        InferenceRules::STEEP.with(rule)
    }

    pub fn with(self, rule: InferenceRule) -> InferenceRules {
        InferenceRules {
            enabled: self.enabled | rule.bit(),
        }
    }

    pub fn without(self, rule: InferenceRule) -> InferenceRules {
        InferenceRules {
            enabled: self.enabled & !rule.bit(),
        }
    }

    pub fn contains(self, rule: InferenceRule) -> bool {
        self.enabled & rule.bit() != 0
    }
}

pub struct Inference {
    pub rules: InferenceRules,
    pub(crate) it_block_parameter_nodes: Vec<NodeId>,
}

impl Inference {
    pub fn steep() -> Inference {
        Inference {
            rules: InferenceRules::STEEP,
            it_block_parameter_nodes: Vec::new(),
        }
    }

    pub fn prepare(
        checker: &Checker,
        rules: InferenceRules,
        source: Source,
    ) -> (Source, Inference) {
        let mut inference = Inference {
            rules,
            ..Inference::steep()
        };
        let source = if rules.contains(InferenceRule::ItAsBlockParameter)
            && checker.builder.steep_version == SteepVersion::Version2_0_0
        {
            let (source, nodes) = with_it_block_parameters(source);
            inference.it_block_parameter_nodes = nodes;
            source
        } else {
            source
        };
        (source, inference)
    }
}

// Why not parse with the grammar of Ruby 3.4 for Steep 2.0: the grammars differ in more
// than `it`, and every other difference would change types that no rule explains.
fn with_it_block_parameters(source: Source) -> (Source, Vec<NodeId>) {
    let Some(node) = &source.node else {
        return (source, Vec::new());
    };
    let mut rewritten = Vec::new();
    let node = rewrite_it_blocks(node, &mut rewritten);
    (
        Source {
            node: Some(node),
            ..source
        },
        rewritten,
    )
}

fn with_children(node: &Rc<Node>, type_: &'static str, children: Vec<Child>) -> Rc<Node> {
    Rc::new(Node {
        id: node.id,
        type_,
        children,
        location: node.location.clone(),
    })
}

fn rewrite_children(node: &Rc<Node>, rewrite: &mut dyn FnMut(&Rc<Node>) -> Rc<Node>) -> Rc<Node> {
    let children: Vec<Child> = node
        .children
        .iter()
        .map(|child| match child {
            Child::Node(child) => Child::Node(rewrite(child)),
            other => other.clone(),
        })
        .collect();
    let unchanged = children.iter().zip(&node.children).all(|pair| match pair {
        (Child::Node(new), Child::Node(old)) => Rc::ptr_eq(new, old),
        _ => true,
    });
    if unchanged {
        node.clone()
    } else {
        with_children(node, node.type_, children)
    }
}

fn rewrite_it_blocks(node: &Rc<Node>, rewritten: &mut Vec<NodeId>) -> Rc<Node> {
    let node = rewrite_children(node, &mut |child| rewrite_it_blocks(child, rewritten));
    if node.type_ != "block" || !has_no_parameters(&node) {
        return node;
    }
    let Some(body) = node.child_node(2) else {
        return node;
    };
    let mut it_calls = Vec::new();
    collect_it_calls(body, &mut it_calls);
    if it_calls.is_empty() {
        return node;
    }
    let body = replace_it_calls(body, &it_calls);
    rewritten.push(node.id);
    rewritten.extend(it_calls);
    with_children(
        &node,
        "itblock",
        vec![
            node.child(0).clone(),
            Child::Symbol("it".to_string()),
            Child::Node(body),
        ],
    )
}

fn has_no_parameters(block: &Node) -> bool {
    block
        .child_node(1)
        .is_some_and(|args| args.children.is_empty() && args.location.expression.is_none())
}

fn is_it_call(node: &Node) -> bool {
    node.type_ == "send"
        && node.child(0).is_nil()
        && node.child_symbol(1) == Some("it")
        && node.children.len() == 2
        && node.location.begin.is_none()
}

fn collect_it_calls(node: &Rc<Node>, it_calls: &mut Vec<NodeId>) {
    if is_it_call(node) {
        it_calls.push(node.id);
        return;
    }
    match node.type_ {
        "def" | "defs" | "class" | "module" | "sclass" => {}
        "block" | "numblock" | "itblock" => {
            if let Some(send) = node.child_node(0) {
                collect_it_calls(send, it_calls);
            }
        }
        _ => {
            for child in node.child_nodes() {
                collect_it_calls(child, it_calls);
            }
        }
    }
}

fn replace_it_calls(node: &Rc<Node>, it_calls: &[NodeId]) -> Rc<Node> {
    if it_calls.contains(&node.id) {
        let expression = node.expression();
        let mut location = Location::new(MapKind::Variable, Some(expression));
        location.name = Some(expression);
        return Rc::new(Node {
            id: node.id,
            type_: "lvar",
            children: vec![Child::Symbol("it".to_string())],
            location,
        });
    }
    rewrite_children(node, &mut |child| replace_it_calls(child, it_calls))
}
