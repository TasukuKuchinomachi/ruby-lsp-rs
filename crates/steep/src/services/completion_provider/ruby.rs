use std::{
    panic::{self, AssertUnwindSafe},
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
};

use rbs_definition::{
    ast::Callable,
    definition_builder::{DefinitionMember, TypeDefinition},
    environment::ConstantEntry,
};
use steep_interface::{
    builder::{Config, Overload},
    factory::method_type_1,
    substitution::Substitution,
    types::Type,
};

use crate::{
    annotation::AnnotationError,
    annotations_helper,
    locator::{self, RubyResult},
    ruby::node::{Child, Node, SourceRange},
    services::{
        completion_provider::{
            CompletionError, CompletionItem, ConstantKind,
            type_name::{Prefix, TypeNameCompletion},
        },
        hover_provider::constant_declaration,
        type_check_service::{TypeCheckService, type_check},
    },
    source::{Source, SourceError},
    type_construction::{Checker, send::method_declarations},
    type_inference::{context::Context, method_call::MethodCallKind},
    typing::Typing,
};

pub struct Ruby {
    path: PathBuf,
    checker: Rc<Checker>,
    source_text: String,
}

pub struct TypeChecked {
    pub source: Source,
    pub typing: Typing,
}

fn unknown_node() -> CompletionError {
    CompletionError::UnknownNode
}

fn runtime_error() -> CompletionError {
    CompletionError::Runtime("RuntimeError".to_string())
}

fn same_member(left: &DefinitionMember, right: &DefinitionMember) -> bool {
    match (left, right) {
        (DefinitionMember::MethodDefinition(left), DefinitionMember::MethodDefinition(right)) => {
            if left.declared_in_ruby || right.declared_in_ruby {
                return Arc::ptr_eq(left, right);
            }
            left.name == right.name
                && left.kind == right.kind
                && left.overloading == right.overloading
                && left.visibility == right.visibility
                && left.overloads.len() == right.overloads.len()
                && left
                    .overloads
                    .iter()
                    .zip(&right.overloads)
                    .all(|(left, right)| {
                        left.method_type == right.method_type
                            && left.annotations == right.annotations
                    })
        }
        (DefinitionMember::Attribute(left), DefinitionMember::Attribute(right)) => {
            if left.declared_in_ruby || right.declared_in_ruby {
                return Arc::ptr_eq(left, right);
            }
            left.attribute_kind == right.attribute_kind
                && left.name == right.name
                && left.type_ == right.type_
                && left.instance_variable == right.instance_variable
                && left.kind == right.kind
                && left.visibility == right.visibility
        }
        _ => false,
    }
}

fn same_definition(left: &TypeDefinition, right: &TypeDefinition) -> bool {
    left.type_ == right.type_
        && same_member(&left.member, &right.member)
        && left.defined_in == right.defined_in
        && left.implemented_in == right.implemented_in
}

fn is_word_name(name: &str) -> bool {
    name.chars()
        .any(|character| character.is_ascii_alphanumeric() || character == '_')
}

fn unwrap_optional(type_: &Type) -> Type {
    match type_ {
        Type::Union(types) if types.contains(&Type::Nil) => Type::Union(
            types
                .iter()
                .filter(|type_| **type_ != Type::Nil)
                .cloned()
                .collect(),
        ),
        _ => type_.clone(),
    }
}

fn at_end(position: usize, range: Option<SourceRange>) -> bool {
    range.is_some_and(|range| range.end_pos == position)
}

fn previous_character_position(text: &str, position: usize) -> Option<usize> {
    text[..position]
        .char_indices()
        .next_back()
        .map(|(index, _)| index)
}

fn character_before(text: &str, position: usize) -> Option<char> {
    match previous_character_position(text, position) {
        Some(index) => text[index..].chars().next(),
        None => text.chars().next_back(),
    }
}

fn blank_character_before(text: &mut String, position: usize) {
    let index = previous_character_position(text, position)
        .or_else(|| text.char_indices().next_back().map(|(index, _)| index));
    if let Some(index) = index {
        let length = text[index..].chars().next().map_or(0, char::len_utf8);
        text.replace_range(index..index + length, " ");
    }
}

