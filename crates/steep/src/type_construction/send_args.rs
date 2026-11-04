use std::rc::Rc;

use rbs_definition::ast::Literal;
use steep_interface::{
    method_type::{Block, KeywordParams, Params, PositionalKind, PositionalParam},
    types::{RecordElement, Type},
};

use crate::{
    diagnostic::ruby::{self as diagnostics, Diagnostic},
    ruby::node::Node,
    type_construction::{instance_type, symbol_type},
};

#[derive(Clone, Debug)]
pub enum SendArg {
    NodeParamPair { node: Rc<Node>, type_: Type },
    NodeTypePair { node: Rc<Node>, type_: Type },
    PositionalSplat { node: Rc<Node>, type_: Option<Type> },
    UnexpectedArg { node: Rc<Node> },
    MissingArg,
    ArgTypePairs { pairs: Vec<(Rc<Node>, Type)> },
    KeywordSplat { node: Rc<Node>, type_: Option<Type> },
    UnexpectedKeyword { node: Rc<Node> },
    MissingKeyword { keywords: Vec<String> },
}

impl SendArg {
    pub fn node_type(node: &Node, type_: &Type) -> Type {
        if node.type_ == "splat" {
            instance_type("::Array", vec![type_.clone()])
        } else {
            type_.clone()
        }
    }
}

#[derive(Clone)]
struct PositionalArgs {
    args: Vec<Rc<Node>>,
    index: usize,
    positional_params: Option<Rc<Vec<PositionalParam>>>,
    param_index: usize,
}

impl PositionalArgs {
    fn node(&self) -> Option<&Rc<Node>> {
        self.args.get(self.index)
    }

    fn param(&self) -> Option<&PositionalParam> {
        self.positional_params
            .as_ref()
            .and_then(|params| params.get(self.param_index))
    }

    fn remaining_params(&self) -> Vec<PositionalParam> {
        match &self.positional_params {
            Some(params) => params[self.param_index.min(params.len())..].to_vec(),
            None => Vec::new(),
        }
    }

    fn advanced(&self, consume_param: bool) -> PositionalArgs {
        let mut next = self.clone();
        next.index += 1;
        if consume_param {
            next.param_index += 1;
        }
        next
    }

    fn next(&self) -> Option<(SendArg, PositionalArgs)> {
        let node = self.node();
        if node.is_some_and(|node| node.type_ == "forwarded_args") {
            return None;
        }
        let param = self.param().cloned();
        match (node, param) {
            (None, Some(param)) if param.kind == PositionalKind::Required => {
                let mut next = self.clone();
                next.positional_params = None;
                Some((SendArg::MissingArg, next))
            }
            (None, _) => None,
            (Some(node), _) if node.type_ == "splat" => Some((
                SendArg::PositionalSplat {
                    node: node.clone(),
                    type_: None,
                },
                self.clone(),
            )),
            (Some(node), Some(param)) => {
                let consume = param.kind != PositionalKind::Rest;
                Some((
                    SendArg::NodeParamPair {
                        node: node.clone(),
                        type_: param.type_.clone(),
                    },
                    self.advanced(consume),
                ))
            }
            (Some(node), None) => Some((
                SendArg::UnexpectedArg { node: node.clone() },
                self.advanced(false),
            )),
        }
    }

    fn uniform_type(&self) -> Option<Type> {
        self.positional_params.as_ref()?;
        let remaining = self.remaining_params();
        remaining
            .iter()
            .any(|param| param.kind == PositionalKind::Rest)
            .then(|| {
                Type::intersection(remaining.iter().map(|param| param.type_.clone()).collect())
            })
    }

    fn consume(
        &self,
        count: usize,
        node: &Rc<Node>,
    ) -> (Result<Vec<Type>, SendArg>, PositionalArgs) {
        let mut types = Vec::new();
        let mut param_index = self.param_index;
        for _ in 0..count {
            let head = self
                .positional_params
                .as_ref()
                .and_then(|params| params.get(param_index));
            match head {
                None => {
                    let mut next = self.advanced(false);
                    next.positional_params = None;
                    return (Err(SendArg::UnexpectedArg { node: node.clone() }), next);
                }
                Some(param) => {
                    types.push(param.type_.clone());
                    if param.kind != PositionalKind::Rest {
                        param_index += 1;
                    }
                }
            }
        }
        let mut next = self.advanced(false);
        next.param_index = param_index;
        (Ok(types), next)
    }
}

