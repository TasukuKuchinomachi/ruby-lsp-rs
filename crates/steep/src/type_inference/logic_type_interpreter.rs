use std::rc::Rc;

use rbs_definition::ast::{Literal, TypeName};
use steep_interface::{
    builder::Config,
    subtyping::{constraints::Constraints, relation::Relation, result::SubtypingResult},
    types::{Logic, Type},
};

use crate::{
    ruby::node::{Child, Node},
    ruby_exception,
    type_construction::{Checker, convert_with_shape, is_instance_of, or_raise},
    type_inference::{
        multiple_assignment::MultipleAssignment,
        type_env::{SPECIAL_LVAR_NAMES, TypeEnv, node_key},
    },
    typing::Typing,
};
use steep_interface::steep_version::SteepVersion;

#[derive(Clone)]
pub struct Result {
    pub env: TypeEnv,
    pub type_: Type,
    pub unreachable: bool,
}

impl Result {
    pub fn new(type_: Type, env: TypeEnv, unreachable: bool) -> Result {
        Result {
            env,
            type_,
            unreachable,
        }
    }

    pub fn update_env(self, update: impl FnOnce(&Result) -> TypeEnv) -> Result {
        let env = update(&self);
        Result { env, ..self }
    }

    pub fn update_type(self, type_: Type) -> Result {
        Result { type_, ..self }
    }

    pub fn unreachable(mut self) -> Result {
        self.unreachable = true;
        self
    }
}

const TRUE: Type = Type::Literal(Literal::Bool(true));
const FALSE: Type = Type::Literal(Literal::Bool(false));

pub struct LogicTypeInterpreter<'checker> {
    pub checker: &'checker Checker<'checker>,
    pub typing: Typing,
    pub config: Config,
}

fn is_special_lvar_name(name: &str) -> bool {
    SPECIAL_LVAR_NAMES.contains(&name)
}

fn union_or_bot(types: Vec<Type>) -> Type {
    if types.is_empty() {
        Type::Bot
    } else {
        Type::union(types)
    }
}