fn position_before(text: &str, position: usize, characters: usize) -> usize {
    let mut position = position;
    for _ in 0..characters {
        match previous_character_position(text, position) {
            Some(index) => position = index,
            None => break,
        }
    }
    position
}

fn deconstruct_sendish_and_block_nodes(nodes: &[Rc<Node>]) -> Option<(Rc<Node>, Option<Rc<Node>>)> {
    let send_node = nodes.first()?;
    match send_node.type_ {
        "send" | "csend" | "super" => {
            if let Some(block_node) = nodes.get(1)
                && matches!(block_node.type_, "block" | "numblock" | "itblock")
                && block_node
                    .child_node(0)
                    .is_some_and(|child| Rc::ptr_eq(child, send_node))
            {
                return Some((send_node.clone(), Some(block_node.clone())));
            }
            Some((send_node.clone(), None))
        }
        "zsuper" => Some((send_node.clone(), None)),
        _ => None,
    }
}

fn private_send(node: &Node) -> Result<bool, CompletionError> {
    match node.type_ {
        "block" | "numblock" | "itblock" => {
            private_send(node.child_node(0).ok_or_else(runtime_error)?)
        }
        "send" | "csend" => Ok(match node.child_node(0) {
            Some(receiver) => receiver.type_ == "self",
            None => true,
        }),
        _ => Err(runtime_error()),
    }
}

impl Ruby {
    pub fn new(service: &TypeCheckService, path: &Path) -> Option<Ruby> {
        let project = &service.project;
        let target = project
            .target_for_inline_source_path(path)
            .or_else(|| project.target_for_source_path(path))
            .or_else(|| service.source_target(path))?;
        let file = service.source_files.get(path)?;
        let checker = service.checker(&target.name)?;
        Some(Ruby {
            path: path.to_path_buf(),
            checker,
            source_text: file.content.clone(),
        })
    }

    fn type_check(&self, text: &str, position: usize) -> Result<TypeChecked, CompletionError> {
        let path = self.path.to_string_lossy().into_owned();
        let source = match Source::parse(&path, text, self.checker.builder.steep_version) {
            Ok(source) => source,
            Err(SourceError::Syntax(_)) => return Err(CompletionError::SyntaxError),
            Err(SourceError::Annotation(AnnotationError::Syntax(_))) => {
                return Err(CompletionError::Runtime(
                    "Steep::AnnotationParser::SyntaxError".to_string(),
                ));
            }
            Err(SourceError::Annotation(AnnotationError::Unexpected { .. })) => {
                return Err(runtime_error());
            }
        };
        let source = source.without_unrelated_defs(position);
        let typing = panic::catch_unwind(AssertUnwindSafe(|| {
            type_check(&self.checker, &source, Some(position))
        }))
        .map_err(|_| runtime_error())?;
        Ok(TypeChecked { source, typing })
    }

    pub fn run(&self, position: usize) -> Result<Vec<CompletionItem>, CompletionError> {
        let possible_trigger = character_before(&self.source_text, position);
        let result = self
            .type_check(&self.source_text, position)
            .and_then(|checked| self.items_for_trigger(&checked, position));
        match result {
            Err(CompletionError::SyntaxError) => {}
            result => return result,
        }
        let mut source_text = self.source_text.clone();
        let before_trigger = previous_character_position(&source_text, position).unwrap_or(0);
        match possible_trigger {
            Some('.') => {
                if character_before(&source_text, before_trigger) == Some('&') {
                    blank_character_before(&mut source_text, position);
                    blank_character_before(&mut source_text, before_trigger);
                    let checked = self.type_check(&source_text, position)?;
                    self.items_for_qcall(&checked, position)
                } else {
                    blank_character_before(&mut source_text, position);
                    let checked = self.type_check(&source_text, position)?;
                    self.items_for_dot(&checked, position)
                }
            }
            Some('@') => {
                blank_character_before(&mut source_text, position);
                let checked = self.type_check(&source_text, position)?;
                self.items_for_atmark(&checked, position)
            }
            Some(':') => {
                if character_before(&source_text, before_trigger) == Some(':') {
                    blank_character_before(&mut source_text, position);
                    blank_character_before(&mut source_text, before_trigger);
                    let checked = self.type_check(&source_text, position)?;
                    self.items_for_colon2(&checked, position)
                } else {
                    Ok(Vec::new())
                }
            }
            _ => self.items_for_following_keyword_arguments(source_text, position),
        }
    }

