use std::{
    rc::Rc,
    sync::atomic::{AtomicUsize, Ordering},
};

use steep_interface::types::Type;

use super::{Pair, TypeConstruction};
use crate::{
    diagnostic::ruby as diagnostics,
    ruby::node::{Child, Node, SourceRange},
    type_inference::{
        logic_type_interpreter::{LogicTypeInterpreter, Result},
        type_env::{PureCall, TypeEnv, node_key},
    },
    typing::Typing,
};

static CASE_WHEN_COUNT: AtomicUsize = AtomicUsize::new(0);

const ALPHANUMERIC: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

fn alphanumeric(count: usize) -> String {
    let mut number = count;
    (0..5)
        .map(|_| {
            let character = ALPHANUMERIC[number % ALPHANUMERIC.len()] as char;
            number /= ALPHANUMERIC.len();
            character
        })
        .collect()
}

pub fn clone_node(node: &Node) -> Rc<Node> {
    let children = node
        .children
        .iter()
        .map(|child| match child {
            Child::Node(child) => Child::Node(clone_node(child)),
            child => child.clone(),
        })
        .collect();
    node.updated(None, Some(children), None)
}

pub struct WhenPatterns<'logic, 'checker> {
    pub logic: &'logic LogicTypeInterpreter<'checker>,
    pub initial_constr: TypeConstruction<'checker>,
    pub unreachable_clause: bool,
    assignment_node: Rc<Node>,
    pub pattern_results: Vec<(Rc<Node>, Result, Result)>,
}

impl<'logic, 'checker> WhenPatterns<'logic, 'checker> {
    pub fn new(
        logic: &'logic LogicTypeInterpreter<'checker>,
        initial_constr: TypeConstruction<'checker>,
        unreachable_clause: bool,
        assignment_node: Rc<Node>,
    ) -> WhenPatterns<'logic, 'checker> {
        WhenPatterns {
            logic,
            initial_constr,
            unreachable_clause,
            assignment_node,
            pattern_results: Vec::new(),
        }
    }

    pub fn add_pattern(&mut self, pattern: &Rc<Node>) {
        let test_node = pattern.updated(
            Some("send"),
            Some(vec![
                Child::Node(pattern.clone()),
                Child::Symbol("===".to_string()),
                Child::Node(self.assignment_node()),
            ]),
            None,
        );

        let (latest_constr, _) = self.latest_result();

        latest_constr.synthesize(&test_node, None, false);
        let (truthy_result, falsy_result) =
            self.logic.eval(&latest_constr.context.type_env, &test_node);

        self.pattern_results
            .push((pattern.clone(), truthy_result, falsy_result));
    }

    pub fn latest_result(&self) -> (TypeConstruction<'checker>, bool) {
        match self.pattern_results.last() {
            Some((_, _, falsy)) => (
                self.initial_constr.update_type_env(|_| falsy.env.clone()),
                falsy.unreachable,
            ),
            None => (self.initial_constr.clone(), self.unreachable_clause),
        }
    }

    pub fn body_result(
        &self,
        propagate: impl FnOnce(&TypeEnv) -> Option<TypeEnv>,
    ) -> (TypeConstruction<'checker>, bool) {
        let type_envs: Vec<TypeEnv> = self
            .pattern_results
            .iter()
            .map(|(_, truthy, _)| truthy.env.clone())
            .collect();
        let env = self.initial_constr.context.type_env.join(&type_envs);

        let env = propagate(&env).unwrap_or(env);

        (
            self.initial_constr.update_type_env(|_| env),
            self.unreachable_clause
                || self
                    .pattern_results
                    .iter()
                    .all(|(_, truthy, _)| truthy.unreachable),
        )
    }

    pub fn falsy_result(&self) -> (TypeConstruction<'checker>, bool) {
        let (_, _, falsy) = self
            .pattern_results
            .last()
            .expect("a when clause has at least one pattern");
        (
            self.initial_constr.update_type_env(|_| falsy.env.clone()),
            self.unreachable_clause || falsy.unreachable,
        )
    }

    pub fn assignment_node(&self) -> Rc<Node> {
        clone_node(&self.assignment_node)
    }
}

