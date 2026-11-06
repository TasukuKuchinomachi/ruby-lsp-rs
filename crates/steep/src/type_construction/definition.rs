use std::rc::Rc;
use std::sync::Arc;

use rbs_definition::{
    ast::{ClassOrModule, Member, Namespace, TypeName},
    collections::IndexMap,
    definition_builder::Definition,
};
use steep_interface::{
    method_type::{
        Block, Function, KeywordParams, MethodType, Params, PositionalKind, PositionalParam,
        TypeParam,
    },
    substitution::Substitution,
    types::{Type, fresh_variable_name},
};

use crate::{
    annotation::Collection,
    diagnostic::ruby::{self as diagnostics, Diagnostic},
    method_name::MethodName,
    ruby::node::{Child, Node},
    ruby_exception,
    type_construction::{
        Pair, TypeConstruction, instance_type, is_special_lvar_name,
        missing_signature::UntypedParametersSource, or_raise, symbol_type,
    },
    type_inference::{
        constant_env::{ConstantEnv, Nesting, ResolverContext},
        context::{
            CallContext, Context, ForwardArgType, ImplementName, MethodContext, ModuleContext,
            TypeVariableContext,
        },
        type_env::TypeEnv,
    },
    typing::CursorRange,
};

pub fn namespace_from_node(node: Option<&Rc<Node>>) -> Option<Namespace> {
    match node {
        None => Some(Namespace {
            absolute: false,
            path: Vec::new(),
        }),
        Some(node) => match node.type_ {
            "cbase" => Some(Namespace::root()),
            "const" => {
                let parent = namespace_from_node(node.child_node(0))?;
                Some(parent.append(node.child_symbol(1)?))
            }
            _ => None,
        },
    }
}

pub fn module_name_from_node(parent: Option<&Rc<Node>>, name: &str) -> Option<TypeName> {
    namespace_from_node(parent).map(|namespace| TypeName::new(namespace, name))
}

pub fn rename_type_params(params: &[TypeParam]) -> (Vec<TypeParam>, Substitution) {
    if params.is_empty() {
        return (Vec::new(), Substitution::empty());
    }
    let names: Vec<String> = params.iter().map(|param| param.name.clone()).collect();
    let new_names: Vec<String> = names.iter().map(|name| fresh_variable_name(name)).collect();
    let substitution = Substitution::build(
        &names,
        &new_names.iter().cloned().map(Type::Var).collect::<Vec<_>>(),
        None,
        None,
        None,
    );
    let renamed = params
        .iter()
        .zip(&new_names)
        .map(|(param, new_name)| TypeParam {
            name: new_name.clone(),
            upper_bound: param
                .upper_bound
                .as_ref()
                .map(|bound| bound.substitute(&substitution)),
            variance: param.variance,
            unchecked: param.unchecked,
            default_type: param
                .default_type
                .as_ref()
                .map(|type_| type_.substitute(&substitution)),
        })
        .collect();
    (renamed, substitution)
}

#[derive(Clone, Debug)]
pub enum MethodParam {
    Positional {
        name: Option<String>,
        type_: Option<Type>,
        node: Rc<Node>,
    },
    Keyword {
        name: Option<String>,
        type_: Option<Type>,
        node: Rc<Node>,
    },
    PositionalRest {
        name: Option<String>,
        type_: Option<Type>,
        node: Rc<Node>,
    },
    KeywordRest {
        name: Option<String>,
        type_: Option<Type>,
        node: Rc<Node>,
    },
    Block {
        name: Option<String>,
        type_: Option<Box<Function>>,
        optional: bool,
        self_type: Option<Type>,
        node: Rc<Node>,
    },
}

impl MethodParam {
    pub fn name(&self) -> Option<&String> {
        match self {
            MethodParam::Positional { name, .. }
            | MethodParam::Keyword { name, .. }
            | MethodParam::PositionalRest { name, .. }
            | MethodParam::KeywordRest { name, .. }
            | MethodParam::Block { name, .. } => name.as_ref(),
        }
    }

    pub fn node(&self) -> &Rc<Node> {
        match self {
            MethodParam::Positional { node, .. }
            | MethodParam::Keyword { node, .. }
            | MethodParam::PositionalRest { node, .. }
            | MethodParam::KeywordRest { node, .. }
            | MethodParam::Block { node, .. } => node,
        }
    }

    pub fn var_type(&self) -> Type {
        match self {
            MethodParam::Positional { type_, .. } | MethodParam::Keyword { type_, .. } => {
                type_.clone().unwrap_or(Type::Any)
            }
            MethodParam::PositionalRest { type_, .. } => {
                instance_type("::Array", vec![type_.clone().unwrap_or(Type::Any)])
            }
            MethodParam::KeywordRest { type_, .. } => instance_type(
                "::Hash",
                vec![symbol_type(), type_.clone().unwrap_or(Type::Any)],
            ),
            MethodParam::Block {
                type_,
                optional,
                self_type,
                ..
            } => match type_ {
                Some(function) => {
                    let proc_type = Type::Proc {
                        function: function.clone(),
                        block: None,
                        self_type: self_type.clone().map(Box::new),
                    };
                    if *optional {
                        Type::union(vec![proc_type, Type::Nil])
                    } else {
                        proc_type
                    }
                }
                None => Type::Nil,
            },
        }
    }
}

pub struct MethodParams {
    pub params: IndexMap<Option<String>, MethodParam>,
    pub errors: Vec<Diagnostic>,
    pub forward_arg_type: Option<ForwardArgType>,
}

impl MethodParams {
    // Why not keep the parameters before `...`: MethodParams#update of Steep builds new
    // parameters from the forward argument type only, so they and their errors are lost.
    fn with_forward_arg_type(forward_arg_type: ForwardArgType) -> MethodParams {
        MethodParams {
            params: IndexMap::default(),
            errors: Vec::new(),
            forward_arg_type: Some(forward_arg_type),
        }
    }
}

pub(super) fn arguments_node(node: &Node) -> Option<&Rc<Node>> {
    match node.type_ {
        "def" => node.child_node(1),
        "defs" => node.child_node(2),
        _ => None,
    }
}

fn argument_name(argument: &Node) -> Option<String> {
    argument.child_symbol(0).map(str::to_string)
}

impl MethodParams {
    pub fn empty(node: &Node) -> MethodParams {
        let mut params = IndexMap::default();
        if let Some(arguments) = arguments_node(node) {
            for argument in arguments.child_nodes() {
                let name = argument_name(argument);
                let node = argument.clone();
                let param = match argument.type_ {
                    "arg" | "optarg" => MethodParam::Positional {
                        name: name.clone(),
                        type_: None,
                        node,
                    },
                    "kwarg" | "kwoptarg" => MethodParam::Keyword {
                        name: name.clone(),
                        type_: None,
                        node,
                    },
                    "restarg" => MethodParam::PositionalRest {
                        name: name.clone(),
                        type_: None,
                        node,
                    },
                    "kwrestarg" => MethodParam::KeywordRest {
                        name: name.clone(),
                        type_: None,
                        node,
                    },
                    "blockarg" => MethodParam::Block {
                        name: name.clone(),
                        type_: None,
                        optional: false,
                        self_type: None,
                        node,
                    },
                    _ => continue,
                };
                params.insert(name, param);
            }
        }
        MethodParams {
            params,
            errors: Vec::new(),
            forward_arg_type: None,
        }
    }

