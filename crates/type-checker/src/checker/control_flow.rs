use rbs_environment::Type;
use ruby_prism::{Node, RescueNode};

use super::narrowing::{merge_locals, narrow_to_class};
use super::{Checker, Context, Locals};
use crate::Severity;
use crate::source::Span;
use crate::subtyping::{Certainty, is_subtype};
use crate::type_utilities::{
    array_of, can_be_falsy, element_type, falsy_part, instance_type_of_class_value, is_bottom,
    members_of, remove_nil, truthy_part,
};

struct Branch {
    locals: Locals,
    result_type: Type,
}

fn merge_branches(base: &Locals, branches: Vec<Branch>) -> (Locals, Type) {
    let result_type = Type::union(
        branches
            .iter()
            .map(|branch| branch.result_type.clone())
            .collect(),
    );
    let reachable: Vec<Locals> = branches
        .into_iter()
        .filter(|branch| !is_bottom(&branch.result_type))
        .map(|branch| branch.locals)
        .collect();
    if reachable.is_empty() {
        return (base.clone(), Type::Bottom);
    }
    (merge_locals(base, reachable), result_type)
}

impl<'pr> Checker<'_, 'pr> {
    pub(crate) fn infer_conditional(
        &mut self,
        node: &Node<'pr>,
        context: &Context,
        locals: &mut Locals,
    ) -> Type {
        let (predicate, statements, subsequent, negated) = match node {
            Node::IfNode { .. } => {
                let Some(if_node) = node.as_if_node() else {
                    return Type::Untyped;
                };
                (
                    if_node.predicate(),
                    if_node.statements(),
                    if_node.subsequent(),
                    false,
                )
            }
            _ => {
                let Some(unless_node) = node.as_unless_node() else {
                    return Type::Untyped;
                };
                (
                    unless_node.predicate(),
                    unless_node.statements(),
                    unless_node
                        .else_clause()
                        .map(|else_clause| else_clause.as_node()),
                    true,
                )
            }
        };
        let predicate_type = self.infer(&predicate, context, locals);
        let (can_be_truthy, can_be_falsy) =
            self.condition_possibilities(&predicate, &predicate_type, context, locals, 0);
        self.report_unreachable_branches(node, negated, can_be_truthy, can_be_falsy);
        let mut then_locals = locals.clone();
        self.narrow(&predicate, !negated, context, &mut then_locals);
        let mut else_locals = locals.clone();
        self.narrow(&predicate, negated, context, &mut else_locals);
        let then_type = self.infer_optional_statements(statements, context, &mut then_locals);
        let else_type = match subsequent {
            Some(Node::ElseNode { .. }) => {
                let else_node = subsequent
                    .as_ref()
                    .and_then(|subsequent| subsequent.as_else_node());
                self.infer_optional_statements(
                    else_node.and_then(|else_node| else_node.statements()),
                    context,
                    &mut else_locals,
                )
            }
            Some(other) => self.infer(&other, context, &mut else_locals),
            None => Type::Nil,
        };
        let (merged, result_type) = merge_branches(
            locals,
            vec![
                Branch {
                    locals: then_locals,
                    result_type: then_type,
                },
                Branch {
                    locals: else_locals,
                    result_type: else_type,
                },
            ],
        );
        *locals = merged;
        result_type
    }

    /// Whether a condition can evaluate truthy and falsy, following what Steep's logic interpreter marks unreachable
    fn condition_possibilities(
        &mut self,
        condition: &Node<'pr>,
        condition_type: &Type,
        context: &Context,
        locals: &Locals,
        depth: usize,
    ) -> (bool, bool) {
        if depth > 16 || condition_type.is_untyped() || is_bottom(condition_type) {
            return (true, true);
        }
        let by_type = (
            !is_bottom(&truthy_part(condition_type)),
            can_be_falsy(condition_type),
        );
        let next = depth + 1;
        let sub_possibilities = |checker: &mut Self, node: &Node<'pr>| -> (bool, bool) {
            let mut scratch_locals = locals.clone();
            let node_type = checker
                .silently(|checker| checker.infer(node, context, &mut scratch_locals))
                .unwrap_or(Type::Untyped);
            checker.condition_possibilities(node, &node_type, context, locals, next)
        };
        match condition {
            Node::ParenthesesNode { .. } => {
                let statement = condition
                    .as_parentheses_node()
                    .and_then(|parentheses| parentheses.body())
                    .and_then(|body| super::narrowing::single_statement(&body));
                match statement {
                    Some(statement) => sub_possibilities(self, &statement),
                    None => by_type,
                }
            }
            Node::AndNode { .. } => {
                let Some(and) = condition.as_and_node() else {
                    return by_type;
                };
                let (left_truthy, left_falsy) = sub_possibilities(self, &and.left());
                let (right_truthy, right_falsy) = sub_possibilities(self, &and.right());
                (left_truthy && right_truthy, left_falsy || right_falsy)
            }
            Node::OrNode { .. } => {
                let Some(or) = condition.as_or_node() else {
                    return by_type;
                };
                let (left_truthy, left_falsy) = sub_possibilities(self, &or.left());
                let (right_truthy, right_falsy) = sub_possibilities(self, &or.right());
                (left_truthy || right_truthy, left_falsy && right_falsy)
            }
            Node::CallNode { .. } => {
                let Some(call) = condition.as_call_node() else {
                    return by_type;
                };
                let method_name = call.name();
                let arguments: Vec<Node<'pr>> = call
                    .arguments()
                    .map(|arguments| arguments.arguments().iter().collect())
                    .unwrap_or_default();
                let Some(receiver) = call.receiver() else {
                    return by_type;
                };
                match (method_name.as_slice(), arguments.as_slice()) {
                    (b"!", []) => {
                        let (truthy, falsy) = sub_possibilities(self, &receiver);
                        (falsy, truthy)
                    }
                    (b"nil?", []) => {
                        let mut scratch_locals = locals.clone();
                        let receiver_type = self
                            .silently(|checker| {
                                checker.infer(&receiver, context, &mut scratch_locals)
                            })
                            .unwrap_or(Type::Untyped);
                        if receiver_type.is_untyped() {
                            return (true, true);
                        }
                        let nil_possible = is_subtype(self.environment, &Type::Nil, &receiver_type)
                            != Certainty::No;
                        let non_nil_possible = !is_bottom(&remove_nil(&receiver_type));
                        (nil_possible, non_nil_possible)
                    }
                    (b"==", [argument]) => self
                        .equality_possibilities(&receiver, argument, context, locals)
                        .unwrap_or(by_type),
                    _ => by_type,
                }
            }
            _ => by_type,
        }
    }

    fn equality_possibilities(
        &mut self,
        receiver: &Node<'pr>,
        argument: &Node<'pr>,
        context: &Context,
        locals: &Locals,
    ) -> Option<(bool, bool)> {
        let mut scratch_locals = locals.clone();
        let receiver_type =
            self.silently(|checker| checker.infer(receiver, context, &mut scratch_locals))?;
        let (equal_types, unequal_types) =
            self.literal_equality_split(&receiver_type, argument, context)?;
        Some((!equal_types.is_empty(), !unequal_types.is_empty()))
    }

    fn report_unreachable_branches(
        &mut self,
        node: &Node<'pr>,
        negated: bool,
        can_be_truthy: bool,
        can_be_falsy: bool,
    ) {
        let (body_reachable, else_reachable) = if negated {
            (can_be_falsy, can_be_truthy)
        } else {
            (can_be_truthy, can_be_falsy)
        };
        let mut locations = Vec::new();
        match node {
            Node::IfNode { .. } => {
                let Some(if_node) = node.as_if_node() else {
                    return;
                };
                if !body_reachable && let Some(statements) = if_node.statements() {
                    let is_ternary = if_node.if_keyword_loc().is_none();
                    let span = if is_ternary && self.options.steep_compatible {
                        Span::of_node(&statements.as_node())
                    } else {
                        if_node
                            .then_keyword_loc()
                            .or_else(|| if_node.if_keyword_loc())
                            .map(|location| Span::of_location(&location))
                            .unwrap_or_else(|| Span::of_node(&statements.as_node()))
                    };
                    locations.push(span);
                }
                if !else_reachable && let Some(subsequent) = if_node.subsequent() {
                    let span = match &subsequent {
                        Node::ElseNode { .. } => subsequent
                            .as_else_node()
                            .map(|else_node| Span::of_location(&else_node.else_keyword_loc())),
                        Node::IfNode { .. } => subsequent
                            .as_if_node()
                            .and_then(|elsif| elsif.if_keyword_loc())
                            .map(|location| Span::of_location(&location)),
                        _ => None,
                    };
                    let span = match (if_node.if_keyword_loc(), &subsequent) {
                        (None, Node::ElseNode { .. }) => subsequent
                            .as_else_node()
                            .and_then(|else_node| else_node.statements())
                            .map(|statements| Span::of_node(&statements.as_node()))
                            .or(span),
                        _ => span,
                    };
                    locations.extend(span);
                }
            }
            _ => {
                let Some(unless_node) = node.as_unless_node() else {
                    return;
                };
                if !body_reachable && unless_node.statements().is_some() {
                    let location = unless_node
                        .then_keyword_loc()
                        .unwrap_or_else(|| unless_node.keyword_loc());
                    locations.push(Span::of_location(&location));
                }
                if !else_reachable && let Some(else_clause) = unless_node.else_clause() {
                    locations.push(Span::of_location(&else_clause.else_keyword_loc()));
                }
            }
        }
        for span in locations {
            self.record_diagnostic(
                span,
                Severity::Hint,
                "Ruby::UnreachableBranch",
                "The branch is unreachable".to_string(),
            );
        }
    }

    pub(crate) fn infer_and(
        &mut self,
        node: &Node<'pr>,
        context: &Context,
        locals: &mut Locals,
    ) -> Type {
        let Some(and) = node.as_and_node() else {
            return Type::Untyped;
        };
        let left = and.left();
        let left_type = self.infer(&left, context, locals);
        let mut right_locals = locals.clone();
        self.narrow(&left, true, context, &mut right_locals);
        let right_type = self.infer(&and.right(), context, &mut right_locals);
        let mut short_circuit_locals = locals.clone();
        self.narrow(&left, false, context, &mut short_circuit_locals);
        let falsy = if left_type.is_untyped() {
            Type::Untyped
        } else {
            falsy_part(&left_type)
        };
        let (merged, _) = merge_branches(
            locals,
            vec![
                Branch {
                    locals: short_circuit_locals,
                    result_type: if is_bottom(&falsy) {
                        Type::Bottom
                    } else {
                        Type::Nil
                    },
                },
                Branch {
                    locals: right_locals,
                    result_type: right_type.clone(),
                },
            ],
        );
        *locals = merged;
        Type::union(vec![falsy, right_type])
    }

    pub(crate) fn infer_or(
        &mut self,
        node: &Node<'pr>,
        context: &Context,
        locals: &mut Locals,
    ) -> Type {
        let Some(or) = node.as_or_node() else {
            return Type::Untyped;
        };
        let left = or.left();
        let left_type = self.infer(&left, context, locals);
        let mut right_locals = locals.clone();
        self.narrow(&left, false, context, &mut right_locals);
        let right_type = self.infer(&or.right(), context, &mut right_locals);
        if !left_type.is_untyped() && !can_be_falsy(&left_type) {
            return left_type;
        }
        let mut short_circuit_locals = locals.clone();
        self.narrow(&left, true, context, &mut short_circuit_locals);
        let truthy = truthy_part(&left_type);
        let (merged, _) = merge_branches(
            locals,
            vec![
                Branch {
                    locals: short_circuit_locals,
                    result_type: if is_bottom(&truthy) {
                        Type::Bottom
                    } else {
                        Type::Nil
                    },
                },
                Branch {
                    locals: right_locals,
                    result_type: right_type.clone(),
                },
            ],
        );
        *locals = merged;
        Type::union(vec![truthy, right_type])
    }

    pub(crate) fn infer_case(
        &mut self,
        node: &Node<'pr>,
        context: &Context,
        locals: &mut Locals,
    ) -> Type {
        let Some(case) = node.as_case_node() else {
            return Type::Untyped;
        };
        let predicate = case.predicate();
        if let Some(predicate) = &predicate {
            self.infer(predicate, context, locals);
        }
        let target = predicate.as_ref().and_then(Checker::narrowing_target);
        let mut branches = Vec::new();
        let mut matched_instance_types: Vec<Type> = Vec::new();
        let mut all_conditions_are_classes = true;
        for condition_node in case.conditions().iter() {
            let Some(when) = condition_node.as_when_node() else {
                continue;
            };
            let mut branch_locals = locals.clone();
            let mut instance_types = Vec::new();
            let mut conditions_are_classes = true;
            let conditions: Vec<Node<'pr>> = when.conditions().iter().collect();
            for condition in &conditions {
                let condition_type = self.infer(condition, context, locals);
                match instance_type_of_class_value(self.environment, &condition_type) {
                    Some(instance_type)
                        if matches!(
                            condition,
                            Node::ConstantReadNode { .. } | Node::ConstantPathNode { .. }
                        ) =>
                    {
                        instance_types.push(instance_type)
                    }
                    _ if matches!(condition, Node::NilNode { .. }) => {
                        instance_types.push(Type::Nil)
                    }
                    _ => conditions_are_classes = false,
                }
            }
            match (&target, predicate.is_some()) {
                (Some(target), _) if conditions_are_classes && !instance_types.is_empty() => {
                    let narrowed_to = Type::union(instance_types.clone());
                    let environment = self.environment;
                    self.narrow_target_with(target, context, &mut branch_locals, |current| {
                        narrow_to_class(environment, current, &narrowed_to)
                    });
                    matched_instance_types.extend(instance_types);
                }
                (None, false) if conditions.len() == 1 => {
                    self.narrow(&conditions[0], true, context, &mut branch_locals)
                }
                _ => all_conditions_are_classes = false,
            }
            all_conditions_are_classes &= conditions_are_classes;
            let result_type =
                self.infer_optional_statements(when.statements(), context, &mut branch_locals);
            branches.push(Branch {
                locals: branch_locals,
                result_type,
            });
        }
        let mut else_locals = locals.clone();
        if let Some(target) = &target
            && all_conditions_are_classes
            && !matched_instance_types.is_empty()
        {
            let environment = self.environment;
            self.narrow_target_with(target, context, &mut else_locals, |current| {
                Type::union(
                    members_of(current)
                        .into_iter()
                        .filter(|member| {
                            !matched_instance_types.iter().any(|matched| {
                                is_subtype(environment, member, matched) == Certainty::Yes
                            })
                        })
                        .collect(),
                )
            });
        }
        let else_type = match case.else_clause() {
            Some(else_clause) => {
                self.infer_optional_statements(else_clause.statements(), context, &mut else_locals)
            }
            None => Type::Nil,
        };
        branches.push(Branch {
            locals: else_locals,
            result_type: else_type,
        });
        let (merged, result_type) = merge_branches(locals, branches);
        *locals = merged;
        result_type
    }

    pub(crate) fn infer_case_match(
        &mut self,
        node: &Node<'pr>,
        context: &Context,
        locals: &mut Locals,
    ) -> Type {
        let Some(case_match) = node.as_case_match_node() else {
            return Type::Untyped;
        };
        self.record_diagnostic(
            Span::of_node(node),
            Severity::Hint,
            "Ruby::UnsupportedSyntax",
            "Syntax `case_match` is not supported in Steep".to_string(),
        );
        let predicate_type = self.infer_optional_node(case_match.predicate(), context, locals);
        let mut branches = Vec::new();
        for condition in case_match.conditions().iter() {
            let Some(in_node) = condition.as_in_node() else {
                continue;
            };
            let mut branch_locals = locals.clone();
            self.bind_pattern(
                &in_node.pattern(),
                &predicate_type,
                context,
                &mut branch_locals,
                0,
            );
            let result_type =
                self.infer_optional_statements(in_node.statements(), context, &mut branch_locals);
            branches.push(Branch {
                locals: branch_locals,
                result_type,
            });
        }
        let mut else_locals = locals.clone();
        let else_type = match case_match.else_clause() {
            Some(else_clause) => {
                self.infer_optional_statements(else_clause.statements(), context, &mut else_locals)
            }
            None => Type::Bottom,
        };
        branches.push(Branch {
            locals: else_locals,
            result_type: else_type,
        });
        let (merged, result_type) = merge_branches(locals, branches);
        *locals = merged;
        result_type
    }

    pub(crate) fn infer_pattern_match(
        &mut self,
        node: &Node<'pr>,
        context: &Context,
        locals: &mut Locals,
    ) -> Type {
        let (value, pattern, is_predicate) = match node {
            Node::MatchPredicateNode { .. } => {
                let Some(predicate) = node.as_match_predicate_node() else {
                    return Type::Untyped;
                };
                (predicate.value(), predicate.pattern(), true)
            }
            _ => {
                let Some(required) = node.as_match_required_node() else {
                    return Type::Untyped;
                };
                (required.value(), required.pattern(), false)
            }
        };
        let value_type = self.infer(&value, context, locals);
        self.bind_pattern(&pattern, &value_type, context, locals, 0);
        if is_predicate { Type::Bool } else { Type::Nil }
    }

    fn bind_pattern(
        &mut self,
        pattern: &Node<'pr>,
        matched_type: &Type,
        context: &Context,
        locals: &mut Locals,
        depth: usize,
    ) {
        if depth > 32 {
            return;
        }
        match pattern {
            Node::LocalVariableTargetNode { .. } => {
                self.assign_target(pattern, matched_type.clone(), context, locals)
            }
            Node::CapturePatternNode { .. } => {
                let Some(capture) = pattern.as_capture_pattern_node() else {
                    return;
                };
                let value = capture.value();
                let captured_type = match self.class_value_instance_type(&value, context, locals) {
                    Some(instance_type) => instance_type,
                    None => matched_type.clone(),
                };
                self.bind_pattern(&value, &captured_type, context, locals, depth + 1);
                self.assign_target(&capture.target().as_node(), captured_type, context, locals);
            }
            Node::ConstantReadNode { .. } | Node::ConstantPathNode { .. } => {
                self.infer(pattern, context, locals);
            }
            Node::ArrayPatternNode { .. }
            | Node::FindPatternNode { .. }
            | Node::HashPatternNode { .. }
            | Node::AlternationPatternNode { .. } => {
                for child in prism_child_nodes::child_nodes(pattern) {
                    self.bind_pattern(&child, &Type::Untyped, context, locals, depth + 1);
                }
            }
            Node::AssocNode { .. } => {
                if let Some(association) = pattern.as_assoc_node() {
                    self.bind_pattern(
                        &association.value(),
                        &Type::Untyped,
                        context,
                        locals,
                        depth + 1,
                    );
                }
            }
            Node::ImplicitNode { .. } => {
                if let Some(implicit) = pattern.as_implicit_node() {
                    self.bind_pattern(
                        &implicit.value(),
                        &Type::Untyped,
                        context,
                        locals,
                        depth + 1,
                    );
                }
            }
            Node::SplatNode { .. } => {
                if let Some(expression) =
                    pattern.as_splat_node().and_then(|splat| splat.expression())
                {
                    self.bind_pattern(
                        &expression,
                        &array_of(Type::Untyped),
                        context,
                        locals,
                        depth + 1,
                    );
                }
            }
            Node::AssocSplatNode { .. } => {
                if let Some(value) = pattern
                    .as_assoc_splat_node()
                    .and_then(|splat| splat.value())
                {
                    self.bind_pattern(&value, &Type::Untyped, context, locals, depth + 1);
                }
            }
            Node::PinnedExpressionNode { .. } | Node::PinnedVariableNode { .. } => {
                for child in prism_child_nodes::child_nodes(pattern) {
                    self.infer(&child, context, locals);
                }
            }
            _ => {}
        }
    }

    pub(crate) fn infer_loop(
        &mut self,
        node: &Node<'pr>,
        context: &Context,
        locals: &mut Locals,
    ) -> Type {
        let (predicate, statements, is_until) = match node {
            Node::WhileNode { .. } => {
                let Some(while_node) = node.as_while_node() else {
                    return Type::Untyped;
                };
                (while_node.predicate(), while_node.statements(), false)
            }
            _ => {
                let Some(until_node) = node.as_until_node() else {
                    return Type::Untyped;
                };
                (until_node.predicate(), until_node.statements(), true)
            }
        };
        self.infer(&predicate, context, locals);
        let mut body_locals = locals.clone();
        self.narrow(&predicate, !is_until, context, &mut body_locals);
        let (body_locals, _) =
            self.infer_repeatedly(&body_locals, &[], &[], |checker, iteration_locals| {
                checker.block_next_frames.push(Vec::new());
                checker.jump_expectations.push(super::JumpExpectation {
                    break_type: None,
                    next_type: None,
                });
                let statements = statements
                    .as_ref()
                    .map(|statements| statements.as_node())
                    .and_then(|node| node.as_statements_node());
                checker.infer_optional_statements(statements, context, iteration_locals);
                checker.block_next_frames.pop();
                checker.jump_expectations.pop();
            });
        let mut exit_locals = merge_locals(locals, vec![locals.clone(), body_locals]);
        self.narrow(&predicate, is_until, context, &mut exit_locals);
        *locals = exit_locals;
        Type::Nil
    }

    pub(crate) fn infer_for(
        &mut self,
        node: &Node<'pr>,
        context: &Context,
        locals: &mut Locals,
    ) -> Type {
        let Some(for_node) = node.as_for_node() else {
            return Type::Untyped;
        };
        let collection_type = self.infer(&for_node.collection(), context, locals);
        let mut body_locals = locals.clone();
        self.assign_target(
            &for_node.index(),
            element_type(&collection_type),
            context,
            &mut body_locals,
        );
        let body_locals = body_locals;
        let (body_locals, _) =
            self.infer_repeatedly(&body_locals, &[], &[], |checker, iteration_locals| {
                checker.block_next_frames.push(Vec::new());
                checker.jump_expectations.push(super::JumpExpectation {
                    break_type: None,
                    next_type: None,
                });
                checker.infer_optional_statements(for_node.statements(), context, iteration_locals);
                checker.block_next_frames.pop();
                checker.jump_expectations.pop();
            });
        *locals = merge_locals(locals, vec![locals.clone(), body_locals]);
        collection_type
    }

    pub(crate) fn infer_begin(
        &mut self,
        node: &Node<'pr>,
        context: &Context,
        locals: &mut Locals,
    ) -> Type {
        let Some(begin) = node.as_begin_node() else {
            return Type::Untyped;
        };
        let base = locals.clone();
        let body_type = self.infer_optional_statements(begin.statements(), context, locals);
        let mut branches = Vec::new();
        if let Some(rescue_clause) = begin.rescue_clause() {
            let rescue_start = merge_locals(&base, vec![base.clone(), locals.clone()]);
            let mut current_rescue = Some(rescue_clause);
            let mut count = 0;
            while let Some(rescue) = current_rescue {
                count += 1;
                if count > 256 {
                    break;
                }
                let mut rescue_locals = rescue_start.clone();
                let result_type = self.infer_rescue_clause(&rescue, context, &mut rescue_locals);
                branches.push(Branch {
                    locals: rescue_locals,
                    result_type,
                });
                current_rescue = rescue.subsequent();
            }
        }
        let main_type = match begin.else_clause() {
            Some(else_clause) if !is_bottom(&body_type) => {
                self.infer_optional_statements(else_clause.statements(), context, locals)
            }
            _ => body_type,
        };
        branches.insert(
            0,
            Branch {
                locals: locals.clone(),
                result_type: main_type,
            },
        );
        let (mut merged, result_type) = merge_branches(&base, branches);
        if let Some(ensure) = begin.ensure_clause() {
            // Steep types `ensure` with the variables known before `begin`, so ones first assigned in the body are
            // `FallbackAny` there
            let mut ensure_locals = merged.clone();
            let assigned_in_body: Vec<String> = merged
                .keys()
                .filter(|name| {
                    !base.contains_key(*name) && !name.starts_with('@') && !name.starts_with('\0')
                })
                .cloned()
                .collect();
            for name in assigned_in_body {
                ensure_locals.insert(super::variables::fallback_local_key(&name), Type::Untyped);
            }
            self.infer_optional_statements(ensure.statements(), context, &mut ensure_locals);
            ensure_locals.retain(|name, _| !name.starts_with("\0fallback:"));
            merged = ensure_locals;
        }
        *locals = merged;
        result_type
    }

    fn infer_rescue_clause(
        &mut self,
        rescue: &RescueNode<'pr>,
        context: &Context,
        locals: &mut Locals,
    ) -> Type {
        let mut exception_types = Vec::new();
        for exception in rescue.exceptions().iter() {
            if self.options.steep_compatible && matches!(exception, Node::SplatNode { .. }) {
                self.record_diagnostic(
                    Span::of_node(&exception),
                    Severity::Hint,
                    "Ruby::UnsupportedSyntax",
                    "Unsupported splat node occurrence".to_string(),
                );
            }
            let exception_value = self.infer(&exception, context, locals);
            match instance_type_of_class_value(self.environment, &exception_value) {
                Some(instance_type) => exception_types.push(instance_type),
                None => exception_types.push(Type::Untyped),
            }
        }
        let exception_type = if exception_types.is_empty() {
            Type::instance("StandardError")
        } else {
            Type::union(exception_types)
        };
        if let Some(reference) = rescue.reference() {
            self.assign_target(&reference, exception_type, context, locals);
        }
        self.infer_optional_statements(rescue.statements(), context, locals)
    }

    pub(crate) fn infer_return(
        &mut self,
        node: &Node<'pr>,
        context: &Context,
        locals: &mut Locals,
    ) -> Type {
        let Some(return_node) = node.as_return_node() else {
            return Type::Bottom;
        };
        let returned_type = self.infer_jump_value(return_node.arguments(), context, locals);
        if let Some(frame) = self.method_return_frames.last_mut() {
            frame.push(returned_type.clone());
        }
        if let Some(method) = &context.method
            && let Some(declared) = &method.declared_return_type
            && !matches!(declared, Type::Void)
            && self.is_certain_mismatch(&returned_type, declared)
            && self.return_value_mismatches(&return_node, declared, &returned_type, context, locals)
        {
            self.record_diagnostic(
                Span::of_node(node),
                Severity::Error,
                "Ruby::ReturnTypeMismatch",
                format!("The method cannot return a value of type `{returned_type}` because declared as type `{declared}`"),
            );
        }
        Type::Bottom
    }

    fn return_value_mismatches(
        &mut self,
        return_node: &ruby_prism::ReturnNode<'pr>,
        declared: &Type,
        returned_type: &Type,
        context: &Context,
        locals: &Locals,
    ) -> bool {
        let hinted =
            self.hinted_return_value_type(return_node, declared, returned_type, context, locals);
        self.is_certain_mismatch(&hinted, declared)
    }

    fn hinted_return_value_type(
        &mut self,
        return_node: &ruby_prism::ReturnNode<'pr>,
        declared: &Type,
        returned_type: &Type,
        context: &Context,
        locals: &Locals,
    ) -> Type {
        let values: Vec<Node<'pr>> = return_node
            .arguments()
            .map(|arguments| arguments.arguments().iter().collect())
            .unwrap_or_default();
        match values.as_slice() {
            [value] => self.hinted_value_type(value, declared, returned_type, context, locals, 0),
            _ => returned_type.clone(),
        }
    }

    pub(crate) fn infer_next(
        &mut self,
        node: &Node<'pr>,
        context: &Context,
        locals: &mut Locals,
    ) -> Type {
        let Some(next_node) = node.as_next_node() else {
            return Type::Bottom;
        };
        let next_type = self.infer_jump_value(next_node.arguments(), context, locals);
        if let Some(expected) = self
            .jump_expectations
            .last()
            .and_then(|expectation| expectation.next_type.clone())
            && self.is_certain_mismatch(&next_type, &expected)
        {
            self.record_diagnostic(
                Span::of_node(node),
                Severity::Hint,
                "Ruby::BreakTypeMismatch",
                format!("Cannot break with a value of type `{next_type}` because type `{expected}` is assumed"),
            );
        }
        if let Some(frame) = self.block_next_frames.last_mut() {
            frame.push(next_type);
        }
        Type::Bottom
    }

    pub(crate) fn infer_break(
        &mut self,
        node: &Node<'pr>,
        context: &Context,
        locals: &mut Locals,
    ) -> Type {
        let Some(break_node) = node.as_break_node() else {
            return Type::Bottom;
        };
        let has_value = break_node.arguments().is_some();
        let break_type = self.infer_jump_value(break_node.arguments(), context, locals);
        let Some(expected) = self
            .jump_expectations
            .last()
            .and_then(|expectation| expectation.break_type.clone())
        else {
            return Type::Bottom;
        };
        if matches!(expected, Type::Void) || !self.is_certain_mismatch(&break_type, &expected) {
            return Type::Bottom;
        }
        if has_value {
            self.record_diagnostic(
                Span::of_node(node),
                Severity::Hint,
                "Ruby::BreakTypeMismatch",
                format!("Cannot break with a value of type `{break_type}` because type `{expected}` is assumed"),
            );
        } else {
            self.record_diagnostic(
                Span::of_node(node),
                Severity::Hint,
                "Ruby::ImplicitBreakValueMismatch",
                format!("Breaking without a value may result an error because a value of type `{expected}` is expected"),
            );
        }
        Type::Bottom
    }

    fn infer_jump_value(
        &mut self,
        arguments: Option<ruby_prism::ArgumentsNode<'pr>>,
        context: &Context,
        locals: &mut Locals,
    ) -> Type {
        let Some(arguments) = arguments else {
            return Type::Nil;
        };
        let values: Vec<Type> = arguments
            .arguments()
            .iter()
            .map(|argument| self.infer(&argument, context, locals))
            .collect();
        match values.len() {
            0 => Type::Nil,
            1 => values.into_iter().next().unwrap_or(Type::Nil),
            _ => Type::Tuple(values),
        }
    }

    pub(crate) fn is_certain_mismatch(&self, actual: &Type, declared: &Type) -> bool {
        !is_bottom(actual) && is_subtype(self.environment, actual, declared) == Certainty::No
    }

    /// Steep does not pass the declared type to the receiver of a call, so `[{ key: 1 }].freeze` stays
    /// `Array[Hash[Symbol, Integer]]` and is certainly not an array of records. A `void` value fits only `void`,
    /// `top` and `untyped`
    pub(crate) fn is_certain_assignment_mismatch(
        &self,
        actual: &Type,
        declared: &Type,
        value_node: Option<&Node<'_>>,
    ) -> bool {
        if self.options.steep_compatible && matches!(actual, Type::Void) {
            return !matches!(declared, Type::Void | Type::Top | Type::Untyped);
        }
        let is_call_on_collection_literal = value_node
            .and_then(|value_node| value_node.as_call_node())
            .and_then(|call| call.receiver())
            .is_some_and(|receiver| {
                matches!(receiver, Node::ArrayNode { .. } | Node::HashNode { .. })
            });
        if !self.options.steep_compatible || !is_call_on_collection_literal {
            return self.is_certain_mismatch(actual, declared);
        }
        !is_bottom(actual)
            && crate::subtyping::is_subtype_with_strict_collections(
                self.environment,
                actual,
                declared,
            ) == Certainty::No
    }
}