    pub fn run_at_comment(
        &self,
        position: usize,
    ) -> Result<Option<(usize, Vec<CompletionItem>)>, CompletionError> {
        let checked = match self.type_check(&self.source_text, position) {
            Ok(checked) => checked,
            Err(CompletionError::SyntaxError) => return Ok(None),
            Err(error) => return Err(error),
        };
        let locator = locator::Ruby::new(&checked.source);
        match locator.find(position) {
            Some(RubyResult::Comment { comment, .. }) => {
                let (prefix_size, mut items) = self.items_for_rbs(&checked, position)?;
                let comment_content = comment.text.get(1..).unwrap_or_default().to_string();
                let annotation_items = [
                    (
                        "steep:ignore:start",
                        "steep:ignore:start",
                        "Open ignore block",
                    ),
                    ("steep:ignore:end", "steep:ignore:end", "Close ignore block"),
                    (
                        "steep:ignore",
                        "steep:ignore ${1:optional diagnostics}",
                        "Ignore line",
                    ),
                    (
                        "@type var x: T",
                        "@type var ${1:variable}: ${2:var type}",
                        "Type of local variable",
                    ),
                    (
                        "@type self: T",
                        "@type self: ${1:self type}",
                        "Type of `self`",
                    ),
                    (
                        "@type block: T",
                        "@type block: ${1:block type}",
                        "Type of `block`",
                    ),
                    (
                        "@type break: T",
                        "@type break: ${1:break type}",
                        "Type of `block`",
                    ),
                ];
                for (label, text, help_text) in annotation_items {
                    if text.starts_with(&comment_content) {
                        let start = position.saturating_sub(comment_content.len());
                        items.push(CompletionItem::Text {
                            text: text.to_string(),
                            help_text: help_text.to_string(),
                            range: SourceRange::new(start, position),
                            label: label.to_string(),
                        });
                    }
                }
                Ok(Some((prefix_size, items)))
            }
            Some(
                RubyResult::TypeApplication { .. }
                | RubyResult::TypeAssertion { .. }
                | RubyResult::Annotation { .. },
            ) => self.items_for_rbs(&checked, position).map(Some),
            _ => Ok(None),
        }
    }

    fn context(&self, checked: &TypeChecked) -> Result<Context, CompletionError> {
        checked
            .typing
            .cursor_context()
            .context()
            .cloned()
            .ok_or_else(runtime_error)
    }

    fn range_for(&self, text: &str, position: usize, prefix: &str) -> SourceRange {
        if prefix.is_empty() {
            SourceRange::new(position, position)
        } else {
            SourceRange::new(
                position_before(text, position, prefix.chars().count()),
                position,
            )
        }
    }

    fn receiver_type(&self, context: &Context, type_: Type, optional: bool) -> Type {
        match type_ {
            Type::SelfType => context.self_type.clone(),
            type_ if optional => unwrap_optional(&type_),
            type_ => type_,
        }
    }