pub struct CaseWhen<'logic, 'checker> {
    pub node: Rc<Node>,
    pub when_nodes: Vec<Rc<Node>>,
    pub else_node: Option<Rc<Node>>,
    pub initial_constr: TypeConstruction<'checker>,
    pub logic: &'logic LogicTypeInterpreter<'checker>,
    pub clause_results: Vec<(Result, Result)>,
    pub else_result: Option<Pair<'checker>>,
    pub assignment_node: Rc<Node>,
    pub value_node: Option<Rc<Node>>,
    pub var_name: String,
}

pub struct CaseNode {
    pub condition_node: Option<Rc<Node>>,
    pub when_nodes: Vec<Rc<Node>>,
    pub else_node: Option<Rc<Node>>,
}

pub fn deconstruct_case_node(node: &Node) -> CaseNode {
    let count = node.children.len();
    CaseNode {
        condition_node: node.child_node(0).cloned(),
        when_nodes: node.children[1..count - 1]
            .iter()
            .filter_map(Child::node)
            .cloned()
            .collect(),
        else_node: node.child_node(count - 1).cloned(),
    }
}

pub fn deconstruct_when_node(node: &Node) -> (Vec<Rc<Node>>, Option<Rc<Node>>) {
    let count = node.children.len();
    let conditions = node.children[..count - 1]
        .iter()
        .filter_map(Child::node)
        .cloned()
        .collect();
    let body = node.child_node(count - 1).cloned();
    (conditions, body)
}

impl<'logic, 'checker> CaseWhen<'logic, 'checker> {
    pub fn type_check(
        constr: &TypeConstruction<'checker>,
        node: &Rc<Node>,
        logic: &'logic LogicTypeInterpreter<'checker>,
        hint: Option<&Type>,
        condition: bool,
    ) -> (Vec<Type>, Vec<TypeEnv>) {
        let mut case_when = CaseWhen::new(node, logic, |condition_node| {
            constr.synthesize(condition_node, None, false)
        });

        case_when.when_clauses(|case_when, when_patterns, patterns, body_node| {
            for pattern in patterns {
                when_patterns.add_pattern(pattern);
            }

            let (body_constr, body_unreachable) =
                when_patterns.body_result(|env| case_when.propagate_value_node_type(env));

            let (type_, body_constr) = match body_node {
                Some(body_node) => {
                    let body_constr = body_constr.for_branch(body_node, None);
                    let pair = body_constr.synthesize(body_node, hint, condition);
                    (pair.type_, pair.constr)
                }
                None => (Type::Nil, body_constr),
            };

            let body_result = Result::new(
                type_,
                body_constr.context.type_env.clone(),
                body_unreachable,
            );

            let (falsy_constr, falsy_unreachable) = when_patterns.falsy_result();
            let next_result = Result::new(
                Type::Any,
                falsy_constr.context.type_env.clone(),
                falsy_unreachable,
            );

            (body_result, next_result)
        });

        case_when.else_clause(|else_node, constr| constr.synthesize(else_node, hint, condition));

        case_when.result()
    }

    pub fn new(
        node: &Rc<Node>,
        logic: &'logic LogicTypeInterpreter<'checker>,
        synthesize_condition: impl FnOnce(&Rc<Node>) -> Pair<'checker>,
    ) -> CaseWhen<'logic, 'checker> {
        let CaseNode {
            condition_node,
            when_nodes,
            else_node,
        } = deconstruct_case_node(node);
        let condition_node =
            condition_node.expect("CaseWhen works for case-when syntax with condition node");

        let pair = synthesize_condition(&condition_node);

        let var_name = format!(
            "__case_when:{}__",
            alphanumeric(CASE_WHEN_COUNT.fetch_add(1, Ordering::Relaxed))
        );
        let (value_node, assignment_node) = rewrite_condition_node(&var_name, &condition_node);

        let initial_constr = pair.constr.update_type_env(|env| {
            let mut env = env.clone();
            env.local_variable_types
                .insert(var_name.clone(), (pair.type_.clone(), None));
            env
        });

        CaseWhen {
            node: node.clone(),
            when_nodes,
            else_node,
            initial_constr,
            logic,
            clause_results: Vec::new(),
            else_result: None,
            assignment_node,
            value_node,
            var_name,
        }
    }

