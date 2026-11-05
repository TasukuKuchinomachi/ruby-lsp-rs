mod assertion;
mod block;
mod case_when;
pub mod definition;
mod deprecation;
mod inference_rules;
mod missing_signature;
mod multiple_assignment;
pub mod send;
mod send_args;
mod singleton_class;
mod special_method;
mod untyped;

use std::{rc::Rc, sync::Arc};

use rbs_definition::{
    ast::{Literal, TypeName},
    collections::HashSet,
    definition_builder::DefinitionBuilder,
    environment::{Environment, TypeNameResolver},
    errors::RbsError,
};
use steep_interface::{
    builder::{Builder, Config, Shape},
    steep_version::SteepVersion,
    subtyping::{
        constraints::{Constraints, SharedConstraints},
        relation::Relation,
        result::SubtypingResult,
    },
    types::{RecordElement, Type},
};

use crate::{
    annotation::Collection,
    diagnostic::ruby::{self as diagnostics, Diagnostic},
    inference::Inference,
    ruby::node::{Child, Node},
    ruby_exception::{self, RubyException},
    source::Source,
    strict::{Observation, StrictCheck},
    type_inference::{
        constant_env::{ConstantResolver, ConstantTables, ResolverContext, nesting_names},
        context::{BreakContext, Context, ContextIdentity, ModuleContext},
        logic_type_interpreter::{self, LogicTypeInterpreter},
        method_call::{MethodCall, MethodCallKind},
        type_env::{SPECIAL_LVAR_NAMES, TypeEnv},
    },
    typing::{CursorRange, Typing},
};

pub struct Checker {
    pub builder: Builder,
    pub constant_resolver: Rc<ConstantResolver>,
    pub type_name_resolver: Arc<TypeNameResolver>,
}

pub struct CheckerTables {
    constant_tables: Arc<ConstantTables>,
    type_name_resolver: Arc<TypeNameResolver>,
}

impl CheckerTables {
    pub fn new(definition_builder: Arc<DefinitionBuilder>) -> CheckerTables {
        let builder = Builder::with_definition_builder(definition_builder);
        CheckerTables {
            constant_tables: Arc::new(ConstantTables::new(&builder)),
            type_name_resolver: Arc::new(TypeNameResolver::new(builder.environment())),
        }
    }
}

impl Checker {
    pub fn new(
        environment: Arc<Environment>,
        implicitly_returns_nil: bool,
        steep_version: SteepVersion,
    ) -> Checker {
        Checker::with_definition_builder(
            Arc::new(DefinitionBuilder::new(environment)),
            implicitly_returns_nil,
            steep_version,
        )
    }

    pub fn with_definition_builder(
        definition_builder: Arc<DefinitionBuilder>,
        implicitly_returns_nil: bool,
        steep_version: SteepVersion,
    ) -> Checker {
        let tables = CheckerTables::new(definition_builder.clone());
        Checker::with_tables(
            definition_builder,
            &tables,
            implicitly_returns_nil,
            steep_version,
        )
    }

    // Why not build the tables for each checker: every thread builds a checker, and
    // the tables of a large project take a noticeable time to build.
    pub fn with_tables(
        definition_builder: Arc<DefinitionBuilder>,
        tables: &CheckerTables,
        implicitly_returns_nil: bool,
        steep_version: SteepVersion,
    ) -> Checker {
        let mut builder = Builder::with_definition_builder(definition_builder);
        builder.implicitly_returns_nil = implicitly_returns_nil;
        builder.steep_version = steep_version;
        Checker {
            builder,
            constant_resolver: Rc::new(ConstantResolver::new(tables.constant_tables.clone())),
            type_name_resolver: tables.type_name_resolver.clone(),
        }
    }

    pub fn environment(&self) -> &Environment {
        self.builder
            .definition_builder
            .ancestor_builder
            .environment()
    }

    pub fn absolute_type_name(
        &self,
        name: &TypeName,
        nesting: &ResolverContext,
    ) -> Option<TypeName> {
        self.type_name_resolver
            .resolve(name, &nesting_names(nesting))
    }

    pub fn annotations(
        &self,
        source: &Source,
        node: Option<&Node>,
        nesting: &ResolverContext,
    ) -> Collection {
        Collection::new(
            source.annotations_of(node),
            &self.builder.factory,
            &self.type_name_resolver,
            &nesting_names(nesting),
        )
    }

    pub fn expand_alias(&self, type_: &Type) -> Type {
        match type_ {
            Type::Alias { name, args } => self
                .builder
                .factory
                .expand_alias(name, args)
                .unwrap_or_else(|_| type_.clone()),
            _ => type_.clone(),
        }
    }

    pub fn deep_expand_alias(&self, type_: &Type) -> Option<Type> {
        self.deep_expand_alias_recursive(type_, &mut Vec::new())
    }

    fn deep_expand_alias_recursive(
        &self,
        type_: &Type,
        recursive: &mut Vec<TypeName>,
    ) -> Option<Type> {
        match type_ {
            Type::Alias { name, .. } => {
                if recursive.contains(name) {
                    return None;
                }
                let unfolded = self.expand_alias(type_);
                recursive.push(name.clone());
                let result = self.deep_expand_alias_recursive(&unfolded, recursive);
                recursive.pop();
                result
            }
            Type::Union(types) => {
                let mut expanded = Vec::new();
                for type_ in types {
                    expanded.push(self.deep_expand_alias_recursive(type_, recursive)?);
                }
                Some(Type::union(expanded))
            }
            Type::Intersection(types) => {
                let mut expanded = Vec::new();
                for type_ in types {
                    expanded.push(self.deep_expand_alias_recursive(type_, recursive)?);
                }
                Some(Type::intersection(expanded))
            }
            _ => Some(type_.clone()),
        }
    }

    pub fn unwrap_optional(&self, type_: &Type) -> Option<Type> {
        match type_ {
            Type::Union(types) => {
                let unwrapped: Vec<Type> = types
                    .iter()
                    .filter(|type_| !matches!(type_, Type::Nil))
                    .cloned()
                    .collect();
                (!unwrapped.is_empty()).then(|| Type::union(unwrapped))
            }
            Type::Nil => None,
            Type::Alias { .. } => {
                let expanded = self.expand_alias(type_);
                if expanded == *type_ {
                    Some(expanded)
                } else {
                    self.unwrap_optional(&expanded)
                }
            }
            _ => Some(type_.clone()),
        }
    }
}

pub fn flatten_union(type_: &Type) -> Vec<Type> {
    match type_ {
        Type::Union(types) => types.iter().flat_map(flatten_union).collect(),
        _ => vec![type_.clone()],
    }
}

pub fn instance_type(name: &str, args: Vec<Type>) -> Type {
    Type::NameInstance {
        name: TypeName::parse(name),
        args,
    }
}

pub fn is_instance_of(type_: &Type, name: &str) -> bool {
    matches!(type_, Type::NameInstance { name: type_name, .. } if *type_name == TypeName::parse(name))
}

pub fn symbol_type() -> Type {
    instance_type("::Symbol", Vec::new())
}

#[derive(Clone)]
pub struct TypeConstruction<'checker> {
    pub checker: &'checker Checker,
    pub source: &'checker Source,
    pub annotations: Rc<Collection>,
    pub typing: Typing,
    pub context: Context,
    pub inference: &'checker Inference,
}

#[derive(Clone)]
pub struct Pair<'checker> {
    pub type_: Type,
    pub constr: TypeConstruction<'checker>,
}

impl<'checker> Pair<'checker> {
    pub fn new(type_: Type, constr: TypeConstruction<'checker>) -> Pair<'checker> {
        Pair { type_, constr }
    }

    pub fn plus(self, other: Pair<'checker>) -> Pair<'checker> {
        if matches!(self.type_, Type::Bot) {
            Pair {
                type_: self.type_,
                constr: other.constr,
            }
        } else {
            other
        }
    }

    pub fn context(&self) -> &Context {
        &self.constr.context
    }
}

pub fn or_raise<Value, Failure: Into<RubyException>>(result: Result<Value, Failure>) -> Value {
    result.unwrap_or_else(|error| ruby_exception::raise(error))
}

pub fn option_or_raise<Value>(result: Result<Option<Value>, RbsError>) -> Option<Value> {
    or_raise(result)
}

pub fn convert_with_shape(builder: &Builder, shape: &Shape, method: &str) -> Option<Type> {
    let entry = option_or_raise(shape.method(builder, method))?;
    entry
        .method_types()
        .into_iter()
        .find(|method_type| match &method_type.type_.params {
            None => true,
            Some(params) => {
                params.positionals.iter().all(|param| {
                    param.kind != steep_interface::method_type::PositionalKind::Required
                }) && params.keywords.requireds.is_empty()
            }
        })
        .map(|method_type| method_type.type_.return_type.clone())
}

pub fn is_special_lvar_name(name: &str) -> bool {
    SPECIAL_LVAR_NAMES.contains(&name)
}

impl<'checker> TypeConstruction<'checker> {
    pub fn module_context(&self) -> &ModuleContext {
        &self.context.module_context
    }

    pub fn self_type(&self) -> &Type {
        &self.context.self_type
    }

    pub fn nesting(&self) -> ResolverContext {
        self.context.module_context.nesting.clone()
    }

    pub fn with_context(&self, context: Context) -> TypeConstruction<'checker> {
        TypeConstruction {
            context: Context {
                identity: ContextIdentity::default(),
                ..context
            },
            ..self.clone()
        }
    }

    pub fn with_new_typing(&self, typing: Typing) -> TypeConstruction<'checker> {
        TypeConstruction {
            typing,
            ..self.clone()
        }
    }

