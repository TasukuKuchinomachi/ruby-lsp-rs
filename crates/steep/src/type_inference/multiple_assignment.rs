use std::rc::Rc;

use rbs_definition::ast::TypeName;
use steep_interface::types::Type;

use crate::{
    ruby::node::Node,
    ruby_exception,
    type_inference::type_env::{SPECIAL_LVAR_NAMES, TypeEnv},
};

pub type NodeTypePair = (Rc<Node>, Type);

pub struct Assignments {
    pub rhs_type: Type,
    pub optional: bool,
    pub leading_assignments: Vec<NodeTypePair>,
    pub trailing_assignments: Vec<NodeTypePair>,
    pub splat_assignment: Option<NodeTypePair>,
}

impl Assignments {
    pub fn each(&self) -> Vec<NodeTypePair> {
        let mut pairs = self.leading_assignments.clone();
        pairs.extend(self.splat_assignment.clone());
        pairs.extend(self.trailing_assignments.iter().cloned());
        pairs
    }
}

fn array_type(element: Type) -> Type {
    Type::NameInstance {
        name: TypeName::parse("::Array"),
        args: vec![element],
    }
}

fn optional(type_: Type) -> Type {
    Type::union(vec![type_, Type::Nil])
}

fn is_array_instance(type_: &Type) -> bool {
    matches!(type_, Type::NameInstance { name, .. } if *name == TypeName::parse("::Array"))
}

#[derive(Default)]
pub struct MultipleAssignment;

impl MultipleAssignment {
    pub fn new() -> MultipleAssignment {
        MultipleAssignment
    }

    pub fn expand(&self, mlhs: &Node, rhs_type: &Type, optional: bool) -> Option<Assignments> {
        let lhss: Vec<Rc<Node>> = mlhs.child_nodes().cloned().collect();
        match rhs_type {
            Type::Tuple(types) => Some(self.expand_tuple(lhss, rhs_type, types.clone(), optional)),
            Type::NameInstance { .. } if is_array_instance(rhs_type) => {
                Some(self.expand_array(lhss, rhs_type, optional))
            }
            Type::Any => Some(self.expand_any(lhss, rhs_type, Type::Any, optional)),
            _ => None,
        }
    }

    fn expand_tuple(
        &self,
        mut lhss: Vec<Rc<Node>>,
        rhs_type: &Type,
        mut tuples: Vec<Type>,
        optional: bool,
    ) -> Assignments {
        let mut leading_assignments = Vec::new();
        let mut trailing_assignments = Vec::new();
        let mut splat_assignment = None;
        while let Some(first) = lhss.first().cloned() {
            if first.type_ == "splat" {
                break;
            }
            let type_ = if tuples.is_empty() {
                Type::Nil
            } else {
                tuples.remove(0)
            };
            leading_assignments.push((first, type_));
            lhss.remove(0);
        }
        while let Some(last) = lhss.last().cloned() {
            if last.type_ == "splat" {
                break;
            }
            trailing_assignments.push((last, tuples.pop().unwrap_or(Type::Nil)));
            lhss.pop();
        }
        match lhss.len() {
            0 => {}
            1 => splat_assignment = Some((lhss[0].clone(), Type::Tuple(tuples))),
            _ => ruby_exception::raise_runtime_error(""),
        }
        Assignments {
            rhs_type: rhs_type.clone(),
            optional,
            leading_assignments,
            trailing_assignments,
            splat_assignment,
        }
    }

    fn expand_array(
        &self,
        mut lhss: Vec<Rc<Node>>,
        rhs_type: &Type,
        optional: bool,
    ) -> Assignments {
        let element_type = match rhs_type {
            Type::NameInstance { args, .. } => args
                .first()
                .cloned()
                .unwrap_or_else(|| ruby_exception::raise_runtime_error("")),
            _ => unreachable!("an array instance type"),
        };
        let mut leading_assignments = Vec::new();
        let mut trailing_assignments = Vec::new();
        let mut splat_assignment = None;
        while let Some(first) = lhss.first().cloned() {
            if first.type_ == "splat" {
                break;
            }
            leading_assignments.push((first, self::optional(element_type.clone())));
            lhss.remove(0);
        }
        while let Some(last) = lhss.last().cloned() {
            if last.type_ == "splat" {
                break;
            }
            trailing_assignments.push((last, self::optional(element_type.clone())));
            lhss.pop();
        }
        match lhss.len() {
            0 => {}
            1 => splat_assignment = Some((lhss[0].clone(), array_type(element_type))),
            _ => ruby_exception::raise_runtime_error(""),
        }
        Assignments {
            rhs_type: rhs_type.clone(),
            optional,
            leading_assignments,
            trailing_assignments,
            splat_assignment,
        }
    }

    fn expand_any(
        &self,
        nodes: Vec<Rc<Node>>,
        rhs_type: &Type,
        element_type: Type,
        optional: bool,
    ) -> Assignments {
        let mut leading_assignments = Vec::new();
        let mut trailing_assignments = Vec::new();
        let mut splat_assignment = None;
        let mut after_splat = false;
        for node in nodes {
            if node.type_ == "splat" {
                splat_assignment = Some((node, array_type(element_type.clone())));
                after_splat = true;
            } else if after_splat {
                trailing_assignments.push((node, element_type.clone()));
            } else {
                leading_assignments.push((node, element_type.clone()));
            }
        }
        Assignments {
            rhs_type: rhs_type.clone(),
            optional,
            leading_assignments,
            trailing_assignments,
            splat_assignment,
        }
    }

    pub fn hint_for_mlhs(&self, mlhs: &Node, env: &TypeEnv) -> Option<Type> {
        match mlhs.type_ {
            "mlhs" => {
                let mut types = Vec::new();
                for node in mlhs.child_nodes() {
                    types.push(self.hint_for_mlhs(node, env)?);
                }
                Some(Type::Tuple(types))
            }
            "lvasgn" | "ivasgn" | "gvasgn" => {
                let name = mlhs.child_symbol(0).unwrap_or_default();
                if SPECIAL_LVAR_NAMES.contains(&name) {
                    Some(Type::Any)
                } else {
                    Some(env.get(name).unwrap_or(Type::Any))
                }
            }
            _ => None,
        }
    }
}