    fn items_for_trigger(
        &self,
        checked: &TypeChecked,
        position: usize,
    ) -> Result<Vec<CompletionItem>, CompletionError> {
        let source = &checked.source;
        let typing = &checked.typing;
        let text = &source.buffer.source;
        let nodes = source.find_nodes(position).unwrap_or_default();
        let (node, parents) = match nodes.split_first() {
            Some((node, parents)) => (node.clone(), parents.to_vec()),
            None => match &source.node {
                Some(node) => (node.clone(), Vec::new()),
                None => return Ok(Vec::new()),
            },
        };
        let mut items = Vec::new();
        let context = self.context(checked)?;
        let receiver = node.child_node(0).cloned();
        let selector = node.location.selector;
        let dot = node.location.dot;
        let dot_source = dot.map(|dot| source.buffer.source_of(dot));
        let name_of = |node: &Node| node.child_symbol(1).unwrap_or_default().to_string();
        match node.type_ {
            "send" if receiver.is_none() && at_end(position, selector) => {
                let prefix = name_of(&node);
                self.method_items_for_receiver_type(
                    checked,
                    &context,
                    &context.self_type,
                    true,
                    &prefix,
                    position,
                    &mut items,
                )?;
                self.local_variable_items(&context, text, position, &prefix, &mut items);
                if let Some((send_node, block_node)) = deconstruct_sendish_and_block_nodes(&parents)
                {
                    self.keyword_argument_items_for_method(
                        checked,
                        &block_node.unwrap_or_else(|| send_node.clone()),
                        &send_node,
                        position,
                        &prefix,
                        &mut items,
                    )?;
                }
            }
            "lvar" if at_end(position, node.location.expression) => {
                let prefix = node.child_symbol(0).unwrap_or_default().to_string();
                self.local_variable_items(&context, text, position, &prefix, &mut items);
            }
            "send" | "csend" if receiver.is_some() && at_end(position, selector) => {
                let receiver = receiver.expect("the receiver is checked");
                let type_ = typing.type_of(receiver.id).ok_or_else(unknown_node)?;
                let receiver_type = self.receiver_type(&context, type_, node.type_ == "csend");
                let prefix = name_of(&node);
                self.method_items_for_receiver_type(
                    checked,
                    &context,
                    &receiver_type,
                    false,
                    &prefix,
                    position,
                    &mut items,
                )?;
            }
            "const" if receiver.is_none() && at_end(position, node.location.expression) => {
                let prefix = name_of(&node);
                self.method_items_for_receiver_type(
                    checked,
                    &context,
                    &context.self_type,
                    false,
                    &prefix,
                    position,
                    &mut items,
                )?;
                self.constant_items(checked, &context, None, position, &prefix, &mut items)?;
            }
            "const" if receiver.is_some() && at_end(position, node.location.expression) => {
                let parent_node = receiver.expect("the scope is checked");
                let parent_type = typing.type_of(parent_node.id).ok_or_else(unknown_node)?;
                let prefix = name_of(&node);
                self.method_items_for_receiver_type(
                    checked,
                    &context,
                    &parent_type,
                    false,
                    &prefix,
                    position,
                    &mut items,
                )?;
                self.constant_items(
                    checked,
                    &context,
                    Some(&parent_node),
                    position,
                    &prefix,
                    &mut items,
                )?;
            }
            "send" if at_end(position, dot) && dot_source == Some(".") => {
                let receiver = receiver.ok_or_else(unknown_node)?;
                let type_ = typing.type_of(receiver.id).ok_or_else(unknown_node)?;
                let receiver_type = self.receiver_type(&context, type_, false);
                self.method_items_for_receiver_type(
                    checked,
                    &context,
                    &receiver_type,
                    false,
                    "",
                    position,
                    &mut items,
                )?;
            }
            "send" if at_end(position, dot) && dot_source == Some("::") => {
                items.extend(self.items_for_colon2(checked, position)?);
            }
            "csend" if at_end(position, dot) => {
                let receiver = receiver.ok_or_else(unknown_node)?;
                let type_ = typing.type_of(receiver.id).ok_or_else(unknown_node)?;
                let receiver_type = self.receiver_type(&context, type_, true);
                self.method_items_for_receiver_type(
                    checked,
                    &context,
                    &receiver_type,
                    false,
                    "",
                    position,
                    &mut items,
                )?;
            }
            "ivar" if at_end(position, node.location.expression) => {
                let prefix = node.child_symbol(0).unwrap_or_default().to_string();
                self.instance_variable_items(&context, text, position, &prefix, &mut items);
            }
            _ => {
                self.method_items_for_receiver_type(
                    checked,
                    &context,
                    &context.self_type,
                    true,
                    "",
                    position,
                    &mut items,
                )?;
                self.local_variable_items(&context, text, position, "", &mut items);
                self.instance_variable_items(&context, text, position, "", &mut items);
                self.constant_items(checked, &context, None, position, "", &mut items)?;
            }
        }
        Ok(items)
    }

