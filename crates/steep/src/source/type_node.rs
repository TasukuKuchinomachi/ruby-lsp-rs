use std::rc::Rc;

use rbs_definition::collections::HashMap;

use crate::{
    ast::node::{type_application::TypeApplication, type_assertion::TypeAssertion},
    ruby::node::{Buffer, Child, Location, MapKind, Node},
    source::Source,
    type_construction::definition::module_name_from_node,
};

#[derive(Clone, Debug)]
pub enum TypeComment {
    Assertion(Rc<TypeAssertion>),
    Application(Rc<TypeApplication>),
}

pub type TypeComments = HashMap<usize, TypeComment>;

fn except(comments: &TypeComments, line: usize) -> TypeComments {
    let mut comments = comments.clone();
    comments.remove(&line);
    comments
}

pub fn updated_children(node: &Rc<Node>, children: Vec<Child>) -> Rc<Node> {
    let unchanged = node.children.len() == children.len()
        && node
            .children
            .iter()
            .zip(&children)
            .all(|(original, updated)| match (original, updated) {
                (Child::Node(original), Child::Node(updated)) => Rc::ptr_eq(original, updated),
                _ => original == updated,
            });
    if unchanged {
        node.clone()
    } else {
        node.updated(None, Some(children), None)
    }
}

fn map_child_node(node: &Rc<Node>, mut map: impl FnMut(&Rc<Node>) -> Rc<Node>) -> Rc<Node> {
    let children = node
        .children
        .iter()
        .map(|child| match child {
            Child::Node(child) => Child::Node(map(child)),
            other => other.clone(),
        })
        .collect();
    updated_children(node, children)
}

fn optional_child(child: Option<Rc<Node>>) -> Child {
    child.map(Child::Node).unwrap_or(Child::Nil)
}

impl Source {
    pub fn skip_arg_assertions(node: &Node) -> bool {
        if node.type_ != "send" {
            return false;
        }
        let Some(receiver) = node.child_node(0) else {
            return false;
        };
        let method = node.child_symbol(1).unwrap_or_default();
        if receiver.type_ == "const"
            && let Some(type_name) = module_name_from_node(
                receiver.child_node(0),
                receiver.child_symbol(1).unwrap_or_default(),
            )
            && type_name.namespace.is_empty()
        {
            if type_name.name == "Data" && method == "define" {
                return true;
            }
            if type_name.name == "Struct" && method == "new" {
                return true;
            }
        }
        false
    }