impl<'checker> LogicTypeInterpreter<'checker> {
    pub fn new(
        checker: &'checker Checker<'checker>,
        typing: Typing,
        config: Config,
    ) -> LogicTypeInterpreter<'checker> {
        LogicTypeInterpreter {
            checker,
            typing,
            config,
        }
    }

    fn type_of(&self, node: &Node) -> Type {
        self.typing.type_of_or_raise(node)
    }

    pub fn guess_type_from_method(&self, node: &Node) -> Option<Type> {
        if node.type_ != "send" {
            return None;
        }
        match node.child_symbol(1)? {
            "is_a?" | "kind_of?" | "instance_of?" => Some(Type::Logic(Logic::ReceiverIsArg)),
            "nil?" => Some(Type::Logic(Logic::ReceiverIsNil)),
            "!" => Some(Type::Logic(Logic::Not)),
            "===" => Some(Type::Logic(Logic::ArgIsReceiver)),
            _ => None,
        }
    }

    pub fn eval(&self, env: &TypeEnv, node: &Rc<Node>) -> (Result, Result) {
        self.evaluate_node(env, node, None)
    }

    pub fn partition_union(&self, type_: &Type) -> (Option<Type>, Option<Type>) {
        match type_ {
            Type::Alias { .. } => {
                let unfold = self.checker.expand_alias(type_);
                if unfold == *type_ {
                    (Some(type_.clone()), Some(type_.clone()))
                } else {
                    self.partition_union(&unfold)
                }
            }
            Type::Union(types) => {
                let mut truthy_types = Vec::new();
                let mut falsy_types = Vec::new();
                for type_ in types {
                    let (truthy, falsy) = self.partition_union(type_);
                    truthy_types.extend(truthy);
                    falsy_types.extend(falsy);
                }
                (
                    (!truthy_types.is_empty()).then(|| Type::union(truthy_types)),
                    (!falsy_types.is_empty()).then(|| Type::union(falsy_types)),
                )
            }
            Type::Any | Type::Boolean | Type::Top | Type::Logic(_) => {
                (Some(type_.clone()), Some(type_.clone()))
            }
            Type::Bot | Type::Void => (None, None),
            Type::Nil => (None, Some(type_.clone())),
            Type::Literal(Literal::Bool(false)) => (None, Some(type_.clone())),
            _ => (Some(type_.clone()), None),
        }
    }

    pub fn evaluate_node(
        &self,
        env: &TypeEnv,
        node: &Rc<Node>,
        type_: Option<Type>,
    ) -> (Result, Result) {
        let mut type_ = match type_ {
            Some(type_) => type_,
            None => {
                let type_ = self.type_of(node);
                if let Some((truthy_env, falsy_env)) = self.typing.logic_env(node.id) {
                    let (truthy_type, falsy_type) = self.partition_union(&type_);
                    return (
                        Result::new(
                            truthy_type.clone().unwrap_or(TRUE),
                            truthy_env,
                            truthy_type.is_none(),
                        ),
                        Result::new(
                            falsy_type.clone().unwrap_or(FALSE),
                            falsy_env,
                            falsy_type.is_none(),
                        ),
                    );
                }
                type_
            }
        };

        if matches!(type_, Type::Bot) {
            return (
                Result::new(type_.clone(), env.clone(), true),
                Result::new(type_, env.clone(), true),
            );
        }

        if let Type::Var(name) = &type_
            && let Some(upper_bound) = self.config.variable_bounds.get(name)
        {
            type_ = upper_bound.clone();
        }

        match node.type_ {
            "lvar" => {
                let name = node.child_symbol(0).unwrap_or_default().to_string();
                let (truthy_type, falsy_type) = self.partition_union(&type_);
                let truthy_result = match truthy_type {
                    Some(truthy_type) => Result::new(
                        truthy_type.clone(),
                        env.refine_types(&[(name.clone(), truthy_type)], &[]),
                        false,
                    ),
                    None => Result::new(type_.clone(), env.clone(), true),
                };
                let falsy_result = match falsy_type {
                    Some(falsy_type) => Result::new(
                        falsy_type.clone(),
                        env.refine_types(&[(name, falsy_type)], &[]),
                        false,
                    ),
                    None => Result::new(type_.clone(), env.clone(), true),
                };
                return (truthy_result, falsy_result);
            }
            "lvasgn" => {
                let name = node.child_symbol(0).unwrap_or_default();
                if is_special_lvar_name(name) {
                    return (
                        Result::new(type_.clone(), env.clone(), false),
                        Result::new(type_, env.clone(), false),
                    );
                }
                if let Some(rhs) = node.child_node(1) {
                    let (truthy_result, falsy_result) = self.evaluate_node(env, rhs, None);
                    return (
                        truthy_result.update_env(|result| {
                            self.evaluate_assignment(node, &result.env, &result.type_)
                        }),
                        falsy_result.update_env(|result| {
                            self.evaluate_assignment(node, &result.env, &result.type_)
                        }),
                    );
                }
            }
            "masgn" => {
                if let Some(rhs) = node.child_node(1) {
                    let (truthy_result, falsy_result) = self.evaluate_node(env, rhs, None);
                    return (
                        truthy_result.update_env(|result| {
                            self.evaluate_assignment(node, &result.env, &result.type_)
                        }),
                        falsy_result.update_env(|result| {
                            self.evaluate_assignment(node, &result.env, &result.type_)
                        }),
                    );
                }
            }
            "begin" => {
                if let Some(last_node) = node.child_nodes().last() {
                    return self.evaluate_node(env, last_node, None);
                }
            }
            "csend" => {
                if matches!(type_, Type::Any) {
                    type_ = self.guess_type_from_method(node).unwrap_or(type_);
                }
                if let Some(receiver) = node.child_node(0) {
                    let (truthy_receiver, falsy_receiver) = self.evaluate_node(env, receiver, None);
                    let (truthy_type, _) = self.partition_union(&type_);
                    let (mut truthy_result, falsy_result) = self.evaluate_node(
                        &truthy_receiver.env,
                        &node.updated(Some("send"), None, None),
                        Some(truthy_type.unwrap_or(type_)),
                    );
                    if truthy_receiver.unreachable {
                        truthy_result = truthy_result.unreachable();
                    }
                    let falsy_result = Result::new(
                        falsy_result.type_.clone(),
                        env.join(&[falsy_receiver.env, falsy_result.env]),
                        falsy_result.unreachable && falsy_receiver.unreachable,
                    );
                    return (truthy_result, falsy_result);
                }
            }
            "send" => {
                if matches!(type_, Type::Any) {
                    type_ = self.guess_type_from_method(node).unwrap_or(type_);
                }
                let receiver = node.child_node(0);
                if let Type::Logic(logic) = type_ {
                    let arguments: Vec<Rc<Node>> = node
                        .children
                        .iter()
                        .skip(2)
                        .filter_map(Child::node)
                        .cloned()
                        .collect();
                    if let Some(results) =
                        self.evaluate_method_call(env, logic, receiver, &arguments)
                    {
                        return results;
                    }
                } else {
                    let mut truthy_result = None;
                    let mut falsy_result = None;
                    if let Some(receiver) = receiver
                        && env.get_node(receiver).is_some()
                        && let receiver_type @ Type::Union(_) = self.type_of(receiver)
                        && let Some((truthy, falsy)) = self.evaluate_union_method_call(
                            node,
                            &type_,
                            env,
                            receiver,
                            &receiver_type,
                        )
                    {
                        if !truthy.unreachable {
                            truthy_result = Some(truthy);
                        }
                        if !falsy.unreachable {
                            falsy_result = Some(falsy);
                        }
                    }
                    let truthy_result = truthy_result
                        .unwrap_or_else(|| Result::new(type_.clone(), env.clone(), false));
                    let falsy_result = falsy_result
                        .unwrap_or_else(|| Result::new(type_.clone(), env.clone(), false));

                    let (truthy_type, falsy_type) = self.partition_union(&type_);
                    let mut truthy_result = match &truthy_type {
                        Some(truthy_type) => truthy_result.update_type(truthy_type.clone()),
                        None => truthy_result.update_type(Type::Bot).unreachable(),
                    };
                    let mut falsy_result = match &falsy_type {
                        Some(falsy_type) => falsy_result.update_type(falsy_type.clone()),
                        None => falsy_result.update_type(Type::Bot).unreachable(),
                    };

                    if truthy_result.env.get_node(node).is_some()
                        && falsy_result.env.get_node(node).is_some()
                    {
                        if let Some(truthy_type) = truthy_type {
                            truthy_result = Result::new(
                                truthy_type.clone(),
                                truthy_result
                                    .env
                                    .refine_types(&[], &[(node_key(node), truthy_type)]),
                                false,
                            );
                        }
                        if let Some(falsy_type) = falsy_type {
                            falsy_result = Result::new(
                                falsy_type.clone(),
                                falsy_result
                                    .env
                                    .refine_types(&[], &[(node_key(node), falsy_type)]),
                                false,
                            );
                        }
                    }

                    return (truthy_result, falsy_result);
                }
            }
            _ => {}
        }

        let (truthy_type, falsy_type) = self.partition_union(&type_);
        (
            Result::new(
                truthy_type.clone().unwrap_or(Type::Bot),
                env.clone(),
                truthy_type.is_none(),
            ),
            Result::new(
                falsy_type.clone().unwrap_or(Type::Bot),
                env.clone(),
                falsy_type.is_none(),
            ),
        )
    }

    pub fn evaluate_assignment(
        &self,
        assignment_node: &Node,
        env: &TypeEnv,
        rhs_type: &Type,
    ) -> TypeEnv {
        match assignment_node.type_ {
            "lvasgn" => {
                let name = assignment_node.child_symbol(0).unwrap_or_default();
                if is_special_lvar_name(name) {
                    env.clone()
                } else {
                    env.refine_types(&[(name.to_string(), rhs_type.clone())], &[])
                }
            }
            "masgn" => {
                let Some(lhs) = assignment_node.child_node(0) else {
                    return env.clone();
                };
                let masgn = MultipleAssignment::new();
                let assignments = masgn.expand(lhs, rhs_type, false).or_else(|| {
                    let rhs_type_converted = self
                        .try_convert(rhs_type, "to_ary")
                        .or_else(|| self.try_convert(rhs_type, "to_a"))
                        .unwrap_or_else(|| Type::Tuple(vec![rhs_type.clone()]));
                    masgn.expand(lhs, &rhs_type_converted, false)
                });
                let Some(assignments) = assignments else {
                    ruby_exception::raise_runtime_error(format!(
                        "Multiple assignment rhs doesn't look correct: {rhs_type}"
                    ));
                };
                assignments
                    .each()
                    .into_iter()
                    .fold(env.clone(), |env, (node, type_)| {
                        self.evaluate_assignment(&node, &env, &type_)
                    })
            }
            _ => env.clone(),
        }
    }

    fn try_convert(&self, type_: &Type, method: &str) -> Option<Type> {
        let shape = or_raise(self.checker.builder.shape(type_, &self.config))?;
        convert_with_shape(&self.checker.builder, &shape, method)
    }

    pub fn refine_node_type(
        &self,
        env: &TypeEnv,
        node: &Rc<Node>,
        truthy_type: &Type,
        falsy_type: &Type,
    ) -> (TypeEnv, TypeEnv) {
        match node.type_ {
            "lvar" => {
                let name = node.child_symbol(0).unwrap_or_default();
                if is_special_lvar_name(name) {
                    (env.clone(), env.clone())
                } else {
                    (
                        env.refine_types(&[(name.to_string(), truthy_type.clone())], &[]),
                        env.refine_types(&[(name.to_string(), falsy_type.clone())], &[]),
                    )
                }
            }
            "lvasgn" => {
                let name = node.child_symbol(0).unwrap_or_default();
                let (truthy_env, falsy_env) = match node.child_node(1) {
                    Some(rhs) => self.refine_node_type(env, rhs, truthy_type, falsy_type),
                    None => (env.clone(), env.clone()),
                };
                if is_special_lvar_name(name) {
                    (truthy_env, falsy_env)
                } else {
                    (
                        truthy_env.refine_types(&[(name.to_string(), truthy_type.clone())], &[]),
                        falsy_env.refine_types(&[(name.to_string(), falsy_type.clone())], &[]),
                    )
                }
            }
            "send" => {
                if env.get_node(node).is_some() {
                    (
                        env.refine_types(&[], &[(node_key(node), truthy_type.clone())]),
                        env.refine_types(&[], &[(node_key(node), falsy_type.clone())]),
                    )
                } else {
                    (env.clone(), env.clone())
                }
            }
            "begin" => match node.child_nodes().last() {
                Some(last_node) => self.refine_node_type(env, last_node, truthy_type, falsy_type),
                None => (env.clone(), env.clone()),
            },
            _ => (env.clone(), env.clone()),
        }
    }

    fn reachable_results(
        truthy_env: TypeEnv,
        falsy_env: TypeEnv,
        truthy_unreachable: bool,
        falsy_unreachable: bool,
    ) -> (Result, Result) {
        (
            Result::new(TRUE, truthy_env, truthy_unreachable),
            Result::new(FALSE, falsy_env, falsy_unreachable),
        )
    }

    pub fn evaluate_method_call(
        &self,
        env: &TypeEnv,
        type_: Logic,
        receiver: Option<&Rc<Node>>,
        arguments: &[Rc<Node>],
    ) -> Option<(Result, Result)> {
        match type_ {
            Logic::ReceiverIsNil => {
                let receiver = receiver?;
                if !arguments.is_empty() {
                    return None;
                }
                let receiver_type = self.type_of(receiver);
                let unwrap = self.checker.unwrap_optional(&receiver_type);
                let truthy_receiver = Type::Nil;
                let falsy_receiver = unwrap.clone().unwrap_or_else(|| receiver_type.clone());
                let (truthy_env, falsy_env) =
                    self.refine_node_type(env, receiver, &truthy_receiver, &falsy_receiver);
                Some(Self::reachable_results(
                    truthy_env,
                    falsy_env,
                    self.no_subtyping(&Type::Nil, &receiver_type).is_some(),
                    unwrap.is_none(),
                ))
            }
            Logic::ReceiverIsArg => {
                let receiver = receiver?;
                let argument = arguments.first()?;
                let receiver_type = self.type_of(receiver);
                let argument_type = self.checker.deep_expand_alias(&self.type_of(argument));
                if let Some(Type::Singleton(name)) = &argument_type {
                    let (truthy_type, falsy_type) = self.type_case_select(&receiver_type, name);
                    let (truthy_env, falsy_env) = self.refine_node_type(
                        env,
                        receiver,
                        &truthy_type
                            .clone()
                            .unwrap_or_else(|| self.instance_type(name)),
                        &falsy_type.clone().unwrap_or(Type::Any),
                    );
                    Some(Self::reachable_results(
                        truthy_env,
                        falsy_env,
                        truthy_type.is_none(),
                        falsy_type.is_none(),
                    ))
                } else if let Some((truthy_types, falsy_types)) =
                    self.literal_var_type_case_select(argument, &receiver_type, true)
                {
                    let truthy_unreachable = truthy_types.is_empty();
                    let falsy_unreachable = falsy_types.is_empty();
                    let (truthy_env, falsy_env) = self.refine_node_type(
                        env,
                        receiver,
                        &union_or_bot(truthy_types),
                        &union_or_bot(falsy_types),
                    );
                    Some(Self::reachable_results(
                        truthy_env,
                        falsy_env,
                        truthy_unreachable,
                        falsy_unreachable,
                    ))
                } else {
                    None
                }
            }
            Logic::ArgIsReceiver => {
                let receiver = receiver?;
                let argument = arguments.first()?;
                let receiver_type = self.checker.deep_expand_alias(&self.type_of(receiver));
                let argument_type = self.type_of(argument);
                let Some(Type::Singleton(name)) = &receiver_type else {
                    return None;
                };
                let (truthy_type, falsy_type) = self.type_case_select(&argument_type, name);
                let (truthy_env, falsy_env) = self.refine_node_type(
                    env,
                    argument,
                    &truthy_type
                        .clone()
                        .unwrap_or_else(|| self.instance_type(name)),
                    &falsy_type.clone().unwrap_or(Type::Any),
                );
                Some(Self::reachable_results(
                    truthy_env,
                    falsy_env,
                    truthy_type.is_none(),
                    falsy_type.is_none(),
                ))
            }
            Logic::ArgEqualsReceiver => {
                let receiver = receiver?;
                let argument = arguments.first()?;
                let argument_type = self.checker.expand_alias(&self.type_of(argument));
                let (truthy_types, falsy_types) =
                    self.literal_var_type_case_select(receiver, &argument_type, false)?;
                let truthy_unreachable = truthy_types.is_empty();
                let falsy_unreachable = falsy_types.is_empty();
                let (truthy_env, falsy_env) = self.refine_node_type(
                    env,
                    argument,
                    &union_or_bot(truthy_types),
                    &union_or_bot(falsy_types),
                );
                Some(Self::reachable_results(
                    truthy_env,
                    falsy_env,
                    truthy_unreachable,
                    falsy_unreachable,
                ))
            }
            Logic::ArgIsAncestor => {
                let receiver = receiver?;
                let argument = arguments.first()?;
                let receiver_type = self.type_of(receiver);
                let argument_type = self.checker.deep_expand_alias(&self.type_of(argument));
                let Some(truthy_type @ Type::Singleton(_)) = argument_type else {
                    return None;
                };
                let falsy_type = receiver_type;
                let (truthy_env, falsy_env) =
                    self.refine_node_type(env, receiver, &truthy_type, &falsy_type);
                Some(Self::reachable_results(truthy_env, falsy_env, false, false))
            }
            Logic::Not => {
                let receiver = receiver?;
                let (truthy_result, falsy_result) = self.evaluate_node(env, receiver, None);
                Some((
                    falsy_result.update_type(TRUE),
                    truthy_result.update_type(FALSE),
                ))
            }
            Logic::ReceiverIsNotNil => None,
        }
    }

    pub fn evaluate_union_method_call(
        &self,
        node: &Rc<Node>,
        type_: &Type,
        env: &TypeEnv,
        receiver: &Rc<Node>,
        receiver_type: &Type,
    ) -> Option<(Result, Result)> {
        let call_type = self.typing.call_of(node.id)?;
        if !call_type.is_typed() {
            return None;
        }
        let Type::Union(receiver_types) = receiver_type else {
            return None;
        };
        let builder = &self.checker.builder;

        let mut truthy_types = Vec::new();
        let mut falsy_types = Vec::new();

        for type_ in receiver_types {
            if let Some(shape) = or_raise(builder.shape(type_, &self.config))
                && let Some(method) = or_raise(shape.method(builder, &call_type.method_name))
            {
                let method_type = method.method_types().into_iter().find(|method_type| {
                    call_type.method_decls.iter().any(|declaration| {
                        builder.factory.method_type(&declaration.method_type) == **method_type
                    })
                });
                if let Some(method_type) = method_type {
                    let (truthy, falsy) = self.partition_union(&method_type.type_.return_type);
                    if truthy.is_some() {
                        truthy_types.push(type_.clone());
                    }
                    if falsy.is_some() {
                        falsy_types.push(type_.clone());
                    }
                    continue;
                }
            }
            truthy_types.push(type_.clone());
            falsy_types.push(type_.clone());
        }

        let truthy_type = union_or_bot(truthy_types);
        let falsy_type = union_or_bot(falsy_types);

        let (truthy_env, falsy_env) =
            self.refine_node_type(env, receiver, &truthy_type, &falsy_type);

        Some((
            Result::new(type_.clone(), truthy_env, false),
            Result::new(type_.clone(), falsy_env, false),
        ))
    }

    fn literal_of_value_node(value_node: &Node) -> Option<Literal> {
        match (value_node.type_, value_node.child(0)) {
            ("int", Child::Integer(value)) => Some(Literal::Integer(value.clone())),
            ("str", Child::String(value)) => Some(Literal::String(value.clone())),
            ("sym", Child::Symbol(value)) => Some(Literal::Symbol(value.clone())),
            _ => None,
        }
    }

    pub fn literal_var_type_case_select(
        &self,
        value_node: &Node,
        argument_type: &Type,
        for_receiver: bool,
    ) -> Option<(Vec<Type>, Vec<Type>)> {
        match argument_type {
            Type::Union(types) => {
                let mut truthy_types = Vec::new();
                let mut falsy_types = Vec::new();
                for type_ in types {
                    let (truthys, falsys) =
                        self.literal_var_type_case_select(value_node, type_, for_receiver)?;
                    truthy_types.extend(truthys);
                    falsy_types.extend(falsys);
                }
                Some((truthy_types, falsy_types))
            }
            Type::Boolean | Type::Top | Type::Any => {
                Some((vec![argument_type.clone()], vec![argument_type.clone()]))
            }
            type_ => {
                let partition = |is_truthy: bool| {
                    if is_truthy {
                        (vec![type_.clone()], Vec::new())
                    } else {
                        (Vec::new(), vec![type_.clone()])
                    }
                };
                match value_node.type_ {
                    "nil" => Some(partition(
                        matches!(type_, Type::Nil) || is_instance_of(type_, "::NilClass"),
                    )),
                    "true" => Some(partition(
                        is_instance_of(type_, "::TrueClass")
                            || matches!(type_, Type::Literal(Literal::Bool(true))),
                    )),
                    "false" => Some(partition(
                        is_instance_of(type_, "::FalseClass")
                            || matches!(type_, Type::Literal(Literal::Bool(false))),
                    )),
                    "int" | "str" | "sym" => {
                        let mut true_types = Vec::new();
                        let mut false_types = Vec::new();
                        let value = Self::literal_of_value_node(value_node)?;
                        let literal_type = Type::Literal(value);
                        if let Type::Literal(_) = type_ {
                            if *type_ == literal_type {
                                true_types.push(type_.clone());
                            } else {
                                false_types.push(type_.clone());
                            }
                        } else {
                            let literal_can_be_value = match self.checker.builder.steep_version {
                                SteepVersion::Version2_0_0 => false,
                                SteepVersion::Version2_1_0 => self.subtyping(&literal_type, type_),
                            };
                            if !for_receiver || literal_can_be_value {
                                true_types.push(literal_type);
                            }
                            false_types.push(type_.clone());
                        }
                        Some((true_types, false_types))
                    }
                    _ => None,
                }
            }
        }
    }

    fn instance_type(&self, name: &TypeName) -> Type {
        self.checker.builder.factory.instance_type(name)
    }

    pub fn type_case_select(&self, type_: &Type, klass: &TypeName) -> (Option<Type>, Option<Type>) {
        let (truth_types, false_types) = self.type_case_select0(type_, klass);
        (
            (!truth_types.is_empty()).then(|| Type::union(truth_types)),
            (!false_types.is_empty()).then(|| Type::union(false_types)),
        )
    }

    pub fn type_case_select0(&self, type_: &Type, klass: &TypeName) -> (Vec<Type>, Vec<Type>) {
        let instance_type = self.instance_type(klass);

        match type_ {
            Type::Union(types) => {
                let mut truthy_types = Vec::new();
                let mut falsy_types = Vec::new();
                for type_ in types {
                    let (truths, falses) = self.type_case_select0(type_, klass);
                    if truths.is_empty() {
                        falsy_types.push(type_.clone());
                    } else {
                        truthy_types.extend(truths);
                        falsy_types.extend(falses);
                    }
                }
                (truthy_types, falsy_types)
            }
            Type::Alias { .. } => {
                let expanded = self.checker.expand_alias(type_);
                if expanded == *type_ {
                    (vec![type_.clone()], vec![type_.clone()])
                } else {
                    self.type_case_select0(&expanded, klass)
                }
            }
            Type::Any | Type::Top | Type::Var(_) | Type::Interface { .. } => {
                (vec![instance_type], vec![type_.clone()])
            }
            _ => {
                if self.subtyping(type_, &instance_type) {
                    (vec![type_.clone()], Vec::new())
                } else if self.subtyping(&instance_type, type_) {
                    (vec![instance_type], vec![type_.clone()])
                } else {
                    (Vec::new(), vec![type_.clone()])
                }
            }
        }
    }

    pub fn no_subtyping(&self, sub_type: &Type, super_type: &Type) -> Option<Rc<SubtypingResult>> {
        let result = or_raise(self.checker.builder.subtyping().check(
            Relation::new(sub_type.clone(), super_type.clone()),
            Constraints::empty().shared(),
            Some(Type::SelfType),
            Some(Type::Instance),
            Some(Type::Class),
        ));
        result.is_failure().then_some(result)
    }

    pub fn subtyping(&self, sub_type: &Type, super_type: &Type) -> bool {
        self.no_subtyping(sub_type, super_type).is_none()
    }
}