    fn node_at_or_root(&self, checked: &TypeChecked, position: usize) -> Option<Rc<Node>> {
        checked
            .source
            .find_nodes(position)
            .and_then(|nodes| nodes.first().cloned())
            .or_else(|| checked.source.node.clone())
    }

    fn items_for_dot(
        &self,
        checked: &TypeChecked,
        position: usize,
    ) -> Result<Vec<CompletionItem>, CompletionError> {
        self.items_after(checked, position, 1, false)
    }

    fn items_for_qcall(
        &self,
        checked: &TypeChecked,
        position: usize,
    ) -> Result<Vec<CompletionItem>, CompletionError> {
        self.items_after(checked, position, 2, true)
    }

    fn items_after(
        &self,
        checked: &TypeChecked,
        position: usize,
        shift: usize,
        optional: bool,
    ) -> Result<Vec<CompletionItem>, CompletionError> {
        let shift_position = position_before(&checked.source.buffer.source, position, shift);
        let Some(node) = self.node_at_or_root(checked, shift_position) else {
            return Ok(Vec::new());
        };
        if !at_end(shift_position, node.location.expression) {
            return Ok(Vec::new());
        }
        let context = self.context(checked)?;
        let Some(type_) = checked.typing.type_of(node.id) else {
            return Ok(Vec::new());
        };
        let receiver_type = self.receiver_type(&context, type_, optional);
        let mut items = Vec::new();
        self.method_items_for_receiver_type(
            checked,
            &context,
            &receiver_type,
            false,
            "",
            position,
            &mut items,
        )?;
        Ok(items)
    }

    fn items_for_colon2(
        &self,
        checked: &TypeChecked,
        position: usize,
    ) -> Result<Vec<CompletionItem>, CompletionError> {
        let text = &checked.source.buffer.source;
        let shift_position = position_before(text, position, 2);
        let node = self.node_at_or_root(checked, shift_position);
        let mut items = Vec::new();
        match node.as_ref().map(|node| node.type_) {
            Some("const") => {
                let context = self.context(checked)?;
                self.constant_items(checked, &context, node.as_ref(), position, "", &mut items)?;
            }
            None => {
                let context = self.context(checked)?;
                self.constant_items(checked, &context, None, position, "", &mut items)?;
            }
            _ => {}
        }
        if node.is_some() {
            items.extend(self.items_for_dot(checked, position_before(text, position, 1))?);
        }
        Ok(items)
    }

    fn items_for_atmark(
        &self,
        checked: &TypeChecked,
        position: usize,
    ) -> Result<Vec<CompletionItem>, CompletionError> {
        let text = &checked.source.buffer.source;
        let shift_position = position_before(text, position, 1);
        if self.node_at_or_root(checked, shift_position).is_none() {
            return Ok(Vec::new());
        }
        let context = self.context(checked)?;
        let mut items = Vec::new();
        self.instance_variable_items(&context, text, position, "@", &mut items);
        Ok(items)
    }

    pub fn items_for_rbs(
        &self,
        checked: &TypeChecked,
        position: usize,
    ) -> Result<(usize, Vec<CompletionItem>), CompletionError> {
        let context = self.context(checked)?;
        let environment = self.checker.environment();
        let completion =
            TypeNameCompletion::new(environment, context.module_context.nesting.clone(), &[]);
        let text = &checked.source.buffer.source;
        let prefix = Prefix::parse(&text[..position]);
        let size = prefix.as_ref().map_or(0, Prefix::size);
        let range = SourceRange::new(position_before(text, position, size), position);
        let mut items = Vec::new();
        for name in completion
            .find_type_names(prefix.as_ref())
            .map_err(CompletionError::Runtime)?
        {
            if let Some((absolute, relative)) = completion.resolve_name_in_context(&name) {
                items.push(CompletionItem::TypeName {
                    absolute_type_name: absolute,
                    relative_type_name: relative,
                    range,
                });
            }
        }
        Ok((size, items))
    }