    pub fn update_type_env(
        &self,
        update: impl FnOnce(&TypeEnv) -> TypeEnv,
    ) -> TypeConstruction<'checker> {
        let mut context = self.context.clone();
        context.type_env = update(&self.context.type_env);
        self.with_context(context)
    }

    pub fn builder(&self) -> &'checker Builder {
        &self.checker.builder
    }

    pub fn steep_version(&self) -> SteepVersion {
        self.checker.builder.steep_version
    }

    pub fn check_relation_with(
        &self,
        sub_type: &Type,
        super_type: &Type,
        constraints: SharedConstraints,
    ) -> Rc<SubtypingResult> {
        let check = self.builder().subtyping();
        let bounds = self.context.variable_context.upper_bounds_list();
        or_raise(check.push_variable_bounds(bounds, || {
            check.check(
                Relation::new(sub_type.clone(), super_type.clone()),
                constraints,
                Some(self.self_type().clone()),
                Some(self.module_context().instance_type.clone()),
                Some(self.module_context().module_type.clone()),
            )
        }))
    }

    pub fn check_relation(&self, sub_type: &Type, super_type: &Type) -> Rc<SubtypingResult> {
        self.check_relation_with(sub_type, super_type, Constraints::empty().shared())
    }

    pub fn no_subtyping(&self, sub_type: &Type, super_type: &Type) -> Option<Rc<SubtypingResult>> {
        let result = self.check_relation(sub_type, super_type);
        result.is_failure().then_some(result)
    }

    pub fn add_typing(&self, node: &Node, type_: Type) -> Pair<'checker> {
        self.typing.add_typing(node.id, type_.clone());
        Pair::new(type_, self.clone())
    }

    pub fn add_error(&self, error: Diagnostic) {
        self.typing.add_error(error);
    }

    pub fn observe(&self, check: StrictCheck, node: &Node, message: impl Into<String>) {
        if let Some(location) = node.location.expression {
            self.typing
                .observe(Observation::new(check, location, message));
        }
    }

    pub fn add_call(&self, call: MethodCall) -> Pair<'checker> {
        match &call.kind {
            MethodCallKind::NoMethodError { error } => self.add_error(error.clone()),
            MethodCallKind::Error { errors } => {
                for error in errors {
                    self.add_error(error.clone());
                }
            }
            _ => {}
        }
        self.typing
            .add_typing(call.node.id, call.return_type.clone());
        self.typing.add_call(call.node.id, call.clone());
        Pair::new(call.return_type, self.clone())
    }

    pub fn fallback_to_any(&self, node: &Node, error: Option<Diagnostic>) -> Pair<'checker> {
        self.add_error(error.unwrap_or_else(|| diagnostics::fallback_any(node)));
        self.add_typing(node, Type::Any)
    }

    pub fn builder_config(&self) -> Config {
        Config {
            self_type: Some(self.self_type().clone()),
            class_type: Some(self.module_context().module_type.clone()),
            instance_type: Some(self.module_context().instance_type.clone()),
            variable_bounds: self.context.variable_context.upper_bounds(),
        }
    }

    pub fn calculate_interface(&self, type_: &Type, private: bool) -> Option<Shape> {
        let shape = or_raise(self.builder().shape(type_, &self.builder_config()))?;
        Some(if private { shape } else { shape.public_shape() })
    }

    pub fn expand_self(&self, type_: &Type) -> Type {
        if matches!(type_, Type::SelfType) {
            self.self_type().clone()
        } else {
            type_.clone()
        }
    }

    pub fn unwrap(&self, type_: &Type) -> Type {
        self.checker.unwrap_optional(type_).unwrap_or(Type::Bot)
    }

    pub fn union_type(&self, types: Vec<Type>) -> Type {
        Type::union(types)
    }

    pub fn union_type_unify(&self, types: Vec<Type>) -> Type {
        let types: Vec<Type> = types
            .into_iter()
            .filter(|type_| !matches!(type_, Type::Bot))
            .collect();
        let mut iterator = types.into_iter();
        let Some(mut accumulated) = iterator.next() else {
            return Type::Bot;
        };
        for type_ in iterator {
            accumulated = if matches!(accumulated, Type::Any) {
                type_
            } else if matches!(type_, Type::Any) {
                accumulated
            } else if self.no_subtyping(&accumulated, &type_).is_none() {
                type_
            } else if self.no_subtyping(&type_, &accumulated).is_none() {
                accumulated
            } else {
                Type::union(vec![accumulated, type_])
            };
        }
        accumulated
    }

    pub fn select_flatten_types(
        &self,
        type_: &Type,
        predicate: impl Fn(&Type) -> bool,
    ) -> Vec<Type> {
        let expanded = self
            .checker
            .deep_expand_alias(type_)
            .unwrap_or_else(|| type_.clone());
        flatten_union(&expanded)
            .into_iter()
            .filter(|type_| predicate(type_))
            .collect()
    }

    pub fn test_literal_type(&self, literal: Literal, hint: Option<&Type>) -> Option<Type> {
        match hint {
            None | Some(Type::Any) => None,
            Some(hint) => {
                let literal_type = Type::Literal(literal);
                self.check_relation(&literal_type, hint)
                    .is_success()
                    .then(|| self.unwrap(hint))
            }
        }
    }

    pub fn check(
        &self,
        node: &Rc<Node>,
        type_: &Type,
        constraints: SharedConstraints,
        on_failure: &mut dyn FnMut(&Type, &Type, &SubtypingResult),
    ) -> Pair<'checker> {
        let pair = self.synthesize(node, Some(type_), false);
        let result = self.check_relation_with(&pair.type_, type_, constraints);
        if result.is_failure() {
            on_failure(type_, &pair.type_, &result);
            Pair::new(type_.clone(), pair.constr)
        } else {
            pair
        }
    }

    pub fn synthesize_children(
        &self,
        node: &Node,
        skips: &[Option<&Rc<Node>>],
    ) -> TypeConstruction<'checker> {
        let mut constr = self.clone();
        for child in node.child_nodes() {
            if skips
                .iter()
                .any(|skip| skip.is_some_and(|skip| skip.id == child.id))
            {
                continue;
            }
            constr = constr.synthesize(child, None, false).constr;
        }
        constr
    }

    pub fn for_branch(
        &self,
        node: &Rc<Node>,
        break_context: Option<Option<BreakContext>>,
    ) -> TypeConstruction<'checker> {
        let annotations = self
            .checker
            .annotations(self.source, Some(node), &self.nesting());
        let mut type_env = self.context.type_env.clone();
        for (name, annotation_type) in &annotations.var_types {
            if let Some(outer_type) = type_env.get(name)
                && let Some(result) = self.no_subtyping(annotation_type, &outer_type)
            {
                self.add_error(diagnostics::incompatible_annotation(
                    node,
                    name,
                    &format!("{annotation_type} <: {outer_type}"),
                    &result,
                ));
            }
            type_env = type_env.assign_local_variable(
                name,
                annotation_type.clone(),
                Some(annotation_type.clone()),
            );
        }
        let mut context = self.context.clone();
        context.type_env = type_env;
        if let Some(break_context) = break_context {
            context.break_context = break_context;
        }
        self.with_context(context)
    }

    pub fn set_node_and_body_context(&self, node: &Node) {
        self.typing.update_cursor_context(|cursor_context| {
            cursor_context.set_node_context(node, &self.context);
            cursor_context.set_body_context(node, &self.context);
        });
    }

    pub fn set_body_context(&self, node: &Node) {
        self.typing.update_cursor_context(|cursor_context| {
            cursor_context.set_body_context(node, &self.context)
        });
    }

    pub fn tap_node_context(self, node: &Node) -> TypeConstruction<'checker> {
        self.set_node_context(node, &self.context);
        self
    }

    pub fn set_node_context(&self, node: &Node, context: &Context) {
        self.typing
            .update_cursor_context(|cursor_context| cursor_context.set_node_context(node, context));
    }

    pub fn save_typing(&self) -> TypeConstruction<'checker> {
        self.typing.save();
        self.with_new_typing(self.typing.parent().expect("a child typing has a parent"))
    }

    pub fn synthesize(
        &self,
        node: &Rc<Node>,
        hint: Option<&Type>,
        condition: bool,
    ) -> Pair<'checker> {
        ruby_exception::rescue(|| self.synthesize_without_rescue(node, hint, condition))
            .unwrap_or_else(|exception| {
                self.add_error(diagnostics::unexpected_error(node, &exception));
                self.type_any_recursive(node, false)
            })
    }

    fn synthesize_without_rescue(
        &self,
        node: &Rc<Node>,
        hint: Option<&Type>,
        condition: bool,
    ) -> Pair<'checker> {
        match node.type_ {
            "begin" | "kwbegin" => self.synthesize_begin(node, hint, condition),
            "lvasgn" => self.synthesize_lvasgn(node, hint),
            "lvar" => {
                let name = node.child_symbol(0).unwrap_or_default();
                if is_special_lvar_name(name) {
                    self.add_typing(node, Type::Any)
                } else if let Some(type_) = self.context.type_env.get(name) {
                    self.add_typing(node, type_)
                } else {
                    self.fallback_to_any(node, None)
                }
            }
            "ivasgn" => {
                let name = node.child_symbol(0).unwrap_or_default().to_string();
                let rhs = node.child_node(1).cloned();
                let hint = self.context.type_env.get(&name);
                let pair = match &rhs {
                    Some(rhs) => self.synthesize(rhs, hint.as_ref(), false),
                    None => Pair::new(Type::Any, self.clone()),
                };
                pair.constr.ivasgn(node, pair.type_)
            }
            "ivar" => {
                let name = node.child_symbol(0).unwrap_or_default();
                match self.context.type_env.get(name) {
                    Some(type_) => self.add_typing(node, type_),
                    None => self
                        .assigned_instance_variable_type(node, name)
                        .unwrap_or_else(|| self.fallback_to_any(node, None)),
                }
            }
            "gvasgn" => {
                let name = node.child_symbol(0).unwrap_or_default().to_string();
                let rhs = node.child_node(1).cloned();
                let hint = self.context.type_env.get(&name);
                let pair = match &rhs {
                    Some(rhs) => self.synthesize(rhs, hint.as_ref(), false),
                    None => Pair::new(Type::Any, self.clone()),
                };
                pair.constr
                    .check_deprecation_global(&name, node, node.location.name);
                pair.constr.gvasgn(node, pair.type_)
            }
            "gvar" => {
                let name = node.child_symbol(0).unwrap_or_default();
                self.check_deprecation_global(name, node, node.location.expression);
                match self.context.type_env.get(name) {
                    Some(type_) => self.add_typing(node, type_),
                    None => self.fallback_to_any(
                        node,
                        Some(diagnostics::unknown_global_variable(node, name)),
                    ),
                }
            }
            "op_asgn" => self.synthesize_op_assign(node, hint),
            "return" => self.synthesize_return(node),
            "break" => self.synthesize_break(node),
            "next" => self.synthesize_next(node),
            "retry" => self.add_typing(node, Type::Bot),
            "float" => self.add_typing(node, instance_type("::Float", Vec::new())),
            "rational" => self.add_typing(node, instance_type("::Rational", Vec::new())),
            "complex" => self.add_typing(node, instance_type("::Complex", Vec::new())),
            "nil" => self.add_typing(node, Type::Nil),
            "int" => {
                let value = match node.child(0) {
                    Child::Integer(value) => value.clone(),
                    _ => String::new(),
                };
                let type_ = self
                    .test_literal_type(Literal::Integer(value), hint)
                    .unwrap_or_else(|| instance_type("::Integer", Vec::new()));
                self.add_typing(node, type_)
            }
            "sym" => {
                let value = node.child_symbol(0).unwrap_or_default().to_string();
                let type_ = self
                    .test_literal_type(Literal::Symbol(value), hint)
                    .unwrap_or_else(symbol_type);
                self.add_typing(node, type_)
            }
            "str" => {
                let type_ = match node.child(0) {
                    Child::String(value) => {
                        self.test_literal_type(Literal::String(value.clone()), hint)
                    }
                    _ => None,
                }
                .unwrap_or_else(|| instance_type("::String", Vec::new()));
                self.add_typing(node, type_)
            }
            "true" | "false" => {
                let literal = Type::Literal(Literal::Bool(node.type_ == "true"));
                match hint {
                    Some(hint)
                        if self.check_relation(&literal, hint).is_success()
                            && !matches!(hint, Type::Any | Type::Top) =>
                    {
                        let unwrapped = self.unwrap(hint);
                        self.add_typing(node, unwrapped)
                    }
                    _ if condition => self.add_typing(node, literal),
                    _ => self.add_typing(node, Type::Boolean),
                }
            }
            "dstr" | "xstr" => {
                let constr = self.synthesize_children(node, &[]);
                constr.add_typing(node, instance_type("::String", Vec::new()))
            }
            "dsym" => {
                let constr = self.synthesize_children(node, &[]);
                constr.add_typing(node, symbol_type())
            }
            "self" => self.add_typing(node, Type::SelfType),
            "cbase" => self.add_typing(node, Type::Void),
            "const" => {
                let (type_, constr, name) = self.synthesize_constant(
                    Some(node),
                    node.child_node(0),
                    node.child_symbol(1).unwrap_or_default(),
                    None,
                );
                if let Some(name) = &name {
                    self.typing.update_source_index(|index| {
                        index.add_constant_reference(name.clone(), node)
                    });
                    constr.check_deprecation_constant(name, node, node.location.expression);
                }
                Pair::new(type_, constr)
            }
            "casgn" => self.synthesize_constant_assignment(node),
            "class" => self.synthesize_class(node),
            "module" => self.synthesize_module(node),
            "def" => self.synthesize_def(node),
            "defs" => self.synthesize_defs(node),
            "args" => {
                let mut constr = self.clone();
                for child in node.child_nodes() {
                    constr = constr.synthesize(child, None, false).constr;
                }
                constr.add_typing(node, Type::Any)
            }
            "arg" | "kwarg" => {
                let name = node.child_symbol(0).unwrap_or_default().to_string();
                if is_special_lvar_name(&name) {
                    self.add_typing(node, Type::Any)
                } else if let Some(type_) = self.context.type_env.get(&name) {
                    self.add_typing(node, type_)
                } else {
                    self.lvasgn(node, Type::Any)
                }
            }
            "optarg" | "kwoptarg" => {
                let name = node.child_symbol(0).unwrap_or_default().to_string();
                let Some(rhs) = node.child_node(1).cloned() else {
                    return self.add_typing(node, Type::Any);
                };
                if is_special_lvar_name(&name) {
                    let constr = self.synthesize(&rhs, None, false).constr;
                    return constr.add_typing(node, Type::Any);
                }
                let pair = match self.context.type_env.get(&name) {
                    Some(variable_type) => {
                        let typing = self.typing.clone();
                        self.check(
                            &rhs,
                            &variable_type,
                            Constraints::empty().shared(),
                            &mut |expected, actual, result| {
                                typing.add_error(diagnostics::incompatible_assignment(
                                    node, expected, actual, result,
                                ));
                            },
                        )
                    }
                    None => self.synthesize(&rhs, None, false),
                };
                pair.constr.add_typing(node, pair.type_)
            }
            "restarg" | "kwrestarg" => {
                let name = node.child_symbol(0);
                match name {
                    None => self.add_typing(node, Type::Any),
                    Some(name) if is_special_lvar_name(name) => self.add_typing(node, Type::Any),
                    Some(name) => match self.context.type_env.get(name) {
                        Some(type_) => self.add_typing(node, type_),
                        None => {
                            self.add_error(diagnostics::fallback_any(node));
                            let type_ = if node.type_ == "restarg" {
                                instance_type("::Array", vec![Type::Any])
                            } else {
                                instance_type("::Hash", vec![symbol_type(), Type::Any])
                            };
                            self.add_typing(node, type_)
                        }
                    },
                }
            }
            "blockarg" => {
                for child in node.child_nodes() {
                    self.synthesize(child, None, false);
                }
                self.add_typing(node, Type::Any)
            }
            "cvasgn" => {
                let name = node.child_symbol(0).unwrap_or_default().to_string();
                let pair = match node.child_node(1) {
                    Some(rhs) => self.synthesize(rhs, hint, false),
                    None => Pair::new(Type::Any, self.clone()),
                };
                match self.class_variable_type(&name) {
                    Some(variable_type) => {
                        let result = pair.constr.check_relation(&pair.type_, &variable_type);
                        if result.is_success() {
                            pair.constr.add_typing(node, pair.type_)
                        } else {
                            self.fallback_to_any(
                                node,
                                Some(diagnostics::incompatible_assignment(
                                    node,
                                    &variable_type,
                                    &pair.type_,
                                    &result,
                                )),
                            )
                        }
                    }
                    None => self.fallback_to_any(node, None),
                }
            }
            "cvar" => {
                let name = node.child_symbol(0).unwrap_or_default().to_string();
                match self.class_variable_type(&name) {
                    Some(type_) => self.add_typing(node, type_),
                    None => self.fallback_to_any(node, None),
                }
            }
            "or_asgn" | "and_asgn" => self.synthesize_or_and_assign(node, hint),
            "forwarded_args" | "forward_arg" => self.add_typing(node, Type::Any),
            "procarg0" => self.synthesize_procarg0(node),
            "mlhs" => self.synthesize_mlhs(node),
            "block_pass" => self.synthesize_block_pass(node, hint),
            "send" | "csend" | "block" | "numblock" | "itblock" => {
                self.synthesize_sendish(node, hint, None)
            }
            "assertion" => self.synthesize_assertion(node, hint),
            "tapp" => self.synthesize_type_application(node, hint),
            "masgn" => self.type_masgn(node),
            "sclass" => self.synthesize_singleton_class(node),
            "match_with_lvasgn" => {
                for child in node.child_nodes() {
                    self.synthesize(child, None, false);
                }
                self.add_typing(node, Type::Any)
            }
            "super" => self.synthesize_super(node, hint),
            "zsuper" => self.synthesize_zsuper(node),
            "yield" => self.synthesize_yield(node),
            "if" => self.synthesize_if(node, hint),
            "case" => self.synthesize_case(node, hint, condition),
            "and" | "or" => self.synthesize_and_or(node, hint, condition),
            "array" => self.synthesize_array(node, hint),
            "hash" | "kwargs" => self.synthesize_hash(node, hint),
            "splat" => {
                self.add_error(diagnostics::unsupported_syntax(
                    node,
                    Some("Unsupported splat node occurrence"),
                ));
                self.observe(
                    StrictCheck::UnexpectedSplat,
                    node,
                    "Steep does not expect a splat here, and types it as untyped without checking how its elements are used",
                );
                for child in node.child_nodes() {
                    self.synthesize(child, None, false);
                }
                self.add_typing(node, Type::Any)
            }
            "irange" | "erange" => self.synthesize_range(node),
            "regexp" => {
                let constr = self.synthesize_children(node, &[]);
                constr.add_typing(node, instance_type("::Regexp", Vec::new()))
            }
            "regopt" => self.add_typing(node, Type::Any),
            "back_ref" => self.synthesize(&node.updated(Some("gvar"), None, None), hint, false),
            "nth_ref" => self.synthesize_numbered_reference(node),
            "while" | "until" | "while_post" | "until_post" => self.synthesize_while(node, hint),
            "rescue" => self.synthesize_rescue(node, hint),
            "resbody" => self.synthesize_resbody(node, hint),
            "ensure" => self.synthesize_ensure(node),
            "for" => self.synthesize_for(node),
            "alias" => self.add_typing(node, Type::Nil),
            "defined?" => {
                self.observe(
                    StrictCheck::DefinedExpression,
                    node,
                    "Steep types the expression inside `defined?` as untyped without type checking it",
                );
                self.type_any_recursive(node, true);
                self.add_typing(
                    node,
                    Type::union(vec![instance_type("::String", Vec::new()), Type::Nil]),
                )
            }
            _ => {
                self.add_error(diagnostics::unsupported_syntax(node, None));
                self.observe(
                    StrictCheck::UnsupportedSyntax,
                    node,
                    format!(
                        "Steep does not support `{}`, and types it as untyped without type checking anything inside it",
                        node.type_
                    ),
                );
                self.add_typing(node, Type::Any)
            }
        }
    }

    pub fn type_any_recursive(&self, node: &Rc<Node>, only_children: bool) -> Pair<'checker> {
        if !only_children {
            self.add_typing(node, Type::Any);
        }
        for child in node.child_nodes() {
            self.type_any_recursive(child, false);
        }
        Pair::new(Type::Any, self.clone())
    }

    fn synthesize_begin(
        &self,
        node: &Rc<Node>,
        hint: Option<&Type>,
        condition: bool,
    ) -> Pair<'checker> {
        let children: Vec<&Rc<Node>> = node.child_nodes().collect();
        let Some((last, middle)) = children.split_last() else {
            return self.add_typing(node, Type::Nil);
        };
        let end_pos = node.expression().end_pos;
        let mut pair = Pair::new(Type::Nil, self.clone());
        for child in middle {
            let next = pair.constr.synthesize(child, None, false);
            let new_pair = pair.clone().plus(next);
            if !new_pair
                .constr
                .context
                .identity
                .same(&pair.constr.context.identity)
            {
                self.typing.update_cursor_context(|cursor_context| {
                    cursor_context.set(
                        CursorRange::new(child.expression().end_pos, end_pos),
                        &new_pair.constr.context,
                    )
                });
            }
            pair = new_pair;
        }
        let last_pair = pair.constr.synthesize(last, hint, condition);
        let last_pair = pair.plus(last_pair);
        last_pair.constr.add_typing(node, last_pair.type_)
    }

    fn synthesize_lvasgn(&self, node: &Rc<Node>, hint: Option<&Type>) -> Pair<'checker> {
        let name = node.child_symbol(0).unwrap_or_default().to_string();
        let rhs = node.child_node(1).cloned();
        match name.as_str() {
            "_" | "__any__" => {
                let constr = match &rhs {
                    Some(rhs) => self.synthesize(rhs, Some(&Type::Any), false).constr,
                    None => self.clone(),
                };
                constr.add_typing(node, Type::Any)
            }
            "__skip__" => {
                self.observe(
                    StrictCheck::SkipAssignment,
                    node,
                    "Steep does not type check the right hand side of an assignment to `__skip__`",
                );
                self.add_typing(node, Type::Any)
            }
            _ => {
                let mut hint = hint.cloned();
                let enforced_type = self.context.type_env.enforced_type(&name);
                if let Some(enforced_type) = &enforced_type {
                    match &hint {
                        None => hint = Some(enforced_type.clone()),
                        Some(current) => {
                            if self.check_relation(enforced_type, current).is_success() {
                                hint = Some(enforced_type.clone());
                            }
                        }
                    }
                }
                let Some(rhs) = rhs else {
                    return self.add_typing(node, enforced_type.unwrap_or(Type::Any));
                };
                let pair = self.synthesize(&rhs, hint.as_ref(), false);
                let rhs_type = pair.type_.clone();
                let mut variable_type = rhs_type.clone();
                let enforced_type = pair.constr.context.type_env.enforced_type(&name);
                if let Some(enforced_type) = &enforced_type {
                    if let Some(result) = pair.constr.no_subtyping(&rhs_type, enforced_type) {
                        self.add_error(diagnostics::incompatible_assignment(
                            node,
                            enforced_type,
                            &rhs_type,
                            &result,
                        ));
                        variable_type = enforced_type.clone();
                    }
                    if matches!(rhs_type, Type::Any) {
                        pair.constr.observe_untyped_assigned_to_declared_variable(
                            node,
                            &name,
                            enforced_type,
                        );
                        variable_type = enforced_type.clone();
                    }
                }
                let constr = pair.constr.update_type_env(|env| {
                    env.assign_local_variable(&name, variable_type, enforced_type.clone())
                });
                constr.add_typing(node, rhs_type)
            }
        }
    }

    pub fn lvasgn(&self, node: &Rc<Node>, type_: Type) -> Pair<'checker> {
        let name = node.child_symbol(0).unwrap_or_default().to_string();
        if is_special_lvar_name(&name) {
            return self.add_typing(node, Type::Any);
        }
        let mut type_ = type_;
        let enforced_type = self.context.type_env.enforced_type(&name);
        if let Some(enforced_type) = &enforced_type
            && let Some(result) = self.no_subtyping(&type_, enforced_type)
        {
            self.add_error(diagnostics::incompatible_assignment(
                node,
                enforced_type,
                &type_,
                &result,
            ));
            type_ = enforced_type.clone();
        }
        let constr = self.update_type_env(|env| {
            env.assign_local_variable(&name, type_.clone(), enforced_type.clone())
        });
        constr.add_typing(node, type_)
    }

    pub fn ivasgn(&self, node: &Rc<Node>, rhs_type: Type) -> Pair<'checker> {
        let name = node.child_symbol(0).unwrap_or_default();
        match self.context.type_env.get(name) {
            Some(lhs_type) => {
                let result = self.check_relation(&rhs_type, &lhs_type);
                if result.is_failure() {
                    self.add_error(diagnostics::incompatible_assignment(
                        node, &lhs_type, &rhs_type, &result,
                    ));
                }
            }
            None => {
                self.add_error(diagnostics::unknown_instance_variable(node, name));
                self.collect_instance_variable_assignment(name, &rhs_type);
            }
        }
        self.add_typing(node, rhs_type)
    }

    pub fn gvasgn(&self, node: &Rc<Node>, rhs_type: Type) -> Pair<'checker> {
        let name = node.child_symbol(0).unwrap_or_default();
        match self.context.type_env.get(name) {
            Some(lhs_type) => {
                if let Some(result) = self.no_subtyping(&rhs_type, &lhs_type) {
                    self.add_error(diagnostics::incompatible_assignment(
                        node, &lhs_type, &rhs_type, &result,
                    ));
                }
            }
            None => self.add_error(diagnostics::unknown_global_variable(node, name)),
        }
        self.add_typing(node, rhs_type)
    }

    fn synthesize_op_assign(&self, node: &Rc<Node>, hint: Option<&Type>) -> Pair<'checker> {
        let (Some(lhs), Some(operator), Some(rhs)) = (
            node.child_node(0).cloned(),
            node.child_symbol(1).map(str::to_string),
            node.child_node(2).cloned(),
        ) else {
            panic!("an op_asgn node has a left hand side, an operator and a right hand side");
        };
        let variable_type = match lhs.type_ {
            "lvasgn" => Some(("lvar", "lvasgn")),
            "ivasgn" => Some(("ivar", "ivasgn")),
            "cvasgn" => Some(("cvar", "cvasgn")),
            "gvasgn" => Some(("gvar", "gvasgn")),
            _ => None,
        };
        let new_node = if let Some((read, write)) = variable_type {
            let variable_node = lhs.updated(Some(read), None, None);
            let send_node = rhs.updated(
                Some("send"),
                Some(vec![
                    Child::Node(variable_node),
                    Child::Symbol(operator),
                    Child::Node(rhs.clone()),
                ]),
                None,
            );
            node.updated(
                Some(write),
                Some(vec![lhs.child(0).clone(), Child::Node(send_node)]),
                None,
            )
        } else if lhs.type_ == "send" {
            let new_rhs = rhs.updated(
                Some("send"),
                Some(vec![
                    Child::Node(lhs.clone()),
                    node.child(1).clone(),
                    node.child(2).clone(),
                ]),
                None,
            );
            let mut children = vec![
                lhs.child(0).clone(),
                Child::Symbol(format!("{}=", lhs.child_symbol(1).unwrap_or_default())),
            ];
            children.extend(lhs.children.iter().skip(2).cloned());
            children.push(Child::Node(new_rhs));
            lhs.updated(Some("send"), Some(children), None)
        } else {
            self.observe(
                StrictCheck::UnexpectedAssignmentTarget,
                node,
                format!(
                    "Steep does not handle an operator assignment to `{}`, and types it as untyped without checking the assignment",
                    lhs.type_
                ),
            );
            let constr = self.synthesize(&rhs, None, false).constr;
            return constr.add_typing(node, Type::Any);
        };
        let pair = self.synthesize(&new_node, hint, false);
        pair.constr.add_typing(node, pair.type_)
    }

    fn synthesize_return(&self, node: &Rc<Node>) -> Pair<'checker> {
        let method_context = self.context.method_context.clone();
        let method_return_type = method_context
            .as_ref()
            .map(|method_context| self.checker.expand_alias(&method_context.return_type));
        let pair = match node.children.len() {
            0 => Pair::new(Type::Nil, self.clone()),
            1 => {
                let value = node
                    .child_node(0)
                    .cloned()
                    .expect("return has a value node");
                self.synthesize(&value, method_return_type.as_ref(), false)
            }
            _ => {
                let array = node.updated(Some("array"), None, None);
                self.synthesize(&array, method_return_type.as_ref(), false)
            }
        };
        if let (Some(method_return_type), Some(method_context)) =
            (&method_return_type, &method_context)
            && !matches!(method_return_type, Type::Void)
        {
            let result = pair.constr.check_relation(&pair.type_, method_return_type);
            if result.is_failure() {
                if method_context.is_attribute_setter() {
                    self.add_error(diagnostics::setter_return_type_mismatch(
                        node,
                        method_context.name.as_deref().unwrap_or_default(),
                        method_return_type,
                        &pair.type_,
                        &result,
                    ));
                } else {
                    self.add_error(diagnostics::return_type_mismatch(
                        node,
                        method_return_type,
                        &pair.type_,
                        &result,
                    ));
                }
            }
        }
        pair.constr.add_typing(node, Type::Bot)
    }

    fn synthesize_break(&self, node: &Rc<Node>) -> Pair<'checker> {
        let value = node.child_node(0).cloned();
        match &self.context.break_context {
            Some(break_context) => {
                let break_type = break_context.break_type.clone();
                match &value {
                    Some(value) => {
                        if break_context.unchecked_break {
                            self.observe(
                                StrictCheck::UncheckedJumpValue,
                                node,
                                "Steep checks the value of this `break` against nothing, because the block or lambda has no type for it",
                            );
                        }
                        let typing = self.typing.clone();
                        self.check(
                            value,
                            &break_type,
                            Constraints::empty().shared(),
                            &mut |expected, actual, result| {
                                typing.add_error(diagnostics::break_type_mismatch(
                                    node, expected, actual, result,
                                ));
                            },
                        );
                    }
                    None => {
                        if !matches!(break_type, Type::Bot) {
                            let result = self.check_relation(&Type::Nil, &break_type);
                            if result.is_failure() {
                                self.add_error(diagnostics::implicit_break_value_mismatch(
                                    node,
                                    &break_type,
                                    &result,
                                ));
                            }
                        }
                    }
                }
            }
            None => {
                if let Some(value) = &value {
                    self.synthesize(value, None, false);
                }
                self.add_error(diagnostics::unexpected_jump(node));
            }
        }
        self.add_typing(node, Type::Bot)
    }

    fn synthesize_next(&self, node: &Rc<Node>) -> Pair<'checker> {
        let value = node.child_node(0).cloned();
        match &self.context.break_context {
            Some(break_context) => match &break_context.next_type {
                Some(next_type) => {
                    let next_type = self
                        .checker
                        .deep_expand_alias(next_type)
                        .unwrap_or_else(|| next_type.clone());
                    match &value {
                        Some(value) => {
                            if break_context.unchecked_next {
                                self.observe(
                                    StrictCheck::UncheckedJumpValue,
                                    node,
                                    "Steep checks the value of this `next` against nothing, because the block or lambda has no type for it",
                                );
                            }
                            let typing = self.typing.clone();
                            self.check(
                                value,
                                &next_type,
                                Constraints::empty().shared(),
                                &mut |expected, actual, result| {
                                    typing.add_error(diagnostics::break_type_mismatch(
                                        node, expected, actual, result,
                                    ));
                                },
                            );
                        }
                        None => {
                            let result = self.check_relation(&Type::Nil, &next_type);
                            if result.is_failure() {
                                self.add_error(diagnostics::break_type_mismatch(
                                    node,
                                    &next_type,
                                    &Type::Nil,
                                    &result,
                                ));
                            }
                        }
                    }
                }
                None => {
                    if let Some(value) = &value {
                        self.synthesize(value, None, false);
                        self.add_error(diagnostics::unexpected_jump_value(node));
                    }
                }
            },
            None => {
                if let Some(value) = &value {
                    self.synthesize(value, None, false);
                }
                self.add_error(diagnostics::unexpected_jump(node));
            }
        }
        self.add_typing(node, Type::Bot)
    }

    fn synthesize_if(&self, node: &Rc<Node>, hint: Option<&Type>) -> Pair<'checker> {
        let condition = node.child_node(0).cloned();
        let true_clause = node.child_node(1).cloned();
        let false_clause = node.child_node(2).cloned();
        let constr = match &condition {
            Some(condition) => self.synthesize(condition, None, true).constr,
            None => self.clone(),
        };
        let (truthy, falsy) = constr.evaluate_condition(condition.as_ref());
        let true_pair = true_clause.as_ref().map(|clause| {
            constr
                .update_type_env(|_| truthy.env.clone())
                .for_branch(clause, None)
                .tap_node_context(clause)
                .synthesize(clause, hint, false)
        });
        let false_pair = false_clause.as_ref().map(|clause| {
            constr
                .update_type_env(|_| falsy.env.clone())
                .for_branch(clause, None)
                .tap_node_context(clause)
                .synthesize(clause, hint, false)
        });
        let mut envs = Vec::new();
        if !truthy.unreachable {
            match &true_pair {
                Some(pair) => {
                    if !matches!(pair.type_, Type::Bot) {
                        envs.push(pair.context().type_env.clone());
                    }
                }
                None => envs.push(truthy.env.clone()),
            }
        }
        match &false_pair {
            Some(pair) => {
                if !falsy.unreachable && !matches!(pair.type_, Type::Bot) {
                    envs.push(pair.context().type_env.clone());
                }
            }
            None => envs.push(falsy.env.clone()),
        }
        let constr = constr.update_type_env(|env| env.join(&envs));
        let keyword = node
            .location
            .keyword
            .map(|keyword| self.source.buffer.source_of(keyword).to_string());
        if truthy.unreachable
            && let Some(true_clause) = &true_clause
        {
            let location = match keyword.as_deref() {
                Some("if" | "elsif") => node.location.begin.or(node.location.keyword),
                Some("unless") => node.location.else_,
                _ => true_clause.location.expression,
            };
            constr.add_error(Diagnostic::new(
                "UnreachableBranch",
                location,
                "The branch is unreachable",
            ));
        }
        if falsy.unreachable
            && let Some(false_clause) = &false_clause
        {
            let location = match keyword.as_deref() {
                Some("if" | "elsif") => node.location.else_,
                Some("unless") => node.location.begin.or(node.location.keyword),
                _ => false_clause.location.expression,
            };
            constr.add_error(Diagnostic::new(
                "UnreachableBranch",
                location,
                "The branch is unreachable",
            ));
        }
        let node_type = self.union_type_unify(vec![
            true_pair
                .as_ref()
                .map(|pair| pair.type_.clone())
                .unwrap_or(Type::Nil),
            false_pair
                .as_ref()
                .map(|pair| pair.type_.clone())
                .unwrap_or(Type::Nil),
        ]);
        let branches: Vec<(&Node, &Type)> =
            [(&true_clause, &true_pair), (&false_clause, &false_pair)]
                .into_iter()
                .filter_map(|(clause, pair)| Some((clause.as_deref()?, &pair.as_ref()?.type_)))
                .collect();
        constr.observe_dropped_untyped_branches(
            node,
            keyword.as_deref().unwrap_or("?:"),
            hint,
            &branches,
            &node_type,
        );
        constr.add_typing(node, node_type)
    }

    fn synthesize_and_or(
        &self,
        node: &Rc<Node>,
        hint: Option<&Type>,
        condition: bool,
    ) -> Pair<'checker> {
        let is_and = node.type_ == "and";
        let left = node.child_node(0).cloned().expect("and/or has a left node");
        let right = node
            .child_node(1)
            .cloned()
            .expect("and/or has a right node");
        let left_hint = if is_and {
            hint.cloned()
        } else {
            hint.map(|hint| {
                self.union_type_unify(vec![
                    hint.clone(),
                    Type::Nil,
                    Type::Literal(Literal::Bool(false)),
                ])
            })
        };
        let left_pair = self.synthesize(&left, left_hint.as_ref(), true);
        let (left_truthy, left_falsy) = left_pair.constr.evaluate_condition(Some(&left));
        let (first, second) = if is_and {
            (&left_truthy, &left_falsy)
        } else {
            (&left_falsy, &left_truthy)
        };
        let right_hint = if is_and {
            hint.cloned()
        } else {
            Some(left_truthy.type_.clone())
        };
        let right_pair = left_pair
            .constr
            .update_type_env(|_| first.env.clone())
            .tap_node_context(&right)
            .for_branch(&right, None)
            .synthesize(&right, right_hint.as_ref(), true);
        let (right_truthy, right_falsy) = right_pair.constr.evaluate_condition(Some(&right));
        let right_context_env = right_pair.context().type_env.clone();
        let (env, mut type_) = if first.unreachable {
            (second.env.clone(), second.type_.clone())
        } else if second.unreachable {
            (right_context_env.clone(), right_pair.type_.clone())
        } else if right_truthy.unreachable && right_falsy.unreachable {
            (second.env.clone(), second.type_.clone())
        } else {
            let env = self
                .context
                .type_env
                .join(&[second.env.clone(), right_context_env.clone()]);
            right_pair.constr.observe_untyped_in_union(
                node,
                &[(&left, &second.type_), (&right, &right_pair.type_)],
            );
            let mut type_ = Type::union(vec![second.type_.clone(), right_pair.type_.clone()]);
            if !matches!(type_, Type::Any)
                && self.check_relation(&type_, &Type::Boolean).is_success()
            {
                type_ = Type::Boolean;
            }
            (env, type_)
        };
        if condition {
            let (truthy_env, falsy_env) = if is_and {
                (
                    right_truthy.env.clone(),
                    self.context
                        .type_env
                        .join(&[left_falsy.env.clone(), right_falsy.env.clone()]),
                )
            } else {
                (
                    self.context
                        .type_env
                        .join(&[left_truthy.env.clone(), right_truthy.env.clone()]),
                    right_falsy.env.clone(),
                )
            };
            self.typing.add_logic_env(node.id, truthy_env, falsy_env);
            type_ = type_.clone();
        }
        right_pair
            .constr
            .update_type_env(|_| env)
            .add_typing(node, type_)
    }

    fn synthesize_range(&self, node: &Rc<Node>) -> Pair<'checker> {
        let mut constr = self.clone();
        let mut types = Vec::new();
        let mut operands = Vec::new();
        for index in 0..2 {
            match node.child_node(index) {
                Some(child) => {
                    let pair = constr.synthesize(child, None, false);
                    types.push(pair.type_.clone());
                    operands.push((child.clone(), pair.type_.clone()));
                    constr = pair.constr;
                }
                None => types.push(Type::Nil),
            }
        }
        let operand_references: Vec<(&Node, &Type)> = operands
            .iter()
            .map(|(child, type_)| (child.as_ref(), type_))
            .collect();
        constr.observe_untyped_in_union(node, &operand_references);
        constr.add_typing(node, instance_type("::Range", vec![Type::union(types)]))
    }

    fn class_variable_type(&self, name: &str) -> Option<Type> {
        let definition = self.module_context().module_definition.as_ref()?;
        let variable = definition.class_variables.get(name)?;
        Some(self.builder().factory.type_(&variable.type_))
    }

    fn synthesize_or_and_assign(&self, node: &Rc<Node>, hint: Option<&Type>) -> Pair<'checker> {
        let (Some(assignment), Some(rhs)) =
            (node.child_node(0).cloned(), node.child_node(1).cloned())
        else {
            panic!("an or_asgn or and_asgn node has a left hand side and a right hand side");
        };
        match assignment.type_ {
            "lvasgn" => {
                let pair = self.synthesize(&rhs, hint, false);
                pair.constr.lvasgn(&assignment, pair.type_)
            }
            "ivasgn" => {
                let pair = self.synthesize(&rhs, hint, false);
                pair.constr.ivasgn(&assignment, pair.type_)
            }
            "gvasgn" => {
                let pair = self.synthesize(&rhs, hint, false);
                pair.constr.gvasgn(&assignment, pair.type_)
            }
            "send" => {
                let mut children = assignment.children.clone();
                children[1] = Child::Symbol(format!(
                    "{}=",
                    assignment.child_symbol(1).unwrap_or_default()
                ));
                children.push(Child::Node(rhs.clone()));
                let new_rhs = node.updated(Some("send"), Some(children), None);
                let node_type = if node.type_ == "or_asgn" { "or" } else { "and" };
                let new_node = node.updated(
                    Some(node_type),
                    Some(vec![Child::Node(assignment.clone()), Child::Node(new_rhs)]),
                    None,
                );
                self.synthesize(&new_node, hint, false)
            }
            _ => {
                self.observe(
                    StrictCheck::UnexpectedAssignmentTarget,
                    node,
                    format!(
                        "Steep does not handle `{}` to `{}`, and types it as untyped without type checking the right hand side",
                        node.type_, assignment.type_
                    ),
                );
                self.fallback_to_any(node, None)
            }
        }
    }

    fn pinned(env: &TypeEnv) -> TypeEnv {
        let mut env = env.clone();
        for (name, (type_, enforced)) in env.pin_local_variables(None) {
            env.local_variable_types
                .insert(name, (type_, Some(enforced)));
        }
        env
    }

    fn synthesize_while(&self, node: &Rc<Node>, hint: Option<&Type>) -> Pair<'checker> {
        let condition = node.child_node(0).cloned();
        let body = node.child_node(1).cloned();
        let break_context = BreakContext {
            break_type: hint.cloned().unwrap_or(Type::Nil),
            next_type: None,
            unchecked_break: false,
            unchecked_next: false,
        };
        if node.type_ == "while_post" || node.type_ == "until_post" {
            let condition_constr = match &condition {
                Some(condition) => self.synthesize(condition, None, false).constr,
                None => self.clone(),
            };
            return match &body {
                Some(body) => {
                    let for_loop = condition_constr
                        .update_type_env(Self::pinned)
                        .for_branch(body, Some(Some(break_context)))
                        .tap_node_context(body);
                    let body_env = for_loop
                        .synthesize(body, None, false)
                        .context()
                        .type_env
                        .clone();
                    let constr =
                        condition_constr.update_type_env(|env| env.join(&[env.clone(), body_env]));
                    constr.add_typing(node, Type::Nil)
                }
                None => condition_constr.add_typing(node, Type::Nil),
            };
        }
        let constr = match &condition {
            Some(condition) => self.synthesize(condition, None, true).constr,
            None => self.clone(),
        };
        let (truthy, falsy) = constr.evaluate_condition(condition.as_ref());
        let (body_env, exit_env) = if node.type_ == "while" {
            (truthy.env, falsy.env)
        } else {
            (falsy.env, truthy.env)
        };
        let constr = match &body {
            Some(body) => {
                let body_env = Self::pinned(&body_env);
                let body_constr = constr
                    .update_type_env(|_| body_env)
                    .for_branch(body, Some(Some(break_context)))
                    .tap_node_context(body)
                    .synthesize(body, None, false)
                    .constr;
                let body_type_env = body_constr.context.type_env.clone();
                constr.update_type_env(|env| env.join(&[exit_env, body_type_env]))
            }
            None => constr.update_type_env(|_| exit_env),
        };
        constr.add_typing(node, Type::Nil)
    }

    fn synthesize_rescue(&self, node: &Rc<Node>, hint: Option<&Type>) -> Pair<'checker> {
        let count = node.children.len();
        let body = node.child_node(0).cloned();
        let resbodies: Vec<Rc<Node>> = node.children[1..count.saturating_sub(1)]
            .iter()
            .filter_map(Child::node)
            .cloned()
            .collect();
        let else_node = if count >= 2 {
            node.child_node(count - 1).cloned()
        } else {
            None
        };
        let body_pair = body.as_ref().map(|body| self.synthesize(body, hint, false));
        let body_constr = match &body_pair {
            Some(pair) => {
                let body_env = pair.context().type_env.clone();
                self.update_type_env(|env| env.join(&[env.clone(), body_env]))
            }
            None => self.clone(),
        };
        let mut resbody_pairs = Vec::new();
        let mut resbody_operands = Vec::new();
        for resbody in &resbodies {
            let exception_classes = resbody.child_node(0).cloned();
            let assignment = resbody.child_node(1).cloned();
            let resbody_body = resbody.child_node(2).cloned();
            let exception_types: Option<Vec<Type>> =
                exception_classes.as_ref().and_then(|classes| {
                    (classes.type_ == "array").then(|| {
                        classes
                            .child_nodes()
                            .map(|child| self.synthesize(child, None, false).type_)
                            .collect()
                    })
                });
            let variable_name = assignment
                .as_ref()
                .filter(|assignment| assignment.type_ == "lvasgn")
                .and_then(|assignment| assignment.child_symbol(0).map(str::to_string));
            self.observe_rescue_variable(
                exception_classes.as_ref(),
                exception_types.as_deref(),
                assignment.as_ref(),
            );
            let resbody_construction =
                body_constr
                    .for_branch(resbody, None)
                    .update_type_env(|env| {
                        let mut assignments = Vec::new();
                        match (&exception_classes, &variable_name, &exception_types) {
                            (Some(_), Some(name), Some(types)) => {
                                let instance_types = types
                                    .iter()
                                    .map(|type_| match self.checker.expand_alias(type_) {
                                        Type::Singleton(name) => self.to_instance_type(&name),
                                        _ => Type::Any,
                                    })
                                    .collect();
                                assignments.push((name.clone(), Type::union(instance_types)));
                            }
                            (_, Some(name), _) => assignments.push((name.clone(), Type::Any)),
                            _ => {}
                        }
                        env.assign_local_variables(&assignments)
                    });
            let pair = match &resbody_body {
                Some(resbody_body) => resbody_construction
                    .tap_node_context(resbody_body)
                    .synthesize(resbody_body, hint, false),
                None => Pair::new(Type::Nil, body_constr.clone()),
            };
            resbody_operands.push((
                resbody_body.unwrap_or_else(|| resbody.clone()),
                pair.type_.clone(),
            ));
            resbody_pairs.push(pair);
        }
        let resbody_pairs: Vec<Pair> = resbody_pairs
            .into_iter()
            .filter(|pair| self.no_subtyping(&pair.type_, &Type::Bot).is_some())
            .collect();
        let resbody_types: Vec<Type> = resbody_pairs
            .iter()
            .map(|pair| pair.type_.clone())
            .collect();
        let resbody_envs: Vec<TypeEnv> = resbody_pairs
            .iter()
            .map(|pair| pair.context().type_env.clone())
            .collect();
        let else_constr = body_pair
            .as_ref()
            .map(|pair| pair.constr.clone())
            .unwrap_or_else(|| self.clone());
        match &else_node {
            Some(else_node) => {
                let else_pair = else_constr
                    .for_branch(else_node, None)
                    .synthesize(else_node, hint, false);
                let mut types = vec![else_pair.type_.clone()];
                types.extend(resbody_types);
                let type_ = Type::union(types);
                else_pair.constr.observe_rescue_branches(
                    node,
                    hint,
                    Some((else_node, &else_pair.type_)),
                    &resbody_operands,
                    &type_,
                );
                else_pair
                    .constr
                    .update_type_env(|env| {
                        let mut envs = resbody_envs.clone();
                        envs.push(env.clone());
                        env.join(&envs)
                    })
                    .add_typing(node, type_)
            }
            None => {
                let main = body
                    .as_deref()
                    .zip(body_pair.as_ref().map(|pair| &pair.type_));
                if resbody_types.is_empty() {
                    let constr = body_pair
                        .as_ref()
                        .map(|pair| pair.constr.clone())
                        .unwrap_or_else(|| self.clone());
                    let type_ = body_pair
                        .as_ref()
                        .map(|pair| pair.type_.clone())
                        .unwrap_or(Type::Nil);
                    constr.observe_rescue_branches(node, hint, main, &resbody_operands, &type_);
                    constr.add_typing(node, type_)
                } else {
                    let else_env = else_constr.context.type_env.clone();
                    let mut types: Vec<Type> = body_pair
                        .as_ref()
                        .map(|pair| pair.type_.clone())
                        .into_iter()
                        .collect();
                    types.extend(resbody_types);
                    let type_ = Type::union(types);
                    self.observe_rescue_branches(node, hint, main, &resbody_operands, &type_);
                    self.update_type_env(|env| {
                        let mut envs = resbody_envs.clone();
                        envs.push(else_env);
                        env.join(&envs)
                    })
                    .add_typing(node, type_)
                }
            }
        }
    }

    fn observe_rescue_variable(
        &self,
        exception_classes: Option<&Rc<Node>>,
        exception_types: Option<&[Type]>,
        assignment: Option<&Rc<Node>>,
    ) {
        let Some(assignment) = assignment else {
            return;
        };
        if assignment.type_ != "lvasgn" {
            self.observe(
                StrictCheck::RescueAssignmentTarget,
                assignment,
                format!(
                    "Steep does not assign the exception to `{}`, and never type checks the target",
                    assignment.type_
                ),
            );
            return;
        }
        let (Some(exception_classes), Some(exception_types)) = (exception_classes, exception_types)
        else {
            self.observe(
                StrictCheck::BareRescueVariable,
                assignment,
                "Steep types the variable of `rescue` without exception classes as untyped, though Ruby rescues only StandardError there; write `rescue StandardError => variable`",
            );
            return;
        };
        for (class_node, type_) in exception_classes.child_nodes().zip(exception_types) {
            if class_node.type_ == "splat" {
                continue;
            }
            match self.checker.expand_alias(type_) {
                Type::Singleton(name) => {
                    if let Type::NameInstance { args, .. } = self.to_instance_type(&name)
                        && !args.is_empty()
                    {
                        self.observe(
                            StrictCheck::UntypedRescueVariable,
                            class_node,
                            format!(
                                "Steep types the rescue variable as `{name}` with untyped type arguments"
                            ),
                        );
                    }
                }
                other => self.observe(
                    StrictCheck::UntypedRescueVariable,
                    class_node,
                    format!(
                        "Steep types the rescue variable as untyped, because the type of this exception class is `{other}`, not a singleton type"
                    ),
                ),
            }
        }
    }

    fn synthesize_resbody(&self, node: &Rc<Node>, hint: Option<&Type>) -> Pair<'checker> {
        if let Some(classes) = node.child_node(0) {
            self.synthesize(classes, None, false);
        }
        if let Some(assignment) = node.child_node(1) {
            self.synthesize(assignment, None, false);
        }
        match node.child_node(2) {
            Some(body) => {
                let body_type = self.synthesize(body, hint, false).type_;
                self.add_typing(node, body_type)
            }
            None => self.add_typing(node, Type::Nil),
        }
    }

    fn synthesize_ensure(&self, node: &Rc<Node>) -> Pair<'checker> {
        let body_type = node
            .child_node(0)
            .map(|body| self.synthesize(body, None, false).type_);
        if let Some(ensure_body) = node.child_node(1) {
            self.for_branch(node, None)
                .synthesize(ensure_body, None, false);
        }
        self.add_typing(node, body_type.unwrap_or(Type::Nil))
    }

    fn synthesize_for(&self, node: &Rc<Node>) -> Pair<'checker> {
        let assignment = node.child_node(0).cloned();
        let collection = node.child_node(1).cloned().expect("for has a collection");
        let body = node.child_node(2).cloned();
        let collection_pair = self.synthesize(&collection, None, false);
        let constr = collection_pair.constr.clone();
        let collection_env = constr.context.type_env.clone();
        let collection_type = self.expand_self(&collection_pair.type_);
        let mut untyped_variable_reason = None;
        let variable_type = if matches!(collection_type, Type::Any) {
            untyped_variable_reason = Some("the collection is untyped");
            Some(Type::Any)
        } else {
            self.calculate_interface(&collection_type, true)
                .and_then(|shape| option_or_raise(shape.method(self.builder(), "each")))
                .and_then(|each| {
                    each.method_types()
                        .into_iter()
                        .find(|method_type| match &method_type.block {
                            Some(block) => match &block.type_.params {
                                Some(params) => !params.positionals.is_empty(),
                                None => true,
                            },
                            None => false,
                        })
                        .cloned()
                })
                .and_then(|method_type| {
                    method_type.block.map(|block| {
                        block
                            .type_
                            .params
                            .as_ref()
                            .and_then(|params| {
                                params.positionals.first().map(|param| param.type_.clone())
                            })
                            .unwrap_or_else(|| {
                                untyped_variable_reason =
                                    Some("the block type of its `each` has `(?)` parameters");
                                Type::Any
                            })
                    })
                })
        };
        if let (Some(reason), Some(assignment)) = (untyped_variable_reason, &assignment) {
            self.observe(
                StrictCheck::UntypedForVariable,
                assignment,
                format!("Steep types the variable of this `for` loop as untyped, because {reason}"),
            );
        }
        let variable_name = assignment
            .as_ref()
            .and_then(|assignment| assignment.child_symbol(0).map(str::to_string));
        match variable_type {
            Some(variable_type) => {
                let constr = match &body {
                    Some(body) => {
                        let variable_name = variable_name.unwrap_or_else(|| {
                            ruby_exception::raise(RubyException::new(
                                "NoMethodError",
                                "undefined method 'start_with?' for an instance of Parser::AST::Node",
                            ))
                        });
                        let body_constr = constr.update_type_env(|env| {
                            Self::pinned(&env.assign_local_variables(&[(
                                variable_name.clone(),
                                variable_type.clone(),
                            )]))
                        });
                        self.typing.update_cursor_context(|cursor_context| {
                            cursor_context.set_body_context(node, &body_constr.context)
                        });
                        let body_env = body_constr
                            .synthesize(body, None, false)
                            .context()
                            .type_env
                            .clone();
                        constr.update_type_env(|env| env.join(&[collection_env.clone(), body_env]))
                    }
                    None => self.clone(),
                };
                constr.add_typing(node, collection_type)
            }
            None => {
                let constr = self.synthesize_children(node, &[Some(&collection)]);
                constr.fallback_to_any(
                    node,
                    Some(diagnostics::no_method(
                        node,
                        &collection_type,
                        "each",
                        self.steep_version(),
                    )),
                )
            }
        }
    }

    pub fn to_instance_type(&self, name: &TypeName) -> Type {
        let environment = self.checker.environment();
        let normalized = environment.normalize_module_name(name);
        let arity = environment
            .class_declarations
            .get(&normalized)
            .map(|entry| entry.type_parameters().len())
            .unwrap_or(0);
        Type::NameInstance {
            name: name.clone(),
            args: vec![Type::Any; arity],
        }
    }

    fn synthesize_array(&self, node: &Rc<Node>, hint: Option<&Type>) -> Pair<'checker> {
        if node.children.is_empty() {
            return match hint {
                Some(hint) => {
                    let array = instance_type("::Array", vec![Type::Any]);
                    if self.check_relation(&array, hint).is_success() {
                        self.add_typing(node, self.unwrap(hint))
                    } else {
                        self.add_typing(node, array)
                    }
                }
                None => {
                    self.add_error(diagnostics::unannotated_empty_collection(node));
                    self.add_typing(node, instance_type("::Array", vec![Type::Any]))
                }
            };
        }
        if let Some(hint) = hint {
            let mut tuples =
                self.select_flatten_types(hint, |type_| matches!(type_, Type::Tuple(_)));
            if tuples.is_empty()
                && self.steep_version() >= SteepVersion::Version2_1_0
                && let Some(converted) = self.try_convert(hint, "to_ary")
            {
                tuples =
                    self.select_flatten_types(&converted, |type_| matches!(type_, Type::Tuple(_)));
            }
            let mut fallback: Option<Pair> = None;
            for tuple in &tuples {
                let child = self.with_new_typing(self.typing.new_child());
                if let Some(pair) = child.try_tuple_type(node, Some(tuple)) {
                    if pair.constr.check_relation(&pair.type_, tuple).is_success() {
                        let constr = pair
                            .constr
                            .save_typing()
                            .with_new_typing(self.typing.clone());
                        return Pair::new(pair.type_, constr);
                    }
                    if fallback.is_none() {
                        let constr = pair
                            .constr
                            .save_typing()
                            .with_new_typing(self.typing.clone());
                        fallback = Some(Pair::new(pair.type_, constr));
                    }
                }
            }
            if let Some(fallback) = fallback {
                return fallback;
            }
            let arrays = self.select_flatten_types(hint, |type_| is_instance_of(type_, "::Array"));
            for array in &arrays {
                let child = self.with_new_typing(self.typing.new_child());
                let pair = child.try_array_type(node, Some(array));
                if pair.constr.check_relation(&pair.type_, hint).is_success() {
                    let constr = pair
                        .constr
                        .save_typing()
                        .with_new_typing(self.typing.clone());
                    return Pair::new(pair.type_, constr);
                }
            }
        }
        self.try_array_type(node, None)
    }

    fn try_tuple_type(&self, node: &Rc<Node>, hint: Option<&Type>) -> Option<Pair<'checker>> {
        let hint_types = match hint {
            Some(Type::Tuple(types)) => Some(types.clone()),
            _ => None,
        };
        let mut constr = self.clone();
        let mut element_types = Vec::new();
        for (index, child) in node.child_nodes().enumerate() {
            if child.type_ == "splat" {
                let inner = child.child_node(0).cloned()?;
                let pair = constr.synthesize(&inner, None, false);
                constr = pair.constr;
                self.typing.add_typing(child.id, pair.type_.clone());
                match constr.try_convert(&pair.type_, "to_a") {
                    Some(Type::Tuple(types)) => element_types.extend(types),
                    Some(_) => return None,
                    None => element_types.push(pair.type_),
                }
            } else {
                let child_hint = hint_types
                    .as_ref()
                    .and_then(|types| types.get(index).cloned());
                let pair = constr.synthesize(child, child_hint.as_ref(), false);
                element_types.push(pair.type_);
                constr = pair.constr;
            }
        }
        Some(constr.add_typing(node, Type::Tuple(element_types)))
    }

    pub fn try_tuple_type_strict(&self, node: &Rc<Node>, hint: Option<&Type>) -> Pair<'checker> {
        if node.type_ == "array" {
            if node.children.len() == 1
                && let Some(splat) = node.child_node(0).filter(|child| child.type_ == "splat")
                && let Some(value) = splat.child_node(0)
            {
                let pair = self.try_tuple_type_strict(value, hint);
                let constr = pair.constr.add_typing(splat, Type::Any).constr;
                return constr.add_typing(node, pair.type_);
            }
            if hint.is_none_or(|hint| matches!(hint, Type::Tuple(_))) {
                let child = self.with_new_typing(self.typing.new_child());
                if let Some(pair) = child.try_tuple_type(node, hint) {
                    return Pair::new(pair.type_, pair.constr.save_typing());
                }
            }
        }
        self.synthesize(node, hint, false)
    }

    pub fn try_convert(&self, type_: &Type, method: &str) -> Option<Type> {
        let shape = self.calculate_interface(type_, false)?;
        convert_with_shape(self.builder(), &shape, method)
    }

    fn flatten_array_elements(&self, type_: &Type) -> Vec<Type> {
        let expanded = self
            .checker
            .deep_expand_alias(type_)
            .unwrap_or_else(|| type_.clone());
        flatten_union(&expanded)
            .into_iter()
            .flat_map(|type_| match &type_ {
                Type::NameInstance { name, args } if *name == TypeName::parse("::Array") => {
                    args.clone()
                }
                _ => vec![type_],
            })
            .collect()
    }

    fn try_array_type(&self, node: &Rc<Node>, hint: Option<&Type>) -> Pair<'checker> {
        let element_hint = match hint {
            Some(Type::NameInstance { args, .. }) => args.first().cloned(),
            _ => None,
        };
        let mut constr = self.clone();
        let mut element_types = Vec::new();
        for child in node.child_nodes() {
            if child.type_ == "splat" {
                let Some(inner) = child.child_node(0) else {
                    continue;
                };
                let pair = constr.synthesize(inner, hint, false);
                constr = pair.constr;
                let type_ = constr
                    .try_convert(&pair.type_, "to_a")
                    .unwrap_or(pair.type_);
                match &type_ {
                    Type::NameInstance { name, args }
                        if *name == TypeName::parse("::Array") && args.len() == 1 =>
                    {
                        element_types.push(args[0].clone())
                    }
                    Type::Tuple(types) => element_types.extend(types.iter().cloned()),
                    _ => element_types.extend(self.flatten_array_elements(&type_)),
                }
            } else {
                let pair = constr.synthesize(child, element_hint.as_ref(), false);
                element_types.push(pair.type_);
                constr = pair.constr;
            }
        }
        constr.add_typing(
            node,
            instance_type("::Array", vec![Type::union(element_types)]),
        )
    }

    pub fn type_hash_record(
        &self,
        node: &Rc<Node>,
        record_type: Option<&Type>,
    ) -> Option<Pair<'checker>> {
        let hints: Vec<RecordElement> = match record_type {
            Some(Type::Record(elements)) => elements.clone(),
            _ => Vec::new(),
        };
        let mut constr = self.clone();
        let mut elements: Vec<RecordElement> = Vec::new();
        for child in node.child_nodes() {
            if child.type_ != "pair" {
                return None;
            }
            let key_node = child.child_node(0)?.clone();
            let value_node = child.child_node(1)?.clone();
            let key = match (key_node.type_, key_node.child(0)) {
                ("sym", Child::Symbol(value)) => Literal::Symbol(value.clone()),
                ("str", Child::String(value)) => Literal::String(value.clone()),
                ("int", Child::Integer(value)) => Literal::Integer(value.clone()),
                ("true", _) => Literal::Bool(true),
                ("false", _) => Literal::Bool(false),
                _ => return None,
            };
            constr = constr
                .synthesize(&key_node, Some(&Type::Literal(key.clone())), false)
                .constr;
            let hint = hints
                .iter()
                .find(|element| element.key.inspect() == key.inspect())
                .map(|element| element.type_.clone());
            let pair = constr.synthesize(&value_node, hint.as_ref(), false);
            constr = pair.constr;
            let mut value_type = pair.type_;
            match &hint {
                Some(hint_type) => {
                    if matches!(value_type, Type::Any) {
                        value_type = hint_type.clone();
                    }
                }
                None => {
                    self.add_error(diagnostics::unknown_record_key(&key_node, &key.inspect()));
                }
            }
            match elements
                .iter_mut()
                .find(|element| element.key.inspect() == key.inspect())
            {
                Some(element) => element.type_ = value_type,
                None => elements.push(RecordElement {
                    key,
                    type_: value_type,
                    required: true,
                }),
            }
        }
        if let Some(Type::Record(record_elements)) = record_type {
            for element in &mut elements {
                element.required = record_elements
                    .iter()
                    .find(|hint| hint.key.inspect() == element.key.inspect())
                    .is_some_and(|hint| hint.required);
            }
        }
        Some(constr.add_typing(node, Type::Record(elements)))
    }

    pub fn type_hash(&self, node: &Rc<Node>, hint: Option<&Type>) -> Pair<'checker> {
        let hint = hint.map(|hint| {
            self.checker
                .deep_expand_alias(hint)
                .unwrap_or_else(|| hint.clone())
        });
        match &hint {
            Some(record @ Type::Record(_)) => {
                let child = self.with_new_typing(self.typing.new_child());
                if let Some(pair) = child.type_hash_record(node, Some(record)) {
                    return Pair::new(pair.type_, pair.constr.save_typing());
                }
            }
            Some(Type::Union(types)) => {
                for type_ in types {
                    let child = self.with_new_typing(self.typing.new_child());
                    let pair = child.type_hash(node, Some(type_));
                    if pair.constr.check_relation(&pair.type_, type_).is_success() {
                        return Pair::new(pair.type_, pair.constr.save_typing());
                    }
                }
            }
            _ => {}
        }
        let (key_hint, value_hint) = match &hint {
            Some(Type::NameInstance { name, args })
                if *name == TypeName::parse("::Hash") && args.len() == 2 =>
            {
                (Some(args[0].clone()), Some(args[1].clone()))
            }
            _ => (None, None),
        };
        let hint_hash = instance_type(
            "::Hash",
            vec![
                key_hint.clone().unwrap_or(Type::Any),
                value_hint.clone().unwrap_or(Type::Any),
            ],
        );
        let mut constr = self.clone();
        let mut key_types = Vec::new();
        let mut value_types = Vec::new();
        let mut key_elements = Vec::new();
        let mut value_elements = Vec::new();
        let mut splats = Vec::new();
        if node.children.is_empty() {
            key_types.extend(key_hint.clone());
            value_types.extend(value_hint.clone());
        } else {
            for element in node.child_nodes() {
                match element.type_ {
                    "pair" => {
                        if let Some(key) = element.child_node(0) {
                            let pair = constr.synthesize(key, key_hint.as_ref(), false);
                            key_elements.push((key.clone(), pair.type_.clone()));
                            key_types.push(pair.type_);
                            constr = pair.constr;
                        }
                        if let Some(value) = element.child_node(1) {
                            let pair = constr.synthesize(value, value_hint.as_ref(), false);
                            value_elements.push((value.clone(), pair.type_.clone()));
                            value_types.push(pair.type_);
                            constr = pair.constr;
                        }
                    }
                    "kwsplat" => {
                        let inner = element.child_node(0).cloned();
                        if let Some(inner) = inner {
                            let pair = constr.synthesize(&inner, Some(&hint_hash), false);
                            pair.constr.add_typing(element, pair.type_.clone());
                            splats.push((element.clone(), pair.type_.clone()));
                            if let Type::NameInstance { name, args } = &pair.type_
                                && *name == TypeName::parse("::Hash")
                                && args.len() == 2
                            {
                                key_elements.push((element.clone(), args[0].clone()));
                                value_elements.push((element.clone(), args[1].clone()));
                                key_types.push(args[0].clone());
                                value_types.push(args[1].clone());
                            }
                        }
                    }
                    _ => ruby_exception::raise_runtime_error(""),
                }
            }
        }
        constr.observe_dropped_hash_elements(
            node,
            hint.is_some(),
            &key_elements,
            &value_elements,
            &splats,
        );
        key_types.retain(|type_| !matches!(type_, Type::Any));
        value_types.retain(|type_| !matches!(type_, Type::Any));
        if key_types.is_empty() {
            key_types.push(Type::Any);
        }
        if value_types.is_empty() {
            value_types.push(Type::Any);
        }
        constr.add_typing(
            node,
            instance_type(
                "::Hash",
                vec![Type::union(key_types), Type::union(value_types)],
            ),
        )
    }

    fn synthesize_hash(&self, node: &Rc<Node>, hint: Option<&Type>) -> Pair<'checker> {
        let pair = self.type_hash(node, hint);
        let empty = instance_type("::Hash", vec![Type::Any, Type::Any]);
        if pair.type_ == empty {
            match hint {
                Some(Type::Any | Type::Top | Type::Void) => {}
                Some(hint) if *hint == pair.type_ => {}
                _ => pair
                    .constr
                    .add_error(diagnostics::unannotated_empty_collection(node)),
            }
        }
        pair
    }

    pub fn logic_type_interpreter(&self) -> LogicTypeInterpreter<'checker> {
        LogicTypeInterpreter::new(self.checker, self.typing.clone(), self.builder_config())
    }

    pub fn evaluate_condition(
        &self,
        node: Option<&Rc<Node>>,
    ) -> (
        logic_type_interpreter::Result,
        logic_type_interpreter::Result,
    ) {
        let node = node.expect("a conditional node has a condition");
        self.logic_type_interpreter()
            .eval(&self.context.type_env, node)
    }

    pub fn known_class_name(&self, name: &TypeName) -> bool {
        self.checker
            .environment()
            .class_declarations
            .contains_key(name)
    }

    pub fn defined_names(&self) -> HashSet<String> {
        HashSet::default()
    }
}
