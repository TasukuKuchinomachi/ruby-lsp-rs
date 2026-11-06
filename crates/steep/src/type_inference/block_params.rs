use std::rc::Rc;

use steep_interface::{
    method_type::{Block, KeywordParams, Params},
    types::Type,
};

use crate::{
    annotation::Collection,
    ruby::node::{Child, Node},
    ruby_exception,
    type_construction::{Checker, instance_type, is_instance_of},
};

#[derive(Clone, Debug)]
pub struct Param {
    pub var: Option<String>,
    pub type_: Option<Type>,
    pub value: Option<Rc<Node>>,
    pub node: Rc<Node>,
}

impl Param {
    fn is_same(&self, other: &Param) -> bool {
        self.var == other.var
            && self.type_ == other.type_
            && self.value.as_ref().map(|value| value.id)
                == other.value.as_ref().map(|value| value.id)
            && self.node.id == other.node.id
    }
}

#[derive(Clone, Debug)]
pub struct MultipleParam {
    pub node: Rc<Node>,
    pub params: Vec<BlockParam>,
}

impl MultipleParam {
    pub fn variable_types(&self) -> Vec<(String, Option<Type>)> {
        self.each_param()
            .into_iter()
            .filter_map(|param| Some((param.var.clone()?, param.type_.clone())))
            .collect()
    }

    pub fn each_param(&self) -> Vec<&Param> {
        let mut params = Vec::new();
        for param in &self.params {
            match param {
                BlockParam::Param(param) => params.push(param),
                BlockParam::MultipleParam(multiple) => params.extend(multiple.each_param()),
            }
        }
        params
    }

    pub fn type_(&self) -> Option<Type> {
        let mut types = Vec::new();
        for param in &self.params {
            match param {
                BlockParam::Param(param) => types.push(param.type_.clone()?),
                BlockParam::MultipleParam(multiple) => types.push(multiple.type_()?),
            }
        }
        Some(Type::Tuple(types))
    }
}

#[derive(Clone, Debug)]
pub enum BlockParam {
    Param(Param),
    MultipleParam(MultipleParam),
}

impl BlockParam {
    pub fn node(&self) -> &Rc<Node> {
        match self {
            BlockParam::Param(param) => &param.node,
            BlockParam::MultipleParam(multiple) => &multiple.node,
        }
    }

    pub fn each_param(&self) -> Vec<&Param> {
        match self {
            BlockParam::Param(param) => vec![param],
            BlockParam::MultipleParam(multiple) => multiple.each_param(),
        }
    }

