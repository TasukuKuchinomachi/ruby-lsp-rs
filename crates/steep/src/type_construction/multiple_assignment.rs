use std::rc::Rc;

use rbs_definition::ast::Literal;
use steep_interface::{
    substitution::Substitution,
    subtyping::{constraints::Constraints, variable_variance::VariableVariance},
    types::{Type, fresh_variable_name},
};

use crate::{
    diagnostic::ruby as diagnostics,
    ruby::node::Node,
    ruby_exception::{self, RubyException},
    strict::StrictCheck,
    type_construction::{
        Pair, TypeConstruction, flatten_union, instance_type, is_instance_of, or_raise,
    },
    type_inference::multiple_assignment::MultipleAssignment,
};

fn each_descendant_node(node: &Node, nodes: &mut Vec<Rc<Node>>) {
    for child in node.child_nodes() {
        nodes.push(child.clone());
        each_descendant_node(child, nodes);
    }
}

impl<'checker> TypeConstruction<'checker> {
    pub fn masgn_lhs(&self, lhs: &Node) -> bool {
        lhs.child_nodes().all(|assignment| {
            let assignment_type = if assignment.type_ == "splat" {
                assignment.child_node(0).map(|node| node.type_)
            } else {
                Some(assignment.type_)
            };
            matches!(assignment_type, None | Some("lvasgn" | "ivasgn" | "gvasgn"))
        })
    }

    pub fn partition_flatten_types(
        &self,
        type_: &Type,
        predicate: impl Fn(&Type) -> bool,
    ) -> (Vec<Type>, Vec<Type>) {
        let expanded = self
            .checker
            .deep_expand_alias(type_)
            .unwrap_or_else(|| type_.clone());
        flatten_union(&expanded).into_iter().partition(predicate)
    }

    pub fn union_of_tuple_to_tuple_of_union(&self, types: &[Type]) -> Option<Type> {
        let tuples: Vec<&Vec<Type>> = types
            .iter()
            .map(|type_| match type_ {
                Type::Tuple(types) => Some(types),
                _ => None,
            })
            .collect::<Option<_>>()?;
        let max = tuples.iter().map(|tuple| tuple.len()).max()?;
        let elements = (0..max)
            .map(|index| {
                self.union_type_unify(
                    tuples
                        .iter()
                        .map(|tuple| tuple.get(index).cloned().unwrap_or(Type::Nil))
                        .collect(),
                )
            })
            .collect();
        Some(Type::Tuple(elements))
    }

    pub fn try_convert_to_array(&self, type_: &Type) -> Option<Type> {
        if let Some(arrayish) = self
            .arrayish_type(type_, true)
            .or_else(|| self.semantically_arrayish_type(type_))
        {
            return Some(arrayish);
        }
        match self.try_convert(type_, "to_ary") {
            Some(converted) => (self.arrayish_type(&converted, true).is_some()
                || self.semantically_arrayish_type(type_).is_some())
            .then_some(converted),
            None => Some(Type::Tuple(vec![type_.clone()])),
        }
    }

    fn arrayish_type(&self, type_: &Type, untyped_is: bool) -> Option<Type> {
        match type_ {
            Type::Any => untyped_is.then(|| type_.clone()),
            Type::NameInstance { .. } if is_instance_of(type_, "::Array") => Some(type_.clone()),
            Type::Tuple(_) => Some(type_.clone()),
            Type::Alias { .. } => {
                let expanded = self.checker.deep_expand_alias(type_)?;
                self.arrayish_type(&expanded, false)
            }
            _ => None,
        }
    }

    fn semantically_arrayish_type(&self, type_: &Type) -> Option<Type> {
        if let Type::Union(types) = Type::union(flatten_union(type_))
            && let Some(tuple) = self.union_of_tuple_to_tuple_of_union(&types)
        {
            return Some(tuple);
        }

        let variable = fresh_variable_name("Elem");
        let array = instance_type("::Array", vec![Type::Var(variable.clone())]);
        let mut constraints = Constraints::new(&[]);
        or_raise(constraints.add_variable(&variable));
        let constraints = constraints.shared();

        if !self
            .check_relation_with(type_, &array, constraints.clone())
            .is_success()
        {
            return None;
        }
        let variance = VariableVariance::from_type(&Type::union(vec![
            type_.clone(),
            Type::Var(variable.clone()),
        ]));
        let mut variables = type_.free_variables();
        variables.push(variable);
        let solution = or_raise(constraints.borrow().solution(
            &self.builder().subtyping(),
            &variance,
            &variables,
            Some(self.self_type().clone()),
            Some(self.module_context().instance_type.clone()),
            Some(self.module_context().module_type.clone()),
        ))
        .unwrap_or_else(|error| {
            ruby_exception::raise(RubyException::new(
                "Steep::Subtyping::Constraints::UnsatisfiableConstraint",
                format!(
                    "Unsatisfiable constraint on {}: {} <: {}",
                    error.variable, error.sub_type, error.super_type
                ),
            ))
        });
        let (names, types): (Vec<String>, Vec<Type>) = solution.into_iter().unzip();
        Some(type_.substitute(&Substitution::build(&names, &types, None, None, None)))
    }

