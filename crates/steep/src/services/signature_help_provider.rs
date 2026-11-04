use std::{
    panic::{self, AssertUnwindSafe},
    path::Path,
    rc::Rc,
};

use rbs_definition::ast::{Callable, MethodType as RbsMethodType};
use steep_interface::{builder::Config, factory::method_type_1};

use crate::{
    annotation::AnnotationError,
    ruby::node::{Child, Node},
    services::type_check_service::{TypeCheckService, type_check},
    source::Source,
    type_construction::{Checker, send::method_declarations},
    type_inference::method_call::MethodCallKind,
    typing::Typing,
};

#[derive(Clone, Debug)]
pub struct Item {
    pub method_type: RbsMethodType,
    pub comment: Option<String>,
    pub active_parameter: Option<i64>,
}

impl Item {
    pub fn parameters(&self) -> Option<Vec<String>> {
        let Callable::Function(function) = &self.method_type.type_ else {
            return None;
        };
        let mut arguments = Vec::new();
        arguments.extend(
            function
                .required_positionals
                .iter()
                .map(ToString::to_string),
        );
        arguments.extend(
            function
                .optional_positionals
                .iter()
                .map(|parameter| format!("?{parameter}")),
        );
        if let Some(rest) = &function.rest_positionals {
            arguments.push(format!("*{rest}"));
        }
        arguments.extend(
            function
                .trailing_positionals
                .iter()
                .map(ToString::to_string),
        );
        arguments.extend(
            function
                .required_keywords
                .iter()
                .map(|(name, parameter)| format!("{name}: {parameter}")),
        );
        arguments.extend(
            function
                .optional_keywords
                .iter()
                .map(|(name, parameter)| format!("?{name}: {parameter}")),
        );
        if let Some(rest) = &function.rest_keywords {
            arguments.push(format!("**{rest}"));
        }
        Some(arguments)
    }
}

#[derive(Clone, Debug)]
pub struct SignatureHelp {
    pub items: Vec<Item>,
    pub active_signature: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignatureHelpError {
    SyntaxError,
    UnknownNode,
    Runtime(String),
}

fn runtime_error(name: &str) -> SignatureHelpError {
    SignatureHelpError::Runtime(name.to_string())
}

fn is_block(node: &Node) -> bool {
    matches!(node.type_, "block" | "numblock" | "itblock")
}

fn private_send(node: &Node) -> Result<bool, SignatureHelpError> {
    match node.type_ {
        "block" | "numblock" | "itblock" => private_send(
            node.child_node(0)
                .ok_or_else(|| runtime_error("RuntimeError"))?,
        ),
        "send" | "csend" => Ok(match node.child_node(0) {
            Some(receiver) => receiver.type_ == "self",
            None => true,
        }),
        _ => Err(runtime_error("RuntimeError")),
    }
}

pub struct SignatureHelpProvider {
    source: Source,
    checker: Rc<Checker>,
}

fn nth_from_end(nodes: &[Rc<Node>], index: usize) -> Result<&Rc<Node>, SignatureHelpError> {
    nodes
        .len()
        .checked_sub(index)
        .and_then(|position| nodes.get(position))
        .ok_or_else(|| runtime_error("IndexError"))
}

fn argument_index(node: &Node, argument: &Node) -> Result<usize, SignatureHelpError> {
    for (index, child) in node.children.iter().skip(2).enumerate() {
        match child {
            Child::Node(child) => {
                if child.location == argument.location {
                    return Ok(index);
                }
            }
            _ => return Err(runtime_error("NoMethodError")),
        }
    }
    Ok(0)
}

fn keyword_index(
    function: &rbs_definition::ast::Function,
    pair: &Node,
    positionals: i64,
    offset: i64,
) -> Option<i64> {
    let name = pair
        .child_node(0)
        .and_then(|key| key.child_symbol(0))
        .unwrap_or_default();
    if let Some(index) = function
        .required_keywords
        .iter()
        .position(|(keyword, _)| keyword == name)
    {
        Some(positionals + index as i64 + offset)
    } else if let Some(index) = function
        .optional_keywords
        .iter()
        .position(|(keyword, _)| keyword == name)
    {
        Some(positionals + function.required_keywords.len() as i64 + index as i64 + offset)
    } else if function.rest_keywords.is_some() {
        Some(
            positionals
                + function.required_keywords.len() as i64
                + function.optional_keywords.len() as i64,
        )
    } else {
        None
    }
}

impl SignatureHelpProvider {
    pub fn new(
        service: &TypeCheckService,
        path: &Path,
        position: usize,
    ) -> Result<Option<SignatureHelpProvider>, SignatureHelpError> {
        let project = &service.project;
        let Some(target) = project
            .target_for_inline_source_path(path)
            .or_else(|| project.target_for_source_path(path))
            .or_else(|| service.source_target(path))
        else {
            return Ok(None);
        };
        let Some(file) = service.source_files.get(path) else {
            return Ok(None);
        };
        let Some(checker) = service.checker(&target.name) else {
            return Ok(None);
        };
        let source = Source::parse(
            &path.to_string_lossy(),
            &file.content,
            checker.builder.steep_version,
        )
        .map_err(|error| match error {
            crate::source::SourceError::Syntax(_) => SignatureHelpError::SyntaxError,
            crate::source::SourceError::Annotation(AnnotationError::Syntax(_)) => {
                runtime_error("Steep::AnnotationParser::SyntaxError")
            }
            crate::source::SourceError::Annotation(AnnotationError::Unexpected { .. }) => {
                runtime_error("RuntimeError")
            }
        })?
        .without_unrelated_defs(position);
        Ok(Some(SignatureHelpProvider { source, checker }))
    }