#[derive(Clone)]
struct KeywordArgs {
    kwarg_nodes: Vec<Rc<Node>>,
    keyword_params: KeywordParams,
    index: usize,
    consumed_keywords: Vec<String>,
}

fn lookup(list: &[(String, Type)], key: &str) -> Option<Type> {
    list.iter()
        .find(|(name, _)| name == key)
        .map(|(_, type_)| type_.clone())
}

impl KeywordArgs {
    fn keyword_type(&self, key: &str) -> Option<Type> {
        lookup(&self.keyword_params.requireds, key)
            .or_else(|| lookup(&self.keyword_params.optionals, key))
    }

    fn rest_type(&self) -> Option<Type> {
        self.keyword_params.rest.clone()
    }

    fn all_keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = Vec::new();
        for (key, _) in self
            .keyword_params
            .requireds
            .iter()
            .chain(self.keyword_params.optionals.iter())
        {
            if !keys.contains(key) {
                keys.push(key.clone());
            }
        }
        keys.sort();
        keys
    }

    fn all_values(&self) -> Vec<Type> {
        let mut values: Vec<Type> = Vec::new();
        for (_, value) in self
            .keyword_params
            .requireds
            .iter()
            .chain(self.keyword_params.optionals.iter())
        {
            if !values.contains(value) {
                values.push(value.clone());
            }
        }
        values.sort_by_key(|type_| type_.to_string());
        values
    }

    fn possible_key_type(&self) -> Type {
        let mut types: Vec<Type> = self
            .all_keys()
            .into_iter()
            .map(|key| Type::Literal(Literal::Symbol(key)))
            .collect();
        if self.rest_type().is_some() {
            types.push(symbol_type());
        }
        Type::union(types)
    }

    fn possible_value_type(&self) -> Type {
        let mut types = self.all_values();
        if let Some(rest) = self.rest_type() {
            types.push(rest);
        }
        Type::intersection(types)
    }

    fn update(&self, index: usize, consumed: &[String]) -> KeywordArgs {
        let mut next = self.clone();
        next.index = index;
        next.consumed_keywords.extend(consumed.iter().cloned());
        next
    }

    fn next(&self) -> Option<(SendArg, KeywordArgs)> {
        match self.kwarg_nodes.get(self.index) {
            Some(node) => match node.type_ {
                "pair" => {
                    let key_node = node.child_node(0).cloned()?;
                    let value_node = node.child_node(1).cloned()?;
                    if key_node.type_ == "sym" {
                        let key = key_node.child_symbol(0).unwrap_or_default().to_string();
                        if let Some(value_type) = self.keyword_type(&key) {
                            Some((
                                SendArg::ArgTypePairs {
                                    pairs: vec![
                                        (key_node, Type::Literal(Literal::Symbol(key.clone()))),
                                        (value_node, value_type),
                                    ],
                                },
                                self.update(self.index + 1, &[key]),
                            ))
                        } else if let Some(value_type) = self.rest_type() {
                            Some((
                                SendArg::ArgTypePairs {
                                    pairs: vec![
                                        (key_node, symbol_type()),
                                        (value_node, value_type),
                                    ],
                                },
                                self.update(self.index + 1, &[key]),
                            ))
                        } else {
                            Some((
                                SendArg::UnexpectedKeyword { node: node.clone() },
                                self.update(self.index + 1, &[]),
                            ))
                        }
                    } else if !self.all_keys().is_empty() || self.rest_type().is_some() {
                        Some((
                            SendArg::ArgTypePairs {
                                pairs: vec![
                                    (key_node, self.possible_key_type()),
                                    (value_node, self.possible_value_type()),
                                ],
                            },
                            self.update(self.index + 1, &[]),
                        ))
                    } else {
                        Some((
                            SendArg::UnexpectedKeyword { node: node.clone() },
                            self.update(self.index + 1, &[]),
                        ))
                    }
                }
                "kwsplat" => Some((
                    SendArg::KeywordSplat {
                        node: node.clone(),
                        type_: None,
                    },
                    self.clone(),
                )),
                _ => None,
            },
            None => {
                let left: Vec<String> = self
                    .keyword_params
                    .requireds
                    .iter()
                    .map(|(key, _)| key.clone())
                    .filter(|key| !self.consumed_keywords.contains(key))
                    .collect();
                if left.is_empty() {
                    None
                } else {
                    let next = self.update(self.index, &left);
                    Some((SendArg::MissingKeyword { keywords: left }, next))
                }
            }
        }
    }

    fn consume_keys(
        &self,
        keys: &[String],
        node: &Rc<Node>,
    ) -> (Result<Vec<Type>, SendArg>, KeywordArgs) {
        let mut consumed = Vec::new();
        let mut types = Vec::new();
        let mut unexpected = None;
        for key in keys {
            if let Some(type_) = self.keyword_type(key) {
                consumed.push(key.clone());
                types.push(type_);
            } else if let Some(type_) = self.rest_type() {
                types.push(type_);
            } else {
                unexpected = Some(key.clone());
            }
        }
        let next = self.update(self.index + 1, &consumed);
        match unexpected {
            Some(_) => (Err(SendArg::UnexpectedKeyword { node: node.clone() }), next),
            None => (Ok(types), next),
        }
    }
}

