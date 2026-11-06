use std::rc::Rc;

use rbs_definition::{ast::TypeName, collections::IndexMap};
use steep_interface::steep_version::SteepVersion;
use steep_interface::{
    method_type::{Block, Function, Params},
    substitution::Substitution,
    types::Type,
};

use crate::{
    annotation::{AnnotationKind, Collection},
    diagnostic::ruby::{self as diagnostics, Diagnostic},
    ruby::node::{Child, Location, MapKind, Node},
    ruby_exception::{self, RubyException},
    strict::StrictCheck,
    type_construction::{
        Pair, TypeConstruction, definition::ANONYMOUS_BLOCK_PASSABLE_LVAR, flatten_union,
        instance_type, is_instance_of, is_special_lvar_name,
    },
    type_inference::{
        block_params::{BlockParam, BlockParams, Param},
        constant_env::nesting_names,
        context::{BlockContext, BreakContext, Context, ImplementName},
        multiple_assignment::MultipleAssignment,
        type_env::SPECIAL_LVAR_NAMES,
    },
    typing::CursorRange,
};

type ParameterTypes = IndexMap<Option<String>, Type>;

#[derive(Clone, Debug, PartialEq)]
enum FreeVariable {
    Variable(String),
    SelfType,
    Instance,
    Class,
}

fn collect_free_variables(type_: &Type, variables: &mut Vec<FreeVariable>) {
    let variable = match type_ {
        Type::Var(name) => Some(FreeVariable::Variable(name.clone())),
        Type::SelfType => Some(FreeVariable::SelfType),
        Type::Instance => Some(FreeVariable::Instance),
        Type::Class => Some(FreeVariable::Class),
        _ => None,
    };
    if let Some(variable) = variable {
        if !variables.contains(&variable) {
            variables.push(variable);
        }
        return;
    }
    match type_ {
        Type::Proc {
            function,
            block,
            self_type,
        } => {
            for child in function.each_type() {
                collect_free_variables(child, variables);
            }
            if let Some(block) = block {
                for child in block.type_.each_type() {
                    collect_free_variables(child, variables);
                }
                if let Some(self_type) = &block.self_type {
                    collect_free_variables(self_type, variables);
                }
            }
            if let Some(self_type) = self_type {
                collect_free_variables(self_type, variables);
            }
        }
        _ => {
            for child in type_.each_child() {
                collect_free_variables(child, variables);
            }
        }
    }
}

pub fn free_variables_subset(sub_type: &Type, super_type: &Type) -> bool {
    let mut sub_variables = Vec::new();
    collect_free_variables(sub_type, &mut sub_variables);
    let mut super_variables = Vec::new();
    collect_free_variables(super_type, &mut super_variables);
    sub_variables
        .iter()
        .all(|variable| super_variables.contains(variable))
}

fn each_descendant_node(node: &Rc<Node>, visit: &mut dyn FnMut(&Rc<Node>)) {
    for child in node.child_nodes() {
        visit(child);
        each_descendant_node(child, visit);
    }
}

pub fn optional_proc(type_: &Type) -> Option<Type> {
    if let Type::Union(types) = type_
        && types.len() == 2
        && types.iter().any(|type_| matches!(type_, Type::Nil))
    {
        return types
            .iter()
            .find(|type_| matches!(type_, Type::Proc { .. }))
            .cloned();
    }
    None
}

impl<'checker> TypeConstruction<'checker> {
    fn observe_untyped_parameter(
        &self,
        check: StrictCheck,
        param: &Param,
        block_node: &Rc<Node>,
        reason: &str,
    ) {
        let name = match &param.var {
            Some(name) => format!("`{name}`"),
            None => format!("`{}`", param.node.type_),
        };
        let message = format!("Steep types the parameter {name} as untyped, because {reason}");
        if param.node.location.expression.is_some() {
            self.observe(check, &param.node, message);
        } else {
            self.observe(check, block_node, message);
        }
    }