    pub fn build(node: &Node, method_type: &MethodType) -> MethodParams {
        let mut instance = MethodParams {
            params: IndexMap::default(),
            errors: Vec::new(),
            forward_arg_type: None,
        };
        let Some(arguments) = arguments_node(node) else {
            return instance;
        };
        let mut arguments: Vec<Rc<Node>> = arguments.child_nodes().cloned().collect();
        let Some(params) = &method_type.type_.params else {
            for argument in &arguments {
                let name = argument_name(argument);
                match argument.type_ {
                    "arg" | "optarg" => {
                        instance.params.insert(
                            name.clone(),
                            MethodParam::Positional {
                                name,
                                type_: Some(Type::Any),
                                node: argument.clone(),
                            },
                        );
                    }
                    "forward_arg" => {
                        return MethodParams::with_forward_arg_type(ForwardArgType::Untyped);
                    }
                    _ => {}
                }
            }
            return instance;
        };
        let positionals = &params.positionals;
        let mut index = 0;
        let mismatch = |instance: &mut MethodParams, param: MethodParam| {
            instance.errors.push(diagnostics::method_parameter_mismatch(
                param.node(),
                method_type,
            ));
            instance.params.insert(param.name().cloned(), param);
        };
        let different_kind = |instance: &mut MethodParams, param: MethodParam| {
            instance
                .errors
                .push(diagnostics::different_method_parameter_kind(
                    param.node(),
                    method_type,
                ));
            instance.params.insert(param.name().cloned(), param);
        };
        while let Some(argument) = arguments.first().cloned() {
            let name = argument_name(&argument);
            let head = positionals.get(index);
            match argument.type_ {
                "arg" => {
                    let param = MethodParam::Positional {
                        name: name.clone(),
                        type_: head.map(|param| param.type_.clone()),
                        node: argument.clone(),
                    };
                    match head.map(|param| param.kind) {
                        Some(PositionalKind::Required) => {
                            instance.params.insert(name, param);
                        }
                        _ => mismatch(&mut instance, param),
                    }
                }
                "optarg" => {
                    let param = MethodParam::Positional {
                        name: name.clone(),
                        type_: head.map(|param| param.type_.clone()),
                        node: argument.clone(),
                    };
                    match head.map(|param| param.kind) {
                        Some(PositionalKind::Optional) => {
                            instance.params.insert(name, param);
                        }
                        _ => different_kind(&mut instance, param),
                    }
                }
                _ => break,
            }
            if head.is_some_and(|param| param.kind != PositionalKind::Rest) {
                index += 1;
            } else if head.is_some() {
                index = positionals.len();
            }
            arguments.remove(0);
        }
        if let Some(argument) = arguments.first()
            && argument.type_ == "forward_arg"
        {
            let mut forward_params = params.clone();
            forward_params.positionals = positionals[index.min(positionals.len())..].to_vec();
            return MethodParams::with_forward_arg_type(ForwardArgType::Typed(Box::new((
                forward_params,
                method_type.block.clone(),
            ))));
        }
        let mut remaining = index < positionals.len();
        if let Some(argument) = arguments.first().cloned()
            && argument.type_ == "restarg"
        {
            let name = argument_name(&argument);
            let mut rest_types = Vec::new();
            let mut has_error = false;
            loop {
                match positionals.get(index) {
                    Some(param) if param.kind == PositionalKind::Rest => {
                        rest_types.push(param.type_.clone());
                        remaining = false;
                        arguments.remove(0);
                        break;
                    }
                    Some(param) => {
                        rest_types.push(param.type_.clone());
                        has_error = true;
                        index += 1;
                    }
                    None => {
                        has_error = true;
                        remaining = false;
                        break;
                    }
                }
            }
            let type_ = (!rest_types.is_empty()).then(|| Type::union(rest_types));
            let param = MethodParam::PositionalRest {
                name: name.clone(),
                type_,
                node: argument.clone(),
            };
            if has_error {
                instance
                    .errors
                    .push(diagnostics::different_method_parameter_kind(
                        &argument,
                        method_type,
                    ));
            }
            instance.params.insert(name, param);
        }
        if remaining {
            instance
                .errors
                .push(diagnostics::method_arity_mismatch(node, method_type));
        }
        let keyword_params = &params.keywords;
        let lookup = |list: &[(String, Type)], key: &str| {
            list.iter()
                .find(|(name, _)| name == key)
                .map(|(_, type_)| type_.clone())
        };
        let mut keywords: Vec<String> = keyword_params
            .requireds
            .iter()
            .chain(keyword_params.optionals.iter())
            .map(|(name, _)| name.clone())
            .collect();
        while let Some(argument) = arguments.first().cloned() {
            let name = argument_name(&argument);
            let key = name.clone().unwrap_or_default();
            match argument.type_ {
                "kwarg" => {
                    if let Some(type_) = lookup(&keyword_params.requireds, &key) {
                        instance.params.insert(
                            name.clone(),
                            MethodParam::Keyword {
                                name,
                                type_: Some(type_),
                                node: argument.clone(),
                            },
                        );
                        keywords.retain(|keyword| *keyword != key);
                    } else if let Some(type_) = lookup(&keyword_params.optionals, &key)
                        .or_else(|| keyword_params.rest.clone())
                    {
                        mismatch(
                            &mut instance,
                            MethodParam::Keyword {
                                name,
                                type_: Some(type_),
                                node: argument.clone(),
                            },
                        );
                        keywords.retain(|keyword| *keyword != key);
                    } else {
                        mismatch(
                            &mut instance,
                            MethodParam::Keyword {
                                name,
                                type_: None,
                                node: argument.clone(),
                            },
                        );
                    }
                }
                "kwoptarg" => {
                    if let Some(type_) = lookup(&keyword_params.requireds, &key) {
                        different_kind(
                            &mut instance,
                            MethodParam::Keyword {
                                name,
                                type_: Some(type_),
                                node: argument.clone(),
                            },
                        );
                        keywords.retain(|keyword| *keyword != key);
                    } else if let Some(type_) = lookup(&keyword_params.optionals, &key) {
                        instance.params.insert(
                            name.clone(),
                            MethodParam::Keyword {
                                name,
                                type_: Some(type_),
                                node: argument.clone(),
                            },
                        );
                        keywords.retain(|keyword| *keyword != key);
                    } else if let Some(type_) = keyword_params.rest.clone() {
                        different_kind(
                            &mut instance,
                            MethodParam::Keyword {
                                name,
                                type_: Some(type_),
                                node: argument.clone(),
                            },
                        );
                        keywords.retain(|keyword| *keyword != key);
                    } else {
                        different_kind(
                            &mut instance,
                            MethodParam::Keyword {
                                name,
                                type_: None,
                                node: argument.clone(),
                            },
                        );
                    }
                }
                _ => break,
            }
            arguments.remove(0);
        }
        if let Some(argument) = arguments.first().cloned()
            && argument.type_ == "kwrestarg"
        {
            let name = argument_name(&argument);
            let mut rest_types = Vec::new();
            let mut has_error = false;
            for keyword in keywords.drain(..) {
                if let Some(type_) = lookup(&keyword_params.requireds, &keyword)
                    .or_else(|| lookup(&keyword_params.optionals, &keyword))
                {
                    rest_types.push(type_);
                }
                has_error = true;
            }
            match &keyword_params.rest {
                Some(rest) => rest_types.push(rest.clone()),
                None => has_error = true,
            }
            let type_ = (!rest_types.is_empty()).then(|| Type::union(rest_types));
            if has_error {
                instance
                    .errors
                    .push(diagnostics::different_method_parameter_kind(
                        &argument,
                        method_type,
                    ));
            }
            instance.params.insert(
                name.clone(),
                MethodParam::KeywordRest {
                    name,
                    type_,
                    node: argument.clone(),
                },
            );
            arguments.remove(0);
        } else if !keywords.is_empty() || keyword_params.rest.is_some() {
            instance
                .errors
                .push(diagnostics::method_arity_mismatch(node, method_type));
        }
        if let Some(argument) = arguments.first().cloned()
            && argument.type_ == "blockarg"
        {
            let name = argument_name(&argument);
            let param = match &method_type.block {
                Some(block) => MethodParam::Block {
                    name: name.clone(),
                    type_: Some(Box::new(block.type_.clone())),
                    optional: block.optional,
                    self_type: block.self_type.clone(),
                    node: argument.clone(),
                },
                None => MethodParam::Block {
                    name: name.clone(),
                    type_: None,
                    optional: false,
                    self_type: None,
                    node: argument.clone(),
                },
            };
            instance.params.insert(name, param);
        }
        instance
    }
}