    fn type_check(&self, position: usize) -> Result<Typing, SignatureHelpError> {
        let source = self.source.without_unrelated_defs(position);
        panic::catch_unwind(AssertUnwindSafe(|| {
            type_check(&self.checker, &source, Some(position))
        }))
        .map_err(|_| runtime_error("RuntimeError"))
    }

    pub fn run(&self, position: usize) -> Result<Option<SignatureHelp>, SignatureHelpError> {
        let Some(mut nodes) = self.source.find_nodes(position) else {
            return Ok(None);
        };
        let typing = self.type_check(position)?;
        let mut argument_nodes: Vec<Rc<Node>> = Vec::new();
        loop {
            if nodes.is_empty() {
                return Ok(None);
            }
            let node = nodes.remove(0);
            let parent = nodes.first().cloned();
            argument_nodes.push(node.clone());
            if matches!(node.type_, "send" | "csend")
                && let (Some(begin), Some(end)) = (node.location.begin, node.location.end)
                && begin.end_pos <= position
                && position <= end.begin_pos
            {
                let send_node = match &parent {
                    Some(parent)
                        if is_block(parent)
                            && parent
                                .child_node(0)
                                .is_some_and(|child| Rc::ptr_eq(child, &node)) =>
                    {
                        parent.clone()
                    }
                    _ => node.clone(),
                };
                let last_argument_nodes = self.last_argument_nodes_for(&argument_nodes, position);
                return self.signature_help_for(
                    &send_node,
                    &argument_nodes,
                    last_argument_nodes.as_deref(),
                    &typing,
                );
            }
        }
    }

    fn last_argument_nodes_for(
        &self,
        argument_nodes: &[Rc<Node>],
        position: usize,
    ) -> Option<Vec<Rc<Node>>> {
        let last = argument_nodes.last()?;
        if last.children.len() <= 2 || last.children[2].is_nil() {
            return None;
        }
        if argument_nodes.len() > 1 {
            return Some(argument_nodes.to_vec());
        }
        let text = &self.source.buffer.source;
        let mut position = position;
        loop {
            position = text[..position].char_indices().next_back()?.0;
            let nodes = self.source.find_nodes(position)?;
            let index = nodes
                .iter()
                .position(|node| matches!(node.type_, "send" | "csend"))
                .unwrap_or(0);
            if index > 0 {
                return Some(nodes[..=index].to_vec());
            }
        }
    }