    pub fn observe_untyped_block_parameters(
        &self,
        node: &Rc<Node>,
        method_name: &str,
        block_params: &BlockParams,
        block_type_params: Option<&Params>,
    ) {
        let params: Vec<Param> = block_params
            .params()
            .iter()
            .flat_map(BlockParam::each_param)
            .filter(|param| {
                param.type_.is_none() && !param.var.as_deref().is_some_and(is_special_lvar_name)
            })
            .cloned()
            .collect();
        let reason = match block_type_params {
            None => format!("the block type of `{method_name}` has `(?)` parameters"),
            Some(block_type_params)
                if BlockParams::untyped_args(block_type_params) && params.len() > 1 =>
            {
                format!(
                    "the block type of `{method_name}` has one untyped parameter, which Steep does not expand to the block parameters"
                )
            }
            Some(_) => return,
        };
        for param in &params {
            self.observe_untyped_parameter(
                StrictCheck::UntypedBlockParameters,
                param,
                node,
                &reason,
            );
        }
    }

    pub fn block_annotations(&self, node: &Node) -> Collection {
        self.checker
            .annotations(self.source, Some(node), &self.nesting())
    }

    fn merge_block_annotations(&self, block_annotations: &Collection) -> Collection {
        let mut annotations: Vec<_> = self
            .annotations
            .annotations
            .iter()
            .filter(|annotation| {
                !matches!(
                    annotation.kind,
                    AnnotationKind::BlockType(_) | AnnotationKind::BreakType(_)
                )
            })
            .cloned()
            .collect();
        annotations.extend(block_annotations.annotations.iter().cloned());
        Collection::new(
            annotations,
            &self.builder().factory,
            &self.checker.type_name_resolver,
            &nesting_names(&self.nesting()),
        )
    }

    pub fn type_name(&self, type_: &Type) -> Option<TypeName> {
        match type_ {
            Type::NameInstance { name, .. } | Type::Singleton(name) => Some(name.clone()),
            Type::Literal(literal) => {
                self.type_name(&steep_interface::types::literal_back_type(literal))
            }
            Type::Tuple(_) => Some(TypeName::parse("::Array")),
            Type::Record(_) => Some(TypeName::parse("::Hash")),
            Type::Proc { .. } => Some(TypeName::parse("::Proc")),
            _ => None,
        }
    }

    pub fn singleton_type(&self, type_: &Type) -> Option<Type> {
        match type_ {
            Type::Union(types) => Some(Type::union(
                types
                    .iter()
                    .map(|type_| self.singleton_type(type_))
                    .collect::<Option<Vec<_>>>()?,
            )),
            Type::Intersection(types) => Some(Type::intersection(
                types
                    .iter()
                    .map(|type_| self.singleton_type(type_))
                    .collect::<Option<Vec<_>>>()?,
            )),
            _ => self.type_name(type_).map(Type::Singleton),
        }
    }

    pub fn instance_type(&self, type_: &Type) -> Option<Type> {
        match type_ {
            Type::Union(types) => Some(Type::union(
                types
                    .iter()
                    .map(|type_| self.instance_type(type_))
                    .collect::<Option<Vec<_>>>()?,
            )),
            Type::Intersection(types) => Some(Type::intersection(
                types
                    .iter()
                    .map(|type_| self.instance_type(type_))
                    .collect::<Option<Vec<_>>>()?,
            )),
            _ => self
                .type_name(type_)
                .map(|name| self.to_instance_type(&name)),
        }
    }

