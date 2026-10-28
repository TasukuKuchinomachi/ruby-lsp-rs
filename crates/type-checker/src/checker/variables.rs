use rbs_environment::Type;
use ruby_prism::{Location, Node};

use super::{CacheState, Checker, Context, Locals};
use crate::annotations::AnnotationTarget;
use crate::file_index::InstanceVariableKey;
use crate::source::{Span, constant_text};
use crate::steep_relation;
use crate::type_utilities::{array_of, can_be_falsy, element_type, falsy_part, truthy_part};
use crate::{Severity, TypeHintKind};

pub(crate) const IT_PARAMETER_KEY: &str = "\0it";

/// Marks a local that Steep leaves out of its type environment (a keyword parameter of a block or lambda without a
/// proc type to follow), so reading it is `FallbackAny` there
pub(crate) fn fallback_local_key(name: &str) -> String {
    format!("\0fallback:{name}")
}

pub(crate) fn instance_variable_local_key(name: &str) -> String {
    name.to_string()
}

impl<'pr> Checker<'_, 'pr> {
    pub(crate) fn local_variable_type(
        &self,
        name: &str,
        context: &Context,
        locals: &Locals,
    ) -> Type {
        locals
            .get(name)
            .or_else(|| context.declared_locals.get(name))
            .cloned()
            .unwrap_or(Type::Untyped)
    }

    pub(crate) fn infer_variable(
        &mut self,
        node: &Node<'pr>,
        hint: Option<&Type>,
        context: &Context,
        locals: &mut Locals,
    ) -> Type {
        let is_assignment = matches!(
            node,
            Node::LocalVariableWriteNode { .. }
                | Node::InstanceVariableWriteNode { .. }
                | Node::InstanceVariableOrWriteNode { .. }
                | Node::InstanceVariableAndWriteNode { .. }
                | Node::InstanceVariableOperatorWriteNode { .. }
        );
        let result = self.infer_variable_node(node, hint, context, locals, is_assignment);
        if is_assignment {
            self.assignment_span = None;
        }
        result
    }

    fn infer_variable_node(
        &mut self,
        node: &Node<'pr>,
        hint: Option<&Type>,
        context: &Context,
        locals: &mut Locals,
        is_assignment: bool,
    ) -> Type {
        match node {
            Node::LocalVariableReadNode { .. } => {
                let Some(read) = node.as_local_variable_read_node() else {
                    return Type::Untyped;
                };
                let name = constant_text(&read.name());
                if locals.contains_key(&fallback_local_key(&name)) {
                    self.record_diagnostic(
                        Span::of_node(node),
                        Severity::Hint,
                        "Ruby::FallbackAny",
                        "Cannot detect the type of the expression".to_string(),
                    );
                }
                let variable_type = self.local_variable_type(&name, context, locals);
                self.record_type_hover(Span::of_node(node), &variable_type);
                variable_type
            }
            Node::ItLocalVariableReadNode { .. } if self.is_it_a_method_call() => {
                self.infer_it_as_method_call(node, context, locals)
            }
            Node::ItLocalVariableReadNode { .. } => {
                let variable_type = self.local_variable_type(IT_PARAMETER_KEY, context, locals);
                self.record_type_hover(Span::of_node(node), &variable_type);
                variable_type
            }
            Node::LocalVariableWriteNode { .. } => {
                let Some(write) = node.as_local_variable_write_node() else {
                    return Type::Untyped;
                };
                let value = write.value();
                let name = constant_text(&write.name());
                // Steep types `_ = value` and `__any__ = value` as `untyped` without keeping the variable, which is the
                // usual way to cast, and does not even look at the value of `__skip__ = value`
                match name.as_str() {
                    "_" | "__any__" => {
                        self.infer_with_hint(&value, Some(&Type::Untyped), context, locals);
                        return Type::Untyped;
                    }
                    "__skip__" => return Type::Untyped,
                    _ => {}
                }
                // Steep's `lvasgn` hints the value with the declared type of the variable unless the context gives a
                // hint the declared type does not fit
                let declared = context.declared_locals.get(&name).cloned();
                let value_hint = match (hint, &declared) {
                    (None, declared) => declared.clone(),
                    (Some(hint), Some(declared))
                        if steep_relation::is_subtype(self.environment, declared, hint) =>
                    {
                        Some(declared.clone())
                    }
                    (Some(hint), _) => Some(hint.clone()),
                };
                let value_type = self.infer_assigned_value(&value, value_hint.as_ref(), context, locals);
                let is_obvious = is_obvious_value(&value);
                if is_assignment {
                    self.assignment_span = Some(Span::of_node(node));
                }
                self.assign_local(
                    &name,
                    &write.name_loc(),
                    value_type.clone(),
                    is_obvious,
                    context,
                    locals,
                );
                value_type
            }
            Node::LocalVariableOrWriteNode { .. } => {
                let Some(write) = node.as_local_variable_or_write_node() else {
                    return Type::Untyped;
                };
                let name = constant_text(&write.name());
                let current = locals.get(&name).cloned().unwrap_or(Type::Nil);
                let declared = context.declared_locals.get(&name).cloned();
                let value_type = self.infer_with_hint(&write.value(), declared.as_ref(), context, locals);
                let combined = or_assignment_type(&current, value_type);
                let is_obvious = is_obvious_value(&write.value());
                if is_assignment {
                    self.assignment_span = Some(Span::of_node(node));
                }
                self.assign_local(
                    &name,
                    &write.name_loc(),
                    combined.clone(),
                    is_obvious,
                    context,
                    locals,
                );
                combined
            }
            Node::LocalVariableAndWriteNode { .. } => {
                let Some(write) = node.as_local_variable_and_write_node() else {
                    return Type::Untyped;
                };
                let name = constant_text(&write.name());
                let current = self.local_variable_type(&name, context, locals);
                let value_type = self.infer(&write.value(), context, locals);
                let combined = Type::union(vec![falsy_part(&current), value_type]);
                locals.insert(name, combined.clone());
                self.record_type_hover(Span::of_location(&write.name_loc()), &combined);
                combined
            }
            Node::LocalVariableOperatorWriteNode { .. } => {
                let Some(write) = node.as_local_variable_operator_write_node() else {
                    return Type::Untyped;
                };
                let name = constant_text(&write.name());
                let current = self.local_variable_type(&name, context, locals);
                let operator = constant_text(&write.binary_operator());
                let value_type = self.infer(&write.value(), context, locals);
                let result = self.operator_result_type(
                    &current,
                    &operator,
                    value_type,
                    Span::of_location(&write.binary_operator_loc()),
                    context,
                );
                locals.insert(name, result.clone());
                self.record_type_hover(Span::of_location(&write.name_loc()), &result);
                result
            }
            Node::InstanceVariableReadNode { .. } => {
                let Some(read) = node.as_instance_variable_read_node() else {
                    return Type::Untyped;
                };
                let name = constant_text(&read.name());
                let variable_type = self.instance_variable_read_type(&name, context, locals);
                if self
                    .declared_instance_variable_type(&name, context)
                    .is_none()
                {
                    self.record_diagnostic(
                        Span::of_node(node),
                        Severity::Hint,
                        "Ruby::FallbackAny",
                        "Cannot detect the type of the expression".to_string(),
                    );
                }
                self.record_type_hover(Span::of_node(node), &variable_type);
                variable_type
            }
            Node::InstanceVariableWriteNode { .. } => {
                let Some(write) = node.as_instance_variable_write_node() else {
                    return Type::Untyped;
                };
                let value = write.value();
                let name = constant_text(&write.name());
                let declared = self.declared_instance_variable_type(&name, context);
                let value_type = self.infer_assigned_value(&value, declared.as_ref(), context, locals);
                if is_assignment {
                    self.assignment_span = Some(Span::of_node(node));
                }
                self.assign_instance_variable(
                    &name,
                    &write.name_loc(),
                    value_type.clone(),
                    Some(&value_type),
                    is_obvious_value(&value),
                    context,
                    locals,
                );
                value_type
            }
            Node::InstanceVariableOrWriteNode { .. } => {
                let Some(write) = node.as_instance_variable_or_write_node() else {
                    return Type::Untyped;
                };
                let name = constant_text(&write.name());
                let current = self.instance_variable_read_type(&name, context, locals);
                let value = write.value();
                let declared = self.declared_instance_variable_type(&name, context);
                let value_type = self.infer_with_hint(&value, declared.as_ref(), context, locals);
                let combined = or_assignment_type(&current, value_type.clone());
                if is_assignment {
                    self.assignment_span = Some(Span::of_node(node));
                }
                self.assign_instance_variable(
                    &name,
                    &write.name_loc(),
                    combined.clone(),
                    Some(&value_type),
                    is_obvious_value(&value),
                    context,
                    locals,
                );
                combined
            }
            Node::InstanceVariableAndWriteNode { .. } => {
                let Some(write) = node.as_instance_variable_and_write_node() else {
                    return Type::Untyped;
                };
                let name = constant_text(&write.name());
                let current = self.instance_variable_read_type(&name, context, locals);
                let value_type = self.infer(&write.value(), context, locals);
                let combined = Type::union(vec![falsy_part(&current), value_type.clone()]);
                if is_assignment {
                    self.assignment_span = Some(Span::of_node(node));
                }
                self.report_instance_variable_write(&name, Span::of_location(&write.name_loc()), &value_type, context);
                self.record_instance_variable_write(&name, value_type, context);
                locals.insert(instance_variable_local_key(&name), combined.clone());
                self.record_type_hover(Span::of_location(&write.name_loc()), &combined);
                combined
            }
            Node::InstanceVariableOperatorWriteNode { .. } => {
                let Some(write) = node.as_instance_variable_operator_write_node() else {
                    return Type::Untyped;
                };
                let name = constant_text(&write.name());
                let current = self.instance_variable_read_type(&name, context, locals);
                let operator = constant_text(&write.binary_operator());
                let value_type = self.infer(&write.value(), context, locals);
                let result = self.operator_result_type(
                    &current,
                    &operator,
                    value_type,
                    Span::of_location(&write.binary_operator_loc()),
                    context,
                );
                if is_assignment {
                    self.assignment_span = Some(Span::of_node(node));
                }
                self.report_instance_variable_write(&name, Span::of_location(&write.name_loc()), &result, context);
                self.record_instance_variable_write(&name, result.clone(), context);
                locals.insert(instance_variable_local_key(&name), result.clone());
                self.record_type_hover(Span::of_location(&write.name_loc()), &result);
                result
            }
            Node::ClassVariableReadNode { .. } => {
                let Some(read) = node.as_class_variable_read_node() else {
                    return Type::Untyped;
                };
                let variable_type = self.class_variable_type(&constant_text(&read.name()), context);
                self.record_type_hover(Span::of_node(node), &variable_type);
                variable_type
            }
            Node::GlobalVariableReadNode { .. } => {
                let Some(read) = node.as_global_variable_read_node() else {
                    return Type::Untyped;
                };
                let name = constant_text(&read.name());
                let variable_type = if name == "$~" {
                    Type::instance("MatchData")
                } else {
                    self.environment
                        .globals
                        .get(&name)
                        .map(|global| global.global_type.clone())
                        .unwrap_or(Type::Untyped)
                };
                self.record_type_hover(Span::of_node(node), &variable_type);
                variable_type
            }
            Node::MultiWriteNode { .. } => self.infer_multiple_write(node, context, locals),
            Node::MatchWriteNode { .. } => {
                let Some(match_write) = node.as_match_write_node() else {
                    return Type::Untyped;
                };
                let call_type = self.infer_call(&match_write.call(), None, context, locals);
                for target in match_write.targets().iter() {
                    self.assign_target(
                        &target,
                        Type::optional(Type::instance("String")),
                        context,
                        locals,
                    );
                }
                call_type
            }
            _ => {
                let mut value_type = Type::Untyped;
                for child in prism_child_nodes::child_nodes(node) {
                    value_type = self.infer(&child, context, locals);
                }
                value_type
            }
        }
    }

    /// A `#: T` assertion hints the value with the asserted type and gives the value that type, as Steep's `assertion`
    fn infer_assigned_value(
        &mut self,
        value: &Node<'pr>,
        hint: Option<&Type>,
        context: &Context,
        locals: &mut Locals,
    ) -> Type {
        match self.inline_assertion_for(value.location().end_offset(), &context.nesting) {
            Some(asserted_type) => {
                self.infer_with_hint(value, Some(&asserted_type), context, locals);
                asserted_type
            }
            None => self.infer_with_hint(value, hint, context, locals),
        }
    }

    fn assign_local(
        &mut self,
        name: &str,
        name_location: &Location<'pr>,
        value_type: Type,
        is_obvious: bool,
        context: &Context,
        locals: &mut Locals,
    ) {
        super::narrowing::invalidate_pure_calls(name, locals);
        locals.remove(&fallback_local_key(name));
        let span = Span::of_location(name_location);
        if let Some(declared) = context.declared_locals.get(name).cloned() {
            self.report_incompatible_assignment(span, &declared, &value_type, "a variable");
        }
        let variable_type = context
            .declared_locals
            .get(name)
            .cloned()
            .unwrap_or(value_type);
        self.record_hint(
            TypeHintKind::VariableType,
            span.end,
            &variable_type,
            is_obvious,
        );
        self.record_type_hover(span, &variable_type);
        locals.insert(name.to_string(), variable_type);
    }

    #[allow(clippy::too_many_arguments)]
    fn assign_instance_variable(
        &mut self,
        name: &str,
        name_location: &Location<'pr>,
        value_type: Type,
        checked_type: Option<&Type>,
        is_obvious: bool,
        context: &Context,
        locals: &mut Locals,
    ) {
        let span = Span::of_location(name_location);
        self.record_hint(
            TypeHintKind::VariableType,
            span.end,
            &value_type,
            is_obvious,
        );
        self.record_type_hover(span, &value_type);
        self.report_instance_variable_write(name, span, checked_type.unwrap_or(&value_type), context);
        self.record_instance_variable_write(name, value_type.clone(), context);
        let flow_type = self
            .declared_instance_variable_type(name, context)
            .unwrap_or(value_type);
        super::narrowing::invalidate_pure_calls(name, locals);
        locals.insert(instance_variable_local_key(name), flow_type);
    }

    /// Steep's `type_ivasgn`: an undeclared instance variable is `UnknownInstanceVariable`, a declared one must
    /// accept the assigned value (`IncompatibleAssignment`)
    pub(crate) fn report_instance_variable_write(
        &mut self,
        name: &str,
        name_span: Span,
        value_type: &Type,
        context: &Context,
    ) {
        match self.declared_instance_variable_type(name, context) {
            None => self.record_diagnostic(
                name_span,
                Severity::Information,
                "Ruby::UnknownInstanceVariable",
                format!("Cannot find the declaration of instance variable: `{name}`"),
            ),
            Some(declared) => self.report_incompatible_assignment(name_span, &declared, value_type, "a variable"),
        }
    }

    pub(crate) fn report_incompatible_assignment(
        &mut self,
        span: Span,
        declared: &Type,
        value_type: &Type,
        element: &str,
    ) {
        if self.is_silent() {
            return;
        }
        if self.is_certain_mismatch(value_type, declared) {
            let message = steep_relation::with_detail_lines(
                format!(
                    "Cannot assign a value of type `{}` to {element} of type `{}`",
                    value_type.steep_display(),
                    declared.steep_display()
                ),
                self.environment,
                value_type,
                declared,
            );
            self.record_diagnostic(
                self.assignment_span.unwrap_or(span),
                Severity::Hint,
                "Ruby::IncompatibleAssignment",
                message,
            );
        }
    }

    pub(crate) fn record_instance_variable_write(
        &mut self,
        name: &str,
        value_type: Type,
        context: &Context,
    ) {
        let Some((owner, singleton)) = context.instance_variable_owner() else {
            return;
        };
        if let Some(frame) = self.instance_variable_write_frames.last_mut() {
            frame.push((
                InstanceVariableKey {
                    owner,
                    singleton,
                    name: name.to_string(),
                },
                value_type,
            ));
        }
    }

    pub(crate) fn instance_variable_read_type(
        &mut self,
        name: &str,
        context: &Context,
        locals: &Locals,
    ) -> Type {
        if self.options.steep_compatible
            && self
                .declared_instance_variable_type(name, context)
                .is_none()
        {
            return Type::Untyped;
        }
        if let Some(flow_type) = locals.get(&instance_variable_local_key(name)) {
            return flow_type.clone();
        }
        let Some((owner, singleton)) = context.instance_variable_owner() else {
            return Type::Untyped;
        };
        self.instance_variable_type(&owner, singleton, name)
    }

    pub(crate) fn declared_instance_variable_type(&self, name: &str, context: &Context) -> Option<Type> {
        // Steep imports the `@type ivar` annotations of a `def` into the method body whatever the class is
        if let Some(method) = &context.method {
            let annotated = self
                .type_annotations_in(method.scope_start)
                .find_map(|(target, annotated_type)| match target {
                    AnnotationTarget::InstanceVariable(annotated_name) if annotated_name == name => {
                        Some(annotated_type.clone())
                    }
                    _ => None,
                });
            if let Some(mut annotated) = annotated {
                self.environment.resolve_type_names(&mut annotated, &context.nesting);
                return Some(annotated);
            }
        }
        let (owner, singleton) = context.instance_variable_owner()?;
        self.declared_instance_variable_type_of(&owner, singleton, name)
    }

    fn declared_instance_variable_type_of(
        &self,
        owner: &str,
        singleton: bool,
        name: &str,
    ) -> Option<Type> {
        if let Some(annotated) = self.instance_variable_annotation(owner, name) {
            return Some(annotated);
        }
        if singleton {
            let declaration = self.environment.classes.get(owner)?;
            return declaration.class_instance_variables.get(name).cloned();
        }
        if !self.environment.classes.contains_key(owner) {
            return None;
        }
        self.environment.instance_variable_type(owner, name)
    }

    pub(crate) fn instance_variable_type(
        &mut self,
        owner: &str,
        singleton: bool,
        name: &str,
    ) -> Type {
        if let Some(declared) = self.declared_instance_variable_type_of(owner, singleton, name) {
            return declared;
        }
        let key = InstanceVariableKey {
            owner: owner.to_string(),
            singleton,
            name: name.to_string(),
        };
        if self.options.steep_compatible {
            return Type::Untyped;
        }
        if let Some(cached) = self.instance_variable_cache.get(&key) {
            return cached.clone();
        }
        let writers = self
            .file_index
            .instance_variable_writers
            .get(&key)
            .cloned()
            .unwrap_or_default();
        if writers.is_empty() {
            return Type::Untyped;
        }
        let mut complete = true;
        for definition_offset in writers {
            match self.method_return_cache.get(&definition_offset) {
                Some(CacheState::Done(_)) => {}
                Some(CacheState::InProgress) => complete = false,
                None => {
                    if self
                        .infer_file_method_return_type(definition_offset)
                        .is_none()
                    {
                        complete = false;
                    }
                }
            }
        }
        let inferred = match self.inferred_instance_variable_writes.get(&key) {
            Some(types) if !types.is_empty() => Type::union(types.clone()),
            _ => Type::Untyped,
        };
        // Only `nil` written in this file means the real value is assigned elsewhere, e.g. by a superclass
        let inferred = if matches!(inferred, Type::Nil) {
            Type::Untyped
        } else {
            inferred
        };
        if complete {
            self.instance_variable_cache.insert(key, inferred.clone());
        }
        inferred
    }

    fn class_variable_type(&self, name: &str, context: &Context) -> Type {
        let owner = match &context.self_type {
            Type::ClassInstance { name, .. } | Type::ClassSingleton { name } => name.clone(),
            _ => return Type::Untyped,
        };
        self.environment
            .classes
            .get(&owner)
            .and_then(|declaration| declaration.class_variables.get(name).cloned())
            .unwrap_or(Type::Untyped)
    }

    fn infer_multiple_write(
        &mut self,
        node: &Node<'pr>,
        context: &Context,
        locals: &mut Locals,
    ) -> Type {
        let Some(multiple_write) = node.as_multi_write_node() else {
            return Type::Untyped;
        };
        let value = multiple_write.value();
        let (value_type, element_types) = match value.as_array_node() {
            Some(array)
                if !array
                    .elements()
                    .iter()
                    .any(|element| matches!(element, Node::SplatNode { .. })) =>
            {
                let element_types: Vec<Type> = array
                    .elements()
                    .iter()
                    .map(|element| self.infer(&element, context, locals))
                    .collect();
                let value_type = array_of(if element_types.is_empty() {
                    Type::Untyped
                } else {
                    Type::union(element_types.clone())
                });
                (value_type, Some(element_types))
            }
            _ => (self.infer(&value, context, locals), None),
        };
        let lefts: Vec<Node<'pr>> = multiple_write.lefts().iter().collect();
        let rights: Vec<Node<'pr>> = multiple_write.rights().iter().collect();
        self.assign_destructured(
            &lefts,
            multiple_write.rest(),
            &rights,
            &value_type,
            element_types,
            context,
            locals,
        );
        value_type
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn assign_destructured(
        &mut self,
        lefts: &[Node<'pr>],
        rest: Option<Node<'pr>>,
        rights: &[Node<'pr>],
        value_type: &Type,
        element_types: Option<Vec<Type>>,
        context: &Context,
        locals: &mut Locals,
    ) {
        let positional_count = lefts.len() + rights.len();
        let element_at = |index: usize| -> Type {
            if let Some(element_types) = &element_types {
                return element_types.get(index).cloned().unwrap_or(Type::Nil);
            }
            destructured_element(value_type, index)
        };
        for (index, target) in lefts.iter().enumerate() {
            self.assign_target(target, element_at(index), context, locals);
        }
        if let Some(rest) = rest {
            let rest_element = match (&element_types, value_type) {
                (Some(element_types), _) => {
                    let remaining: Vec<Type> = element_types
                        .iter()
                        .skip(positional_count)
                        .cloned()
                        .collect();
                    if remaining.is_empty() {
                        Type::Untyped
                    } else {
                        Type::union(remaining)
                    }
                }
                (None, Type::Tuple(elements)) => {
                    Type::union(elements.iter().skip(positional_count).cloned().collect())
                }
                (None, _) => element_type(value_type),
            };
            if let Some(splat) = rest.as_splat_node()
                && let Some(expression) = splat.expression()
            {
                self.assign_target(&expression, array_of(rest_element), context, locals);
            }
        }
        for (offset, target) in rights.iter().enumerate() {
            let element = match (&element_types, value_type) {
                (Some(element_types), _) => {
                    let index = element_types.len().saturating_sub(rights.len()) + offset;
                    element_types.get(index).cloned().unwrap_or(Type::Nil)
                }
                (None, Type::Tuple(elements)) => {
                    let index = elements.len().saturating_sub(rights.len()) + offset;
                    elements.get(index).cloned().unwrap_or(Type::Nil)
                }
                _ => destructured_element(value_type, lefts.len() + offset),
            };
            self.assign_target(target, element, context, locals);
        }
    }

    pub(crate) fn assign_target(
        &mut self,
        target: &Node<'pr>,
        assigned_type: Type,
        context: &Context,
        locals: &mut Locals,
    ) {
        match target {
            Node::LocalVariableTargetNode { .. } => {
                let Some(local) = target.as_local_variable_target_node() else {
                    return;
                };
                self.assign_local(
                    &constant_text(&local.name()),
                    &target.location(),
                    assigned_type,
                    false,
                    context,
                    locals,
                );
            }
            Node::InstanceVariableTargetNode { .. } => {
                let Some(instance_variable) = target.as_instance_variable_target_node() else {
                    return;
                };
                self.assign_instance_variable(
                    &constant_text(&instance_variable.name()),
                    &target.location(),
                    assigned_type,
                    None,
                    false,
                    context,
                    locals,
                );
            }
            Node::MultiTargetNode { .. } => {
                let Some(multiple_target) = target.as_multi_target_node() else {
                    return;
                };
                let lefts: Vec<Node<'pr>> = multiple_target.lefts().iter().collect();
                let rights: Vec<Node<'pr>> = multiple_target.rights().iter().collect();
                self.assign_destructured(
                    &lefts,
                    multiple_target.rest(),
                    &rights,
                    &assigned_type,
                    None,
                    context,
                    locals,
                );
            }
            Node::SplatNode { .. } => {
                let Some(splat) = target.as_splat_node() else {
                    return;
                };
                if let Some(expression) = splat.expression() {
                    self.assign_target(
                        &expression,
                        array_of(element_type(&assigned_type)),
                        context,
                        locals,
                    );
                }
            }
            Node::RequiredParameterNode { .. } => {
                let Some(parameter) = target.as_required_parameter_node() else {
                    return;
                };
                let name = constant_text(&parameter.name());
                let span = Span::of_node(target);
                self.record_hint(
                    TypeHintKind::BlockParameterType,
                    span.end,
                    &assigned_type,
                    false,
                );
                self.record_type_hover(span, &assigned_type);
                locals.insert(name, assigned_type);
            }
            Node::CallTargetNode { .. } | Node::IndexTargetNode { .. } => {
                for child in prism_child_nodes::child_nodes(target) {
                    self.infer(&child, context, locals);
                }
            }
            _ => {}
        }
    }
}

fn destructured_element(value_type: &Type, index: usize) -> Type {
    match value_type {
        Type::Tuple(elements) => elements.get(index).cloned().unwrap_or(Type::Nil),
        Type::ClassInstance { name, arguments } if name == "Array" => {
            arguments.first().cloned().unwrap_or(Type::Untyped)
        }
        Type::Untyped => Type::Untyped,
        Type::ClassInstance { .. } | Type::Nil | Type::Bool | Type::Literal(_) => {
            if index == 0 {
                value_type.clone()
            } else {
                Type::Nil
            }
        }
        _ => Type::Untyped,
    }
}

pub(crate) fn or_assignment_type(current: &Type, value_type: Type) -> Type {
    if !can_be_falsy(current) {
        return current.clone();
    }
    Type::union(vec![truthy_part(current), value_type])
}

pub(crate) fn is_obvious_value(value: &Node<'_>) -> bool {
    match value {
        Node::IntegerNode { .. }
        | Node::FloatNode { .. }
        | Node::RationalNode { .. }
        | Node::ImaginaryNode { .. }
        | Node::StringNode { .. }
        | Node::InterpolatedStringNode { .. }
        | Node::XStringNode { .. }
        | Node::InterpolatedXStringNode { .. }
        | Node::SymbolNode { .. }
        | Node::InterpolatedSymbolNode { .. }
        | Node::RegularExpressionNode { .. }
        | Node::InterpolatedRegularExpressionNode { .. }
        | Node::NilNode { .. }
        | Node::TrueNode { .. }
        | Node::FalseNode { .. }
        | Node::ArrayNode { .. }
        | Node::HashNode { .. }
        | Node::RangeNode { .. }
        | Node::LambdaNode { .. }
        | Node::SourceFileNode { .. }
        | Node::SourceLineNode { .. } => true,
        Node::CallNode { .. } => {
            let Some(call) = value.as_call_node() else {
                return false;
            };
            let method_name = call.name();
            let method_name = method_name.as_slice();
            let receiver_is_constant = call.receiver().is_some_and(|receiver| {
                matches!(
                    receiver,
                    Node::ConstantReadNode { .. } | Node::ConstantPathNode { .. }
                )
            });
            receiver_is_constant && (method_name == b"new" || method_name == b"[]")
        }
        _ => false,
    }
}