pub struct BlockPassArg {
    pub node: Option<Rc<Node>>,
    pub block: Option<Block>,
}

impl BlockPassArg {
    pub fn is_compatible(&self) -> bool {
        match &self.node {
            Some(_) => self.block.is_some(),
            None => self.block.as_ref().is_none_or(|block| block.optional),
        }
    }

    pub fn is_block_missing(&self) -> bool {
        self.node.is_none() && self.block.as_ref().is_some_and(|block| !block.optional)
    }

    pub fn is_unexpected_block(&self) -> bool {
        self.node.is_some() && self.block.is_none()
    }

    pub fn proc_type(&self) -> Option<Type> {
        let block = self.block.as_ref()?;
        Some(Type::Proc {
            function: Box::new(block.type_.clone()),
            block: None,
            self_type: block.self_type.clone().map(Box::new),
        })
    }

    pub fn node_type(&self) -> Option<Type> {
        let block = self.block.as_ref()?;
        let proc_type = self.proc_type()?;
        Some(if block.optional {
            Type::union(vec![proc_type, Type::Nil])
        } else {
            proc_type
        })
    }
}

pub struct ForwardedArgs {
    pub node: Rc<Node>,
    pub params: Params,
}

pub struct SendArgs {
    pub node: Rc<Node>,
    pub arguments: Vec<Rc<Node>>,
    pub params: Params,
    pub block: Option<Block>,
}

impl SendArgs {
    fn kwargs_node(&self) -> Option<&Rc<Node>> {
        if self.keyword_params_empty() {
            return None;
        }
        self.arguments.iter().find(|node| node.type_ == "kwargs")
    }

    fn keyword_params_empty(&self) -> bool {
        let keywords = &self.params.keywords;
        keywords.requireds.is_empty() && keywords.optionals.is_empty() && keywords.rest.is_none()
    }

    fn positional_arg(&self) -> PositionalArgs {
        let args: Vec<Rc<Node>> = if self.keyword_params_empty() {
            self.arguments
                .iter()
                .take_while(|node| node.type_ != "block_pass")
                .cloned()
                .collect()
        } else {
            self.arguments
                .iter()
                .take_while(|node| node.type_ != "kwargs" && node.type_ != "block_pass")
                .cloned()
                .collect()
        };
        PositionalArgs {
            args,
            index: 0,
            positional_params: Some(Rc::new(self.params.positionals.clone())),
            param_index: 0,
        }
    }

    pub fn forwarded_args_node(&self) -> Option<&Rc<Node>> {
        self.arguments
            .iter()
            .find(|node| node.type_ == "forwarded_args")
    }

    pub fn block_pass_arg(&self) -> BlockPassArg {
        BlockPassArg {
            node: self
                .arguments
                .iter()
                .find(|node| node.type_ == "block_pass")
                .cloned(),
            block: self.block.clone(),
        }
    }