    fn set_up_block_mlhs_params_env(
        &self,
        node: &Rc<Node>,
        type_: &Type,
        hash: &mut ParameterTypes,
        on_error: &mut dyn FnMut(&Rc<Node>, &mut ParameterTypes),
    ) {
        let Some(arrayish) = self.try_convert_to_array(type_) else {
            on_error(node, hash);
            return;
        };
        if node.type_ == "restarg" && node.child_symbol(0).is_some() {
            ruby_exception::raise(RubyException::new(
                "NoMethodError",
                "undefined method 'type' for an instance of Symbol",
            ));
        }
        let assignments = MultipleAssignment::new()
            .expand(node, &arrayish, false)
            .unwrap_or_else(|| {
                ruby_exception::raise_runtime_error(format!(
                    "{arrayish} is expected to be array-ish"
                ))
            });
        for (node, type_) in assignments.leading_assignments {
            if node.type_ == "arg" {
                hash.insert(node.child_symbol(0).map(str::to_string), type_);
            } else {
                self.set_up_block_mlhs_params_env(&node, &type_, hash, on_error);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn for_block(
        &self,
        body_node: Option<&Rc<Node>>,
        block_params: &BlockParams,
        block_param_hint: Option<&Params>,
        block_next_type: Option<&Type>,
        block_block_hint: Option<&Block>,
        block_annotations: &Collection,
        node_type_hint: Option<&Type>,
        block_self_hint: Option<&Type>,
    ) -> TypeConstruction<'checker> {
        for keyword_param in &block_params.keyword_params {
            self.observe(
                StrictCheck::IgnoredBlockParameter,
                keyword_param,
                "Steep ignores keyword parameters of blocks, and types their uses as untyped",
            );
        }
        let mut param_types_hash: ParameterTypes = IndexMap::default();
        match block_param_hint {
            Some(block_param_hint) => {
                let pairs =
                    block_params.zip(Some(block_param_hint), block_block_hint, self.checker);
                for param in block_params.params() {
                    if !pairs
                        .iter()
                        .any(|(paired, _)| paired.node().id == param.node().id)
                    {
                        self.observe(
                            StrictCheck::IgnoredBlockParameter,
                            param.node(),
                            "Steep gives a block parameter after optional or rest parameters no type from the block type, and types its uses as untyped",
                        );
                    }
                }
                for (param, type_) in pairs {
                    match &param {
                        BlockParam::Param(param) => {
                            param_types_hash.insert(param.var.clone(), type_);
                        }
                        BlockParam::MultipleParam(multiple) => {
                            self.set_up_block_mlhs_params_env(
                                &multiple.node,
                                &type_,
                                &mut param_types_hash,
                                &mut |error_node, hash| {
                                    let annotation_types = multiple.variable_types();
                                    each_descendant_node(error_node, &mut |descendant| {
                                        if descendant.type_ == "arg"
                                            && let Some(name) = descendant.child_symbol(0)
                                        {
                                            let type_ = annotation_types
                                                .iter()
                                                .find(|(variable, _)| variable == name)
                                                .and_then(|(_, type_)| type_.clone())
                                                .unwrap_or(Type::Any);
                                            hash.insert(Some(name.to_string()), type_);
                                        }
                                    });
                                },
                            );
                        }
                    }
                }
            }
            None => {
                for param in block_params.params() {
                    match &param {
                        BlockParam::Param(param) => {
                            param_types_hash.insert(
                                param.var.clone(),
                                param.type_.clone().unwrap_or(Type::Any),
                            );
                        }
                        BlockParam::MultipleParam(multiple) => {
                            for param in multiple.each_param() {
                                param_types_hash.insert(
                                    param.var.clone(),
                                    param.type_.clone().unwrap_or(Type::Any),
                                );
                            }
                        }
                    }
                }
            }
        }
        param_types_hash.retain(|name, _| {
            name.as_ref()
                .is_none_or(|name| !SPECIAL_LVAR_NAMES.contains(&name.as_str()))
        });

        let pins = self.context.type_env.pin_local_variables(None);
        let mut type_env = self.context.type_env.clone();
        if block_self_hint.is_some() || block_annotations.self_type.is_some() {
            let self_node = Node::new("self", Vec::new(), Location::new(MapKind::Map, None));
            type_env = type_env.invalidate_pure_node(&self_node);
        }
        for (name, (type_, enforced_type)) in pins {
            type_env
                .local_variable_types
                .insert(name, (type_, Some(enforced_type)));
        }
        for (name, type_) in param_types_hash {
            if let Some(name) = name {
                type_env.local_variable_types.insert(name, (type_, None));
            }
        }
        if let Some(self_binding) = block_annotations.self_type.as_ref().or(block_self_hint) {
            let definition = match self_binding {
                Type::NameInstance { name, .. } => Some(super::or_raise(
                    self.builder().definition_builder.build_instance(name),
                )),
                Type::Singleton(name) => Some(super::or_raise(
                    self.builder().definition_builder.build_singleton(name),
                )),
                _ => None,
            };
            if let Some(definition) = definition {
                type_env.instance_variable_types = Rc::new(
                    definition
                        .instance_variables
                        .iter()
                        .map(|(name, variable)| {
                            (name.clone(), self.builder().factory.type_(&variable.type_))
                        })
                        .collect(),
                );
            }
        }
        let mut local_variable_types = Vec::new();
        for (name, annotation_type) in &block_annotations.var_types {
            if let Some(outer_type) = type_env.get(name)
                && !matches!(outer_type, Type::Var(_))
                && !matches!(annotation_type, Type::Var(_))
                && body_node.is_some()
                && let Some(result) = self.no_subtyping(&outer_type, annotation_type)
            {
                self.add_error(diagnostics::incompatible_annotation(
                    body_node.expect("checked above"),
                    name,
                    &format!("{outer_type} <: {annotation_type}"),
                    &result,
                ));
            }
            local_variable_types.push((name.clone(), annotation_type.clone()));
        }
        for (name, annotation_type) in local_variable_types {
            type_env
                .local_variable_types
                .insert(name, (annotation_type.clone(), Some(annotation_type)));
        }

        let break_type = match &block_annotations.break_type {
            Some(break_type) => Some(
                self.union_type(
                    node_type_hint
                        .cloned()
                        .into_iter()
                        .chain([break_type.clone()])
                        .collect(),
                ),
            ),
            None => node_type_hint.cloned(),
        };
        let block_context = BlockContext {
            body_type: block_annotations.block_type.clone(),
        };
        let break_context = BreakContext {
            unchecked_break: break_type.is_none(),
            unchecked_next: block_next_type.is_none(),
            break_type: break_type.unwrap_or(Type::Any),
            next_type: Some(block_next_type.cloned().unwrap_or(Type::Any)),
        };

        let mut self_type = block_self_hint
            .cloned()
            .unwrap_or_else(|| self.self_type().clone());
        let mut module_context = self.context.module_context.clone();
        if let Some((name, args)) = &block_annotations.implement_module {
            let nesting = self.nesting();
            let module_name = self
                .checker
                .absolute_type_name(name, &nesting)
                .unwrap_or_else(|| ruby_exception::raise_runtime_error(""));
            let implement_name = ImplementName {
                name: module_name,
                args: args.clone(),
            };
            module_context = Rc::new(self.default_module_context(Some(&implement_name), nesting));
            self_type = module_context.module_type.clone();
        }
        if let Some(annotation_self_type) = &block_annotations.self_type {
            self_type = annotation_self_type.clone();
        }
        let self_type = self.expand_self(&self_type);

        TypeConstruction {
            checker: self.checker,
            source: self.source,
            inference: self.inference,
            annotations: Rc::new(self.merge_block_annotations(block_annotations)),
            typing: self.typing.clone(),
            context: Context {
                identity: Default::default(),
                method_context: self.context.method_context.clone(),
                block_context: Some(block_context),
                break_context: Some(break_context),
                module_context,
                self_type,
                type_env,
                call_context: self.context.call_context.clone(),
                variable_context: self.context.variable_context.clone(),
            },
        }
    }

    pub fn synthesize_block(
        &self,
        node: &Rc<Node>,
        block_type_hint: Option<&Type>,
        block_body: Option<&Rc<Node>>,
    ) -> Type {
        let Some(block_body) = block_body else {
            return Type::Nil;
        };
        let annotated_body_type = self
            .context
            .block_context
            .as_ref()
            .and_then(|context| context.body_type.clone());
        let hint = annotated_body_type.as_ref().or(block_type_hint);
        let body_pair = self.synthesize(block_body, hint, false);
        let mut body_type = body_pair.type_.clone();
        if let Some(annotated_body_type) = annotated_body_type {
            if let Some(result) = self.no_subtyping(&body_type, &annotated_body_type) {
                self.add_error(diagnostics::block_body_type_mismatch(
                    node,
                    &annotated_body_type,
                    &body_type,
                    &result,
                ));
            }
            body_type = annotated_body_type;
        }
        let end = node.location.end.expect("a block has the end keyword");
        self.typing.update_cursor_context(|cursor_context| {
            cursor_context.set(
                CursorRange::new(block_body.expression().end_pos, end.begin_pos),
                body_pair.context(),
            )
        });
        body_type
    }

    pub fn type_block_without_hint(
        &self,
        node: &Rc<Node>,
        block_annotations: &Collection,
        block_params: &BlockParams,
        block_body: Option<&Rc<Node>>,
        errors: Option<&mut Vec<Diagnostic>>,
    ) {
        let mut block_constr = self.for_block(
            block_body,
            block_params,
            None,
            None,
            None,
            block_annotations,
            None,
            None,
        );
        block_constr.set_body_context(node);
        for param in block_params.params() {
            for param in param.each_param() {
                block_constr = block_constr
                    .synthesize(&param.node, param.type_.as_ref(), false)
                    .constr;
            }
        }
        let block_type = block_constr.synthesize_block(node, None, block_body);
        if let Some(expected_block_type) = block_constr
            .context
            .block_context
            .as_ref()
            .and_then(|context| context.body_type.clone())
        {
            let result = block_constr.check_relation(&block_type, &expected_block_type);
            if result.is_failure() {
                let error = diagnostics::block_body_type_mismatch(
                    node,
                    &expected_block_type,
                    &block_type,
                    &result,
                );
                match errors {
                    Some(errors) => errors.push(error),
                    None => block_constr.add_error(error),
                }
            }
        }
    }

    pub fn type_lambda(
        &self,
        node: &Rc<Node>,
        params_node: &Rc<Node>,
        body_node: Option<&Rc<Node>>,
        type_hint: Option<&Type>,
    ) -> Pair<'checker> {
        let block_annotations = self.block_annotations(node);
        let params = BlockParams::from_node(params_node, &block_annotations);

        let mut params_hint: Option<Params> = None;
        let mut return_hint: Option<Type> = None;
        let mut block_hint: Option<Block> = None;
        let mut self_hint: Option<Type> = None;
        if let Some(original_hint) = type_hint {
            let type_hint = self
                .checker
                .deep_expand_alias(original_hint)
                .unwrap_or_else(|| original_hint.clone());
            if !matches!(type_hint, Type::Any) {
                let proc_instance = instance_type("::Proc", Vec::new());
                let procs: Vec<Type> = flatten_union(&type_hint)
                    .into_iter()
                    .filter(|type_| {
                        self.check_relation(type_, &proc_instance).is_success()
                            && !matches!(type_, Type::Any)
                    })
                    .collect();
                let (proc_instances, proc_types): (Vec<Type>, Vec<Type>) = procs
                    .into_iter()
                    .partition(|type_| is_instance_of(type_, "::Proc"));
                if !proc_instances.is_empty() && proc_types.is_empty() {
                } else if proc_types.len() == 1 {
                    if let Type::Proc {
                        function,
                        block,
                        self_type,
                    } = &proc_types[0]
                    {
                        params_hint = function.params.clone();
                        return_hint = Some(function.return_type.clone());
                        block_hint = block.as_deref().cloned();
                        self_hint = self_type.as_deref().cloned();
                    }
                } else {
                    self.add_error(diagnostics::proc_hint_ignored(node, original_hint));
                }
            }
        }

        if params_hint.is_none() {
            for param in params.params() {
                for param in param.each_param() {
                    if param.type_.is_none()
                        && !param.var.as_deref().is_some_and(is_special_lvar_name)
                    {
                        self.observe_untyped_parameter(
                            StrictCheck::UntypedLambdaParameters,
                            param,
                            node,
                            "no proc type hint gives the lambda its parameter types",
                        );
                    }
                }
            }
        }
        if body_node.is_none() {
            self.observe(
                StrictCheck::UntypedEmptyLambda,
                node,
                "Steep types the return value of a lambda without a body as untyped, though it returns nil",
            );
        }

        let mut block_constr = self.for_block(
            body_node,
            &params,
            params_hint.as_ref(),
            return_hint.as_ref(),
            block_hint.as_ref(),
            &block_annotations,
            None,
            self_hint.as_ref(),
        );
        block_constr.set_body_context(node);

        for param in params.each_single_param() {
            block_constr = block_constr
                .synthesize(&param.node, param.type_.as_ref(), false)
                .constr;
        }

        let block = match &params.block_param {
            Some(block_param) => match &block_param.type_ {
                Some(block_param_type) => {
                    let expanded_type = match self.steep_version() {
                        SteepVersion::Version2_0_0 => block_param_type.clone(),
                        SteepVersion::Version2_1_0 => self
                            .checker
                            .deep_expand_alias(block_param_type)
                            .unwrap_or_else(|| block_param_type.clone()),
                    };
                    match &expanded_type {
                        Type::Proc {
                            function,
                            self_type,
                            ..
                        } => Some(Block {
                            type_: (**function).clone(),
                            optional: false,
                            self_type: self_type.as_deref().cloned(),
                        }),
                        _ => match optional_proc(&expanded_type) {
                            Some(Type::Proc {
                                function,
                                self_type,
                                ..
                            }) => Some(Block {
                                type_: *function,
                                optional: true,
                                self_type: self_type.map(|type_| *type_),
                            }),
                            _ => {
                                block_constr.add_error(diagnostics::proc_type_expected(
                                    &block_param.node,
                                    block_param_type,
                                ));
                                Some(Block {
                                    type_: Function {
                                        params: Some(Params::build(
                                            Vec::new(),
                                            Vec::new(),
                                            None,
                                            Default::default(),
                                        )),
                                        return_type: Type::Any,
                                    },
                                    optional: false,
                                    self_type: None,
                                })
                            }
                        },
                    }
                }
                None => block_hint.clone(),
            },
            None => None,
        };

        let return_type = match body_node {
            Some(body_node) => {
                let return_type =
                    block_constr.synthesize_block(node, return_hint.as_ref(), Some(body_node));
                let mut return_type = return_type.substitute(&Substitution::build(
                    &[],
                    &[],
                    None,
                    None,
                    Some(block_constr.self_type().clone()),
                ));
                if let Some(expected_block_type) = block_constr
                    .context
                    .block_context
                    .as_ref()
                    .and_then(|context| context.body_type.clone())
                {
                    let type_variables = expected_block_type.free_variables();
                    let substitution = Substitution::build(
                        &type_variables,
                        &vec![Type::Any; type_variables.len()],
                        None,
                        None,
                        None,
                    );
                    let expected_block_type = expected_block_type.substitute(&substitution);
                    let result = self.check_relation(&return_type, &expected_block_type);
                    if result.is_failure() {
                        block_constr.add_error(diagnostics::block_body_type_mismatch(
                            node,
                            &expected_block_type,
                            &return_type,
                            &result,
                        ));
                        return_type = expected_block_type;
                    }
                }
                return_type
            }
            None => Type::Any,
        };

        let block_type = Type::Proc {
            function: Box::new(Function {
                params: Some(params_hint.unwrap_or_else(|| params.params_type(None))),
                return_type,
            }),
            block: block.map(Box::new),
            self_type: block_annotations
                .self_type
                .clone()
                .or(self_hint)
                .map(Box::new),
        };

        self.add_typing(node, block_type)
    }

    pub fn synthesize_procarg0(&self, node: &Rc<Node>) -> Pair<'checker> {
        let mut constr = self.clone();
        for argument in &node.children {
            match argument {
                Child::Symbol(name) => {
                    if is_special_lvar_name(name) {
                        constr = self.add_typing(node, Type::Any).constr;
                    } else if let Some(type_) = self.context.type_env.get(name) {
                        constr = self.add_typing(node, type_).constr;
                    } else {
                        constr = self.lvasgn(node, Type::Any).constr;
                    }
                }
                Child::Node(argument) => {
                    constr = constr.synthesize(argument, None, false).constr;
                }
                _ => {}
            }
        }
        Pair::new(Type::Any, constr)
    }

