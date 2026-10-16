use rbs_environment::Type;
use ruby_prism::Node;

use super::variables::instance_variable_local_key;
use super::{Checker, Context, Locals};
use crate::source::constant_text;
use crate::subtyping::{Certainty, is_subtype};
use crate::type_utilities::{
    can_be_falsy, falsy_part, instance_type_of_class_value, is_bottom, members_of, remove_nil,
    truthy_part,
};

const MAXIMUM_NARROWING_DEPTH: usize = 32;

pub(crate) enum NarrowingTarget<'pr> {
    Local(String),
    InstanceVariable(String),
    /// A call that Steep treats as pure (`%a{pure}`, an attribute reader, `Array#[]`, `Hash#[]`), refined under the
    /// key from `pure_call_key` until the variable it is called on is assigned again
    PureCall {
        key: String,
        node: Node<'pr>,
    },
}

pub(crate) const PURE_CALL_KEY_PREFIX: &str = "\0pure:";

/// Key under which a refined pure call is kept in the locals, together with the variable the call chain starts from
pub(crate) fn pure_call_key(call: &ruby_prism::CallNode<'_>) -> Option<(String, String)> {
    if call.block().is_some() {
        return None;
    }
    let arguments_are_values = call.arguments().is_none_or(|arguments| {
        arguments.arguments().iter().all(|argument| {
            matches!(
                argument,
                Node::IntegerNode { .. }
                    | Node::FloatNode { .. }
                    | Node::StringNode { .. }
                    | Node::SymbolNode { .. }
                    | Node::NilNode { .. }
                    | Node::TrueNode { .. }
                    | Node::FalseNode { .. }
            )
        })
    });
    if !arguments_are_values {
        return None;
    }
    let root = match call.receiver() {
        None | Some(Node::SelfNode { .. }) => "self".to_string(),
        Some(Node::LocalVariableReadNode { .. }) => {
            constant_text(&call.receiver()?.as_local_variable_read_node()?.name())
        }
        Some(Node::InstanceVariableReadNode { .. }) => {
            constant_text(&call.receiver()?.as_instance_variable_read_node()?.name())
        }
        Some(Node::CallNode { .. }) => pure_call_key(&call.receiver()?.as_call_node()?)?.0,
        Some(_) => return None,
    };
    let key = format!("{PURE_CALL_KEY_PREFIX}{root}\0{}", call_chain_text(call)?);
    Some((root, key))
}

/// The call written out from its receiver variable, independent of how the source spells it: the implicit value of
/// `{ name: }` is a `name` call whose location also covers the `:`
fn call_chain_text(call: &ruby_prism::CallNode<'_>) -> Option<String> {
    let receiver_text = match call.receiver() {
        None | Some(Node::SelfNode { .. }) => String::new(),
        Some(Node::CallNode { .. }) => {
            format!("{}.", call_chain_text(&call.receiver()?.as_call_node()?)?)
        }
        Some(receiver) => format!(
            "{}.",
            String::from_utf8_lossy(receiver.location().as_slice())
        ),
    };
    let arguments_text = call
        .arguments()
        .map(|arguments| String::from_utf8_lossy(arguments.location().as_slice()).into_owned())
        .unwrap_or_default();
    Some(format!(
        "{receiver_text}{}({arguments_text})",
        constant_text(&call.name())
    ))
}

/// Drops refined pure calls that start from `root`, because assigning `root` may change what they return
pub(crate) fn invalidate_pure_calls(root: &str, locals: &mut Locals) {
    let prefix = format!("{PURE_CALL_KEY_PREFIX}{root}\0");
    locals.retain(|key, _| !key.starts_with(&prefix));
}

pub(crate) fn merge_locals(base: &Locals, branches: Vec<Locals>) -> Locals {
    let Some(first) = branches.first() else {
        return base.clone();
    };
    if branches.len() == 1 {
        return first.clone();
    }
    let mut names: Vec<&String> = Vec::new();
    for branch in &branches {
        for name in branch.keys() {
            if !names.contains(&name) {
                names.push(name);
            }
        }
    }
    let mut merged = Locals::new();
    for name in names {
        let is_instance_variable = name.starts_with('@') || name.starts_with(PURE_CALL_KEY_PREFIX);
        let mut types = Vec::new();
        let mut missing = false;
        for branch in &branches {
            match branch.get(name) {
                Some(branch_type) => types.push(branch_type.clone()),
                None => missing = true,
            }
        }
        if missing {
            if is_instance_variable {
                continue;
            }
            types.push(Type::Nil);
        }
        merged.insert(name.clone(), Type::union(types));
    }
    merged
}