    fn items_for_following_keyword_arguments(
        &self,
        mut source_text: String,
        position: usize,
    ) -> Result<Vec<CompletionItem>, CompletionError> {
        let is_argument_character = |character: Option<char>| {
            character.is_some_and(|character| character.is_ascii_alphanumeric())
        };
        let mut items = Vec::new();
        let mut index = position;
        if !is_argument_character(character_before(&source_text, index)) {
            return Ok(items);
        }
        let mut argument_name = Vec::new();
        while is_argument_character(character_before(&source_text, index)) {
            argument_name.insert(0, character_before(&source_text, index).unwrap_or_default());
            blank_character_before(&mut source_text, index);
            match previous_character_position(&source_text, index) {
                Some(previous) => index = previous,
                None => break,
            }
        }
        let checked = match self.type_check(&source_text, position) {
            Ok(checked) => checked,
            Err(CompletionError::SyntaxError) => return Ok(items),
            Err(error) => return Err(error),
        };
        if let Some(nodes) = checked.source.find_nodes(position)
            && let Some((send_node, block_node)) = deconstruct_sendish_and_block_nodes(&nodes)
        {
            let prefix: String = argument_name.into_iter().collect();
            self.keyword_argument_items_for_method(
                &checked,
                &block_node.unwrap_or_else(|| send_node.clone()),
                &send_node,
                position,
                &prefix,
                &mut items,
            )?;
        }
        Ok(items)
    }