    pub fn synthesize_mlhs(&self, node: &Rc<Node>) -> Pair<'checker> {
        let mut constr = self.clone();
        for argument in node.child_nodes() {
            constr = constr.synthesize(argument, None, false).constr;
        }
        Pair::new(Type::Any, constr)
    }

    pub fn synthesize_block_pass(&self, node: &Rc<Node>, hint: Option<&Type>) -> Pair<'checker> {
        let value_node = node.child_node(0).cloned();
        let mut constr = self.clone();
        let type_ = match &value_node {
            Some(value_node) => {
                let pair = self.synthesize(value_node, hint, false);
                let mut type_ = pair.type_;
                constr = pair.constr;
                match hint {
                    Some(Type::Proc { .. })
                        if value_node.type_ == "send"
                            && value_node.child_symbol(1) == Some("method")
                            && is_instance_of(&type_, "::Method") =>
                    {
                        let receiver_type = match value_node.child_node(0) {
                            Some(receiver) => self.typing.type_of_or_raise(receiver),
                            None => self.self_type().clone(),
                        };
                        let method_name = value_node
                            .child_node(2)
                            .unwrap_or_else(|| {
                                ruby_exception::raise(RubyException::new(
                                    "NoMethodError",
                                    "undefined method 'children' for nil",
                                ))
                            })
                            .child_symbol(0);
                        if let Some(method_name) = method_name
                            && let Some(shape) = self.calculate_interface(&receiver_type, true)
                            && let Some(method) =
                                super::or_raise(shape.method(self.builder(), method_name))
                            && let Some(method_type) = method
                                .method_types()
                                .into_iter()
                                .find(|method_type| accept_one_arg(method_type))
                            && method_type.type_params.is_empty()
                        {
                            type_ = Type::Proc {
                                function: Box::new(method_type.type_.clone()),
                                block: method_type.block.clone().map(Box::new),
                                self_type: None,
                            };
                        }
                    }
                    Some(Type::Proc { function, .. }) if value_node.type_ == "sym" => {
                        if proc_one_arg(function) {
                            if let Some(params) = &function.params {
                                let param_type = params
                                    .required_types()
                                    .first()
                                    .map(|type_| (*type_).clone())
                                    .expect("one arg proc has a required parameter");
                                match &param_type {
                                    Type::Any => type_ = Type::Any,
                                    _ => {
                                        let method_name =
                                            value_node.child_symbol(0).unwrap_or_default();
                                        if let Some(shape) =
                                            self.calculate_interface(&param_type, true)
                                            && let Some(method) = super::or_raise(
                                                shape.method(self.builder(), method_name),
                                            )
                                        {
                                            let return_types: Vec<Type> = method
                                                .method_types()
                                                .into_iter()
                                                .filter(|method_type| {
                                                    method_type.type_.params.as_ref().is_none_or(
                                                        |params| {
                                                            params.required_types().is_empty()
                                                                && params
                                                                    .keywords
                                                                    .requireds
                                                                    .is_empty()
                                                        },
                                                    )
                                                })
                                                .map(|method_type| {
                                                    method_type.type_.return_type.clone()
                                                })
                                                .collect();
                                            if let Some(return_type) = return_types.first() {
                                                type_ = Type::Proc {
                                                    function: Box::new(Function {
                                                        params: Some(Params::required(vec![
                                                            param_type.clone(),
                                                        ])),
                                                        return_type: return_type.clone(),
                                                    }),
                                                    block: None,
                                                    self_type: None,
                                                };
                                            }
                                        }
                                    }
                                }
                            }
                        } else {
                            eprintln!(
                                "ruby-type-check: passing multiple args through Symbol#to_proc is not supported ({}:{})",
                                self.source.buffer.name,
                                self.source.buffer.line(node.expression().begin_pos)
                            );
                        }
                    }
                    _ => {}
                }
                match &type_ {
                    Type::Proc { .. } => type_,
                    _ if is_instance_of(&type_, "::Proc") => type_,
                    _ => constr.try_convert(&type_, "to_proc").unwrap_or(type_),
                }
            }
            None => {
                if let Some(type_) = self.context.type_env.get(ANONYMOUS_BLOCK_PASSABLE_LVAR) {
                    type_
                } else if let Some(block_type) = self
                    .context
                    .method_context
                    .as_ref()
                    .expect("anonymous block pass appears in a method")
                    .block_type()
                {
                    let proc_type = Type::Proc {
                        function: Box::new(block_type.type_.clone()),
                        block: None,
                        self_type: block_type.self_type.clone().map(Box::new),
                    };
                    if block_type.optional {
                        self.union_type(vec![proc_type, Type::Nil])
                    } else {
                        proc_type
                    }
                } else {
                    Type::Nil
                }
            }
        };
        constr.add_typing(node, type_)
    }
}