pub const ANONYMOUS_BLOCK_PASSABLE_LVAR: &str = "*block";

fn optional_union(types: Vec<Type>, null: bool) -> Type {
    let mut types = types;
    if null {
        types.push(Type::Nil);
    }
    Type::union(types)
}

fn positional(
    kind: PositionalKind,
    type_: Type,
    rest: Vec<PositionalParam>,
) -> Vec<PositionalParam> {
    let mut params = vec![PositionalParam { kind, type_ }];
    params.extend(rest);
    params
}

fn merge_positionals_for_overload(
    left: &[PositionalParam],
    right: &[PositionalParam],
) -> Vec<PositionalParam> {
    use PositionalKind::{Optional, Required, Rest};
    let left_head = left.first();
    let right_head = right.first();
    let left_tail = left.get(1..).unwrap_or(&[]);
    let right_tail = right.get(1..).unwrap_or(&[]);
    match (left_head, right_head) {
        (Some(x), Some(y)) => match (x.kind, y.kind) {
            (Required, Required) => positional(
                Required,
                optional_union(vec![x.type_.clone(), y.type_.clone()], false),
                merge_positionals_for_overload(left_tail, right_tail),
            ),
            (Required, Optional) | (Optional, Required) => positional(
                Optional,
                optional_union(vec![x.type_.clone(), y.type_.clone()], true),
                merge_positionals_for_overload(left_tail, right_tail),
            ),
            (Required, Rest) => positional(
                Optional,
                optional_union(vec![x.type_.clone(), y.type_.clone()], true),
                merge_positionals_for_overload(left_tail, right),
            ),
            (Optional, Optional) => positional(
                Optional,
                optional_union(vec![x.type_.clone(), y.type_.clone()], false),
                merge_positionals_for_overload(left_tail, right_tail),
            ),
            (Optional, Rest) => positional(
                Optional,
                optional_union(vec![x.type_.clone(), y.type_.clone()], false),
                merge_positionals_for_overload(left_tail, right),
            ),
            (Rest, Required) => positional(
                Optional,
                optional_union(vec![x.type_.clone(), y.type_.clone()], true),
                merge_positionals_for_overload(left, right_tail),
            ),
            (Rest, Optional) => positional(
                Optional,
                optional_union(vec![x.type_.clone(), y.type_.clone()], false),
                merge_positionals_for_overload(left, right_tail),
            ),
            (Rest, Rest) => vec![PositionalParam {
                kind: Rest,
                type_: optional_union(vec![x.type_.clone(), y.type_.clone()], false),
            }],
        },
        (Some(x), None) => match x.kind {
            Required => positional(
                Optional,
                optional_union(vec![x.type_.clone()], true),
                merge_positionals_for_overload(left_tail, &[]),
            ),
            Optional => positional(
                Optional,
                x.type_.clone(),
                merge_positionals_for_overload(left_tail, &[]),
            ),
            Rest => left.to_vec(),
        },
        (None, Some(y)) => match y.kind {
            Required => positional(
                Optional,
                optional_union(vec![y.type_.clone()], true),
                merge_positionals_for_overload(&[], right_tail),
            ),
            Optional => positional(
                Optional,
                y.type_.clone(),
                merge_positionals_for_overload(&[], right_tail),
            ),
            Rest => right.to_vec(),
        },
        (None, None) => Vec::new(),
    }
}

fn lookup_keyword(list: &[(String, Type)], key: &str) -> Option<Type> {
    list.iter()
        .find(|(name, _)| name == key)
        .map(|(_, type_)| type_.clone())
}

fn merge_keywords_for_overload(left: &KeywordParams, right: &KeywordParams) -> KeywordParams {
    let mut all_keys: Vec<String> = Vec::new();
    for (key, _) in left
        .requireds
        .iter()
        .chain(&left.optionals)
        .chain(&right.requireds)
        .chain(&right.optionals)
    {
        if !all_keys.contains(key) {
            all_keys.push(key.clone());
        }
    }
    let mut requireds = Vec::new();
    let mut optionals = Vec::new();
    for key in all_keys {
        let left_required = lookup_keyword(&left.requireds, &key);
        let left_optional = lookup_keyword(&left.optionals, &key);
        let right_required = lookup_keyword(&right.requireds, &key);
        let right_optional = lookup_keyword(&right.optionals, &key);
        if let Some(t) = left_required {
            if let Some(s) = right_required {
                requireds.push((key, optional_union(vec![t, s], false)));
            } else if let Some(s) = right_optional.or_else(|| right.rest.clone()) {
                optionals.push((key, optional_union(vec![t, s], true)));
            } else {
                optionals.push((key, optional_union(vec![t], true)));
            }
        } else if let Some(t) = left_optional {
            if let Some(s) = right_required {
                optionals.push((key, optional_union(vec![t, s], true)));
            } else if let Some(s) = right_optional.or_else(|| right.rest.clone()) {
                optionals.push((key, optional_union(vec![t, s], false)));
            } else {
                optionals.push((key, t));
            }
        } else if let Some(t) = left.rest.clone() {
            if let Some(s) = right_required {
                optionals.push((key, optional_union(vec![t, s], true)));
            } else if let Some(s) = right_optional {
                optionals.push((key, optional_union(vec![t, s], false)));
            }
        } else if let Some(s) = right_required {
            optionals.push((key, optional_union(vec![s], true)));
        } else if let Some(s) = right_optional {
            optionals.push((key, s));
        }
    }
    let rest = match (&left.rest, &right.rest) {
        (Some(left), Some(right)) => Some(optional_union(vec![left.clone(), right.clone()], false)),
        (left, right) => left.clone().or(right.clone()),
    };
    KeywordParams {
        requireds,
        optionals,
        rest,
    }
}

fn merge_params_for_overload(left: &Params, right: &Params) -> Params {
    Params {
        positionals: merge_positionals_for_overload(&left.positionals, &right.positionals),
        keywords: merge_keywords_for_overload(&left.keywords, &right.keywords),
    }
}