    fn observe_untyped_multiple_assignment(&self, mlhs_node: &Rc<Node>) {
        self.observe(
            StrictCheck::UntypedMultipleAssignment,
            mlhs_node,
            "Steep types every target of this multiple assignment as untyped, because the value assigned is untyped",
        );
    }

    pub fn type_masgn_type(
        &self,
        mlhs_node: &Rc<Node>,
        rhs_type: Option<&Type>,
        masgn: &MultipleAssignment,
        optional: bool,
    ) -> Option<TypeConstruction<'checker>> {
        let mut constr = self.clone();
        let assignments = masgn.expand(mlhs_node, rhs_type.unwrap_or(&Type::Any), optional)?;
        for (node, type_) in assignments.each() {
            let type_ = if assignments.optional {
                Type::union(vec![type_, Type::Nil])
            } else {
                type_
            };
            let assignment_node = if node.type_ == "splat" {
                match node.child_node(0) {
                    Some(assignment_node) => assignment_node.clone(),
                    None => continue,
                }
            } else {
                node.clone()
            };
            constr = match assignment_node.type_ {
                "lvasgn" => constr.lvasgn(&assignment_node, type_.clone()).constr,
                "ivasgn" => constr.ivasgn(&assignment_node, type_.clone()).constr,
                "gvasgn" => constr.gvasgn(&assignment_node, type_.clone()).constr,
                "mlhs" => {
                    if matches!(type_, Type::Any) && !matches!(rhs_type, Some(Type::Any) | None) {
                        constr.observe_untyped_multiple_assignment(&assignment_node);
                    }
                    constr.type_masgn_type(&assignment_node, Some(&type_), masgn, optional)?
                }
                _ => {
                    constr.observe(
                        StrictCheck::MultipleAssignmentTarget,
                        &assignment_node,
                        format!(
                            "Steep does not check the value a multiple assignment gives to `{}`, and types the target as untyped",
                            assignment_node.type_
                        ),
                    );
                    constr
                        .synthesize_children(&assignment_node, &[])
                        .add_typing(&assignment_node, Type::Any)
                        .constr
                }
            };
            if node.type_ == "splat" {
                constr = constr.add_typing(&node, type_).constr;
            }
        }
        Some(constr)
    }

    pub fn type_masgn(&self, node: &Rc<Node>) -> Pair<'checker> {
        let lhs = node.child_node(0).cloned().expect("masgn has a left side");
        let rhs = node.child_node(1).cloned().expect("masgn has a right side");
        let masgn = MultipleAssignment::new();
        let hint = masgn.hint_for_mlhs(&lhs, &self.context.type_env);
        let pair = self.try_tuple_type_strict(&rhs, hint.as_ref());
        let lhs_constr = pair.constr;
        let rhs_type = self
            .checker
            .deep_expand_alias(&pair.type_)
            .unwrap_or(pair.type_);
        let (falsys, truthys) = self.partition_flatten_types(&rhs_type, |type_| {
            matches!(type_, Type::Nil | Type::Literal(Literal::Bool(false)))
        });
        let mut truthy_rhs_type = self.union_type_unify(truthys);
        if let Type::Union(types) = &truthy_rhs_type
            && let Some(tuple) = self.union_of_tuple_to_tuple_of_union(types)
        {
            truthy_rhs_type = tuple;
        }
        let optional = !falsys.is_empty();
        if matches!(truthy_rhs_type, Type::Any) {
            self.observe_untyped_multiple_assignment(&lhs);
        }
        let mut array_type = None;
        let constr = if matches!(truthy_rhs_type, Type::Tuple(_) | Type::Any)
            || is_instance_of(&truthy_rhs_type, "::Array")
        {
            lhs_constr.type_masgn_type(&lhs, Some(&truthy_rhs_type), &masgn, optional)
        } else {
            let converted = self
                .try_convert(&truthy_rhs_type, "to_ary")
                .or_else(|| self.try_convert(&truthy_rhs_type, "to_a"))
                .unwrap_or_else(|| Type::Tuple(vec![truthy_rhs_type.clone()]));
            array_type = Some(converted.clone());
            lhs_constr.type_masgn_type(&lhs, Some(&converted), &masgn, optional)
        };
        let constr = match constr {
            Some(constr) => constr,
            None => {
                self.add_error(diagnostics::multiple_assignment_conversion_error(
                    &rhs,
                    &rhs_type,
                    &array_type.unwrap_or(Type::Bot),
                ));
                let mut constr = lhs_constr;
                let mut descendants = Vec::new();
                each_descendant_node(&lhs, &mut descendants);
                for node in descendants {
                    constr = match node.type_ {
                        "lvasgn" => constr.lvasgn(&node, Type::Any).constr,
                        "ivasgn" => constr.ivasgn(&node, Type::Any).constr,
                        "gvasgn" => constr.gvasgn(&node, Type::Any).constr,
                        _ => constr.add_typing(&node, Type::Any).constr,
                    };
                }
                constr
            }
        };
        constr.add_typing(node, truthy_rhs_type)
    }
}