fn proc_one_arg(function: &Function) -> bool {
    match &function.params {
        Some(params) => {
            params.required_types().len() == 1
                && params.optional_types().is_empty()
                && params.rest_type().is_none()
                && params.keywords.requireds.is_empty()
                && params.keywords.optionals.is_empty()
                && params.keywords.rest.is_none()
        }
        None => true,
    }
}

fn accept_one_arg(method_type: &steep_interface::method_type::MethodType) -> bool {
    if method_type
        .block
        .as_ref()
        .is_some_and(|block| !block.optional)
    {
        return false;
    }
    let Some(params) = &method_type.type_.params else {
        return false;
    };
    if !params.keywords.requireds.is_empty() {
        return false;
    }
    // Steep's Function#accept_one_arg? tests the tail list against a parameter class, which never matches, so a second required parameter does not reject the method type.
    !params.positionals.is_empty()
}

pub fn numbered_block_parameters(maximum: usize) -> Rc<Node> {
    let argument_nodes = if maximum == 1 {
        vec![Child::Node(Node::new(
            "procarg0",
            vec![Child::Symbol("_1".to_string())],
            Location::new(MapKind::Map, None),
        ))]
    } else {
        (1..=maximum)
            .map(|index| {
                Child::Node(Node::new(
                    "arg",
                    vec![Child::Symbol(format!("_{index}"))],
                    Location::new(MapKind::Map, None),
                ))
            })
            .collect()
    };
    Node::new("args", argument_nodes, Location::new(MapKind::Map, None))
}

pub fn it_block_parameters() -> Rc<Node> {
    Node::new(
        "args",
        vec![Child::Node(Node::new(
            "procarg0",
            vec![Child::Symbol("it".to_string())],
            Location::new(MapKind::Map, None),
        ))],
        Location::new(MapKind::Map, None),
    )
}