fn unify_overloads(method_types: Vec<MethodType>) -> Option<MethodType> {
    let mut iterator = method_types.into_iter();
    let first = iterator.next()?;
    let mut result = first;
    for other in iterator {
        let (type_params_1, substitution_1) = rename_type_params(&result.type_params);
        let (type_params_2, substitution_2) = rename_type_params(&other.type_params);
        let mut type_params = type_params_1;
        type_params.extend(type_params_2);
        let block = match (&result.block, &other.block) {
            (Some(left), Some(right)) => {
                let left = left.substitute(&substitution_1);
                let right = right.substitute(&substitution_2);
                Some(Block {
                    type_: Function {
                        params: match (&left.type_.params, &right.type_.params) {
                            (Some(left), Some(right)) => {
                                Some(merge_params_for_overload(left, right))
                            }
                            _ => None,
                        },
                        return_type: Type::union(vec![
                            left.type_.return_type.clone(),
                            right.type_.return_type.clone(),
                        ]),
                    },
                    optional: left.optional || right.optional,
                    self_type: left.self_type.clone().or(right.self_type.clone()),
                })
            }
            (Some(left), None) => Some(Block {
                optional: true,
                ..left.substitute(&substitution_1)
            }),
            (None, Some(right)) => Some(Block {
                optional: true,
                ..right.substitute(&substitution_2)
            }),
            (None, None) => None,
        };
        let left = result.type_.substitute(&substitution_1);
        let right = other.type_.substitute(&substitution_2);
        result = MethodType {
            type_params,
            type_: Function {
                params: match (&left.params, &right.params) {
                    (Some(left), Some(right)) => Some(merge_params_for_overload(left, right)),
                    _ => None,
                },
                return_type: Type::union(vec![left.return_type, right.return_type]),
            },
            block,
        };
    }
    Some(result)
}

impl<'checker> TypeConstruction<'checker> {
    pub fn annotations_for(&self, node: Option<&Node>, nesting: &ResolverContext) -> Collection {
        self.checker.annotations(self.source, node, nesting)
    }

    pub(super) fn type_env_with_local_annotations(
        &self,
        env: &TypeEnv,
        annotations: &Collection,
    ) -> TypeEnv {
        let mut env = env.clone();
        for (name, type_) in &annotations.var_types {
            env.local_variable_types
                .insert(name.clone(), (type_.clone(), Some(type_.clone())));
        }
        env
    }

    pub(super) fn import_globals(&self, env: &mut TypeEnv) {
        let mut names: Vec<&String> = self
            .checker
            .environment()
            .global_declarations
            .keys()
            .collect();
        names.sort();
        for name in names {
            let declaration = &self.checker.environment().global_declarations[name];
            Rc::make_mut(&mut env.global_types).insert(
                name.clone(),
                self.builder().factory.type_(&declaration.type_),
            );
        }
    }

