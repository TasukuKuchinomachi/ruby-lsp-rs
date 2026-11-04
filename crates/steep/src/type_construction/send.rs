use std::rc::Rc;

use rbs_definition::{
    ast::{MemberKind, MethodKind},
    definition_builder::{DefinitionMember, TypeDefinition},
};
use steep_interface::steep_version::SteepVersion;
use steep_interface::{
    builder::{Entry, Overload},
    method_type::MethodType,
    substitution::Substitution,
    subtyping::{
        constraints::{Constraints, SharedConstraints},
        variable_variance::VariableVariance,
    },
    types::Type,
};

use crate::{
    ast::node::type_application::TypeApplication,
    diagnostic::ruby::{self as diagnostics, Diagnostic},
    ruby::node::{Child, Node},
    ruby_exception,
    strict::StrictCheck,
    type_construction::{
        Pair, TypeConstruction,
        block::{free_variables_subset, it_block_parameters, numbered_block_parameters},
        definition::rename_type_params,
        send_args::{SendArg, SendArgs},
    },
    type_inference::{
        block_params::{BlockParam, BlockParams},
        context::{ForwardArgType, TypeVariableContext},
        method_call::{MethodCall, MethodCallKind, MethodDeclaration},
    },
};

pub fn method_declarations(method_name: &str, overload: &Overload) -> Vec<MethodDeclaration> {
    overload
        .definitions
        .iter()
        .map(|definition| method_declaration(method_name, definition))
        .collect()
}

fn method_declaration(method_name: &str, definition: &TypeDefinition) -> MethodDeclaration {
    let mut annotations = definition.member_annotations.clone();
    annotations.extend(definition.overload_annotations.iter().cloned());
    let (type_name, singleton, attribute) = match &definition.member {
        DefinitionMember::MethodDefinition(member)
            if method_name == "new" && member.name == "initialize" =>
        {
            (
                definition
                    .implemented_in
                    .clone()
                    .unwrap_or_else(|| definition.defined_in.clone()),
                true,
                false,
            )
        }
        DefinitionMember::MethodDefinition(member) => (
            definition.defined_in.clone(),
            !member.declared_in_ruby && member.kind == MethodKind::Singleton,
            false,
        ),
        DefinitionMember::Attribute(attribute) => (
            definition.defined_in.clone(),
            !attribute.declared_in_ruby && attribute.kind == MemberKind::Singleton,
            !attribute.declared_in_ruby,
        ),
    };
    MethodDeclaration {
        type_name,
        singleton,
        method_name: method_name.to_string(),
        annotations,
        attribute,
        method_type: (*definition.type_).clone(),
        definition: definition.clone(),
    }
}

pub(super) fn unique(declarations: Vec<MethodDeclaration>) -> Vec<MethodDeclaration> {
    let mut unique: Vec<MethodDeclaration> = Vec::new();
    for declaration in declarations {
        if !unique.contains(&declaration) {
            unique.push(declaration);
        }
    }
    unique
}

fn deprecated_message(annotations: &[String]) -> Option<Option<String>> {
    for annotation in annotations {
        if annotation == "deprecated" {
            return Some(None);
        }
        if let Some(message) = annotation.strip_prefix("deprecated:") {
            return Some(Some(message.trim().to_string()));
        }
    }
    None
}

fn eliminate_variables(method_type: &MethodType, variables: &[String]) -> MethodType {
    if variables.is_empty() {
        return method_type.clone();
    }
    let substitution = Substitution::build(
        variables,
        &vec![Type::Any; variables.len()],
        None,
        None,
        None,
    );
    method_type.substitute(&substitution)
}

impl<'checker> TypeConstruction<'checker> {
    fn forward_arg_type_or_raise(&self) -> Option<ForwardArgType> {
        let Some(method_context) = &self.context.method_context else {
            ruby_exception::raise_runtime_error("");
        };
        if method_context.forward_arg_type.is_none() && method_context.method_type.is_some() {
            ruby_exception::raise_runtime_error(
                "Method context must have `forwarded_arg_type` if `...` node appears in it",
            );
        }
        method_context.forward_arg_type.clone()
    }