    fn is_param(&self, other: Option<&Param>) -> bool {
        match (self, other) {
            (BlockParam::Param(param), Some(other)) => param.is_same(other),
            _ => false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct BlockParams {
    pub leading_params: Vec<BlockParam>,
    pub optional_params: Vec<Param>,
    pub rest_param: Option<Param>,
    pub trailing_params: Vec<BlockParam>,
    pub block_param: Option<Param>,
    pub keyword_params: Vec<Rc<Node>>,
}

enum DefaultParams {
    Leading,
    Trailing,
}

impl BlockParams {
    pub fn empty() -> BlockParams {
        BlockParams {
            leading_params: Vec::new(),
            optional_params: Vec::new(),
            rest_param: None,
            trailing_params: Vec::new(),
            block_param: None,
            keyword_params: Vec::new(),
        }
    }

    pub fn params(&self) -> Vec<BlockParam> {
        let mut params = Vec::new();
        params.extend(self.leading_params.iter().cloned());
        params.extend(self.optional_params.iter().cloned().map(BlockParam::Param));
        params.extend(self.rest_param.iter().cloned().map(BlockParam::Param));
        params.extend(self.trailing_params.iter().cloned());
        params.extend(self.block_param.iter().cloned().map(BlockParam::Param));
        params
    }

    pub fn from_node(node: &Node, annotations: &Collection) -> BlockParams {
        let mut params = BlockParams::empty();
        let mut default_params = DefaultParams::Leading;
        let push = |params: &mut BlockParams, default_params: &DefaultParams, param: BlockParam| {
            match default_params {
                DefaultParams::Leading => params.leading_params.push(param),
                DefaultParams::Trailing => params.trailing_params.push(param),
            }
        };
        for argument in node.child_nodes() {
            if argument.type_ == "mlhs"
                || (argument.type_ == "procarg0" && argument.children.len() > 1)
            {
                let multiple = BlockParams::from_multiple(argument, annotations);
                push(
                    &mut params,
                    &default_params,
                    BlockParam::MultipleParam(multiple),
                );
                continue;
            }
            let var = argument.child_symbol(0).map(str::to_string);
            let type_ = var
                .as_ref()
                .and_then(|var| annotations.var_types.get(var).cloned());
            match argument.type_ {
                "arg" => push(
                    &mut params,
                    &default_params,
                    BlockParam::Param(Param {
                        var,
                        type_,
                        value: None,
                        node: argument.clone(),
                    }),
                ),
                "procarg0" => {
                    let var = match argument.child(0) {
                        Child::Symbol(name) => Some(name.clone()),
                        Child::Node(inner) => inner.child_symbol(0).map(str::to_string),
                        _ => None,
                    };
                    push(
                        &mut params,
                        &default_params,
                        BlockParam::Param(Param {
                            var,
                            type_,
                            value: None,
                            node: argument.clone(),
                        }),
                    );
                }
                "optarg" => {
                    default_params = DefaultParams::Trailing;
                    params.optional_params.push(Param {
                        var,
                        type_,
                        value: argument.children.last().and_then(Child::node).cloned(),
                        node: argument.clone(),
                    });
                }
                "restarg" => {
                    default_params = DefaultParams::Trailing;
                    params.rest_param = Some(Param {
                        var,
                        type_,
                        value: None,
                        node: argument.clone(),
                    });
                }
                "blockarg" => {
                    params.block_param = Some(Param {
                        var,
                        type_,
                        value: None,
                        node: argument.clone(),
                    });
                    break;
                }
                "kwarg" | "kwoptarg" | "kwrestarg" => params.keyword_params.push(argument.clone()),
                _ => {}
            }
        }
        params
    }

    fn from_multiple(node: &Rc<Node>, annotations: &Collection) -> MultipleParam {
        let mut params = Vec::new();
        for child in node.child_nodes() {
            if child.type_ == "mlhs" {
                params.push(BlockParam::MultipleParam(BlockParams::from_multiple(
                    child,
                    annotations,
                )));
            } else {
                let var = child
                    .child_symbol(0)
                    .map(str::to_string)
                    .unwrap_or_else(|| ruby_exception::raise_runtime_error(""));
                let type_ = annotations.var_types.get(&var).cloned();
                params.push(BlockParam::Param(Param {
                    var: Some(var),
                    node: child.clone(),
                    value: None,
                    type_,
                }));
            }
        }
        MultipleParam {
            node: node.clone(),
            params,
        }
    }

    pub fn params_type(&self, hint: Option<&Params>) -> Params {
        self.params_type0(hint)
            .or_else(|| self.params_type0(None))
            .expect("params_type0 without hint always succeeds")
    }

    fn params_type0(&self, hint: Option<&Params>) -> Option<Params> {
        let leading_types = |count: usize, hint_types: &[&Type]| -> Vec<Type> {
            self.leading_params
                .iter()
                .take(count)
                .enumerate()
                .map(|(index, param)| {
                    leading_param_type(param).unwrap_or_else(|| hint_types[index].clone())
                })
                .collect()
        };
        let optional_types = |count: usize, hint_types: &[&Type]| -> Vec<Type> {
            self.optional_params
                .iter()
                .take(count)
                .enumerate()
                .map(|(index, param)| {
                    param
                        .type_
                        .clone()
                        .unwrap_or_else(|| hint_types[index].clone())
                })
                .collect()
        };
        let leadings: Vec<Type>;
        let optionals: Vec<Type>;
        let mut rest: Option<Type> = None;
        match hint {
            Some(hint) => {
                let required = hint.required_types();
                let optional = hint.optional_types();
                let hint_rest = hint.rest_type();
                let leading_size = self.leading_params.len();
                if leading_size == required.len() {
                    leadings = leading_types(leading_size, &required);
                } else if hint_rest.is_none()
                    && optional.is_empty()
                    && leading_size > required.len()
                {
                    leadings = leading_types(required.len(), &required);
                } else if hint_rest.is_none()
                    && optional.is_empty()
                    && leading_size < required.len()
                {
                    let mut types = leading_types(leading_size, &required);
                    types.extend(
                        required
                            .iter()
                            .skip(leading_size)
                            .map(|type_| (*type_).clone()),
                    );
                    leadings = types;
                } else {
                    return None;
                }
                let optional_size = self.optional_params.len();
                if optional_size == optional.len() {
                    optionals = optional_types(optional_size, &optional);
                } else if hint_rest.is_none() && optional_size > optional.len() {
                    optionals = optional_types(optional.len(), &optional);
                } else if hint_rest.is_none() && optional_size < optional.len() {
                    let mut types = optional_types(optional_size, &optional);
                    types.extend(
                        optional
                            .iter()
                            .skip(optional_size)
                            .map(|type_| (*type_).clone()),
                    );
                    optionals = types;
                } else {
                    return None;
                }
                if let Some(rest_param) = &self.rest_param
                    && let Some(hint_rest) = hint_rest
                {
                    if let Some(rest_type) = &rest_param.type_
                        && is_instance_of(rest_type, "::Array")
                        && let Type::NameInstance { args, .. } = rest_type
                    {
                        rest = Some(
                            args.first()
                                .cloned()
                                .unwrap_or_else(|| ruby_exception::raise_runtime_error("")),
                        );
                    }
                    if rest.is_none() {
                        rest = Some(hint_rest.clone());
                    }
                }
            }
            None => {
                leadings = self
                    .leading_params
                    .iter()
                    .map(|param| leading_param_type(param).unwrap_or(Type::Any))
                    .collect();
                optionals = self
                    .optional_params
                    .iter()
                    .map(|param| param.type_.clone().unwrap_or(Type::Any))
                    .collect();
                if let Some(rest_param) = &self.rest_param {
                    if let Some(rest_type) = &rest_param.type_
                        && is_instance_of(rest_type, "::Array")
                        && let Type::NameInstance { args, .. } = rest_type
                    {
                        rest = Some(
                            args.first()
                                .cloned()
                                .unwrap_or_else(|| ruby_exception::raise_runtime_error("")),
                        );
                    }
                    if rest.is_none() {
                        rest = Some(Type::Any);
                    }
                }
            }
        }
        Some(Params::build(
            leadings,
            optionals,
            rest,
            KeywordParams::default(),
        ))
    }

    pub fn zip(
        &self,
        params_type: Option<&Params>,
        block: Option<&Block>,
        checker: &Checker,
    ) -> Vec<(BlockParam, Type)> {
        let mut zip: Vec<(BlockParam, Type)> = Vec::new();
        let rest_param = self.rest_param.as_ref();
        let params_type = match params_type {
            Some(params_type) if !BlockParams::untyped_args(params_type) => params_type,
            _ => {
                for param in self.params() {
                    if param.is_param(rest_param) {
                        zip.push((param, instance_type("::Array", vec![Type::Any])));
                    } else {
                        zip.push((param, Type::Any));
                    }
                }
                return zip;
            }
        };
        let expandable_type = if self.expandable() {
            BlockParams::expandable_params(params_type, checker)
        } else {
            None
        };
        match expandable_type {
            Some(type_) => match &type_ {
                Type::NameInstance { args, .. } if is_instance_of(&type_, "::Array") => {
                    let type_argument = ruby_exception::fetch(args, 0).clone();
                    for param in self.params() {
                        if !param.is_param(rest_param) {
                            zip.push((param, Type::union(vec![type_argument.clone(), Type::Nil])));
                        } else {
                            zip.push((
                                param,
                                instance_type("::Array", vec![type_argument.clone()]),
                            ));
                        }
                    }
                }
                Type::Tuple(types) => {
                    let mut types = types.clone().into_iter();
                    let leading_and_optional = self
                        .leading_params
                        .iter()
                        .cloned()
                        .chain(self.optional_params.iter().cloned().map(BlockParam::Param));
                    for param in leading_and_optional {
                        match types.next() {
                            Some(type_) => zip.push((param, type_)),
                            None => zip.push((param, Type::Nil)),
                        }
                    }
                    if let Some(rest_param) = &self.rest_param {
                        let remaining: Vec<Type> = types.collect();
                        if !remaining.is_empty() {
                            let union = Type::union(remaining);
                            zip.push((
                                BlockParam::Param(rest_param.clone()),
                                instance_type("::Array", vec![union]),
                            ));
                        } else {
                            zip.push((BlockParam::Param(rest_param.clone()), Type::Nil));
                        }
                    }
                }
                _ => {}
            },
            None => {
                let mut types = params_type
                    .flat_unnamed_params()
                    .into_iter()
                    .map(|(_, type_)| type_.clone())
                    .collect::<Vec<_>>()
                    .into_iter();
                let leading_and_optional = self
                    .leading_params
                    .iter()
                    .cloned()
                    .chain(self.optional_params.iter().cloned().map(BlockParam::Param));
                for param in leading_and_optional {
                    let type_ = types.next().or_else(|| params_type.rest_type().cloned());
                    match type_ {
                        Some(type_) => zip.push((param, type_)),
                        None => zip.push((param, Type::Nil)),
                    }
                }
                if let Some(rest_param) = &self.rest_param {
                    let remaining: Vec<Type> = types.collect();
                    if remaining.is_empty() {
                        let array = instance_type(
                            "::Array",
                            vec![params_type.rest_type().cloned().unwrap_or(Type::Any)],
                        );
                        zip.push((BlockParam::Param(rest_param.clone()), array));
                    } else {
                        let mut union_members = remaining;
                        union_members.extend(params_type.rest_type().cloned());
                        let union = Type::union(union_members);
                        zip.push((
                            BlockParam::Param(rest_param.clone()),
                            instance_type("::Array", vec![union]),
                        ));
                    }
                }
            }
        }
        if let Some(block_param) = &self.block_param {
            match block {
                Some(block) => {
                    let mut proc_type = Type::Proc {
                        function: Box::new(block.type_.clone()),
                        block: None,
                        self_type: block.self_type.clone().map(Box::new),
                    };
                    if block.optional {
                        proc_type = Type::union(vec![proc_type, Type::Nil]);
                    }
                    zip.push((BlockParam::Param(block_param.clone()), proc_type));
                }
                None => zip.push((BlockParam::Param(block_param.clone()), Type::Nil)),
            }
        }
        zip
    }

    fn expandable_params(params_type: &Params, checker: &Checker) -> Option<Type> {
        if params_type.flat_unnamed_params().len() != 1 {
            return None;
        }
        let type_ = params_type
            .required_types()
            .first()
            .map(|type_| (*type_).clone())
            .unwrap_or_else(|| ruby_exception::raise_runtime_error(""));
        let type_ = checker.deep_expand_alias(&type_).unwrap_or(type_);
        match &type_ {
            Type::Tuple(_) => Some(type_),
            Type::NameInstance { .. } if is_instance_of(&type_, "::Array") => Some(type_),
            _ => None,
        }
    }

    fn expandable(&self) -> bool {
        let params = self.params();
        self.leading_params.len() + self.trailing_params.len() > 1
            || ((!self.leading_params.is_empty() || !self.trailing_params.is_empty())
                && self.rest_param.is_some())
            || (params.len() == 1 && params[0].node().type_ == "arg")
    }

    pub fn each_single_param(&self) -> Vec<Param> {
        self.params()
            .iter()
            .flat_map(|param| param.each_param().into_iter().cloned().collect::<Vec<_>>())
            .collect()
    }

    pub fn untyped_args(params: &Params) -> bool {
        let flat = params.flat_unnamed_params();
        flat.len() == 1 && matches!(flat[0].1, Type::Any)
    }
}

fn leading_param_type(param: &BlockParam) -> Option<Type> {
    match param {
        BlockParam::Param(param) => param.type_.clone(),
        BlockParam::MultipleParam(multiple) => multiple.type_(),
    }
}
