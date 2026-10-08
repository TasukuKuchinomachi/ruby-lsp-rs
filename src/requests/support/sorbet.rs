use ruby_prism::CallNode;

use crate::prism::{constant_id_name, location_slice};

#[derive(Debug, Clone, Copy)]
enum Arity {
    Exactly(usize),
    AtLeast(usize),
    Between(usize, usize),
}

#[derive(Debug, Clone, Copy)]
struct Annotation {
    arity: Arity,
    receiver: bool,
}

impl Annotation {
    const fn new(arity: Arity, receiver: bool) -> Annotation {
        Annotation { arity, receiver }
    }

    fn matches(&self, node: &CallNode<'_>) -> bool {
        self.receiver_matches(node) && self.arity_matches(node)
    }

    fn receiver_matches(&self, node: &CallNode<'_>) -> bool {
        match node.receiver() {
            Some(receiver) => self.receiver && location_slice(&receiver.location()) == "T",
            None => !self.receiver,
        }
    }

    fn arity_matches(&self, node: &CallNode<'_>) -> bool {
        let node_arity = node.arguments().map_or(0, |arguments| arguments.arguments().iter().count());
        match self.arity {
            Arity::Exactly(arity) => node_arity == arity,
            Arity::AtLeast(minimum) => node_arity >= minimum,
            Arity::Between(minimum, maximum) => (minimum..=maximum).contains(&node_arity),
        }
    }
}

const ANNOTATIONS: &[(&str, Annotation)] = &[
    ("abstract!", Annotation::new(Arity::Exactly(0), false)),
    ("absurd", Annotation::new(Arity::Exactly(1), true)),
    ("all", Annotation::new(Arity::AtLeast(2), true)),
    ("any", Annotation::new(Arity::AtLeast(2), true)),
    ("assert_type!", Annotation::new(Arity::Exactly(2), true)),
    ("attached_class", Annotation::new(Arity::Exactly(0), true)),
    ("bind", Annotation::new(Arity::Exactly(2), true)),
    ("cast", Annotation::new(Arity::Exactly(2), true)),
    ("class_of", Annotation::new(Arity::Exactly(1), true)),
    ("enums", Annotation::new(Arity::Exactly(0), false)),
    ("interface!", Annotation::new(Arity::Exactly(0), false)),
    ("let", Annotation::new(Arity::Exactly(2), true)),
    ("mixes_in_class_methods", Annotation::new(Arity::Exactly(1), false)),
    ("must", Annotation::new(Arity::Exactly(1), true)),
    ("must_because", Annotation::new(Arity::Exactly(1), true)),
    ("nilable", Annotation::new(Arity::Exactly(1), true)),
    ("noreturn", Annotation::new(Arity::Exactly(0), true)),
    ("requires_ancestor", Annotation::new(Arity::Exactly(0), false)),
    ("reveal_type", Annotation::new(Arity::Exactly(1), true)),
    ("sealed!", Annotation::new(Arity::Exactly(0), false)),
    ("self_type", Annotation::new(Arity::Exactly(0), true)),
    ("sig", Annotation::new(Arity::Exactly(0), false)),
    ("type_member", Annotation::new(Arity::Between(0, 1), false)),
    ("type_template", Annotation::new(Arity::Exactly(0), false)),
    ("unsafe", Annotation::new(Arity::Exactly(1), false)),
    ("untyped", Annotation::new(Arity::Exactly(0), true)),
];

pub fn annotation(node: &CallNode<'_>) -> bool {
    let name = constant_id_name(&node.name());
    ANNOTATIONS
        .iter()
        .find(|(annotation_name, _)| *annotation_name == name)
        .is_some_and(|(_, annotation)| annotation.matches(node))
}