    fn config(&self, context: &Context) -> Config {
        Config {
            self_type: Some(context.self_type.clone()),
            class_type: Some(context.module_context.module_type.clone()),
            instance_type: Some(context.module_context.instance_type.clone()),
            variable_bounds: context.variable_context.upper_bounds(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn method_items_for_receiver_type(
        &self,
        checked: &TypeChecked,
        context: &Context,
        type_: &Type,
        include_private: bool,
        prefix: &str,
        position: usize,
        items: &mut Vec<CompletionItem>,
    ) -> Result<(), CompletionError> {
        let range = self.range_for(&checked.source.buffer.source, position, prefix);
        let builder = &self.checker.builder;
        let Some(shape) = builder
            .shape(type_, &self.config(context))
            .map_err(|error| CompletionError::Runtime(error.to_string()))?
        else {
            return Ok(());
        };
        let shape = if include_private {
            shape
        } else {
            shape.public_shape()
        };
        let entries = shape
            .methods(builder)
            .map_err(|error| CompletionError::Runtime(error.to_string()))?;
        for entry in entries {
            let name = &entry.method_name;
            if name == "initialize" || !name.starts_with(prefix) || !is_word_name(name) {
                continue;
            }
            match type_ {
                Type::NameInstance { .. } | Type::Interface { .. } | Type::Singleton(_) => {
                    let mut all_declarations = Vec::new();
                    for overload in &entry.overloads {
                        for declaration in method_declarations(name, overload) {
                            if !all_declarations.iter().any(
                                |existing: &crate::type_inference::method_call::MethodDeclaration| {
                                    existing.name() == declaration.name()
                                        && same_definition(&existing.definition, &declaration.definition)
                                },
                            ) {
                                all_declarations.push(declaration);
                            }
                        }
                    }
                    all_declarations.sort_by_key(|declaration| declaration.full_name());
                    let mut all_members: Vec<DefinitionMember> = Vec::new();
                    for declaration in &all_declarations {
                        if !all_members
                            .iter()
                            .any(|member| same_member(member, &declaration.definition.member))
                        {
                            all_members.push(declaration.definition.member.clone());
                        }
                    }
                    for member in all_members {
                        let Some(associated) = all_declarations.iter().find(|declaration| {
                            same_member(&declaration.definition.member, &member)
                        }) else {
                            continue;
                        };
                        let overloads: Vec<&Overload> = entry
                            .overloads
                            .iter()
                            .filter(|overload| {
                                overload
                                    .definitions
                                    .iter()
                                    .any(|definition| same_member(&definition.member, &member))
                            })
                            .collect();
                        let deprecated = annotations_helper::deprecated_annotation(
                            &associated.definition.member_annotations,
                        )
                        .is_some();
                        items.push(CompletionItem::SimpleMethodName {
                            identifier: name.clone(),
                            range,
                            receiver_type: type_.clone(),
                            method_types: overloads
                                .iter()
                                .map(|overload| method_type_1(&overload.method_type))
                                .collect(),
                            method_member: member,
                            method_name: associated.name(),
                            deprecated,
                        });
                    }
                }
                _ => {
                    let (generated, defined): (Vec<&Overload>, Vec<&Overload>) = entry
                        .overloads
                        .iter()
                        .partition(|overload| overload.definitions.is_empty());
                    if !defined.is_empty() {
                        let mut method_decls: Vec<_> = defined
                            .iter()
                            .flat_map(|overload| method_declarations(name, overload))
                            .collect();
                        method_decls.sort_by_key(|declaration| declaration.full_name());
                        items.push(CompletionItem::ComplexMethodName {
                            identifier: name.clone(),
                            range,
                            receiver_type: type_.clone(),
                            method_types: defined
                                .iter()
                                .map(|overload| method_type_1(&overload.method_type))
                                .collect(),
                            method_decls,
                        });
                    }
                    if !generated.is_empty() {
                        items.push(CompletionItem::GeneratedMethodName {
                            identifier: name.clone(),
                            range,
                            receiver_type: type_.clone(),
                            method_types: generated
                                .iter()
                                .map(|overload| method_type_1(&overload.method_type))
                                .collect(),
                        });
                    }
                }
            }
        }
        Ok(())
    }

    fn local_variable_items(
        &self,
        context: &Context,
        text: &str,
        position: usize,
        prefix: &str,
        items: &mut Vec<CompletionItem>,
    ) {
        let range = self.range_for(text, position, prefix);
        for (name, (type_, _)) in &context.type_env.local_variable_types {
            if name.starts_with(prefix) {
                items.push(CompletionItem::LocalVariable {
                    identifier: name.clone(),
                    range,
                    type_: type_.clone(),
                });
            }
        }
    }

    fn instance_variable_items(
        &self,
        context: &Context,
        text: &str,
        position: usize,
        prefix: &str,
        items: &mut Vec<CompletionItem>,
    ) {
        let range = self.range_for(text, position, prefix);
        for (name, type_) in context.type_env.instance_variable_types.iter() {
            if name.starts_with(prefix) {
                items.push(CompletionItem::InstanceVariable {
                    identifier: name.clone(),
                    range,
                    type_: type_.clone(),
                });
            }
        }
    }

    fn constant_items(
        &self,
        checked: &TypeChecked,
        context: &Context,
        parent: Option<&Rc<Node>>,
        position: usize,
        prefix: &str,
        items: &mut Vec<CompletionItem>,
    ) -> Result<(), CompletionError> {
        let range = self.range_for(&checked.source.buffer.source, position, prefix);
        let builder = &self.checker.builder;
        let constant_env = &context.type_env.constant_env;
        let constants = match parent {
            Some(parent) if parent.type_ == "const" => {
                let constant_name = checked
                    .typing
                    .source_index()
                    .reference(parent.id)
                    .cloned()
                    .ok_or_else(runtime_error)?;
                Some(constant_env.resolver.children(builder, &constant_name))
            }
            Some(_) => None,
            None => Some(
                constant_env
                    .resolver
                    .constants(builder, &constant_env.context)
                    .ok_or_else(runtime_error)?,
            ),
        };
        let environment = self.checker.environment();
        for (name, constant) in constants.into_iter().flatten() {
            if !name.starts_with(prefix) {
                continue;
            }
            let kind = match environment.constant_entry(&constant.name) {
                Some(ConstantEntry::Class(entry))
                    if entry.kind == rbs_definition::ast::ClassOrModule::Module =>
                {
                    ConstantKind::Module
                }
                Some(ConstantEntry::Class(_)) => ConstantKind::Class,
                Some(ConstantEntry::ClassAlias(entry))
                    if entry.declaration.kind == rbs_definition::ast::ClassOrModule::Module =>
                {
                    ConstantKind::Module
                }
                Some(ConstantEntry::ClassAlias(_)) => ConstantKind::Class,
                _ => ConstantKind::Constant,
            };
            let (declaration, comments) = match environment.constant_entry(&constant.name) {
                Some(entry) => {
                    let (declaration, comments) = constant_declaration(entry);
                    (Some(declaration), comments)
                }
                None => (None, Vec::new()),
            };
            items.push(CompletionItem::Constant {
                identifier: name,
                range,
                type_: constant.type_,
                full_name: constant.name.clone(),
                kind,
                declaration,
                comments,
                deprecated: annotations_helper::deprecated_type_name(&constant.name, environment)
                    .is_some(),
            });
        }
        Ok(())
    }

    fn keyword_argument_items_for_method(
        &self,
        checked: &TypeChecked,
        call_node: &Rc<Node>,
        send_node: &Rc<Node>,
        position: usize,
        prefix: &str,
        items: &mut Vec<CompletionItem>,
    ) -> Result<(), CompletionError> {
        if !matches!(send_node.type_, "send" | "csend") {
            return Err(runtime_error());
        }
        let argument_nodes: Vec<&Rc<Node>> = send_node.children[2..]
            .iter()
            .filter_map(Child::node)
            .collect();
        let call = checked
            .typing
            .call_of(call_node.id)
            .ok_or_else(unknown_node)?;
        if !matches!(
            call.kind,
            MethodCallKind::Typed { .. }
                | MethodCallKind::Special { .. }
                | MethodCallKind::Error { .. }
        ) {
            return Ok(());
        }
        let context = self.context(checked)?;
        let substitution = Substitution::build(
            &[],
            &[],
            Some(context.module_context.instance_type.clone()),
            Some(context.module_context.module_type.clone()),
            Some(context.self_type.clone()),
        );
        let type_ = call.receiver_type.substitute(&substitution);
        let config = Config {
            self_type: Some(type_.clone()),
            class_type: None,
            instance_type: None,
            variable_bounds: context.variable_context.upper_bounds(),
        };
        let builder = &self.checker.builder;
        let Some(shape) = builder
            .shape(&type_, &config)
            .map_err(|error| CompletionError::Runtime(error.to_string()))?
        else {
            return Ok(());
        };
        let shape = if private_send(call_node)? {
            shape.public_shape()
        } else {
            shape
        };
        let Some(method) = shape
            .method(builder, &call.method_name)
            .map_err(|error| CompletionError::Runtime(error.to_string()))?
        else {
            return Ok(());
        };
        let range = self.range_for(&checked.source.buffer.source, position, prefix);
        let used_keywords: Vec<String> = argument_nodes
            .iter()
            .find(|argument| argument.type_ == "kwargs")
            .map(|kwargs| {
                kwargs
                    .child_nodes()
                    .filter(|argument| argument.type_ == "pair")
                    .filter_map(|pair| {
                        pair.child_node(0)
                            .and_then(|key| key.child_symbol(0).map(str::to_string))
                    })
                    .collect()
            })
            .unwrap_or_default();
        for overload in &method.overloads {
            let Some(declaration) = method_declarations(&call.method_name, overload)
                .into_iter()
                .next()
            else {
                continue;
            };
            let keywords: Vec<String> = match &declaration.definition.type_.type_ {
                Callable::Untyped { .. } => Vec::new(),
                Callable::Function(function) => function
                    .required_keywords
                    .iter()
                    .chain(&function.optional_keywords)
                    .map(|(name, _)| name.clone())
                    .collect(),
            };
            for name in keywords {
                if name.starts_with(prefix) && !used_keywords.contains(&name) {
                    items.push(CompletionItem::KeywordArgument {
                        identifier: format!("{name}:"),
                        range,
                    });
                }
            }
        }
        Ok(())
    }
}
