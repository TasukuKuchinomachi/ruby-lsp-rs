use std::rc::Rc;

use rbs_definition::ast::{Literal, TypeName};
use steep_interface::types::Type;

use crate::{
    ruby::node::{Child, Node, NodeId, SourceRange},
    strict::{Observation, StrictCheck},
    type_construction::TypeConstruction,
    type_inference::method_call::MethodCallKind,
};

fn is_untyped(type_: &Type) -> bool {
    matches!(type_, Type::Any)
}

fn is_hash_instance(type_: &Type) -> bool {
    matches!(type_, Type::NameInstance { name, args } if *name == TypeName::parse("::Hash") && args.len() == 2)
}

fn tells_more_than_truthiness(type_: &Type) -> bool {
    match type_ {
        Type::Any
        | Type::Bot
        | Type::Top
        | Type::Void
        | Type::Boolean
        | Type::Nil
        | Type::Logic(_)
        | Type::Literal(Literal::Bool(_)) => false,
        Type::Alias { name, .. } => *name != TypeName::parse("::boolish"),
        Type::Union(types) => types.iter().any(tells_more_than_truthiness),
        _ => true,
    }
}

fn other_types(types: &[&Type]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for type_ in types {
        if is_untyped(type_) || matches!(type_, Type::Bot) {
            continue;
        }
        let name = type_.to_string();
        if !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

fn parenthesized_expression(node: &Node) -> &Node {
    let mut node = node;
    while node.type_ == "begin" && node.children.len() == 1 {
        match node.child_node(0) {
            Some(child) => node = child,
            None => break,
        }
    }
    node
}

fn is_made_untyped_explicitly(node: &Node) -> bool {
    let node = parenthesized_expression(node);
    match node.type_ {
        "lvasgn"
            if node
                .child_symbol(0)
                .is_some_and(|name| name == "_" || name == "__any__") =>
        {
            true
        }
        "lvasgn" | "ivasgn" | "gvasgn" => node
            .child_node(1)
            .is_some_and(|value| is_made_untyped_explicitly(value)),
        "begin" | "kwbegin" => node
            .child_nodes()
            .last()
            .is_some_and(|last| is_made_untyped_explicitly(last)),
        "assertion" => true,
        _ => false,
    }
}

fn path_to(root: &Rc<Node>, id: NodeId, path: &mut Vec<Rc<Node>>) -> bool {
    path.push(root.clone());
    if root.id == id {
        return true;
    }
    for child in root.child_nodes() {
        if path_to(child, id, path) {
            return true;
        }
    }
    path.pop();
    false
}

fn collect_parameter_names(node: &Node, names: &mut Vec<String>) {
    match node.type_ {
        "arg" | "optarg" | "restarg" | "kwarg" | "kwoptarg" | "kwrestarg" | "blockarg"
        | "shadowarg" => names.extend(node.child_symbol(0).map(str::to_string)),
        "procarg0" if node.child_symbol(0).is_some() => {
            names.extend(node.child_symbol(0).map(str::to_string))
        }
        _ => {
            for child in node.child_nodes() {
                collect_parameter_names(child, names);
            }
        }
    }
}

fn block_parameter_names(block: &Node) -> Vec<String> {
    match block.type_ {
        "numblock" => match block.child(1) {
            Child::Integer(maximum) => (1..=maximum.parse::<usize>().unwrap_or(0))
                .map(|number| format!("_{number}"))
                .collect(),
            _ => Vec::new(),
        },
        "itblock" => vec!["it".to_string()],
        _ => {
            let mut names = Vec::new();
            if let Some(parameters) = block.child_node(1) {
                collect_parameter_names(parameters, &mut names);
            }
            names
        }
    }
}

const SCOPE_NODE_TYPES: &[&str] = &["def", "defs", "class", "module", "sclass"];

const PARAMETER_NODE_TYPES: &[&str] = &[
    "arg",
    "optarg",
    "restarg",
    "kwarg",
    "kwoptarg",
    "kwrestarg",
    "blockarg",
    "shadowarg",
    "procarg0",
];

const MAXIMUM_ORIGIN_DEPTH: usize = 16;

const ORIGIN_CHECKS: &[StrictCheck] = &[
    StrictCheck::UnsupportedSyntax,
    StrictCheck::DefinedExpression,
    StrictCheck::UnexpectedSplat,
    StrictCheck::UnexpectedAssignmentTarget,
    StrictCheck::UntypedMethodParameter,
    StrictCheck::UntypedYieldArguments,
    StrictCheck::UntypedBlockParameters,
    StrictCheck::IgnoredBlockParameter,
    StrictCheck::BlockGivenToUntypedMethod,
    StrictCheck::UntypedLambdaParameters,
    StrictCheck::BareRescueVariable,
    StrictCheck::UntypedRescueVariable,
    StrictCheck::UntypedForVariable,
    StrictCheck::UntypedMultipleAssignment,
];

enum UntypedOrigin {
    Unreported,
    Explained,
    ReportedBy(StrictCheck),
}

struct Binding {
    node: Rc<Node>,
    owner: Option<Rc<Node>>,
    parent: Option<Rc<Node>>,
}

fn collect_bindings(
    node: &Rc<Node>,
    name: &str,
    owner: Option<&Rc<Node>>,
    parent: Option<&Rc<Node>>,
    is_scope: bool,
    bindings: &mut Vec<Binding>,
) {
    if !is_scope && SCOPE_NODE_TYPES.contains(&node.type_) {
        return;
    }
    let binds_name = node.child_symbol(0) == Some(name);
    if binds_name && node.type_ == "lvasgn" {
        bindings.push(Binding {
            node: node.clone(),
            owner: None,
            parent: parent.cloned(),
        });
    } else if binds_name && PARAMETER_NODE_TYPES.contains(&node.type_) {
        bindings.push(Binding {
            node: node.clone(),
            owner: owner.cloned(),
            parent: parent.cloned(),
        });
    }
    for child in node.child_nodes() {
        let child_owner = if child.type_ == "args" && matches!(node.type_, "block" | "def" | "defs")
        {
            Some(node)
        } else if matches!(node.type_, "args" | "mlhs" | "procarg0") {
            owner
        } else {
            None
        };
        collect_bindings(child, name, child_owner, Some(node), false, bindings);
    }
}

fn is_child_at(parent: &Node, index: usize, child: &Node) -> bool {
    parent
        .child_node(index)
        .is_some_and(|node| node.id == child.id)
}

impl<'checker> TypeConstruction<'checker> {
    fn ancestors(&self, node: &Node) -> Vec<Rc<Node>> {
        let mut path = Vec::new();
        if let Some(root) = &self.source.node
            && path_to(root, node.id, &mut path)
        {
            path.pop();
        }
        path
    }

    fn value_is_discarded(&self, node: &Node) -> bool {
        let Some(root) = &self.source.node else {
            return false;
        };
        let mut path = Vec::new();
        if !path_to(root, node.id, &mut path) {
            return false;
        }
        for index in (1..path.len()).rev() {
            let (parent, child) = (&path[index - 1], &path[index]);
            let is_last = parent
                .child_nodes()
                .last()
                .is_some_and(|last| last.id == child.id);
            match parent.type_ {
                "begin" | "kwbegin" if !is_last => return true,
                "begin" | "kwbegin" | "rescue" => {}
                "if" | "case" if !is_child_at(parent, 0, child) => {}
                "when" if is_last => {}
                "resbody" if is_child_at(parent, 2, child) => {}
                "ensure" => {
                    if is_child_at(parent, 1, child) {
                        return true;
                    }
                }
                "while" | "until" | "while_post" | "until_post" | "module" | "sclass" => {
                    return is_child_at(parent, 1, child);
                }
                "for" | "class" => return is_child_at(parent, 2, child),
                _ => return false,
            }
        }
        true
    }

    fn value_is_only_tested_or_interpolated(&self, node: &Node) -> bool {
        let ancestors = self.ancestors(node);
        let mut child_id = node.id;
        for parent in ancestors.iter().rev() {
            match parent.type_ {
                "and" | "or" => {}
                "begin" if parent.children.len() == 1 => {}
                "send" if parent.child_symbol(1) == Some("!") => return true,
                "dstr" | "dsym" | "xstr" | "regexp" => return true,
                "if" | "while" | "until" | "while_post" | "until_post" => {
                    return parent
                        .child_node(0)
                        .is_some_and(|condition| condition.id == child_id);
                }
                _ => return false,
            }
            child_id = parent.id;
        }
        false
    }

    fn value_is_unused(&self, node: &Node) -> bool {
        self.is_argument_of_untyped_receiver(node)
            || self.value_is_discarded(node)
            || self.value_is_only_tested_or_interpolated(node)
    }

    fn value_is_stored_or_received(&self, node: &Node) -> bool {
        let ancestors = self.ancestors(node);
        let mut child: &Node = node;
        for parent in ancestors.iter().rev() {
            let is_last = parent
                .child_nodes()
                .last()
                .is_some_and(|last| last.id == child.id);
            match parent.type_ {
                "begin" | "kwbegin" if is_last => {}
                "if" | "case" if !is_child_at(parent, 0, child) => {}
                "when" if is_last => {}
                "resbody" if is_child_at(parent, 2, child) => {}
                "rescue" | "and" | "or" => {}
                "ensure" if is_child_at(parent, 0, child) => {}
                "lvasgn" | "ivasgn" | "gvasgn" | "cvasgn" | "casgn" | "op_asgn" | "or_asgn"
                | "and_asgn" | "masgn" | "array" => return true,
                "pair" => return is_child_at(parent, 1, child),
                "send" | "csend" => return is_child_at(parent, 0, child),
                _ => return false,
            }
            child = parent.as_ref();
        }
        false
    }

    fn origin_observation(&self, predicate: &dyn Fn(SourceRange) -> bool) -> Option<StrictCheck> {
        self.typing
            .find_observation(&|observation| {
                ORIGIN_CHECKS.contains(&observation.check) && predicate(observation.location)
            })
            .map(|observation| observation.check)
    }

    fn binding_of(&self, use_node: &Node, name: &str) -> Option<Binding> {
        let use_position = use_node.location.expression?.begin_pos;
        let ancestors = self.ancestors(use_node);
        let scope = ancestors
            .iter()
            .rev()
            .find(|ancestor| SCOPE_NODE_TYPES.contains(&ancestor.type_))
            .or(ancestors.first())?;
        let ancestor_ids: Vec<NodeId> = ancestors.iter().map(|ancestor| ancestor.id).collect();
        let mut candidates = Vec::new();
        collect_bindings(scope, name, None, None, true, &mut candidates);
        for ancestor in &ancestors {
            if matches!(ancestor.type_, "numblock" | "itblock")
                && block_parameter_names(ancestor)
                    .iter()
                    .any(|parameter| parameter == name)
            {
                candidates.push(Binding {
                    node: ancestor.clone(),
                    owner: Some(ancestor.clone()),
                    parent: None,
                });
            }
        }
        candidates
            .into_iter()
            .filter(|binding| {
                let Some(range) = binding.node.location.expression else {
                    return false;
                };
                range.begin_pos <= use_position
                    && (binding.owner.is_some() || range.end_pos <= use_position)
                    && binding
                        .owner
                        .as_ref()
                        .is_none_or(|owner| ancestor_ids.contains(&owner.id))
            })
            .max_by_key(|binding| {
                binding
                    .node
                    .location
                    .expression
                    .map(|range| range.begin_pos)
            })
    }

    fn origin_of_binding(&self, binding: &Binding, depth: usize) -> UntypedOrigin {
        if let Some(owner) = &binding.owner
            && matches!(owner.type_, "block" | "numblock" | "itblock")
            && owner
                .child_node(0)
                .and_then(|send| send.child_node(0))
                .and_then(|receiver| self.typing.type_of(receiver.id))
                .as_ref()
                .is_some_and(is_untyped)
        {
            return UntypedOrigin::ReportedBy(StrictCheck::UntypedReceiver);
        }
        let Some(range) = binding.node.location.expression else {
            return UntypedOrigin::Unreported;
        };
        if let Some(check) = self.origin_observation(&|location| {
            location.begin_pos <= range.begin_pos && range.end_pos <= location.end_pos
        }) {
            return UntypedOrigin::ReportedBy(check);
        }
        if let Some(parent) = &binding.parent
            && parent.type_ == "resbody"
            && let Some(classes) = parent
                .child_node(0)
                .and_then(|classes| classes.location.expression)
            && let Some(check) = self.origin_observation(&|location| {
                classes.begin_pos <= location.begin_pos && location.end_pos <= classes.end_pos
            })
        {
            return UntypedOrigin::ReportedBy(check);
        }
        match binding.node.child_node(1) {
            Some(value) if binding.node.type_ == "lvasgn" => self.untyped_origin(value, depth + 1),
            _ => UntypedOrigin::Unreported,
        }
    }

    fn untyped_origin(&self, node: &Node, depth: usize) -> UntypedOrigin {
        let expression = parenthesized_expression(node);
        if is_made_untyped_explicitly(expression) {
            return UntypedOrigin::Explained;
        }
        match self.typing.call_of(expression.id).map(|call| call.kind) {
            Some(MethodCallKind::NoMethodError { .. }) => return UntypedOrigin::Explained,
            Some(MethodCallKind::Untyped) => {
                return UntypedOrigin::ReportedBy(StrictCheck::UntypedReceiver);
            }
            _ => {}
        }
        let Some(range) = expression.location.expression else {
            return UntypedOrigin::Unreported;
        };
        if self.typing.has_error_at(range) {
            return UntypedOrigin::Explained;
        }
        if expression.type_ == "const"
            && expression
                .child_node(0)
                .and_then(|scope| self.typing.type_of(scope.id))
                .as_ref()
                .is_some_and(is_untyped)
        {
            return UntypedOrigin::ReportedBy(StrictCheck::UntypedReceiver);
        }
        if let Some(check) = self.origin_observation(&|location| location == range) {
            return UntypedOrigin::ReportedBy(check);
        }
        if expression.type_ == "lvar"
            && depth < MAXIMUM_ORIGIN_DEPTH
            && let Some(name) = expression.child_symbol(0)
            && let Some(binding) = self.binding_of(expression, name)
        {
            return self.origin_of_binding(&binding, depth);
        }
        UntypedOrigin::Unreported
    }

    fn reportable_untyped(&self, node: &Node, type_: &Type) -> Option<Option<StrictCheck>> {
        if !is_untyped(type_) {
            return None;
        }
        match self.untyped_origin(node, 0) {
            UntypedOrigin::Explained => None,
            UntypedOrigin::Unreported => Some(None),
            UntypedOrigin::ReportedBy(check) => Some(Some(check)),
        }
    }

    fn observe_covered(
        &self,
        check: StrictCheck,
        location: SourceRange,
        message: impl Into<String>,
        covered_by: Option<StrictCheck>,
    ) {
        self.typing
            .observe(Observation::new(check, location, message).covered_by(covered_by));
    }

    fn observe_node_covered(
        &self,
        check: StrictCheck,
        node: &Node,
        message: impl Into<String>,
        covered_by: Option<StrictCheck>,
    ) {
        if let Some(location) = node.location.expression {
            self.observe_covered(check, location, message, covered_by);
        }
    }

    pub(super) fn observe_untyped_receiver(
        &self,
        send_node: &Node,
        receiver: Option<&Rc<Node>>,
        method_name: &str,
    ) {
        let Some(receiver) = receiver else {
            return;
        };
        let receiver = parenthesized_expression(receiver);
        let covered_by = match self.untyped_origin(receiver, 0) {
            UntypedOrigin::Explained | UntypedOrigin::ReportedBy(StrictCheck::UntypedReceiver) => {
                return;
            }
            UntypedOrigin::ReportedBy(check) => Some(check),
            UntypedOrigin::Unreported => None,
        };
        let ancestors = self.ancestors(send_node);
        let Some(receiver_location) = receiver.location.expression else {
            return;
        };
        let Some(location) = send_node
            .location
            .selector
            .or(send_node.location.expression)
        else {
            return;
        };
        let scope = ancestors.iter().rev().find(|ancestor| {
            matches!(
                ancestor.type_,
                "def" | "defs" | "class" | "module" | "sclass"
            )
        });
        let name = self.source.buffer.source_of(receiver_location);
        if name.contains('\n') || ancestors.is_empty() {
            self.observe_covered(
                StrictCheck::UntypedReceiver,
                location,
                format!(
                    "The receiver of `{method_name}` is untyped, so Steep types the call as untyped without type checking its arguments and block against any method type"
                ),
                covered_by,
            );
            return;
        }
        let message = format!(
            "`{name}` is untyped, so Steep types the calls on it as untyped without type checking their arguments and blocks against any method type (reported at the first call in each method)"
        );
        let scope_range = scope.and_then(|scope| scope.location.expression);
        let reported = self.typing.find_observation(&|observation| {
            observation.check == StrictCheck::UntypedReceiver
                && observation.message == message
                && scope_range.is_none_or(|range| {
                    range.begin_pos <= observation.location.begin_pos
                        && observation.location.end_pos <= range.end_pos
                })
        });
        if reported.is_none() {
            self.observe_covered(StrictCheck::UntypedReceiver, location, message, covered_by);
        }
    }

    fn operator_of(&self, node: &Node) -> String {
        match node.location.operator {
            Some(location) => self.source.buffer.source_of(location).to_string(),
            None => node.type_.to_string(),
        }
    }

    pub(super) fn observe_untyped_in_union(&self, node: &Node, operands: &[(&Node, &Type)]) {
        let types: Vec<&Type> = operands.iter().map(|(_, type_)| *type_).collect();
        let others = other_types(&types);
        if !types.iter().any(|type_| tells_more_than_truthiness(type_))
            || !operands
                .iter()
                .any(|(operand, type_)| self.reportable_untyped(operand, type_).is_some())
            || self.value_is_unused(node)
        {
            return;
        }
        let others = others.join(" | ");
        let message = if node.type_ == "rescue" {
            format!(
                "This value is untyped, so Steep types the value of the `rescue` as untyped instead of a union with `{others}` from the other branches"
            )
        } else {
            let operator = self.operator_of(node);
            format!(
                "This operand of `{operator}` is untyped, so Steep types the `{operator}` as untyped instead of a union with `{others}`"
            )
        };
        for (operand, type_) in operands {
            if let Some(covered_by) = self.reportable_untyped(operand, type_) {
                self.observe_node_covered(
                    StrictCheck::UntypedInUnion,
                    operand,
                    message.clone(),
                    covered_by,
                );
            }
        }
    }

    pub(super) fn observe_dropped_untyped_branches(
        &self,
        node: &Node,
        keyword: &str,
        hint: Option<&Type>,
        branches: &[(&Node, &Type)],
        result: &Type,
    ) {
        if hint.is_some()
            || is_untyped(result)
            || matches!(result, Type::Bot)
            || !branches
                .iter()
                .any(|(branch, type_)| self.reportable_untyped(branch, type_).is_some())
            || !self.value_is_stored_or_received(node)
        {
            return;
        }
        for (branch, type_) in branches {
            if let Some(covered_by) = self.reportable_untyped(branch, type_) {
                self.observe_node_covered(
                    StrictCheck::DroppedUntypedBranch,
                    branch,
                    format!(
                        "This branch of `{keyword}` is untyped, and Steep leaves it out of the type of the value, typing the value as `{result}`"
                    ),
                    covered_by,
                );
            }
        }
    }

    pub(super) fn observe_rescue_branches(
        &self,
        node: &Node,
        hint: Option<&Type>,
        main: Option<(&Node, &Type)>,
        resbodies: &[(Rc<Node>, Type)],
        result: &Type,
    ) {
        let mut operands: Vec<(&Node, &Type)> = main.into_iter().collect();
        operands.extend(
            resbodies
                .iter()
                .filter(|(_, type_)| !is_untyped(type_) && !matches!(type_, Type::Bot))
                .map(|(resbody, type_)| (resbody.as_ref(), type_)),
        );
        self.observe_untyped_in_union(node, &operands);
        let branches: Vec<(&Node, &Type)> = resbodies
            .iter()
            .map(|(resbody, type_)| (resbody.as_ref(), type_))
            .collect();
        self.observe_dropped_untyped_branches(node, "rescue", hint, &branches, result);
    }

    fn observe_dropped_untyped_hash_element(
        &self,
        role: &str,
        elements: &[(Rc<Node>, Type)],
        others: &[String],
    ) {
        for (node, type_) in elements {
            let expression = match node.type_ {
                "kwsplat" => node
                    .child_node(0)
                    .map_or(node.as_ref(), |inner| inner.as_ref()),
                _ => node.as_ref(),
            };
            let Some(covered_by) = self.reportable_untyped(expression, type_) else {
                continue;
            };
            let subject = if node.type_ == "kwsplat" {
                format!("The {role}s of this `**` are untyped, and Steep leaves them")
            } else {
                format!("This {role} is untyped, and Steep leaves it")
            };
            self.observe_node_covered(
                StrictCheck::DroppedUntypedHashElement,
                node,
                format!(
                    "{subject} out of the type of the hash, typing its {role}s as `{}`",
                    others.join(" | ")
                ),
                covered_by,
            );
        }
    }

    fn is_argument_of_untyped_receiver(&self, node: &Node) -> bool {
        self.ancestors(node).last().is_some_and(|parent| {
            matches!(parent.type_, "send" | "csend")
                && parent
                    .child_node(0)
                    .and_then(|receiver| self.typing.type_of(receiver.id))
                    .as_ref()
                    .is_some_and(is_untyped)
        })
    }

    pub(super) fn observe_dropped_hash_elements(
        &self,
        node: &Node,
        has_hint: bool,
        keys: &[(Rc<Node>, Type)],
        values: &[(Rc<Node>, Type)],
        splats: &[(Rc<Node>, Type)],
    ) {
        let key_types: Vec<&Type> = keys.iter().map(|(_, type_)| type_).collect();
        let value_types: Vec<&Type> = values.iter().map(|(_, type_)| type_).collect();
        let other_keys = other_types(&key_types);
        let other_values = other_types(&value_types);
        let has_untyped = key_types
            .iter()
            .chain(&value_types)
            .any(|type_| is_untyped(type_))
            || splats.iter().any(|(_, type_)| !is_hash_instance(type_));
        if !has_untyped || node.type_ == "kwargs" || self.value_is_unused(node) {
            return;
        }
        if !has_hint && self.value_is_stored_or_received(node) {
            if !other_keys.is_empty() {
                self.observe_dropped_untyped_hash_element("key", keys, &other_keys);
            }
            if !other_values.is_empty() {
                self.observe_dropped_untyped_hash_element("value", values, &other_values);
            }
        }
        for (splat, type_) in splats {
            if is_hash_instance(type_)
                || (is_untyped(type_) && other_keys.is_empty() && other_values.is_empty())
            {
                continue;
            }
            let covered_by = match splat.child_node(0) {
                Some(value) if is_untyped(type_) => match self.reportable_untyped(value, type_) {
                    Some(covered_by) => covered_by,
                    None => continue,
                },
                _ => None,
            };
            self.observe_node_covered(
                StrictCheck::DroppedHashSplat,
                splat,
                format!(
                    "The value of `**` has type `{type_}`, which is not a Hash, so Steep leaves its keys and values out of the type of the hash"
                ),
                covered_by,
            );
        }
    }

    pub(super) fn observe_untyped_assigned_to_declared_variable(
        &self,
        node: &Node,
        name: &str,
        enforced_type: &Type,
    ) {
        if is_untyped(enforced_type) {
            return;
        }
        let covered_by = match node.child_node(1) {
            Some(value) => match self.reportable_untyped(value, &Type::Any) {
                Some(covered_by) => covered_by,
                None => return,
            },
            None => None,
        };
        self.observe_node_covered(
            StrictCheck::UntypedAssignedToDeclaredVariable,
            node,
            format!(
                "An untyped value is assigned to `{name}`, and Steep assumes it is `{enforced_type}` without checking"
            ),
            covered_by,
        );
    }
}