    pub fn when_clauses(
        &mut self,
        mut type_when_clause: impl FnMut(
            &CaseWhen<'logic, 'checker>,
            &mut WhenPatterns<'logic, 'checker>,
            &[Rc<Node>],
            Option<&Rc<Node>>,
        ) -> (Result, Result),
    ) {
        for when_node in self.when_nodes.clone() {
            let (clause_constr, unreachable) = self.latest_result();

            let (patterns, body) = deconstruct_when_node(&when_node);

            let mut when_patterns = WhenPatterns::new(
                self.logic,
                clause_constr,
                unreachable,
                self.assignment_node.clone(),
            );

            let (body_result, next_result) =
                type_when_clause(self, &mut when_patterns, &patterns, body.as_ref());

            if body_result.unreachable
                && (matches!(body_result.type_, Type::Any)
                    || self
                        .initial_constr
                        .no_subtyping(&body_result.type_, &Type::Bot)
                        .is_some())
            {
                self.typing()
                    .add_error(diagnostics::unreachable_value_branch(
                        &when_node,
                        &body_result.type_,
                        when_node.location.keyword,
                    ));
            }

            self.clause_results.push((body_result, next_result));
        }
    }

    pub fn else_clause(
        &mut self,
        synthesize_else: impl FnOnce(&Rc<Node>, &TypeConstruction<'checker>) -> Pair<'checker>,
    ) {
        let Some(else_location) = self.has_else_clause() else {
            return;
        };

        let (constr, unreachable) = self.latest_result();

        let constr = constr.update_type_env(|env| {
            self.propagate_value_node_type(env)
                .unwrap_or_else(|| env.clone())
        });

        let else_result = match &self.else_node {
            Some(else_node) => synthesize_else(else_node, &constr),
            None => Pair::new(Type::Nil, constr),
        };

        if unreachable
            && (matches!(else_result.type_, Type::Any)
                || self
                    .initial_constr
                    .no_subtyping(&else_result.type_, &Type::Bot)
                    .is_some())
        {
            self.typing()
                .add_error(diagnostics::unreachable_value_branch(
                    self.else_node.as_ref().unwrap_or(&self.node),
                    &else_result.type_,
                    Some(else_location),
                ));
        }

        self.else_result = Some(else_result);
    }

    pub fn latest_result(&self) -> (TypeConstruction<'checker>, bool) {
        match self.clause_results.last() {
            Some((_, falsy_result)) => (
                self.initial_constr
                    .update_type_env(|_| falsy_result.env.clone()),
                falsy_result.unreachable,
            ),
            None => (self.initial_constr.clone(), false),
        }
    }

    pub fn result(&self) -> (Vec<Type>, Vec<TypeEnv>) {
        let mut results: Vec<Result> = self
            .clause_results
            .iter()
            .filter(|(body, _)| !body.unreachable)
            .map(|(body, _)| body.clone())
            .collect();
        let (next_constr, next_clause_unreachable) = self.latest_result();

        if !next_clause_unreachable {
            match &self.else_result {
                Some(else_result) => results.push(Result::new(
                    else_result.type_.clone(),
                    else_result.constr.context.type_env.clone(),
                    false,
                )),
                None => results.push(Result::new(
                    Type::Nil,
                    next_constr.context.type_env.clone(),
                    false,
                )),
            }
        }

        results.retain(|result| !matches!(result.type_, Type::Bot));

        let types = results.iter().map(|result| result.type_.clone()).collect();
        let envs = results.into_iter().map(|result| result.env).collect();

        (types, envs)
    }

    pub fn has_else_clause(&self) -> Option<SourceRange> {
        self.node.location.else_
    }

    pub fn typing(&self) -> &Typing {
        &self.logic.typing
    }

    pub fn propagate_value_node_type(&self, env: &TypeEnv) -> Option<TypeEnv> {
        let value_node = self.value_node.as_ref()?;
        let call = self.initial_constr.typing.call_of(value_node.id)?;
        if !call.is_typed() {
            return None;
        }
        env.get_node(value_node)?;
        let mut env = env.clone();
        let type_ = env.get(&self.var_name);
        env.pure_method_calls.insert(
            node_key(value_node),
            PureCall {
                node: value_node.clone(),
                call: Rc::new(call),
                type_,
            },
        );
        Some(env)
    }
}

pub fn rewrite_condition_node(var_name: &str, node: &Rc<Node>) -> (Option<Rc<Node>>, Rc<Node>) {
    match node.type_ {
        "lvasgn" => {
            let rhs = node
                .child_node(1)
                .expect("an assignment in a condition has a right hand side");
            let (value, rhs) = rewrite_condition_node(var_name, rhs);
            (
                value,
                node.updated(
                    None,
                    Some(vec![node.child(0).clone(), Child::Node(rhs)]),
                    None,
                ),
            )
        }
        "lvar" => (
            None,
            node.updated(
                Some("lvasgn"),
                Some(vec![
                    node.child(0).clone(),
                    Child::Node(node.updated(
                        Some("lvar"),
                        Some(vec![Child::Symbol(var_name.to_string())]),
                        None,
                    )),
                ]),
                None,
            ),
        ),
        "begin" => {
            let mut children = node.children.clone();
            let last = children
                .pop()
                .and_then(|child| child.node().cloned())
                .expect("a begin node in a condition has a last expression");
            let (value_node, last) = rewrite_condition_node(var_name, &last);
            children.push(Child::Node(last));
            (value_node, node.updated(None, Some(children), None))
        }
        _ => (
            Some(node.clone()),
            node.updated(
                Some("lvar"),
                Some(vec![Child::Symbol(var_name.to_string())]),
                None,
            ),
        ),
    }
}

impl<'checker> TypeConstruction<'checker> {
    pub fn synthesize_case(
        &self,
        node: &Rc<Node>,
        hint: Option<&Type>,
        condition: bool,
    ) -> Pair<'checker> {
        let CaseNode {
            condition_node,
            when_nodes: whens,
            else_node,
        } = deconstruct_case_node(node);