    pub fn synthesize_sendish(
        &self,
        node: &Rc<Node>,
        hint: Option<&Type>,
        tapp: Option<&Rc<TypeApplication>>,
    ) -> Pair<'checker> {
        match node.type_ {
            "send" => self.type_send(node, node, None, None, false, tapp, hint),
            "csend" => {
                let pair = self.type_send(node, node, None, None, true, tapp, hint);
                let joined = self.context.type_env.join(&[
                    pair.context().type_env.clone(),
                    self.context.type_env.clone(),
                ]);
                pair.constr
                    .update_type_env(|_| joined)
                    .add_typing(node, Type::union(vec![pair.type_, Type::Nil]))
            }
            "block" | "numblock" | "itblock" => {
                let send_node = node
                    .child_node(0)
                    .cloned()
                    .expect("a block has a send node");
                let params = match node.type_ {
                    "numblock" => {
                        let maximum = match node.child(1) {
                            Child::Integer(maximum) => maximum
                                .parse()
                                .expect("a numbered block has the maximum number"),
                            _ => panic!("a numbered block has the maximum number"),
                        };
                        numbered_block_parameters(maximum)
                    }
                    "itblock" => it_block_parameters(),
                    _ => node
                        .child_node(1)
                        .cloned()
                        .expect("a block has a parameters node"),
                };
                let body = node.child_node(2).cloned();
                if send_node.type_ == "lambda" {
                    return self.type_lambda(node, &params, body.as_ref(), hint);
                }
                self.type_send(
                    node,
                    &send_node,
                    Some(params),
                    body,
                    send_node.type_ == "csend",
                    tapp,
                    hint,
                )
            }
            _ => ruby_exception::raise_runtime_error(format!(
                "Unexpected node is given to `#synthesize_sendish` ({}, {})",
                node.type_,
                self.source.buffer.line(node.expression().begin_pos)
            )),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn type_send(
        &self,
        node: &Rc<Node>,
        send_node: &Rc<Node>,
        block_params: Option<Rc<Node>>,
        block_body: Option<Rc<Node>>,
        unwrap: bool,
        tapp: Option<&Rc<TypeApplication>>,
        hint: Option<&Type>,
    ) -> Pair<'checker> {
        let receiver: Option<Rc<Node>>;
        let method_name: String;
        let arguments: Vec<Rc<Node>>;
        let mut receiver_type;
        let constr;
        match send_node.type_ {
            "super" | "zsuper" => {
                let method_context = self.context.method_context.clone();
                let name = method_context
                    .as_ref()
                    .and_then(|context| context.name.clone());
                if name.is_none()
                    || method_context
                        .as_ref()
                        .is_none_or(|context| context.super_method.is_none())
                {
                    return self.fallback_to_any(
                        send_node,
                        Some(diagnostics::unexpected_super(send_node, name.as_deref())),
                    );
                }
                receiver = None;
                method_name = name.unwrap_or_default();
                arguments = send_node.child_nodes().cloned().collect();
                receiver_type = Type::SelfType;
                constr = self.clone();
            }
            _ => {
                receiver = send_node.child_node(0).cloned();
                method_name = send_node.child_symbol(1).unwrap_or_default().to_string();
                arguments = send_node
                    .children
                    .iter()
                    .skip(2)
                    .filter_map(Child::node)
                    .cloned()
                    .collect();
                match &receiver {
                    Some(receiver) => {
                        let pair = self.synthesize(receiver, None, false);
                        receiver_type = pair.type_;
                        constr = pair.constr;
                    }
                    None => {
                        receiver_type = Type::SelfType;
                        constr = self.clone();
                    }
                }
            }
        }
        if unwrap {
            receiver_type = self.unwrap(&receiver_type);
        }
        let receiver_type = self
            .checker
            .deep_expand_alias(&receiver_type)
            .unwrap_or(receiver_type);
        let private = receiver
            .as_ref()
            .is_none_or(|receiver| receiver.type_ == "self");
        if matches!(receiver_type, Type::Any) {
            constr.observe_untyped_receiver(send_node, receiver.as_ref(), &method_name);
            let constr = if let Some(block_params) = &block_params {
                let block_annotations = constr.block_annotations(node);
                let constr = constr.synthesize_children(send_node, &[receiver.as_ref()]);
                constr.type_block_without_hint(
                    node,
                    &block_annotations,
                    &BlockParams::from_node(block_params, &block_annotations),
                    block_body.as_ref(),
                    None,
                );
                constr
            } else {
                constr.synthesize_children(node, &[receiver.as_ref()])
            };
            return constr.add_call(MethodCall {
                node: node.clone(),
                method_name,
                receiver_type: Type::Any,
                return_type: Type::Any,
                method_decls: Vec::new(),
                kind: MethodCallKind::Untyped,
            });
        }
        match constr.calculate_interface(&receiver_type, private) {
            Some(interface) => constr.type_send_interface(
                node,
                &interface,
                receiver.as_ref(),
                &receiver_type,
                &method_name,
                &arguments,
                block_params,
                block_body,
                tapp,
                hint,
            ),
            None => {
                let constr = constr.synthesize_children(node, &[receiver.as_ref()]);
                let error = diagnostics::no_method(
                    node,
                    &receiver_type,
                    &method_name,
                    self.steep_version(),
                );
                constr.add_call(MethodCall {
                    node: node.clone(),
                    method_name,
                    receiver_type: receiver_type.clone(),
                    return_type: Type::Any,
                    method_decls: Vec::new(),
                    kind: MethodCallKind::NoMethodError { error },
                })
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn type_send_interface(
        &self,
        node: &Rc<Node>,
        interface: &steep_interface::builder::Shape,
        receiver: Option<&Rc<Node>>,
        receiver_type: &Type,
        method_name: &str,
        arguments: &[Rc<Node>],
        block_params: Option<Rc<Node>>,
        block_body: Option<Rc<Node>>,
        tapp: Option<&Rc<TypeApplication>>,
        hint: Option<&Type>,
    ) -> Pair<'checker> {
        let method = super::or_raise(interface.method(self.builder(), method_name));
        let Some(method) = method else {
            let block_node = (node.type_ == "block")
                .then(|| node.child_node(0))
                .flatten();
            let constr = self.synthesize_children(
                node,
                &[
                    receiver,
                    block_node,
                    block_params.as_ref(),
                    block_body.as_ref(),
                ],
            );
            if let Some(block_params) = &block_params {
                let block_annotations = self.block_annotations(node);
                constr.type_block_without_hint(
                    node,
                    &block_annotations,
                    &BlockParams::from_node(block_params, &block_annotations),
                    block_body.as_ref(),
                    None,
                );
            }
            if node.type_ == "block"
                && let Some(send_node) = node.child_node(0)
                && (send_node.type_ == "super" || send_node.type_ == "zsuper")
            {
                let name = self
                    .context
                    .method_context
                    .as_ref()
                    .and_then(|context| context.name.clone());
                return self.fallback_to_any(
                    send_node,
                    Some(diagnostics::unexpected_super(send_node, name.as_deref())),
                );
            }
            let error =
                diagnostics::no_method(node, &interface.type_, method_name, self.steep_version());
            return constr.add_call(MethodCall {
                node: node.clone(),
                method_name: method_name.to_string(),
                receiver_type: receiver_type.clone(),
                return_type: Type::Any,
                method_decls: Vec::new(),
                kind: MethodCallKind::NoMethodError { error },
            });
        };
        let result = self.type_method_call(
            node,
            method_name,
            receiver_type,
            &method,
            arguments,
            block_params.clone(),
            block_body.clone(),
            tapp,
            hint,
        );
        let (mut call, constr) = match result {
            Some((mut call, constr)) => {
                if method_name == "[]=" {
                    let without_dot = node.type_ == "send" && node.location.dot.is_none();
                    if without_dot
                        && let Some(last) = arguments.last()
                        && let Some(type_) = constr
                            .typing
                            .type_of(last.id)
                            .filter(|_| constr.typing.has_type(last.id))
                    {
                        call = call.with_return_type(type_);
                    }
                } else if method_name.ends_with('=')
                    && method_name.len() >= 2
                    && method_name[..method_name.len() - 1]
                        .chars()
                        .last()
                        .is_some_and(|character| character.is_alphanumeric() || character == '_')
                    && let Some(last) = arguments.last()
                    && constr.typing.has_type(last.id)
                    && let Some(type_) = constr.typing.type_of(last.id)
                {
                    call = call.with_return_type(type_);
                }
                let mut constr = constr;
                if call.is_typed() {
                    let pure = constr
                        .context
                        .type_env
                        .pure_method_calls
                        .get(&crate::type_inference::type_env::node_key(node))
                        .cloned();
                    match pure {
                        Some(pure) => {
                            if let Some(type_) = pure.type_ {
                                call = MethodCall {
                                    node: node.clone(),
                                    return_type: type_.clone(),
                                    ..(*pure.call).clone()
                                };
                                constr.add_typing(node, type_);
                            }
                        }
                        None => {
                            if self.is_pure_send(&call, receiver, arguments, &constr) {
                                let call_rc = Rc::new(call.clone());
                                let return_type = call.return_type.clone();
                                constr = constr.update_type_env(|env| {
                                    env.add_pure_call(node, call_rc, Some(return_type))
                                });
                            } else if let Some(receiver) = receiver {
                                constr = constr
                                    .update_type_env(|env| env.invalidate_pure_node(receiver));
                            }
                        }
                    }
                    if node.type_ == "send" || node.type_ == "csend" {
                        for declaration in &call.method_decls {
                            if let Some(message) = deprecated_message(&declaration.annotations) {
                                constr.add_error(diagnostics::deprecated_reference(
                                    node,
                                    node.location.selector,
                                    message.as_deref(),
                                ));
                                break;
                            }
                        }
                    }
                }
                let csend = node.type_ == "csend"
                    || (matches!(node.type_, "block" | "numblock" | "itblock")
                        && node.child_node(0).is_some_and(|send| send.type_ == "csend"));
                if csend {
                    let optional = Type::union(vec![call.return_type.clone(), Type::Nil]);
                    call = call.with_return_type(optional);
                }
                (call, constr)
            }
            None => {
                let mut errors: Vec<Diagnostic> = Vec::new();
                let block_node = (node.type_ == "block")
                    .then(|| node.child_node(0))
                    .flatten();
                let constr = self.synthesize_children(node, &[receiver, block_node]);
                if let Some(block_params) = &block_params {
                    let block_annotations = self.block_annotations(node);
                    constr.type_block_without_hint(
                        node,
                        &block_annotations,
                        &BlockParams::from_node(block_params, &block_annotations),
                        block_body.as_ref(),
                        Some(&mut errors),
                    );
                }
                errors.push(diagnostics::unresolved_overloading(
                    node,
                    &interface.type_,
                    method_name,
                    &method
                        .method_types()
                        .into_iter()
                        .cloned()
                        .collect::<Vec<_>>(),
                ));
                let declarations = unique(
                    method
                        .overloads
                        .iter()
                        .flat_map(|overload| method_declarations(method_name, overload))
                        .collect(),
                );
                let call = MethodCall {
                    node: node.clone(),
                    method_name: method_name.to_string(),
                    receiver_type: receiver_type.clone(),
                    return_type: Type::Any,
                    method_decls: declarations,
                    kind: MethodCallKind::Error { errors },
                };
                (call, constr)
            }
        };
        if method_name == "block_given?"
            && call.is_typed()
            && call
                .method_decls
                .iter()
                .all(|declaration| declaration.full_name() == "::Kernel#block_given?")
            && let Some(method_context) = constr.context.method_context.clone()
            && let Some(block_variable) = &method_context.block_param_name
            && let Some(variable_type) = constr.context.type_env.get(block_variable)
            && let Some(unwrapped) = self.checker.unwrap_optional(&variable_type)
        {
            let truthy = constr
                .context
                .type_env
                .refine_types(&[(block_variable.clone(), unwrapped)], &[]);
            let falsy = constr
                .context
                .type_env
                .refine_types(&[(block_variable.clone(), Type::Nil)], &[]);
            constr.typing.add_logic_env(node.id, truthy, falsy);
        }
        let _ = &mut call;
        constr.add_call(call)
    }

    fn is_pure_send(
        &self,
        call: &MethodCall,
        receiver: Option<&Rc<Node>>,
        arguments: &[Rc<Node>],
        constr: &TypeConstruction,
    ) -> bool {
        if call.node.type_ != "send" && call.node.type_ != "csend" {
            return false;
        }
        if !(call.is_pure() || call.is_known_pure()) {
            return false;
        }
        arguments
            .iter()
            .chain(receiver)
            .all(|node| is_value_node(node) || constr.context.type_env.get_node(node).is_some())
    }

    #[allow(clippy::too_many_arguments)]
    fn type_method_call(
        &self,
        node: &Rc<Node>,
        method_name: &str,
        receiver_type: &Type,
        method: &Entry,
        arguments: &[Rc<Node>],
        block_params: Option<Rc<Node>>,
        block_body: Option<Rc<Node>>,
        tapp: Option<&Rc<TypeApplication>>,
        hint: Option<&Type>,
    ) -> Option<(MethodCall, TypeConstruction<'checker>)> {
        let mut fails: Vec<(MethodCall, TypeConstruction<'checker>)> = Vec::new();
        for overload in &method.overloads {
            let child_typing = self.typing.new_child();
            let constr = self.with_new_typing(child_typing.clone());
            let (call, constr) = match constr.try_special_method(
                node,
                receiver_type,
                method_name,
                overload,
                arguments,
                block_params.as_ref(),
                block_body.as_ref(),
                hint,
            ) {
                Some(result) => result,
                None => constr.try_method_type(
                    node,
                    receiver_type,
                    method_name,
                    overload,
                    arguments,
                    block_params.clone(),
                    block_body.clone(),
                    tapp,
                    hint,
                ),
            };
            if call.is_typed() {
                child_typing.save();
                let type_env = constr.context.type_env.clone();
                return Some((call, self.update_type_env(|_| type_env)));
            }
            fails.push((call, constr));
        }
        let non_arity: Vec<(MethodCall, TypeConstruction<'checker>)> = fails
            .iter()
            .filter(|(call, _)| match &call.kind {
                MethodCallKind::Error { errors } => !errors.iter().any(Diagnostic::is_arity_error),
                _ => true,
            })
            .cloned()
            .collect();
        if !non_arity.is_empty() {
            fails = non_arity;
        }
        if fails.len() == 1 {
            let (call, constr) = fails.remove(0);
            constr.typing.save();
            let type_env = constr.context.type_env.clone();
            Some((call, self.update_type_env(|_| type_env)))
        } else {
            None
        }
    }

    fn apply_solution(
        &self,
        errors: &mut Vec<Diagnostic>,
        node: &Rc<Node>,
        method_type: &MethodType,
        constraints: &SharedConstraints,
        variables: &[String],
        variance: &VariableVariance,
    ) -> (MethodType, bool, Substitution) {
        let check = self.builder().subtyping();
        let solution = super::or_raise(constraints.borrow().solution(
            &check,
            variance,
            variables,
            Some(self.self_type().clone()),
            Some(self.module_context().instance_type.clone()),
            Some(self.module_context().module_type.clone()),
        ));
        match solution {
            Ok(solution) => {
                let names: Vec<String> = solution.iter().map(|(name, _)| name.clone()).collect();
                let types: Vec<Type> = solution.iter().map(|(_, type_)| type_.clone()).collect();
                let substitution = Substitution::build(&names, &types, None, None, None);
                (method_type.substitute(&substitution), true, substitution)
            }
            Err(error) => {
                errors.push(diagnostics::unsatisfiable_constraint(
                    node,
                    method_type,
                    &error.variable,
                    &error.sub_type,
                    &error.super_type,
                    &error.result,
                ));
                (method_type.clone(), false, Substitution::empty())
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn try_method_type(
        &self,
        node: &Rc<Node>,
        receiver_type: &Type,
        method_name: &str,
        overload: &Overload,
        arguments: &[Rc<Node>],
        block_params: Option<Rc<Node>>,
        block_body: Option<Rc<Node>>,
        tapp: Option<&Rc<TypeApplication>>,
        hint: Option<&Type>,
    ) -> (MethodCall, TypeConstruction<'checker>) {
        let declarations = unique(method_declarations(method_name, overload));
        let (type_params, method_type) = match tapp
            .and_then(|tapp| self.apply_explicit_type_arguments(&overload.method_type, tapp))
        {
            Some(method_type) => (Vec::new(), method_type),
            None => {
                let (type_params, instantiation) =
                    rename_type_params(&overload.method_type.type_params);
                (
                    type_params,
                    overload.method_type.instantiate(&instantiation),
                )
            }
        };
        let type_param_names: Vec<String> =
            type_params.iter().map(|param| param.name.clone()).collect();
        let mut context = self.context.clone();
        context.variable_context = Rc::new(TypeVariableContext::new(
            &type_params,
            Some(&self.context.variable_context),
        ));
        let mut constr = self.with_context(context);
        let variance = VariableVariance::from_method_type(&method_type);
        let constraints = Constraints::new(&type_param_names).shared();
        let mut upper_bounds = Vec::new();
        for param in &type_params {
            if let Some(bound) = &param.upper_bound {
                super::or_raise(
                    constraints
                        .borrow_mut()
                        .add(&param.name, None, Some(bound), true),
                );
                upper_bounds.push((param.name.clone(), Some(bound.clone())));
            }
        }
        let check = self.builder().subtyping();
        let (call, constr) = check.push_variable_bounds(upper_bounds, || {
            let mut errors: Vec<Diagnostic> = Vec::new();
            let instantiated_method_type = method_type.clone();
            let mut method_type = method_type.clone();
            let mut return_type: Option<Type> = None;
            let args = method_type.type_.params.clone().map(|params| SendArgs {
                node: node.clone(),
                arguments: arguments.to_vec(),
                params,
                block: method_type.block.clone(),
            });
            match &args {
                Some(args) => {
                    constr =
                        constr.type_check_args(Some(method_name), args, &constraints, &mut errors);
                }
                None => {
                    self.observe_untyped_call_arguments(node, method_name, arguments);
                    constr = constr.type_check_untyped_args(arguments);
                }
            }
            if let Some(block_params) = &block_params {
                let block_annotations = self.block_annotations(node);
                let block_params_ = BlockParams::from_node(block_params, &block_annotations);
                if method_type.block.is_some() {
                    let free_variables = method_type.type_.return_type.free_variables();
                    if let Some(hint) = hint
                        && !free_variables.is_empty()
                        && free_variables_subset(hint, self.self_type())
                        && self
                            .check_relation_with(
                                &method_type.type_.return_type,
                                hint,
                                constraints.clone(),
                            )
                            .is_success()
                    {
                        let (solved_type, _, _) = self.apply_solution(
                            &mut errors,
                            node,
                            &method_type,
                            &constraints,
                            &free_variables,
                            &variance,
                        );
                        method_type = solved_type;
                    }
                    let method_block = method_type
                        .block
                        .clone()
                        .expect("the method type accepts a block");
                    let pairs =
                        block_params_.zip(method_block.type_.params.as_ref(), None, self.checker);
                    constr.observe_untyped_block_parameters(
                        node,
                        method_name,
                        &block_params_,
                        method_block.type_.params.as_ref(),
                    );

                    let block_constr = constr.for_block(
                        block_body.as_ref(),
                        &block_params_,
                        method_block.type_.params.as_ref(),
                        Some(&method_block.type_.return_type),
                        None,
                        &block_annotations,
                        Some(&method_type.type_.return_type),
                        method_block.self_type.as_ref(),
                    );
                    let mut block_constr =
                        block_constr.with_new_typing(block_constr.typing.new_child());
                    block_constr.set_body_context(node);

                    for (param, type_) in &pairs {
                        match param {
                            BlockParam::Param(param) => {
                                block_constr = block_constr
                                    .synthesize(
                                        &param.node,
                                        Some(param.type_.as_ref().unwrap_or(type_)),
                                        false,
                                    )
                                    .constr;
                                if let Some(param_type) = &param.type_ {
                                    let result = self.check_relation_with(
                                        type_,
                                        param_type,
                                        constraints.clone(),
                                    );
                                    if result.is_failure() {
                                        errors.push(diagnostics::incompatible_assignment(
                                            &param.node,
                                            param_type,
                                            type_,
                                            &result,
                                        ));
                                    }
                                }
                            }
                            BlockParam::MultipleParam(multiple) => {
                                for param in multiple.each_param() {
                                    block_constr = block_constr
                                        .synthesize(
                                            &param.node,
                                            Some(param.type_.as_ref().unwrap_or(type_)),
                                            false,
                                        )
                                        .constr;
                                    if let Some(param_type) = &param.type_ {
                                        let result = self.check_relation_with(
                                            type_,
                                            param_type,
                                            constraints.clone(),
                                        );
                                        if result.is_failure() {
                                            errors.push(diagnostics::incompatible_assignment(
                                                &param.node,
                                                param_type,
                                                type_,
                                                &result,
                                            ));
                                        }
                                    }
                                }
                                block_constr = block_constr
                                    .add_typing(&multiple.node, type_.clone())
                                    .constr;
                            }
                        }
                    }

                    let mut block_free_variables: Vec<String> = Vec::new();
                    let parameter_types = method_type
                        .type_
                        .params
                        .iter()
                        .flat_map(|params| params.each_type())
                        .chain(
                            method_block
                                .type_
                                .params
                                .iter()
                                .flat_map(|params| params.each_type()),
                        );
                    for type_ in parameter_types {
                        for variable in type_.free_variables() {
                            if !block_free_variables.contains(&variable) {
                                block_free_variables.push(variable);
                            }
                        }
                    }
                    let mut return_free_variables = method_type.type_.return_type.free_variables();
                    return_free_variables.extend(method_block.type_.return_type.free_variables());
                    for variable in return_free_variables {
                        let unconstrained = {
                            let constraints = constraints.borrow();
                            constraints.is_unknown(&variable)
                                && !super::or_raise(constraints.has_constraint(&variable))
                        };
                        if unconstrained {
                            block_free_variables.retain(|free_variable| *free_variable != variable);
                        }
                    }
                    let (solved_type, solved, substitution) = self.apply_solution(
                        &mut errors,
                        node,
                        &method_type,
                        &constraints,
                        &block_free_variables,
                        &variance,
                    );
                    method_type = solved_type;
                    let method_block = method_type
                        .block
                        .clone()
                        .expect("the method type accepts a block");

                    if solved {
                        let mut context = block_constr.context.clone();
                        context.self_type = context.self_type.substitute(&substitution);
                        context.type_env = context
                            .type_env
                            .substitute(&substitution)
                            .substitute(&substitution);
                        context.block_context = context
                            .block_context
                            .as_ref()
                            .map(|block_context| block_context.substitute(&substitution));
                        context.break_context = context
                            .break_context
                            .as_ref()
                            .map(|break_context| break_context.substitute(&substitution));
                        let block_constr = block_constr.with_context(context);

                        let mut block_body_type = block_constr.synthesize_block(
                            node,
                            Some(&method_block.type_.return_type),
                            block_body.as_ref(),
                        );
                        if let Some(block_self_type) = &method_block.self_type {
                            block_body_type = block_body_type.substitute(&Substitution::build(
                                &[],
                                &[],
                                Some(self.instance_type(block_self_type).unwrap_or(Type::Top)),
                                Some(self.singleton_type(block_self_type).unwrap_or(Type::Top)),
                                Some(block_self_type.clone()),
                            ));
                        }

                        let result = self.check_relation_with(
                            &block_body_type,
                            &method_block.type_.return_type,
                            constraints.clone(),
                        );
                        if result.is_success() {
                            let (solved_type, solved, _) = self.apply_solution(
                                &mut errors,
                                node,
                                &method_type,
                                &constraints,
                                &type_param_names,
                                &variance,
                            );
                            method_type = if solved {
                                solved_type
                            } else {
                                eliminate_variables(&solved_type, &type_param_names)
                            };
                            let mut node_return_type = method_type.type_.return_type.clone();
                            if let Some(break_type) = &block_annotations.break_type {
                                node_return_type =
                                    self.union_type(vec![break_type.clone(), node_return_type]);
                            }
                            return_type = Some(node_return_type);
                        } else {
                            errors.push(diagnostics::block_body_type_mismatch(
                                node,
                                &method_block.type_.return_type,
                                &block_body_type,
                                &result,
                            ));
                            method_type = eliminate_variables(&method_type, &type_param_names);
                            return_type = Some(method_type.type_.return_type.clone());
                        }

                        block_constr.typing.save();
                    } else {
                        constr.type_block_without_hint(
                            node,
                            &block_annotations,
                            &block_params_,
                            block_body.as_ref(),
                            Some(&mut errors),
                        );
                        method_type = eliminate_variables(&method_type, &type_param_names);
                        return_type = Some(method_type.type_.return_type.clone());
                    }
                } else if args.is_some() {
                    constr.type_block_without_hint(
                        node,
                        &block_annotations,
                        &block_params_,
                        block_body.as_ref(),
                        Some(&mut errors),
                    );
                    match node.child_node(0).map(|send| send.type_) {
                        Some("super" | "zsuper") => {
                            if self
                                .context
                                .method_context
                                .as_ref()
                                .is_some_and(|context| context.super_method.is_none())
                            {
                                errors.push(diagnostics::unexpected_super(
                                    node.child_node(0).expect("super node"),
                                    Some(method_name),
                                ));
                            }
                        }
                        _ => errors.push(diagnostics::unexpected_block_given(node)),
                    }
                    method_type = eliminate_variables(&method_type, &type_param_names);
                    return_type = Some(method_type.type_.return_type.clone());
                } else {
                    self.observe(
                        StrictCheck::BlockGivenToUntypedMethod,
                        node,
                        format!(
                            "Steep types this block without any hint and checks its type against nothing, because `{method_name}` has an untyped function type `(?)` without a block type"
                        ),
                    );
                    if block_body.is_some() {
                        self.type_block_without_hint(
                            node,
                            &block_annotations,
                            &block_params_,
                            block_body.as_ref(),
                            None,
                        );
                    }
                }
            } else if let Some(args) = &args {
                let arg = args.block_pass_arg();
                let free_variables = method_type_free_variables(&method_type);
                if let Some(forwarded) = args.forwarded_args_node() {
                    if let Some(ForwardArgType::Typed(typed)) = self.forward_arg_type_or_raise() {
                        let method_block_type = method_type
                            .block
                            .as_ref()
                            .map(block_proc_type)
                            .unwrap_or(Type::Nil);
                        let forwarded_block_type =
                            typed.1.as_ref().map(block_proc_type).unwrap_or(Type::Nil);
                        if let Some(result) =
                            constr.no_subtyping(&forwarded_block_type, &method_block_type)
                        {
                            errors.push(
                                Diagnostic::new(
                                    "IncompatibleArgumentForwarding",
                                    forwarded.location.expression,
                                    format!("Cannot forward block to `{method_name}`:"),
                                )
                                .with_detail_lines(
                                    diagnostics::result_printer2_detail_lines(&result),
                                ),
                            );
                        }
                    }
                } else if arg.is_compatible() {
                    if let Some(block_pass) = &arg.node {
                        let hint = match self.steep_version() {
                            SteepVersion::Version2_0_0 => arg.node_type(),
                            SteepVersion::Version2_1_0 => arg.proc_type(),
                        };
                        let pair = constr.synthesize(block_pass, hint.as_ref(), false);
                        constr = pair.constr.clone();
                        let node_type = pair.type_;
                        let nil_given = constr.check_relation(&node_type, &Type::Nil).is_success()
                            && !matches!(node_type, Type::Any);
                        if nil_given {
                            let (solved_type, solved, _) = self.apply_solution(
                                &mut errors,
                                node,
                                &method_type,
                                &constraints,
                                &free_variables,
                                &variance,
                            );
                            method_type = if solved {
                                solved_type
                            } else {
                                eliminate_variables(&solved_type, &type_param_names)
                            };
                            errors.push(diagnostics::required_block_missing(node));
                        } else {
                            let expected = arg
                                .node_type()
                                .expect("a compatible block pass has a block");
                            let result = constr.check_relation_with(
                                &node_type,
                                &expected,
                                constraints.clone(),
                            );
                            if result.is_failure() {
                                errors.push(diagnostics::block_type_mismatch(
                                    block_pass, &expected, &node_type, &result,
                                ));
                            }
                            let (solved_type, solved, _) = self.apply_solution(
                                &mut errors,
                                node,
                                &method_type,
                                &constraints,
                                &free_variables,
                                &variance,
                            );
                            method_type = if solved {
                                solved_type
                            } else {
                                eliminate_variables(&solved_type, &type_param_names)
                            };
                        }
                    } else {
                        let (solved_type, solved, _) = self.apply_solution(
                            &mut errors,
                            node,
                            &method_type,
                            &constraints,
                            &free_variables,
                            &variance,
                        );
                        method_type = if solved {
                            solved_type
                        } else {
                            eliminate_variables(&solved_type, &type_param_names)
                        };
                    }
                    return_type = Some(method_type.type_.return_type.clone());
                } else if arg.is_block_missing() {
                    let (solved_type, solved, _) = self.apply_solution(
                        &mut errors,
                        node,
                        &method_type,
                        &constraints,
                        &free_variables,
                        &variance,
                    );
                    method_type = if solved {
                        solved_type
                    } else {
                        eliminate_variables(&solved_type, &type_param_names)
                    };
                    return_type = Some(method_type.type_.return_type.clone());
                    errors.push(diagnostics::required_block_missing(node));
                } else if arg.is_unexpected_block() {
                    let (solved_type, solved, _) = self.apply_solution(
                        &mut errors,
                        node,
                        &method_type,
                        &constraints,
                        &free_variables,
                        &variance,
                    );
                    method_type = if solved {
                        solved_type
                    } else {
                        eliminate_variables(&solved_type, &type_param_names)
                    };
                    return_type = Some(method_type.type_.return_type.clone());
                    let block_pass = arg
                        .node
                        .clone()
                        .expect("an unexpected block pass has a node");
                    let pair = constr.synthesize(&block_pass, None, false);
                    constr = pair.constr;
                    if !constr.check_relation(&pair.type_, &Type::Nil).is_success() {
                        errors.push(diagnostics::unexpected_block_given(node));
                    }
                }
            }
            let return_type = return_type.unwrap_or_else(|| method_type.type_.return_type.clone());
            if hint.is_none() && errors.is_empty() {
                constr.observe_unconstrained_type_variables(
                    node,
                    method_name,
                    &overload.method_type,
                    &type_param_names,
                    &instantiated_method_type,
                    &return_type,
                    &constraints,
                    arguments,
                    block_params.is_some(),
                );
            }
            let kind = if errors.is_empty() {
                MethodCallKind::Typed {
                    actual_method_type: method_type.clone(),
                }
            } else {
                MethodCallKind::Error { errors }
            };
            let call = MethodCall {
                node: node.clone(),
                method_name: method_name.to_string(),
                receiver_type: receiver_type.clone(),
                return_type,
                method_decls: declarations.clone(),
                kind,
            };
            let mut context = constr.context.clone();
            context.variable_context = self.context.variable_context.clone();
            (call, constr.with_context(context))
        });
        (call, constr)
    }

    pub fn type_check_untyped_args(&self, arguments: &[Rc<Node>]) -> TypeConstruction<'checker> {
        let mut constr = self.clone();
        for argument in arguments {
            match argument.type_ {
                "splat" => {
                    let pair = match argument.child_node(0) {
                        Some(child) => constr.synthesize(child, None, false),
                        None => Pair::new(Type::Any, constr.clone()),
                    };
                    constr = pair.constr.add_typing(argument, pair.type_).constr;
                }
                "kwargs" => {
                    constr = match constr.type_hash_record(argument, None) {
                        Some(pair) => pair.constr,
                        None => constr.type_hash(argument, None).constr,
                    };
                }
                _ => {
                    constr = constr.synthesize(argument, None, false).constr;
                }
            }
        }
        constr
    }

    fn bypass_splat(
        &self,
        node: &Rc<Node>,
        synthesize: impl FnOnce(&TypeConstruction<'checker>, &Rc<Node>) -> Pair<'checker>,
    ) -> Pair<'checker> {
        if node.type_ == "splat" || node.type_ == "kwsplat" {
            let child = node.child_node(0).cloned().expect("a splat has a child");
            let pair = synthesize(self, &child);
            pair.constr.add_typing(node, pair.type_.clone());
            pair
        } else {
            synthesize(self, node)
        }
    }

    pub fn type_check_argument(
        &self,
        node: &Rc<Node>,
        type_: &Type,
        constraints: &SharedConstraints,
        report_node: &Rc<Node>,
        errors: &mut Vec<Diagnostic>,
    ) -> Pair<'checker> {
        let mut found = Vec::new();
        let pair = self.check(
            node,
            type_,
            constraints.clone(),
            &mut |expected, actual, result| {
                found.push(diagnostics::argument_type_mismatch(
                    report_node,
                    expected,
                    actual,
                    result,
                ));
            },
        );
        errors.extend(found);
        pair
    }

    fn type_check_args(
        &self,
        method_name: Option<&str>,
        args: &SendArgs,
        constraints: &SharedConstraints,
        errors: &mut Vec<Diagnostic>,
    ) -> TypeConstruction<'checker> {
        let mut constr = self.clone();
        let mut argument_errors: Vec<Diagnostic> = Vec::new();
        let (forwarded_args, diagnostics_list) = args.each(&mut |arg| match arg {
            SendArg::NodeParamPair { node, type_ } => {
                constr = constr
                    .type_check_argument(node, type_, constraints, node, &mut argument_errors)
                    .constr;
            }
            SendArg::NodeTypePair { node, type_ } => {
                let node_type = SendArg::node_type(node, type_);
                let report_node = node.clone();
                let mut found = Vec::new();
                let pair = constr.bypass_splat(node, |constr, inner| {
                    constr.type_check_argument(
                        inner,
                        &node_type,
                        constraints,
                        &report_node,
                        &mut found,
                    )
                });
                argument_errors.extend(found);
                constr = pair.constr;
            }
            SendArg::UnexpectedArg { node } => {
                constr = constr
                    .bypass_splat(node, |constr, inner| constr.synthesize(inner, None, false))
                    .constr;
            }
            SendArg::PositionalSplat { node, type_ } => {
                let child = constr.with_new_typing(constr.typing.new_child());
                let inner = node.child_node(0).cloned().expect("a splat has a child");
                let pair = child.try_tuple_type_strict(&inner, None);
                *type_ = Some(pair.type_);
            }
            SendArg::ArgTypePairs { pairs } => {
                for (node, type_) in pairs.clone() {
                    let mut found = Vec::new();
                    let pair = constr.bypass_splat(&node, |constr, inner| {
                        constr.type_check_argument(inner, &type_, constraints, inner, &mut found)
                    });
                    argument_errors.extend(found);
                    constr = pair.constr;
                }
            }
            SendArg::UnexpectedKeyword { node, .. } => {
                if node.type_ == "pair" {
                    for child in node.child_nodes() {
                        constr = constr.synthesize(child, None, false).constr;
                    }
                } else {
                    constr = constr
                        .bypass_splat(node, |constr, inner| constr.synthesize(inner, None, false))
                        .constr;
                }
            }
            SendArg::KeywordSplat { node, type_ } => {
                let pair = constr.bypass_splat(node, |constr, inner| {
                    if inner.type_ == "hash"
                        && let Some(pair) = constr.type_hash_record(inner, None)
                    {
                        return pair;
                    }
                    constr.synthesize(inner, None, false)
                });
                *type_ = Some(pair.type_);
            }
            SendArg::MissingArg | SendArg::MissingKeyword { .. } => {}
        });
        errors.extend(argument_errors);
        errors.extend(diagnostics_list);
        if let Some(forwarded_args) = forwarded_args
            && let Some(ForwardArgType::Typed(typed)) = &self.forward_arg_type_or_raise()
        {
            let check = self.builder().subtyping();
            let result = super::or_raise(check.with_context(
                Some(self.self_type().clone()),
                Some(self.module_context().instance_type.clone()),
                Some(self.module_context().module_type.clone()),
                constraints.clone(),
                || {
                    check.check_method_params(
                        "... (argument forwarding)",
                        steep_interface::subtyping::relation::Relation::new(
                            forwarded_args.params.clone(),
                            typed.0.clone(),
                        ),
                    )
                },
            ));
            if result.is_failure() {
                errors.push(
                    Diagnostic::new(
                        "IncompatibleArgumentForwarding",
                        forwarded_args.node.location.expression,
                        format!(
                            "Cannot forward arguments to `{}`:",
                            method_name.unwrap_or_default()
                        ),
                    )
                    .with_detail_lines(diagnostics::result_printer2_detail_lines(&result)),
                );
            }
        }
        constr
    }

    pub fn synthesize_super(&self, node: &Rc<Node>, hint: Option<&Type>) -> Pair<'checker> {
        let arguments: Vec<Rc<Node>> = node.child_nodes().cloned().collect();
        let method_context = self.context.method_context.clone();
        match &method_context {
            Some(method_context) if method_context.method.is_some() => {
                match &method_context.super_method {
                    Some(super_method) => {
                        let overloads = super_method
                            .defs
                            .iter()
                            .map(|definition| Overload {
                                method_type: self.builder().factory.method_type(&definition.type_),
                                definitions: vec![Rc::new(definition.clone())],
                            })
                            .collect();
                        let entry = Entry {
                            method_name: method_context.name.clone().unwrap_or_default(),
                            private_method: true,
                            overloads,
                        };
                        let name = method_context.name.clone().unwrap_or_default();
                        let self_type = self.self_type().clone();
                        match self.type_method_call(
                            node, &name, &self_type, &entry, &arguments, None, None, None, hint,
                        ) {
                            Some((call, constr)) => constr.add_call(call),
                            None => {
                                let error = diagnostics::unresolved_overloading(
                                    node,
                                    &self_type,
                                    &name,
                                    &entry
                                        .method_types()
                                        .into_iter()
                                        .cloned()
                                        .collect::<Vec<_>>(),
                                );
                                let constr = self.synthesize_children(node, &[]);
                                constr.fallback_to_any(node, Some(error))
                            }
                        }
                    }
                    None => self.type_check_untyped_args(&arguments).fallback_to_any(
                        node,
                        Some(diagnostics::unexpected_super(
                            node,
                            method_context.name.as_deref(),
                        )),
                    ),
                }
            }
            _ => self
                .type_check_untyped_args(&arguments)
                .fallback_to_any(node, None),
        }
    }

    pub fn synthesize_zsuper(&self, node: &Rc<Node>) -> Pair<'checker> {
        match &self.context.method_context {
            Some(method_context) if method_context.method.is_some() => {
                match &method_context.super_method {
                    Some(super_method) => {
                        let types: Vec<Type> = super_method
                            .method_types()
                            .map(|method_type| {
                                self.builder()
                                    .factory
                                    .method_type(method_type)
                                    .type_
                                    .return_type
                            })
                            .collect();
                        self.add_typing(node, Type::union(types))
                    }
                    None => self.fallback_to_any(
                        node,
                        Some(diagnostics::unexpected_super(
                            node,
                            method_context.name.as_deref(),
                        )),
                    ),
                }
            }
            _ => self.fallback_to_any(node, None),
        }
    }

    pub fn synthesize_yield(&self, node: &Rc<Node>) -> Pair<'checker> {
        let arguments: Vec<Rc<Node>> = node.child_nodes().cloned().collect();
        let Some(method_context) = self.context.method_context.clone() else {
            return self.fallback_to_any(node, None);
        };
        let Some(method_type) = &method_context.method_type else {
            return self.fallback_to_any(node, None);
        };
        match method_context.block_type() {
            Some(block_type) => {
                let constr = match &block_type.type_.params {
                    Some(params) => {
                        let args = SendArgs {
                            node: node.clone(),
                            arguments: arguments.clone(),
                            params: params.clone(),
                            block: None,
                        };
                        let mut errors = Vec::new();
                        let constr = self.type_check_args(
                            None,
                            &args,
                            &Constraints::new(&[]).shared(),
                            &mut errors,
                        );
                        for error in errors {
                            self.add_error(error);
                        }
                        constr
                    }
                    None => {
                        self.observe_untyped_yield_arguments(node, true);
                        self.type_check_untyped_args(&arguments)
                    }
                };
                constr.add_typing(node, block_type.type_.return_type.clone())
            }
            None => {
                if method_type.type_.params.is_some() {
                    self.add_error(diagnostics::unexpected_yield(node));
                    self.fallback_to_any(node, None)
                } else {
                    self.observe_untyped_yield_arguments(node, false);
                    let constr = self.type_check_untyped_args(&arguments);
                    constr.add_typing(node, Type::Any)
                }
            }
        }
    }
}

fn block_proc_type(block: &steep_interface::method_type::Block) -> Type {
    Type::Proc {
        function: Box::new(block.type_.clone()),
        block: None,
        self_type: block.self_type.clone().map(Box::new),
    }
}

fn method_type_free_variables(method_type: &MethodType) -> Vec<String> {
    let mut variables = Vec::new();
    if let Some(params) = &method_type.type_.params {
        for type_ in params.each_type() {
            for variable in type_.free_variables() {
                if !variables.contains(&variable) {
                    variables.push(variable);
                }
            }
        }
    }
    for variable in method_type.type_.return_type.free_variables() {
        if !variables.contains(&variable) {
            variables.push(variable);
        }
    }
    if let Some(block) = &method_type.block {
        for type_ in block.type_.each_type() {
            for variable in type_.free_variables() {
                if !variables.contains(&variable) {
                    variables.push(variable);
                }
            }
        }
    }
    variables
}

pub fn is_value_node(node: &Node) -> bool {
    match node.type_ {
        "self" | "true" | "false" | "str" | "sym" | "int" | "float" | "nil" | "lvar" => true,
        "const" => node
            .child_nodes()
            .all(|child| child.type_ == "cbase" || is_value_node(child)),
        "array" | "dstr" | "begin" => node.child_nodes().all(|child| is_value_node(child)),
        "hash" => node
            .child_nodes()
            .all(|pair| pair.child_nodes().all(|child| is_value_node(child))),
        _ => false,
    }
}