impl<'pr> Checker<'_, 'pr> {
    pub(crate) fn narrowing_target_in(
        &mut self,
        node: &Node<'pr>,
        context: &Context,
        locals: &Locals,
    ) -> Option<NarrowingTarget<'pr>> {
        if let Some(target) = Checker::narrowing_target(node) {
            return Some(target);
        }
        match node {
            Node::ParenthesesNode { .. } => {
                let body = node.as_parentheses_node()?.body()?;
                self.narrowing_target_in(&single_statement(&body)?, context, locals)
            }
            Node::CallNode { .. } => self.pure_call_target(node, context, locals),
            _ => None,
        }
    }

    fn pure_call_target(
        &mut self,
        node: &Node<'pr>,
        context: &Context,
        locals: &Locals,
    ) -> Option<NarrowingTarget<'pr>> {
        let call = node.as_call_node()?;
        let (_, key) = pure_call_key(&call)?;
        let receiver_type = match call.receiver() {
            Some(receiver) => {
                let mut scratch_locals = locals.clone();
                self.silently(|checker| checker.infer(&receiver, context, &mut scratch_locals))?
            }
            None => context.self_type.clone(),
        };
        let receiver_type = if call.is_safe_navigation() {
            remove_nil(&receiver_type)
        } else {
            receiver_type
        };
        let method_name = constant_text(&call.name());
        for member in members_of(&receiver_type) {
            match self.lookup_member(&member, &method_name, context, 0) {
                super::calls::Lookup::Found(found) if found.is_pure => {}
                super::calls::Lookup::Unreachable => {}
                _ => return None,
            }
        }
        Some(NarrowingTarget::PureCall {
            key,
            node: prism_child_nodes::copy_node(node),
        })
    }

    pub(crate) fn narrowing_target(node: &Node<'pr>) -> Option<NarrowingTarget<'pr>> {
        match node {
            Node::LocalVariableReadNode { .. } => Some(NarrowingTarget::Local(constant_text(
                &node.as_local_variable_read_node()?.name(),
            ))),
            Node::LocalVariableWriteNode { .. } => Some(NarrowingTarget::Local(constant_text(
                &node.as_local_variable_write_node()?.name(),
            ))),
            Node::InstanceVariableReadNode { .. } => Some(NarrowingTarget::InstanceVariable(
                constant_text(&node.as_instance_variable_read_node()?.name()),
            )),
            Node::InstanceVariableWriteNode { .. } => Some(NarrowingTarget::InstanceVariable(
                constant_text(&node.as_instance_variable_write_node()?.name()),
            )),
            Node::ParenthesesNode { .. } => {
                let body = node.as_parentheses_node()?.body()?;
                Checker::narrowing_target(&single_statement(&body)?)
            }
            _ => None,
        }
    }

    fn target_type(
        &mut self,
        target: &NarrowingTarget<'pr>,
        context: &Context,
        locals: &Locals,
    ) -> Type {
        match target {
            NarrowingTarget::PureCall { key, node } => match locals.get(key) {
                Some(refined) => refined.clone(),
                None => {
                    let mut scratch_locals = locals.clone();
                    self.silently(|checker| checker.infer(node, context, &mut scratch_locals))
                        .unwrap_or(Type::Untyped)
                }
            },
            NarrowingTarget::Local(name) => self.local_variable_type(name, context, locals),
            NarrowingTarget::InstanceVariable(name) => self
                .silently(|checker| checker.instance_variable_read_type(name, context, locals))
                .unwrap_or(Type::Untyped),
        }
    }

    /// A refinement that leaves nothing means this checker believes the branch is unreachable. It may be wrong, so
    /// the variable becomes `untyped` there instead of keeping a type that would report errors in that branch
    pub(crate) fn set_target_type(
        target: &NarrowingTarget<'pr>,
        narrowed: Type,
        locals: &mut Locals,
    ) {
        let narrowed = if is_bottom(&narrowed) {
            Type::Untyped
        } else {
            narrowed
        };
        match target {
            NarrowingTarget::Local(name) => {
                locals.insert(name.clone(), narrowed);
            }
            NarrowingTarget::InstanceVariable(name) => {
                locals.insert(instance_variable_local_key(name), narrowed);
            }
            NarrowingTarget::PureCall { key, .. } => {
                locals.insert(key.clone(), narrowed);
            }
        }
    }

    pub(crate) fn narrow_target_with(
        &mut self,
        target: &NarrowingTarget<'pr>,
        context: &Context,
        locals: &mut Locals,
        refine: impl FnOnce(&Type) -> Type,
    ) {
        let current = self.target_type(target, context, locals);
        if current.is_untyped() {
            return;
        }
        let narrowed = refine(&current);
        Checker::set_target_type(target, narrowed, locals);
    }

    /// Steep replaces `==` of the core value classes with `ReceiverIsArg`, so `receiver == literal` splits the
    /// receiver type into what can be equal and what cannot by `literal_var_type_case_select` with `for_receiver`:
    /// only an equal literal type can be equal, and a non-literal type can be equal only when an integer, string or
    /// symbol literal can be its value, as `Symbol` for `:fatal`. `None` when Steep does not split the type
    pub(crate) fn literal_equality_split(
        &mut self,
        receiver_type: &Type,
        argument: &Node<'pr>,
        context: &Context,
    ) -> Option<(Vec<Type>, Vec<Type>)> {
        use rbs_environment::Literal;
        let literal = match argument {
            Node::NilNode { .. } => None,
            Node::TrueNode { .. } => Some(Literal::True),
            Node::FalseNode { .. } => Some(Literal::False),
            Node::IntegerNode { .. } | Node::StringNode { .. } | Node::SymbolNode { .. } => {
                match super::arguments::literal_type_of(argument)? {
                    Type::Literal(literal) => Some(literal),
                    _ => return None,
                }
            }
            _ => return None,
        };
        let receiver_type = self.expand_alias_deeply(receiver_type, 0);
        let mut equal_types = Vec::new();
        let mut unequal_types = Vec::new();
        for member in members_of(&receiver_type) {
            match &member {
                Type::Untyped | Type::Top | Type::Bool => return None,
                _ => {}
            }
            let owner_is_primitive = match self.lookup_member(&member, "==", context, 0) {
                super::calls::Lookup::Found(found) => matches!(
                    found.owner.as_str(),
                    "BasicObject"
                        | "Object"
                        | "Kernel"
                        | "String"
                        | "Integer"
                        | "Symbol"
                        | "TrueClass"
                        | "FalseClass"
                        | "NilClass"
                ),
                _ => false,
            };
            if !owner_is_primitive {
                return None;
            }
            let matches = match (&literal, &member) {
                (None, Type::Nil) => true,
                (None, Type::ClassInstance { name, .. }) => name == "NilClass",
                (Some(Literal::True), Type::ClassInstance { name, .. }) => name == "TrueClass",
                (Some(Literal::False), Type::ClassInstance { name, .. }) => name == "FalseClass",
                (Some(expected), Type::Literal(actual)) => expected == actual,
                _ => false,
            };
            if matches {
                equal_types.push(member);
                continue;
            }
            if let Some(literal @ (Literal::Integer(_) | Literal::String(_) | Literal::Symbol(_))) =
                &literal
                && !matches!(member, Type::Literal(_))
            {
                let literal_type = Type::Literal(literal.clone());
                if is_subtype(self.environment, &literal_type, &member) != Certainty::No {
                    equal_types.push(literal_type);
                }
            }
            unequal_types.push(member);
        }
        Some((equal_types, unequal_types))
    }

    /// Refines `locals` assuming `condition` evaluated to a truthy (or falsy) value. The condition itself must
    /// already have been inferred, so nothing here records hovers or diagnostics
    pub(crate) fn narrow(
        &mut self,
        condition: &Node<'pr>,
        truthy: bool,
        context: &Context,
        locals: &mut Locals,
    ) {
        self.narrow_at(condition, truthy, context, locals, 0);
    }

    fn narrow_at(
        &mut self,
        condition: &Node<'pr>,
        truthy: bool,
        context: &Context,
        locals: &mut Locals,
        depth: usize,
    ) {
        if depth > MAXIMUM_NARROWING_DEPTH {
            return;
        }
        let next = depth + 1;
        if let Some(target) = self.narrowing_target_in(condition, context, locals) {
            self.narrow_target_with(&target, context, locals, |current| {
                if truthy {
                    truthy_part(current)
                } else {
                    falsy_part(current)
                }
            });
            return;
        }
        match condition {
            Node::ParenthesesNode { .. } => {
                if let Some(body) = condition
                    .as_parentheses_node()
                    .and_then(|parentheses| parentheses.body())
                    && let Some(statement) = single_statement(&body)
                {
                    self.narrow_at(&statement, truthy, context, locals, next);
                }
            }
            Node::AndNode { .. } => {
                let Some(and) = condition.as_and_node() else {
                    return;
                };
                if truthy {
                    self.narrow_at(&and.left(), true, context, locals, next);
                    self.narrow_at(&and.right(), true, context, locals, next);
                } else {
                    let mut left_falsy = locals.clone();
                    self.narrow_at(&and.left(), false, context, &mut left_falsy, next);
                    let mut right_falsy = locals.clone();
                    self.narrow_at(&and.left(), true, context, &mut right_falsy, next);
                    self.narrow_at(&and.right(), false, context, &mut right_falsy, next);
                    *locals = merge_locals(locals, vec![left_falsy, right_falsy]);
                }
            }
            Node::OrNode { .. } => {
                let Some(or) = condition.as_or_node() else {
                    return;
                };
                if truthy {
                    let mut left_truthy = locals.clone();
                    self.narrow_at(&or.left(), true, context, &mut left_truthy, next);
                    let mut right_truthy = locals.clone();
                    self.narrow_at(&or.left(), false, context, &mut right_truthy, next);
                    self.narrow_at(&or.right(), true, context, &mut right_truthy, next);
                    *locals = merge_locals(locals, vec![left_truthy, right_truthy]);
                } else {
                    self.narrow_at(&or.left(), false, context, locals, next);
                    self.narrow_at(&or.right(), false, context, locals, next);
                }
            }
            Node::CallNode { .. } => self.narrow_call(condition, truthy, context, locals, next),
            _ => {}
        }
    }

    fn narrow_call(
        &mut self,
        condition: &Node<'pr>,
        truthy: bool,
        context: &Context,
        locals: &mut Locals,
        depth: usize,
    ) {
        let Some(call) = condition.as_call_node() else {
            return;
        };
        let method_name = constant_text(&call.name());
        let arguments: Vec<Node<'pr>> = call
            .arguments()
            .map(|arguments| arguments.arguments().iter().collect())
            .unwrap_or_default();
        let Some(receiver) = call.receiver() else {
            return;
        };
        if self.fails_to_build_receiver_definition(&call, &receiver, context, locals) {
            // Steep types the receiver `untyped` along with the failed call, and `nil?` still refines the receiver,
            // to `untyped`, through `guess_type_from_method`
            if method_name == "nil?"
                && arguments.is_empty()
                && let Some(target) = self.narrowing_target_in(&receiver, context, locals)
            {
                Checker::set_target_type(&target, Type::Untyped, locals);
            }
            return;
        }
        match (method_name.as_str(), arguments.as_slice()) {
            ("!", []) => self.narrow_at(&receiver, !truthy, context, locals, depth),
            ("nil?", []) => {
                if let Some(target) = self.narrowing_target_in(&receiver, context, locals) {
                    self.narrow_target_with(&target, context, locals, |current| {
                        if truthy {
                            Type::Nil
                        } else {
                            remove_nil(current)
                        }
                    });
                }
            }
            ("is_a?" | "kind_of?" | "instance_of?", [class_argument]) => {
                let Some(target) = self.narrowing_target_in(&receiver, context, locals) else {
                    return;
                };
                let Some(instance_type) =
                    self.class_value_instance_type(class_argument, context, locals)
                else {
                    return;
                };
                self.narrow_to_instance(&target, &instance_type, truthy, context, locals);
            }
            ("==" | "!=", [argument]) => {
                let is_equal = (method_name == "==") == truthy;
                let compares_with_nil = matches!(argument, Node::NilNode { .. });
                if !compares_with_nil {
                    if method_name == "=="
                        && let Some(target) = self.narrowing_target_in(&receiver, context, locals)
                    {
                        let current = self.target_type(&target, context, locals);
                        if let Some((equal_types, unequal_types)) =
                            self.literal_equality_split(&current, argument, context)
                        {
                            let narrowed = if is_equal { equal_types } else { unequal_types };
                            Checker::set_target_type(&target, Type::union(narrowed), locals);
                        }
                    }
                    return;
                }
                if let Some(target) = self.narrowing_target_in(&receiver, context, locals) {
                    self.narrow_target_with(&target, context, locals, |current| {
                        if is_equal {
                            Type::Nil
                        } else {
                            remove_nil(current)
                        }
                    });
                }
            }
            ("===", [argument]) => {
                let Some(target) = self.narrowing_target_in(argument, context, locals) else {
                    return;
                };
                let Some(instance_type) =
                    self.class_value_instance_type(&receiver, context, locals)
                else {
                    return;
                };
                self.narrow_to_instance(&target, &instance_type, truthy, context, locals);
            }
            _ => {
                if truthy && call.is_safe_navigation() {
                    self.narrow_at(&receiver, true, context, locals, depth);
                }
                self.narrow_by_return_type(&call, &receiver, truthy, context, locals);
            }
        }
    }

    fn fails_to_build_receiver_definition(
        &mut self,
        call: &ruby_prism::CallNode<'pr>,
        receiver: &Node<'pr>,
        context: &Context,
        locals: &Locals,
    ) -> bool {
        if !self.options.steep_compatible {
            return false;
        }
        let mut scratch_locals = locals.clone();
        let Some(receiver_type) =
            self.silently(|checker| checker.infer(receiver, context, &mut scratch_locals))
        else {
            return false;
        };
        let receiver_type = if call.is_safe_navigation() {
            remove_nil(&receiver_type)
        } else {
            receiver_type
        };
        self.definition_error_of(&receiver_type).is_some()
    }

    /// Steep's `evaluate_union_method_call`: on a union receiver, a member stays in the branch only when its own
    /// method can return a value of that truthiness, e.g. `NilClass#present?: () -> false` drops `nil` from the then
    /// branch of `if x.present?`
    fn narrow_by_return_type(
        &mut self,
        call: &ruby_prism::CallNode<'pr>,
        receiver: &Node<'pr>,
        truthy: bool,
        context: &Context,
        locals: &mut Locals,
    ) {
        if call.block().is_some() {
            return;
        }
        let Some(target) = self.narrowing_target_in(receiver, context, locals) else {
            return;
        };
        let current = self.target_type(&target, context, locals);
        let members = members_of(&current);
        if members.len() < 2 {
            return;
        }
        let method_name = constant_text(&call.name());
        let mut scratch_locals = locals.clone();
        let Some(arguments) = self.silently(|checker| {
            checker.infer_arguments(call.arguments(), context, &mut scratch_locals)
        }) else {
            return;
        };
        let mut kept = Vec::new();
        for member in members {
            let return_type = match self.lookup_member(&member, &method_name, context, 0) {
                super::calls::Lookup::Found(found) => self.selected_return_type(&found, &arguments),
                _ => None,
            };
            let fits = match &return_type {
                Some(return_type) if truthy => !is_bottom(&truthy_part(return_type)),
                Some(return_type) => can_be_falsy(return_type),
                None => true,
            };
            if fits {
                kept.push(member);
            }
        }
        Checker::set_target_type(&target, Type::union(kept), locals);
    }

    pub(crate) fn class_value_instance_type(
        &mut self,
        class_node: &Node<'pr>,
        context: &Context,
        locals: &mut Locals,
    ) -> Option<Type> {
        if !matches!(
            class_node,
            Node::ConstantReadNode { .. } | Node::ConstantPathNode { .. }
        ) {
            return None;
        }
        let class_value = self.silently(|checker| checker.infer(class_node, context, locals))?;
        instance_type_of_class_value(self.environment, &class_value)
    }

    pub(crate) fn narrow_to_instance(
        &mut self,
        target: &NarrowingTarget<'pr>,
        instance_type: &Type,
        truthy: bool,
        context: &Context,
        locals: &mut Locals,
    ) {
        let current = self.target_type(target, context, locals);
        let narrowed = if truthy {
            narrow_to_class(self.environment, &current, instance_type)
        } else {
            if current.is_untyped() {
                return;
            }
            Type::union(
                members_of(&current)
                    .into_iter()
                    .filter(|member| {
                        is_subtype(self.environment, member, instance_type) != Certainty::Yes
                    })
                    .collect(),
            )
        };
        Checker::set_target_type(target, narrowed, locals);
    }
}

pub(crate) fn narrow_to_class(
    environment: &rbs_environment::Environment,
    current: &Type,
    instance_type: &Type,
) -> Type {
    if current.is_untyped() {
        return instance_type.clone();
    }
    let mut narrowed = Vec::new();
    for member in members_of(current) {
        if is_subtype(environment, &member, instance_type) == Certainty::Yes {
            narrowed.push(member);
            continue;
        }
        match is_subtype(environment, instance_type, &member) {
            Certainty::Yes | Certainty::Unknown => narrowed.push(instance_type.clone()),
            Certainty::No => {}
        }
    }
    let narrowed = Type::union(narrowed);
    if is_bottom(&narrowed) {
        instance_type.clone()
    } else {
        narrowed
    }
}

pub(crate) fn single_statement<'pr>(body: &Node<'pr>) -> Option<Node<'pr>> {
    match body {
        Node::StatementsNode { .. } => {
            let statements = body.as_statements_node()?.body();
            if statements.len() == 1 {
                statements.first()
            } else {
                None
            }
        }
        _ => Some(prism_child_nodes::copy_node(body)),
    }
}