    pub fn insert_type_node(
        buffer: &Buffer,
        node: &Rc<Node>,
        comments: &mut TypeComments,
    ) -> Rc<Node> {
        let mut node = node.clone();
        if let Some(expression) = node.location.expression {
            let first_line = buffer.line(expression.begin_pos);
            let last_line = buffer.line(expression.end_pos);
            let last_comment = comments.get(&last_line).cloned();
            if !(first_line..=last_line).any(|line| comments.contains_key(&line)) {
                return node;
            }
            if let Some(TypeComment::Assertion(assertion)) = &last_comment {
                match node.type_ {
                    "lvasgn" | "ivasgn" | "gvasgn" | "cvasgn" | "casgn" | "return" | "break"
                    | "next" | "def" | "defs" | "kwargs" | "when" => {}
                    "pair" => {
                        let key = node.child_node(0).cloned().expect("a pair has a key");
                        let value = node.child_node(1).cloned().expect("a pair has a value");
                        let key =
                            Self::insert_type_node(buffer, &key, &mut except(comments, last_line));
                        let value = Self::insert_type_node(buffer, &value, comments);
                        let node =
                            updated_children(&node, vec![Child::Node(key), Child::Node(value)]);
                        return Self::adjust_location(&node);
                    }
                    "masgn" => {
                        let lhs = node.child(0).clone();
                        let rhs = node.child_node(1).cloned().expect("masgn has a value");
                        let rhs = Self::insert_type_node(buffer, &rhs, comments);
                        let node = updated_children(&node, vec![lhs, Child::Node(rhs)]);
                        return Self::adjust_location(&node);
                    }
                    "begin" => {
                        if node.location.begin.is_some() {
                            let mut child_assertions = except(comments, last_line);
                            node = map_child_node(&node, |child| {
                                Self::insert_type_node(buffer, child, &mut child_assertions)
                            });
                            node = Self::adjust_location(&node);
                            return Self::assertion_node(&node, assertion);
                        }
                    }
                    _ => {
                        if matches!(node.type_, "send" | "csend")
                            && node.child_node(0).is_none()
                            && matches!(
                                node.child_symbol(1),
                                Some("attr_reader" | "attr_writer" | "attr_accessor")
                            )
                        {
                            let mut child_assertions = except(comments, last_line);
                            node = map_child_node(&node, |child| {
                                Self::insert_type_node(buffer, child, &mut child_assertions)
                            });
                            return Self::adjust_location(&node);
                        }
                        let mut child_assertions = except(comments, last_line);
                        node = map_child_node(&node, |child| {
                            Self::insert_type_node(buffer, child, &mut child_assertions)
                        });
                        node = Self::adjust_location(&node);
                        return Self::assertion_node(&node, assertion);
                    }
                }
            } else if let Some(selector_line) = Self::sendish_node(buffer, &node)
                && let Some(TypeComment::Application(application)) =
                    comments.get(&selector_line).cloned()
            {
                let mut child_assertions = except(comments, selector_line);
                match node.type_ {
                    "block" => {
                        let send = node
                            .child_node(0)
                            .cloned()
                            .expect("a block has a send node");
                        let mut children = vec![Child::Node(map_child_node(&send, |child| {
                            Self::insert_type_node(buffer, child, &mut child_assertions)
                        }))];
                        for child in &node.children[1..] {
                            children.push(match child {
                                Child::Node(child) => Child::Node(Self::insert_type_node(
                                    buffer,
                                    child,
                                    &mut child_assertions,
                                )),
                                _ => Child::Nil,
                            });
                        }
                        node = updated_children(&node, children);
                    }
                    "numblock" | "itblock" => {
                        let send = node
                            .child_node(0)
                            .cloned()
                            .expect("a block has a send node");
                        let send = map_child_node(&send, |child| {
                            Self::insert_type_node(buffer, child, &mut child_assertions)
                        });
                        let body = node.child_node(2).cloned().map(|body| {
                            Self::insert_type_node(buffer, &body, &mut child_assertions)
                        });
                        node = updated_children(
                            &node,
                            vec![
                                Child::Node(send),
                                node.child(1).clone(),
                                optional_child(body),
                            ],
                        );
                    }
                    _ => {
                        node = map_child_node(&node, |child| {
                            Self::insert_type_node(buffer, child, &mut child_assertions)
                        });
                    }
                }
                node = Self::adjust_location(&node);
                return Self::type_application_node(&node, &application);
            }
        }

        match node.type_ {
            "class" => {
                let body = node
                    .child_node(2)
                    .cloned()
                    .map(|body| Self::insert_type_node(buffer, &body, comments));
                let node = updated_children(
                    &node,
                    vec![
                        node.child(0).clone(),
                        node.child(1).clone(),
                        optional_child(body),
                    ],
                );
                Self::adjust_location(&node)
            }
            "module" => {
                let body = node
                    .child_node(1)
                    .cloned()
                    .map(|body| Self::insert_type_node(buffer, &body, comments));
                let node =
                    updated_children(&node, vec![node.child(0).clone(), optional_child(body)]);
                Self::adjust_location(&node)
            }
            "def" => {
                let arguments = node.child_node(1).cloned();
                let assertion_location = arguments
                    .as_ref()
                    .and_then(|arguments| arguments.location.expression)
                    .or(node.location.name)
                    .expect("a method definition has a name");
                let mut no_assertion_comments =
                    except(comments, buffer.line(assertion_location.end_pos));
                let arguments = arguments.map(|arguments| {
                    Self::insert_type_node(buffer, &arguments, &mut no_assertion_comments)
                });
                let body = node
                    .child_node(2)
                    .cloned()
                    .map(|body| Self::insert_type_node(buffer, &body, comments));
                let node = updated_children(
                    &node,
                    vec![
                        node.child(0).clone(),
                        optional_child(arguments),
                        optional_child(body),
                    ],
                );
                Self::adjust_location(&node)
            }
            "defs" => {
                let arguments = node.child_node(2).cloned();
                let assertion_location = arguments
                    .as_ref()
                    .and_then(|arguments| arguments.location.expression)
                    .or(node.location.name)
                    .expect("a method definition has a name");
                let mut no_assertion_comments =
                    except(comments, buffer.line(assertion_location.end_pos));
                let object = node.child_node(0).cloned().map(|object| {
                    Self::insert_type_node(buffer, &object, &mut no_assertion_comments)
                });
                let arguments = arguments.map(|arguments| {
                    Self::insert_type_node(buffer, &arguments, &mut no_assertion_comments)
                });
                let body = node
                    .child_node(3)
                    .cloned()
                    .map(|body| Self::insert_type_node(buffer, &body, comments));
                let node = updated_children(
                    &node,
                    vec![
                        optional_child(object),
                        node.child(1).clone(),
                        optional_child(arguments),
                        optional_child(body),
                    ],
                );
                Self::adjust_location(&node)
            }
            _ => {
                if Self::skip_arg_assertions(&node)
                    && let Some(expression) = node.location.expression
                {
                    let first_line = buffer.line(expression.begin_pos);
                    let last_line = buffer.line(expression.end_pos);
                    comments.retain(|line, _| !(first_line < *line && *line < last_line));
                    let node = map_child_node(&node, |child| {
                        Self::insert_type_node(buffer, child, comments)
                    });
                    return Self::adjust_location(&node);
                }
                let node = map_child_node(&node, |child| {
                    Self::insert_type_node(buffer, child, comments)
                });
                Self::adjust_location(&node)
            }
        }
    }