        let constr = self.clone();
        let interpreter = self.logic_type_interpreter();

        let (types, envs) = if condition_node.is_some() {
            CaseWhen::type_check(&constr, node, &interpreter, hint, condition)
        } else {
            let mut branch_results: Vec<Pair<'checker>> = Vec::new();

            let mut condition_constr = constr.clone();

            for when_clause in &whens {
                let when_clause_constr = condition_constr.clone();
                let mut body_envs: Vec<TypeEnv> = Vec::new();

                let (tests, body) = deconstruct_when_node(when_clause);

                let mut branch_reachable = false;

                for test in &tests {
                    condition_constr = condition_constr.synthesize(test, None, true).constr;
                    let (truthy, falsy) =
                        interpreter.eval(&condition_constr.context.type_env, test);
                    let truthy_env = truthy.env;
                    let falsy_env = falsy.env;

                    condition_constr = condition_constr.update_type_env(|_| falsy_env);
                    body_envs.push(truthy_env);

                    branch_reachable |= !truthy.unreachable;
                }

                let branch_result = match &body {
                    Some(body) => when_clause_constr
                        .update_type_env(|env| env.join(&body_envs))
                        .for_branch(body, None)
                        .synthesize(body, hint, false),
                    None => Pair::new(Type::Nil, when_clause_constr),
                };

                if !branch_reachable && !matches!(branch_result.type_, Type::Bot) {
                    self.add_error(diagnostics::unreachable_value_branch(
                        when_clause,
                        &branch_result.type_,
                        when_clause.location.keyword,
                    ));
                }

                branch_results.push(branch_result);
            }

            match &else_node {
                Some(else_node) => {
                    branch_results.push(condition_constr.synthesize(else_node, hint, false))
                }
                None => branch_results.push(Pair::new(Type::Nil, condition_constr)),
            }

            branch_results.retain(|result| !matches!(result.type_, Type::Bot));

            let types = branch_results
                .iter()
                .map(|result| result.type_.clone())
                .collect();
            let envs = branch_results
                .into_iter()
                .map(|result| result.constr.context.type_env)
                .collect();
            (types, envs)
        };

        let constr = constr.update_type_env(|env| env.join(&envs));

        let type_ = self.union_type_unify(types);
        let branch_types: Vec<(Rc<Node>, Type)> = whens
            .iter()
            .filter_map(|when| deconstruct_when_node(when).1)
            .chain(else_node)
            .filter_map(|body| {
                let body_type = constr.typing.type_of(body.id)?;
                Some((body, body_type))
            })
            .collect();
        let branches: Vec<(&Node, &Type)> = branch_types
            .iter()
            .map(|(body, body_type)| (body.as_ref(), body_type))
            .collect();
        constr.observe_dropped_untyped_branches(node, "case", hint, &branches, &type_);
        constr.add_typing(node, type_)
    }
}