    fn signature_help_for(
        &self,
        node: &Rc<Node>,
        argument: &[Rc<Node>],
        last_argument: Option<&[Rc<Node>]>,
        typing: &Typing,
    ) -> Result<Option<SignatureHelp>, SignatureHelpError> {
        let call = typing
            .call_of(node.id)
            .ok_or(SignatureHelpError::UnknownNode)?;
        let context = typing
            .cursor_context()
            .context()
            .cloned()
            .ok_or_else(|| runtime_error("RuntimeError"))?;
        let mut items = Vec::new();
        let mut index = None;
        let typed = match call.kind {
            MethodCallKind::Typed { .. } | MethodCallKind::Special { .. } => true,
            MethodCallKind::Error { .. } => false,
            MethodCallKind::Untyped | MethodCallKind::NoMethodError { .. } => return Ok(None),
        };
        let builder = &self.checker.builder;
        let config = Config {
            self_type: Some(context.self_type.clone()),
            class_type: None,
            instance_type: None,
            variable_bounds: context.variable_context.upper_bounds(),
        };
        if let Some(shape) = builder
            .shape(&call.receiver_type, &config)
            .map_err(|error| runtime_error(&error.to_string()))?
        {
            let shape = if private_send(node)? {
                shape.public_shape()
            } else {
                shape
            };
            if let Some(method) = shape
                .method(builder, &call.method_name)
                .map_err(|error| runtime_error(&error.to_string()))?
            {
                for (overload_index, overload) in method.overloads.iter().enumerate() {
                    let definition = overload.definitions.first();
                    let active_parameter = self.active_parameter_for(
                        definition.map(|definition| &*definition.type_),
                        argument,
                        last_argument,
                        node,
                    )?;
                    items.push(Item {
                        method_type: method_type_1(&overload.method_type),
                        comment: definition.and_then(|definition| match &definition.member {
                            rbs_definition::definition_builder::DefinitionMember::MethodDefinition(member) => {
                                member.comment.clone()
                            }
                            rbs_definition::definition_builder::DefinitionMember::Attribute(member) => {
                                member.comment.clone()
                            }
                        }),
                        active_parameter,
                    });
                    if typed {
                        let declarations = method_declarations(&call.method_name, overload);
                        let intersects = call.method_decls.iter().any(|declaration| {
                            declarations.iter().any(|other| {
                                declaration == other
                                    && declaration.definition.defined_in
                                        == other.definition.defined_in
                                    && declaration.definition.implemented_in
                                        == other.definition.implemented_in
                            })
                        });
                        if intersects {
                            index = Some(overload_index);
                        }
                    }
                }
            }
        }
        Ok(Some(SignatureHelp {
            items,
            active_signature: index,
        }))
    }

    fn active_parameter_for(
        &self,
        method_type: Option<&RbsMethodType>,
        argument_nodes: &[Rc<Node>],
        last_argument_nodes: Option<&[Rc<Node>]>,
        node: &Node,
    ) -> Result<Option<i64>, SignatureHelpError> {
        let Some(method_type) = method_type else {
            return Ok(None);
        };
        let Callable::Function(function) = &method_type.type_ else {
            return Ok(None);
        };
        let positionals = (function.required_positionals.len()
            + function.optional_positionals.len()
            + usize::from(function.rest_positionals.is_some())
            + function.trailing_positionals.len()) as i64;
        let required_keywords = function.required_keywords.len() as i64;
        let optional_keywords = function.optional_keywords.len() as i64;
        let leading =
            (function.required_positionals.len() + function.optional_positionals.len()) as i64;
        let keywords_end = || positionals + required_keywords + optional_keywords;
        if argument_nodes.len() == 1 {
            let Some(last_argument_nodes) = last_argument_nodes else {
                return Ok(Some(0));
            };
            let last = nth_from_end(last_argument_nodes, 2)?;
            return Ok(match last.type_ {
                "splat" => function.rest_positionals.as_ref().map(|_| leading + 1),
                "kwargs" => {
                    let argument = nth_from_end(last_argument_nodes, 3)?;
                    match argument.type_ {
                        "pair" => keyword_index(function, argument, positionals, 1),
                        "kwsplat" => function.rest_keywords.as_ref().map(|_| keywords_end()),
                        _ => None,
                    }
                }
                _ => {
                    let index = argument_index(node, last)? as i64;
                    if function.rest_positionals.is_some() {
                        Some((index + 1).min(positionals - 1))
                    } else {
                        Some((index + 1).min(positionals))
                    }
                }
            });
        }
        let current = nth_from_end(argument_nodes, 2)?;
        Ok(match current.type_ {
            "splat" => function.rest_positionals.as_ref().map(|_| leading),
            "kwargs" => match argument_nodes
                .len()
                .checked_sub(3)
                .map(|index| &argument_nodes[index])
            {
                Some(argument) => match argument.type_ {
                    "pair" => keyword_index(function, argument, positionals, 0),
                    "kwsplat" => function.rest_keywords.as_ref().map(|_| keywords_end()),
                    _ => None,
                },
                None => None,
            },
            _ => {
                let index = argument_index(node, current)? as i64;
                Some(index.min(positionals - 1))
            }
        })
    }
}