    pub(super) fn import_instance_variables(
        &self,
        env: &mut TypeEnv,
        definition: Option<&Arc<Definition>>,
    ) {
        if let Some(definition) = definition {
            env.instance_variable_types = Rc::new(
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

    fn for_new_method(
        &self,
        method_name: &str,
        node: &Rc<Node>,
        self_type: Option<Type>,
        definition: Option<Arc<Definition>>,
    ) -> TypeConstruction<'checker> {
        let annotations = self.annotations_for(Some(node), &self.nesting());
        let definition_method = definition
            .as_ref()
            .and_then(|definition| definition.methods.get(method_name).cloned());
        let definition_method_type = definition_method.as_ref().and_then(|method| {
            unify_overloads(
                method
                    .method_types()
                    .map(|method_type| self.builder().factory.method_type(method_type))
                    .collect(),
            )
        });
        let annotation_method_type = self.annotations.method_types.get(method_name).cloned();
        let untyped_parameters_source = match (&annotation_method_type, &definition_method_type) {
            (Some(method_type), _) if method_type.type_.params.is_none() => {
                Some(UntypedParametersSource::Annotation)
            }
            (None, Some(method_type)) if method_type.type_.params.is_none() => definition_method
                .as_ref()
                .map(|method| match method.method_types().count() {
                    1 => UntypedParametersSource::MethodType,
                    _ => UntypedParametersSource::Overload,
                }),
            _ => None,
        };
        let method_type = annotation_method_type.or(definition_method_type);
        if method_type.is_none() {
            match &definition {
                Some(definition) => self.add_error(diagnostics::undeclared_method_definition(
                    node,
                    method_name,
                    &definition.type_name.to_string(),
                )),
                None => self.add_error(diagnostics::method_definition_in_undeclared_module(
                    node,
                    method_name,
                )),
            }
        }
        if let (Some(annotation_return_type), Some(method_type)) =
            (&annotations.return_type, &method_type)
        {
            let result =
                self.check_relation(annotation_return_type, &method_type.type_.return_type);
            if result.is_failure() {
                self.add_error(diagnostics::method_return_type_annotation_mismatch(
                    node,
                    &method_type.type_.return_type,
                    annotation_return_type,
                    &result,
                ));
            }
        }
        let super_method = definition_method.as_ref().and_then(|method| {
            if self.module_context().class_name == *method.defined_in() {
                method.super_method.clone()
            } else {
                Some(method.clone())
            }
        });
        let variable_context = match (&definition, &method_type) {
            (Some(_), Some(method_type)) => Rc::new(TypeVariableContext::new(
                &method_type.type_params,
                Some(&self.context.variable_context),
            )),
            _ => self.context.variable_context.clone(),
        };
        if let Some(source) = untyped_parameters_source {
            self.observe_untyped_method_parameters(node, method_name, source, &annotations);
        }
        let method_params = match &method_type {
            Some(method_type) => MethodParams::build(node, method_type),
            None => MethodParams::empty(node),
        };
        let mut block_param_name = None;
        for param in method_params.params.values() {
            if let MethodParam::Block { name, .. } = param {
                block_param_name = Some(
                    name.clone()
                        .unwrap_or_else(|| ANONYMOUS_BLOCK_PASSABLE_LVAR.to_string()),
                );
            }
        }
        let method_context = MethodContext {
            name: Some(method_name.to_string()),
            method: definition_method.clone(),
            method_type: method_type.clone(),
            return_type: annotations
                .return_type
                .clone()
                .or_else(|| {
                    method_type
                        .as_ref()
                        .map(|method_type| method_type.type_.return_type.clone())
                })
                .unwrap_or(Type::Any),
            super_method,
            forward_arg_type: method_params.forward_arg_type.clone(),
            block_param_name,
        };
        let mut local_variable_types = Vec::new();
        for param in method_params.params.values() {
            match param.name() {
                Some(name) => {
                    if !is_special_lvar_name(name) {
                        local_variable_types.push((name.clone(), param.var_type()));
                    }
                }
                None => {
                    if let MethodParam::Block { .. } = param {
                        local_variable_types
                            .push((ANONYMOUS_BLOCK_PASSABLE_LVAR.to_string(), param.var_type()));
                    }
                }
            }
        }
        let mut type_env = self
            .context
            .type_env
            .assign_local_variables(&local_variable_types);
        for (name, annotated) in &annotations.var_types {
            if let Some(original) = type_env.get(name)
                && let Some(param) = method_params.params.get(&Some(name.clone()))
                && let Some(result) = self.no_subtyping(&original, annotated)
            {
                self.add_error(diagnostics::incompatible_annotation(
                    param.node(),
                    name,
                    &format!("{original} <: {annotated}"),
                    &result,
                ));
            }
            type_env
                .local_variable_types
                .insert(name.clone(), (annotated.clone(), Some(annotated.clone())));
        }
        self.import_instance_variables(&mut type_env, definition.as_ref());
        for (name, type_) in &annotations.ivar_types {
            Rc::make_mut(&mut type_env.instance_variable_types).insert(name.clone(), type_.clone());
        }
        for error in method_params.errors {
            self.add_error(error);
        }
        let call_context = match &self_type {
            None => CallContext::Unknown,
            Some(Type::Singleton(_)) => CallContext::Method {
                type_name: self.module_context().class_name.clone(),
                singleton: true,
                method_name: method_name.to_string(),
            },
            Some(_) => CallContext::Method {
                type_name: self.module_context().class_name.clone(),
                singleton: false,
                method_name: method_name.to_string(),
            },
        };
        let self_type = annotations
            .self_type
            .clone()
            .or(self_type)
            .unwrap_or_else(|| self.self_type().clone());
        TypeConstruction {
            checker: self.checker,
            source: self.source,
            inference: self.inference,
            annotations: Rc::new(annotations),
            typing: self.typing.clone(),
            context: Context {
                identity: Default::default(),
                method_context: Some(Rc::new(method_context)),
                block_context: None,
                break_context: None,
                module_context: self.context.module_context.clone(),
                self_type,
                type_env,
                call_context,
                variable_context,
            },
        }
    }

    fn method_body_error(
        &self,
        node: &Rc<Node>,
        method_context: &MethodContext,
        expected: &Type,
        actual: &Type,
        result: &steep_interface::subtyping::result::SubtypingResult,
    ) -> Diagnostic {
        if method_context.is_attribute_setter() {
            diagnostics::setter_body_type_mismatch(
                node,
                expected,
                actual,
                result,
                method_context.name.as_deref().unwrap_or_default(),
            )
        } else {
            diagnostics::method_body_type_mismatch(node, expected, actual, result)
        }
    }

    pub fn synthesize_def(&self, node: &Rc<Node>) -> Pair<'checker> {
        let name = node.child_symbol(0).unwrap_or_default().to_string();
        let arguments = node.child_node(1).cloned();
        let body = node.child_node(2).cloned();
        let new = self.for_new_method(
            &name,
            node,
            Some(self.module_context().instance_type.clone()),
            self.module_context().instance_definition.clone(),
        );
        new.set_node_and_body_context(node);
        if let Some(method) = new
            .context
            .method_context
            .as_ref()
            .and_then(|method_context| method_context.method.as_ref())
        {
            let owner = method
                .implemented_in()
                .unwrap_or_else(|| method.defined_in())
                .clone();
            new.typing.update_source_index(|index| {
                index.add_method_definition(MethodName::instance(owner, name.clone()), node)
            });
        }
        self.module_context()
            .defined_instance_methods
            .borrow_mut()
            .push(name.clone());
        let new = match &arguments {
            Some(arguments) => new.synthesize_children(arguments, &[]),
            None => new,
        };
        let method_context = new
            .context
            .method_context
            .clone()
            .expect("a method context");
        let return_type = self.checker.expand_alias(&method_context.return_type);
        let body_pair = match &body {
            Some(body) => Some(if !matches!(return_type, Type::Void) {
                let typing = self.typing.clone();
                let this = self.clone();
                let expected_type = method_context.return_type.clone();
                let method_context_for_error = method_context.clone();
                new.with_variable_bounds(|new| {
                    new.check(
                        body,
                        &return_type,
                        steep_interface::subtyping::constraints::Constraints::empty().shared(),
                        &mut |_, actual, result| {
                            typing.add_error(this.method_body_error(
                                node,
                                &method_context_for_error,
                                &expected_type,
                                actual,
                                result,
                            ));
                        },
                    )
                })
            } else {
                new.with_variable_bounds(|new| new.synthesize(body, None, false))
            }),
            None => {
                if !matches!(return_type, Type::Void) {
                    let result = self.check_relation(&Type::Nil, &return_type);
                    if result.is_failure() {
                        self.add_error(self.method_body_error(
                            node,
                            &method_context,
                            &method_context.return_type,
                            &Type::Nil,
                            &result,
                        ));
                    }
                }
                None
            }
        };
        if let (Some(body), Some(body_pair), Some(end)) = (&body, &body_pair, node.location.end) {
            self.typing.update_cursor_context(|cursor_context| {
                cursor_context.set(
                    CursorRange::new(body.expression().end_pos, end.begin_pos),
                    body_pair.context(),
                )
            });
        }
        self.add_typing(node, symbol_type())
    }

    pub fn with_variable_bounds<Output>(
        &self,
        body: impl FnOnce(&TypeConstruction<'checker>) -> Output,
    ) -> Output {
        let check = self.builder().subtyping();
        let bounds = self.context.variable_context.upper_bounds_list();
        check.push_variable_bounds(bounds, || body(self))
    }

    pub fn synthesize_defs(&self, node: &Rc<Node>) -> Pair<'checker> {
        let receiver = node.child_node(0).cloned().expect("defs has a receiver");
        let receiver_type = self.synthesize(&receiver, None, false).type_;
        let self_type = self.expand_self(&receiver_type);
        let definition = match &self_type {
            Type::NameInstance { name, .. } => Some(or_raise(
                self.builder().definition_builder.build_instance(name),
            )),
            Type::Singleton(name) => Some(or_raise(
                self.builder().definition_builder.build_singleton(name),
            )),
            _ => None,
        };
        let name = node.child_symbol(1).unwrap_or_default().to_string();
        if receiver.type_ == "self" {
            self.module_context()
                .defined_module_methods
                .borrow_mut()
                .push(name.clone());
        }
        let new = self.for_new_method(&name, node, Some(self_type.clone()), definition);
        new.set_node_and_body_context(node);
        if let Some(method) = new
            .context
            .method_context
            .as_ref()
            .and_then(|method_context| method_context.method.as_ref())
        {
            let owner = method.implemented_in().cloned().unwrap_or_else(|| {
                ruby_exception::raise_runtime_error("a method of defs has implemented_in")
            });
            let method_name = match &self_type {
                Type::NameInstance { .. } => Some(MethodName::instance(owner, name.clone())),
                Type::Singleton(_) => Some(MethodName::singleton(owner, name.clone())),
                _ => None,
            };
            if let Some(method_name) = method_name {
                new.typing
                    .update_source_index(|index| index.add_method_definition(method_name, node));
            }
        }
        let arguments = node.child_node(2).cloned();
        let mut new = match &arguments {
            Some(arguments) => new.synthesize_children(arguments, &[]),
            None => new,
        };
        if let Some(arguments) = &arguments {
            for argument in arguments.child_nodes() {
                new = new.synthesize(argument, None, false).constr;
            }
        }
        if let Some(body) = node.child_node(3).cloned() {
            let method_context = new
                .context
                .method_context
                .clone()
                .expect("a method context");
            let return_type = self.checker.expand_alias(&method_context.return_type);
            if !matches!(return_type, Type::Void) {
                let typing = self.typing.clone();
                new.check(
                    &body,
                    &return_type,
                    steep_interface::subtyping::constraints::Constraints::empty().shared(),
                    &mut |expected, actual, result| {
                        typing.add_error(diagnostics::method_body_type_mismatch(
                            node, expected, actual, result,
                        ));
                    },
                );
            } else {
                new.synthesize(&body, None, false);
            }
        }
        self.add_typing(node, symbol_type())
    }

    pub fn synthesize_constant(
        &self,
        node: Option<&Rc<Node>>,
        parent: Option<&Rc<Node>>,
        constant_name: &str,
        on_unknown: Option<&dyn Fn()>,
    ) -> (Type, TypeConstruction<'checker>, Option<TypeName>) {
        if let Some(name) = module_name_from_node(parent, constant_name)
            && let Some(type_) = self.context.type_env.constant_types.get(&name).cloned()
        {
            return match node {
                Some(node) => {
                    let constr = self.synthesize_children(node, &[]);
                    let pair = constr.add_typing(node, type_);
                    (pair.type_, pair.constr, None)
                }
                None => (type_, self.clone(), None),
            };
        }
        let mut constr = self.clone();
        let builder = self.builder();
        let constant_env = self.context.type_env.constant_env.clone();
        match parent {
            None => {
                if let Some(constant) = constant_env.resolve(builder, constant_name) {
                    if let Some(node) = node {
                        constr = constr.add_typing(node, constant.type_.clone()).constr;
                    }
                    return (constant.type_, constr, Some(constant.name));
                }
            }
            Some(parent) if parent.type_ == "cbase" => {
                constr = constr.add_typing(parent, Type::Nil).constr;
                if let Some(constant) = constant_env.toplevel(constant_name) {
                    if let Some(node) = node {
                        constr = constr.add_typing(node, constant.type_.clone()).constr;
                    }
                    return (constant.type_, constr, Some(constant.name));
                }
            }
            Some(parent) => {
                let pair = self.synthesize(parent, None, false);
                constr = pair.constr;
                let parent_type = self.expand_self(&pair.type_);
                let parent_type = self
                    .checker
                    .deep_expand_alias(&parent_type)
                    .unwrap_or(parent_type);
                match &parent_type {
                    Type::Singleton(name) => {
                        if let Some(constant) =
                            constant_env.resolve_child(builder, name, constant_name)
                        {
                            if let Some(node) = node {
                                constr = constr.add_typing(node, constant.type_.clone()).constr;
                            }
                            return (constant.type_, constr, Some(constant.name));
                        }
                    }
                    Type::Any => {
                        if let Some(node) = node {
                            constr = constr.add_typing(node, Type::Any).constr;
                        }
                        return (Type::Any, constr, None);
                    }
                    _ => {}
                }
            }
        }
        match on_unknown {
            Some(on_unknown) => on_unknown(),
            None => {
                if let Some(node) = node {
                    constr.add_error(diagnostics::unknown_constant(
                        node,
                        constant_name,
                        "constant",
                    ));
                }
            }
        }
        if let Some(node) = node {
            constr = constr.add_typing(node, Type::Any).constr;
        }
        (Type::Any, constr, None)
    }

    pub fn synthesize_constant_decl(
        &self,
        node: Option<&Rc<Node>>,
        parent: Option<&Rc<Node>>,
        constant_name: &str,
        on_unknown: Option<&dyn Fn()>,
    ) -> (Type, TypeConstruction<'checker>, Option<TypeName>) {
        if let Some(name) = module_name_from_node(parent, constant_name)
            && let Some(type_) = self.context.type_env.constant_types.get(&name).cloned()
        {
            return match node {
                Some(node) => {
                    let constr = self.synthesize_children(node, &[]);
                    let pair = constr.add_typing(node, type_);
                    (pair.type_, pair.constr, None)
                }
                None => (type_, self.clone(), None),
            };
        }
        if parent.is_some() {
            return self.synthesize_constant(node, parent, constant_name, on_unknown);
        }
        let nesting = self.nesting();
        let Some(nesting) = nesting else {
            return self.synthesize_constant(node, None, constant_name, on_unknown);
        };
        if let Some(parent_name) = &nesting.name {
            let children = self.checker.constant_resolver.children_table(parent_name);
            if let Some(constant) = children.and_then(|table| table.get(constant_name).cloned()) {
                return (constant.type_, self.clone(), Some(constant.name));
            }
        }
        match on_unknown {
            Some(on_unknown) => on_unknown(),
            None => {
                if let Some(node) = node {
                    self.add_error(diagnostics::unknown_constant(
                        node,
                        constant_name,
                        "constant",
                    ));
                }
            }
        }
        let constr = match node {
            Some(node) => self.add_typing(node, Type::Any).constr,
            None => self.clone(),
        };
        (Type::Any, constr, None)
    }

    pub fn synthesize_constant_assignment(&self, node: &Rc<Node>) -> Pair<'checker> {
        let parent = node.child_node(0).cloned();
        let name = node.child_symbol(1).unwrap_or_default().to_string();
        let typing = self.typing.clone();
        let unknown = || typing.add_error(diagnostics::unknown_constant(node, &name, "constant"));
        let (constant_type, constr, constant_name) =
            self.synthesize_constant_decl(None, parent.as_ref(), &name, Some(&unknown));
        if let Some(constant_name) = &constant_name {
            self.typing.update_source_index(|index| {
                index.add_constant_definition(constant_name.clone(), node)
            });
            constr.check_deprecation_constant(constant_name, node, node.location.name);
        }
        let Some(value) = node.children.last().and_then(Child::node).cloned() else {
            return constr.add_typing(node, constant_type);
        };
        let pair = constr.synthesize(&value, Some(&constant_type), false);
        let result = self.check_relation(&pair.type_, &constant_type);
        if result.is_failure() {
            self.add_error(diagnostics::incompatible_assignment(
                node,
                &constant_type,
                &pair.type_,
                &result,
            ));
            pair.constr.add_typing(node, constant_type)
        } else {
            pair.constr.add_typing(node, pair.type_)
        }
    }