    pub fn each(
        &self,
        callback: &mut dyn FnMut(&mut SendArg),
    ) -> (Option<ForwardedArgs>, Vec<Diagnostic>) {
        let mut errors: Vec<SendArg> = Vec::new();
        let mut last_positional_args = self.positional_arg();
        let mut args = self.positional_arg();
        while let Some((mut value, next)) = args.next() {
            args = next;
            callback(&mut value);
            match &value {
                SendArg::PositionalSplat { node, type_ } => match type_ {
                    Some(Type::Tuple(types)) => {
                        let (result, next) = args.consume(types.len(), node);
                        args = next;
                        match result {
                            Ok(types) => {
                                let mut pair = SendArg::NodeTypePair {
                                    node: node.clone(),
                                    type_: Type::Tuple(types),
                                };
                                callback(&mut pair);
                            }
                            Err(mut unexpected) => {
                                errors.push(unexpected.clone());
                                callback(&mut unexpected);
                            }
                        }
                    }
                    _ => {
                        let following: Vec<Rc<Node>> = args.args[args.index..].to_vec();
                        match args.uniform_type() {
                            Some(type_) => {
                                for node in following {
                                    let mut pair = SendArg::NodeTypePair {
                                        node,
                                        type_: type_.clone(),
                                    };
                                    callback(&mut pair);
                                }
                            }
                            None => {
                                for node in following {
                                    let mut arg = SendArg::UnexpectedArg { node };
                                    callback(&mut arg);
                                    errors.push(arg);
                                }
                            }
                        }
                        break;
                    }
                },
                SendArg::UnexpectedArg { .. } | SendArg::MissingArg => errors.push(value.clone()),
                _ => {}
            }
            last_positional_args = args.clone();
        }
        let mut forwarded_args = None;
        if let Some(node) = self.forwarded_args_node() {
            forwarded_args = Some(ForwardedArgs {
                node: node.clone(),
                params: Params {
                    positionals: last_positional_args.remaining_params(),
                    keywords: self.params.keywords.clone(),
                },
            });
        } else {
            let mut args = KeywordArgs {
                kwarg_nodes: self
                    .kwargs_node()
                    .map(|node| node.child_nodes().cloned().collect())
                    .unwrap_or_default(),
                keyword_params: self.params.keywords.clone(),
                index: 0,
                consumed_keywords: Vec::new(),
            };
            while let Some((mut value, next)) = args.next() {
                args = next;
                if matches!(
                    value,
                    SendArg::MissingKeyword { .. } | SendArg::UnexpectedKeyword { .. }
                ) {
                    errors.push(value.clone());
                }
                callback(&mut value);
                if let SendArg::KeywordSplat { node, type_ } = &value {
                    match type_ {
                        Some(Type::Record(elements)) => {
                            let keys: Vec<String> = elements
                                .iter()
                                .map(|element| match &element.key {
                                    Literal::Symbol(key)
                                    | Literal::String(key)
                                    | Literal::Integer(key) => key.clone(),
                                    Literal::Bool(value) => value.to_string(),
                                })
                                .collect();
                            let (result, next) = args.consume_keys(&keys, node);
                            args = next;
                            match result {
                                Err(mut unexpected) => {
                                    callback(&mut unexpected);
                                    errors.push(unexpected);
                                }
                                Ok(types) => {
                                    let record = Type::Record(
                                        elements
                                            .iter()
                                            .zip(types)
                                            .map(|(element, type_)| RecordElement {
                                                key: element.key.clone(),
                                                type_,
                                                required: true,
                                            })
                                            .collect(),
                                    );
                                    let mut pairs = SendArg::ArgTypePairs {
                                        pairs: vec![(node.clone(), record)],
                                    };
                                    callback(&mut pairs);
                                }
                            }
                        }
                        _ => {
                            args = args.update(args.index + 1, &[]);
                            if args.rest_type().is_some() {
                                let type_ = instance_type(
                                    "::Hash",
                                    vec![symbol_type(), args.possible_value_type()],
                                );
                                let mut pairs = SendArg::ArgTypePairs {
                                    pairs: vec![(node.clone(), type_)],
                                };
                                callback(&mut pairs);
                            } else {
                                let mut unexpected =
                                    SendArg::UnexpectedKeyword { node: node.clone() };
                                callback(&mut unexpected);
                            }
                        }
                    }
                }
            }
        }
        let mut diagnostics_list = Vec::new();
        let mut missing_keywords = Vec::new();
        for error in errors {
            match error {
                SendArg::UnexpectedKeyword { node, .. } => {
                    diagnostics_list.push(diagnostics::unexpected_keyword_argument(&node));
                }
                SendArg::MissingKeyword { keywords } => missing_keywords.extend(keywords),
                SendArg::UnexpectedArg { node } => {
                    if node.type_ == "kwargs" {
                        for keyword_argument in node.child_nodes() {
                            if keyword_argument.type_ == "pair" {
                                diagnostics_list.push(diagnostics::unexpected_keyword_argument(
                                    keyword_argument,
                                ));
                            }
                        }
                    } else {
                        diagnostics_list.push(diagnostics::unexpected_positional_argument(&node));
                    }
                }
                SendArg::MissingArg => {
                    diagnostics_list
                        .push(diagnostics::insufficient_positional_arguments(&self.node));
                }
                _ => {}
            }
        }
        if !missing_keywords.is_empty() {
            diagnostics_list.push(diagnostics::insufficient_keyword_arguments(
                &self.node,
                &missing_keywords,
            ));
        }
        (forwarded_args, diagnostics_list)
    }
}