    pub fn sendish_node(buffer: &Buffer, node: &Node) -> Option<usize> {
        let send_node = match node.type_ {
            "send" | "csend" => node,
            "block" | "numblock" | "itblock" => {
                let send = node.child_node(0)?;
                match send.type_ {
                    "send" | "csend" => send,
                    _ => return None,
                }
            }
            _ => return None,
        };
        let location = &send_node.location;
        if send_node.child_node(0).is_some() {
            if location.dot.is_some()
                && let Some(selector) = location.selector
            {
                return Some(buffer.line(selector.begin_pos));
            }
            None
        } else {
            location
                .selector
                .map(|selector| buffer.line(selector.begin_pos))
        }
    }

    pub fn adjust_location(node: &Rc<Node>) -> Rc<Node> {
        if let Some(expression) = node.location.expression
            && let Some(last_pos) = node
                .child_nodes()
                .filter_map(|child| child.location.expression)
                .map(|expression| expression.end_pos)
                .max()
            && last_pos > expression.end_pos
        {
            let location = node
                .location
                .with_expression(expression.with_end_pos(last_pos));
            return node.updated(None, None, Some(location));
        }
        node.clone()
    }

    pub fn assertion_node(node: &Rc<Node>, type_: &Rc<TypeAssertion>) -> Rc<Node> {
        let expression = node.expression().with_end_pos(type_.location.end_pos);
        Node::new(
            "assertion",
            vec![
                Child::Node(node.clone()),
                Child::TypeAssertion(type_.clone()),
            ],
            Location::new(MapKind::Map, Some(expression)),
        )
    }

    pub fn type_application_node(node: &Rc<Node>, application: &Rc<TypeApplication>) -> Rc<Node> {
        let expression = if node.expression().end_pos > application.location.end_pos {
            node.expression()
        } else {
            node.expression().with_end_pos(application.location.end_pos)
        };
        let node = Node::new(
            "tapp",
            vec![
                Child::Node(node.clone()),
                Child::TypeApplication(application.clone()),
            ],
            Location::new(MapKind::Map, Some(expression)),
        );
        application.set_node(&node);
        node
    }
}