    fn implement_module(
        &self,
        module_name: Option<&TypeName>,
        super_name: Option<&TypeName>,
        annotations: &Collection,
        nesting: &ResolverContext,
    ) -> Option<ImplementName> {
        if let Some((name, args)) = &annotations.implement_module {
            let absolute = self.checker.absolute_type_name(name, nesting)?;
            return self.known_class_name(&absolute).then(|| ImplementName {
                name: absolute,
                args: args.clone(),
            });
        }
        let name = module_name.or(super_name)?;
        if !self.known_class_name(name) {
            return None;
        }
        let definition = or_raise(self.builder().definition_builder.build_instance(name));
        Some(ImplementName {
            name: name.clone(),
            args: definition
                .type_parameters
                .iter()
                .map(|parameter| parameter.name.clone())
                .collect(),
        })
    }

    pub(super) fn default_module_context(
        &self,
        implement_name: Option<&ImplementName>,
        nesting: ResolverContext,
    ) -> ModuleContext {
        match implement_name {
            Some(implement_name) => {
                let module_name = implement_name.name.clone();
                ModuleContext {
                    instance_type: Type::NameInstance {
                        name: module_name.clone(),
                        args: implement_name.args.iter().cloned().map(Type::Var).collect(),
                    },
                    module_type: Type::Singleton(module_name.clone()),
                    implement_name: Some(implement_name.clone()),
                    nesting,
                    class_name: module_name.clone(),
                    instance_definition: Some(or_raise(
                        self.builder()
                            .definition_builder
                            .build_instance(&module_name),
                    )),
                    module_definition: Some(or_raise(
                        self.builder()
                            .definition_builder
                            .build_singleton(&module_name),
                    )),
                    defined_instance_methods: Rc::default(),
                    defined_module_methods: Rc::default(),
                }
            }
            None => ModuleContext {
                instance_type: instance_type("::Object", Vec::new()),
                module_type: Type::Singleton(TypeName::parse("::Object")),
                implement_name: None,
                nesting,
                class_name: self.module_context().class_name.clone(),
                instance_definition: None,
                module_definition: None,
                defined_instance_methods: Rc::default(),
                defined_module_methods: Rc::default(),
            },
        }
    }

    fn type_params_of(&self, name: &TypeName) -> Vec<TypeParam> {
        let definition = or_raise(self.builder().definition_builder.build_instance(name));
        definition
            .type_parameters
            .iter()
            .map(|parameter| self.builder().factory.type_param(parameter))
            .collect()
    }

    fn body_constr(
        &self,
        annotations: Collection,
        module_context: ModuleContext,
        variable_context: TypeVariableContext,
        nesting: ResolverContext,
        instance_definition_for_env: Option<Arc<Definition>>,
        instance_annotations: bool,
    ) -> TypeConstruction<'checker> {
        let constant_env = Rc::new(ConstantEnv {
            context: nesting,
            resolver: self.checker.constant_resolver.clone(),
        });
        let mut type_env = TypeEnv::new(constant_env);
        self.import_globals(&mut type_env);
        type_env = self.type_env_with_local_annotations(&type_env, &annotations);
        self.import_instance_variables(&mut type_env, instance_definition_for_env.as_ref());
        if instance_annotations {
            for (name, type_) in &annotations.ivar_types {
                Rc::make_mut(&mut type_env.instance_variable_types)
                    .insert(name.clone(), type_.clone());
            }
        }
        for (name, type_) in &annotations.const_types {
            type_env.constant_types.insert(name.clone(), type_.clone());
        }
        let class_name = module_context.class_name.clone();
        let self_type = module_context.module_type.clone();
        TypeConstruction {
            checker: self.checker,
            source: self.source,
            inference: self.inference,
            annotations: Rc::new(annotations),
            typing: self.typing.clone(),
            context: Context {
                identity: Default::default(),
                method_context: None,
                block_context: None,
                break_context: None,
                module_context: Rc::new(module_context),
                self_type,
                type_env,
                call_context: CallContext::Module(class_name),
                variable_context: Rc::new(variable_context),
            },
        }
    }

    fn for_module(
        &self,
        node: &Rc<Node>,
        module_name: Option<&TypeName>,
    ) -> TypeConstruction<'checker> {
        let nesting: ResolverContext = Some(Rc::new(Nesting {
            parent: self.nesting(),
            name: module_name.cloned(),
        }));
        let annotations = self.annotations_for(Some(node), &nesting);
        let implement_name = self.implement_module(module_name, None, &annotations, &nesting);
        let mut module_context =
            self.default_module_context(implement_name.as_ref(), nesting.clone());
        if implement_name.is_none() {
            module_context.module_type = instance_type("::Module", Vec::new());
            module_context.instance_type = instance_type("::BasicObject", Vec::new());
        }
        if let Some(implement_name) = &implement_name {
            let environment = self.checker.environment();
            let normalized = environment.normalize_module_name(&implement_name.name);
            match environment.class_declarations.get(&normalized) {
                Some(entry) if entry.kind == ClassOrModule::Module => {
                    let mut types = vec![instance_type("::Object", Vec::new())];
                    for module_self in entry.self_types(environment.rbs_version) {
                        let args = module_self
                            .args
                            .iter()
                            .map(|arg| self.builder().factory.type_(arg))
                            .collect();
                        types.push(
                            if module_self.name.kind()
                                == rbs_definition::ast::TypeNameKind::Interface
                            {
                                Type::Interface {
                                    name: module_self.name,
                                    args,
                                }
                            } else {
                                Type::NameInstance {
                                    name: module_self.name,
                                    args,
                                }
                            },
                        );
                    }
                    types.push(module_context.instance_type.clone());
                    module_context.instance_type = Type::intersection(types);
                }
                Some(_) => {
                    self.add_error(diagnostics::class_module_mismatch(
                        node,
                        &module_name.map(ToString::to_string).unwrap_or_default(),
                    ));
                }
                None => {}
            }
        }
        if let Some(type_) = &annotations.instance_type {
            module_context.instance_type = type_.clone();
        }
        if let Some(type_) = &annotations.module_type {
            module_context.module_type = type_.clone();
        }
        if let Some(type_) = &annotations.self_type {
            module_context.module_type = type_.clone();
        }
        let variable_context = match &implement_name {
            Some(implement_name) => {
                TypeVariableContext::new(&self.type_params_of(&implement_name.name), None)
            }
            None => TypeVariableContext::empty(),
        };
        let module_definition = module_context.module_definition.clone();
        self.body_constr(
            annotations,
            module_context,
            variable_context,
            nesting,
            module_definition,
            true,
        )
    }

    fn for_class(
        &self,
        node: &Rc<Node>,
        class_name: Option<&TypeName>,
        super_name: Option<&TypeName>,
    ) -> TypeConstruction<'checker> {
        let nesting: ResolverContext = Some(Rc::new(Nesting {
            parent: self.nesting(),
            name: class_name.cloned(),
        }));
        let annotations = self.annotations_for(Some(node), &nesting);
        let implement_name = self.implement_module(class_name, super_name, &annotations, &nesting);
        let mut module_context =
            self.default_module_context(implement_name.as_ref(), nesting.clone());
        match &implement_name {
            Some(implement_name) => {
                if let Some(super_name) = super_name
                    && self
                        .checker
                        .absolute_type_name(super_name, &self.nesting())
                        .as_ref()
                        == Some(&implement_name.name)
                {
                    module_context.instance_definition = None;
                    module_context.module_definition = None;
                }
                let environment = self.checker.environment();
                let normalized = environment.normalize_module_name(&implement_name.name);
                if environment
                    .class_declarations
                    .get(&normalized)
                    .is_some_and(|entry| entry.kind == ClassOrModule::Module)
                {
                    self.add_error(diagnostics::class_module_mismatch(
                        node,
                        &class_name.map(ToString::to_string).unwrap_or_default(),
                    ));
                }
            }
            None => {
                module_context.instance_type = instance_type("::Object", Vec::new());
                module_context.module_type = Type::Singleton(TypeName::parse("::Object"));
            }
        }
        if let Some(type_) = &annotations.instance_type {
            module_context.instance_type = type_.clone();
        }
        if let Some(type_) = &annotations.module_type {
            module_context.module_type = type_.clone();
        }
        if let Some(type_) = &annotations.self_type {
            module_context.module_type = type_.clone();
        }
        let variable_context =
            TypeVariableContext::new(&self.type_params_of(&module_context.class_name), None);
        let singleton_definition = Some(or_raise(
            self.builder()
                .definition_builder
                .build_singleton(&module_context.class_name),
        ));
        self.body_constr(
            annotations,
            module_context,
            variable_context,
            nesting,
            singleton_definition,
            false,
        )
    }

    pub fn synthesize_class(&self, node: &Rc<Node>) -> Pair<'checker> {
        let name_node = node.child_node(0).cloned().expect("class has a name");
        let super_node = node.child_node(1).cloned();
        let mut constr = self.clone();
        let mut class_name = None;
        if name_node.type_ == "const" {
            let typing = self.typing.clone();
            let constant_name = name_node.child_symbol(1).unwrap_or_default().to_string();
            let unknown = || {
                typing.add_error(diagnostics::unknown_constant(
                    &name_node,
                    &constant_name,
                    "class",
                ))
            };
            let (_, new_constr, name) = self.synthesize_constant_decl(
                Some(&name_node),
                name_node.child_node(0),
                &constant_name,
                Some(&unknown),
            );
            constr = new_constr;
            if let Some(name) = &name {
                self.check_deprecation_constant(name, &name_node, name_node.location.expression);
            }
            class_name = name;
        } else {
            constr = constr.synthesize(&name_node, None, false).constr;
        }
        if let Some(class_name) = &class_name {
            self.typing.update_source_index(|index| {
                index.add_constant_definition(class_name.clone(), &name_node)
            });
        }
        let mut super_name = None;
        if let Some(super_node) = &super_node {
            if super_node.type_ == "const" {
                let typing = self.typing.clone();
                let constant_name = super_node.child_symbol(1).unwrap_or_default().to_string();
                let unknown = || {
                    typing.add_error(diagnostics::unknown_constant(
                        super_node,
                        &constant_name,
                        "class",
                    ))
                };
                let (_, _, name) = constr.synthesize_constant(
                    Some(super_node),
                    super_node.child_node(0),
                    &constant_name,
                    Some(&unknown),
                );
                if let Some(name) = &name {
                    self.typing.update_source_index(|index| {
                        index.add_constant_reference(name.clone(), super_node)
                    });
                }
                super_name = name;
            } else {
                constr.synthesize(super_node, None, false);
            }
        }
        let constructor = self.for_class(node, class_name.as_ref(), super_name.as_ref());
        constructor.with_variable_bounds(|constructor| {
            let module_type = constructor.module_context().module_type.clone();
            constructor.add_typing(&name_node, module_type);
            constructor.set_node_and_body_context(node);
            if let Some(body) = node.child_node(2) {
                constructor.synthesize(body, None, false);
            }
            if let Some(implement_name) = &constructor.module_context().implement_name
                && !namespace_module(node)
            {
                constructor.validate_method_definitions(node, implement_name);
            }
        });
        self.add_typing(node, Type::Nil)
    }

    pub fn synthesize_module(&self, node: &Rc<Node>) -> Pair<'checker> {
        let name_node = node.child_node(0).cloned().expect("module has a name");
        let mut module_name = None;
        if name_node.type_ == "const" {
            let typing = self.typing.clone();
            let constant_name = name_node.child_symbol(1).unwrap_or_default().to_string();
            let unknown = || {
                typing.add_error(diagnostics::unknown_constant(
                    &name_node,
                    &constant_name,
                    "module",
                ))
            };
            let (_, _, name) = self.synthesize_constant_decl(
                Some(&name_node),
                name_node.child_node(0),
                &constant_name,
                Some(&unknown),
            );
            if let Some(name) = &name {
                self.check_deprecation_constant(name, &name_node, name_node.location.expression);
            }
            module_name = name;
        } else {
            self.synthesize(&name_node, None, false);
        }
        if let Some(module_name) = &module_name {
            self.typing.update_source_index(|index| {
                index.add_constant_definition(module_name.clone(), &name_node)
            });
        }
        let constructor = self.for_module(node, module_name.as_ref());
        constructor.with_variable_bounds(|constructor| {
            let module_type = constructor.module_context().module_type.clone();
            constructor.add_typing(&name_node, module_type);
            constructor.set_node_and_body_context(node);
            if let Some(body) = node.child_node(1) {
                constructor.synthesize(body, None, false);
            }
            if let Some(implement_name) = &constructor.module_context().implement_name
                && !namespace_module(node)
            {
                constructor.validate_method_definitions(node, implement_name);
            }
        });
        self.add_typing(node, Type::Nil)
    }

    pub fn validate_method_definitions(&self, node: &Rc<Node>, implement_name: &ImplementName) {
        let environment = self.checker.environment();
        let normalized = environment.normalize_module_name(&implement_name.name);
        let Some(entry) = environment.class_declarations.get(&normalized) else {
            return;
        };
        let member_declaration_count = entry
            .declarations()
            .filter(|declaration| {
                !declaration.declared_in_ruby()
                    && declaration
                        .members()
                        .iter()
                        .any(|member| !matches!(member, Member::Declaration(_)))
            })
            .count();
        if member_declaration_count != 1 {
            return;
        }
        let module_context = self.module_context();
        let expected_names =
            |definition: &Option<Arc<Definition>>, skip_new: bool| -> Vec<String> {
                let Some(definition) = definition else {
                    return Vec::new();
                };
                definition
                    .methods
                    .iter()
                    .filter(|(name, _)| !(skip_new && name.as_str() == "new"))
                    .filter(|(_, method)| method.implemented_in() == Some(&definition.type_name))
                    .map(|(name, _)| name.clone())
                    .collect()
            };
        let expected_instance_method_names =
            expected_names(&module_context.instance_definition, false);
        let expected_module_method_names = expected_names(&module_context.module_definition, true);
        let instance_dynamics = self.annotations.instance_dynamics();
        let module_dynamics = self.annotations.module_dynamics();
        let module_name = implement_name.name.to_string();
        let same_class = implement_name.name == module_context.class_name;
        for method_name in &expected_instance_method_names {
            if module_context
                .defined_instance_methods
                .borrow()
                .contains(method_name)
                || instance_dynamics.contains(method_name)
            {
                continue;
            }
            if same_class {
                self.add_error(diagnostics::method_definition_missing(
                    node,
                    &module_name,
                    false,
                    method_name,
                ));
            }
        }
        for method_name in &expected_module_method_names {
            if module_context
                .defined_module_methods
                .borrow()
                .contains(method_name)
                || module_dynamics.contains(method_name)
            {
                continue;
            }
            if same_class {
                self.add_error(diagnostics::method_definition_missing(
                    node,
                    &module_name,
                    true,
                    method_name,
                ));
            }
        }
        for method_name in &instance_dynamics {
            if !expected_instance_method_names.contains(method_name) {
                self.add_error(diagnostics::unexpected_dynamic_method(node, method_name));
            }
        }
        for method_name in &module_dynamics {
            if !expected_module_method_names.contains(method_name) {
                self.add_error(diagnostics::unexpected_dynamic_method(node, method_name));
            }
        }
    }
}

pub fn namespace_module(node: &Node) -> bool {
    let nodes: Vec<&Rc<Node>> = match node.children.last().and_then(Child::node) {
        Some(child) if child.type_ == "begin" => child.child_nodes().collect(),
        Some(child) => vec![child],
        None => Vec::new(),
    };
    !nodes.is_empty()
        && nodes
            .iter()
            .all(|child| child.type_ == "class" || child.type_ == "module")
}
