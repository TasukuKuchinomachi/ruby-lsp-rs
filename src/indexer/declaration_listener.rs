use std::collections::HashMap;
use std::sync::Arc;

use ruby_prism::{Node, ParseResult, Visit};

use super::comments::{CommentLine, collect_comments, comments_by_line};
use super::entry::{
    BLOCK_PARAMETER_DEFAULT_NAME, Entry, EntryId, EntryKind, KEYWORD_REST_PARAMETER_DEFAULT_NAME, ModuleOperation,
    Namespace, NamespaceKind, Parameter, REST_PARAMETER_DEFAULT_NAME, Signature, Visibility,
};
use super::index::{Index, actual_nesting};
use crate::position::{LineIndex, Location};
use crate::prism::{bytes_to_string, constant_full_name, constant_id_name, location_slice, node_slice};

#[derive(Debug, Clone, Copy)]
struct VisibilityScope {
    visibility: Visibility,
    module_function: bool,
}

impl VisibilityScope {
    fn public_scope() -> Self {
        VisibilityScope { visibility: Visibility::Public, module_function: false }
    }

    fn module_function_scope() -> Self {
        VisibilityScope { visibility: Visibility::Private, module_function: true }
    }

    fn with_visibility(visibility: Visibility) -> Self {
        VisibilityScope { visibility, module_function: false }
    }
}

/// Mirrors `RubyIndexer::DeclarationListener`, populating the index with the declarations found in a parsed document
pub struct DeclarationListener<'index, 'source> {
    index: &'index mut Index,
    uri: Arc<str>,
    line_index: LineIndex<'source>,
    comments_by_line: HashMap<usize, CommentLine>,
    source_lines: Vec<&'source str>,
    visibility_stack: Vec<VisibilityScope>,
    inside_def: bool,
    stack: Vec<String>,
    owner_stack: Vec<EntryId>,
    collect_comments: bool,
}

impl<'index, 'source> DeclarationListener<'index, 'source> {
    pub fn new(
        index: &'index mut Index,
        parse_result: &ParseResult<'source>,
        source: &'source [u8],
        uri: Arc<str>,
        collect_comments: bool,
    ) -> Self {
        let line_index = LineIndex::new(source, super::index::indexer_column_encoding(index.configuration.encoding));
        let comments_by_line = if collect_comments { comments_by_line(parse_result, &line_index) } else { HashMap::new() };
        let source_lines = if collect_comments { split_lines(source) } else { Vec::new() };
        DeclarationListener {
            index,
            uri,
            line_index,
            comments_by_line,
            source_lines,
            visibility_stack: vec![VisibilityScope::public_scope()],
            inside_def: false,
            stack: Vec::new(),
            owner_stack: Vec::new(),
            collect_comments,
        }
    }

    fn location(&self, location: &ruby_prism::Location<'_>) -> Location {
        self.line_index.location_of(location.start_offset(), location.end_offset())
    }

    fn start_line(&self, node: &Node<'_>) -> usize {
        self.line_index.line_of_offset(node.location().start_offset()) + 1
    }

    fn comments(&self, node: &Node<'_>) -> Option<String> {
        if !self.collect_comments {
            return None;
        }
        Some(collect_comments(&self.comments_by_line, &self.source_lines, self.start_line(node), &self.index.configuration))
    }

    fn current_owner(&self) -> Option<EntryId> {
        self.owner_stack.last().copied()
    }

    fn current_owner_name(&self) -> Option<String> {
        self.current_owner().map(|id| self.index.entry(id).name.clone())
    }

    fn current_visibility_scope(&self) -> VisibilityScope {
        self.visibility_stack.last().copied().unwrap_or_else(VisibilityScope::public_scope)
    }

    fn pop_namespace_stack(&mut self) {
        self.stack.pop();
        self.owner_stack.pop();
        self.visibility_stack.pop();
    }

    fn advance_namespace_stack(&mut self, short_name: String, entry: Entry) {
        self.visibility_stack.push(VisibilityScope::public_scope());
        let id = self.index.add(entry, false);
        self.owner_stack.push(id);
        self.stack.push(short_name);
    }

    fn last_name_in_stack(&self) -> Option<String> {
        let name = self.stack.last()?;
        Some(name.split("::").filter(|part| !part.is_empty()).last().unwrap_or("").to_string())
    }

    fn fully_qualify_name(&self, name: &str) -> String {
        let qualified =
            if self.stack.is_empty() || name.starts_with("::") { name.to_string() } else { format!("{}::{name}", self.stack.join("::")) };
        qualified.strip_prefix("::").map(str::to_string).unwrap_or(qualified)
    }

    fn add_namespace_entry(
        &self,
        nesting: Vec<String>,
        full_location: &ruby_prism::Location<'_>,
        name_location: &ruby_prism::Location<'_>,
        comments: Option<String>,
        kind: NamespaceKind,
        parent_class: Option<String>,
    ) -> Entry {
        Entry::new(
            nesting.join("::"),
            self.uri.clone(),
            self.location(full_location),
            comments,
            EntryKind::Namespace(Namespace {
                kind,
                nesting,
                name_location: self.location(name_location),
                mixin_operations: Vec::new(),
                parent_class,
            }),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn add_method_entry(
        &mut self,
        name: String,
        location: Location,
        name_location: Location,
        comments: Option<String>,
        signatures: Vec<Signature>,
        visibility: Visibility,
        owner: Option<String>,
    ) -> EntryId {
        let mut entry = Entry::new(
            name,
            self.uri.clone(),
            location,
            comments,
            EntryKind::Method { owner, signatures, name_location },
        );
        entry.visibility = visibility;
        self.index.add(entry, false)
    }

    fn add_constant(&mut self, node: &Node<'_>, name: String, value: Option<Node<'_>>) {
        let comments = self.comments(node);
        let location = self.location(&node.location());
        let alias_target = value.as_ref().and_then(|value| {
            if value.as_constant_read_node().is_some() || value.as_constant_path_node().is_some() {
                return Some(node_slice(value));
            }
            if let Some(write) = value.as_constant_write_node() {
                return Some(constant_id_name(&write.name()));
            }
            if let Some(write) = value.as_constant_and_write_node() {
                return Some(constant_id_name(&write.name()));
            }
            if let Some(write) = value.as_constant_or_write_node() {
                return Some(constant_id_name(&write.name()));
            }
            if let Some(write) = value.as_constant_operator_write_node() {
                return Some(constant_id_name(&write.name()));
            }
            if let Some(write) = value.as_constant_path_write_node() {
                return Some(location_slice(&write.target().location()));
            }
            if let Some(write) = value.as_constant_path_or_write_node() {
                return Some(location_slice(&write.target().location()));
            }
            if let Some(write) = value.as_constant_path_operator_write_node() {
                return Some(location_slice(&write.target().location()));
            }
            if let Some(write) = value.as_constant_path_and_write_node() {
                return Some(location_slice(&write.target().location()));
            }
            None
        });

        let kind = match alias_target {
            Some(target) => EntryKind::UnresolvedConstantAlias { target, nesting: self.stack.clone() },
            None => EntryKind::Constant,
        };
        self.index.add(Entry::new(name, self.uri.clone(), location, comments, kind), false);
    }

    fn handle_constant_path_write(&mut self, node: &Node<'_>, target: &ruby_prism::ConstantPathNode<'_>, value: Node<'_>) {
        let ignored = target.parent().is_some_and(|parent| parent.as_constant_read_node().is_none());
        if ignored {
            return;
        }
        let name = self.fully_qualify_name(&location_slice(&target.location()));
        self.add_constant(node, name, Some(value));
    }

    fn handle_global_variable(&mut self, node: &Node<'_>, name: String, location: &ruby_prism::Location<'_>) {
        let comments = self.comments(node);
        let location = self.location(location);
        self.index.add(Entry::new(name, self.uri.clone(), location, comments, EntryKind::GlobalVariable), false);
    }

    fn handle_class_variable(&mut self, node: &Node<'_>, name: String, location: &ruby_prism::Location<'_>) {
        if name == "@@" {
            return;
        }
        let comments = self.comments(node);
        let mut owner = self.current_owner();
        if owner.is_some_and(|id| self.index.entry(id).is_singleton_class()) {
            owner = self.owner_stack.iter().rev().copied().find(|id| !self.index.entry(*id).name.contains("<Class:"));
        }
        let owner = owner.map(|id| self.index.entry(id).name.clone());
        let location = self.location(location);
        self.index.add(Entry::new(name, self.uri.clone(), location, comments, EntryKind::ClassVariable { owner }), false);
    }

    fn handle_instance_variable(&mut self, node: &Node<'_>, name: String, location: &ruby_prism::Location<'_>) {
        if name == "@" {
            return;
        }
        let mut owner = self.current_owner();
        if let Some(id) = owner
            && !self.inside_def
        {
            let owner_name = self.index.entry(id).name.clone();
            owner = Some(self.index.existing_or_new_singleton_class(&owner_name));
        }
        let owner = owner.map(|id| self.index.entry(id).name.clone());
        let location = self.location(location);
        let comments = self.comments(node);
        self.index.add(Entry::new(name, self.uri.clone(), location, comments, EntryKind::InstanceVariable { owner }), false);
    }

    fn handle_private_constant(&mut self, call: &ruby_prism::CallNode<'_>) {
        let Some(arguments) = call.arguments() else { return };
        let Some(first_argument) = arguments.arguments().iter().next() else { return };
        let Some(mut name) = string_or_symbol_value(&first_argument) else { return };
        if let Some(receiver) = call.receiver() {
            name = format!("{}::{name}", node_slice(&receiver));
        }
        let fully_qualified_name = self.fully_qualify_name(&name);
        if let Some(ids) = self.index.get_owned(&fully_qualified_name) {
            for id in ids {
                self.index.entry_mut(id).visibility = Visibility::Private;
            }
        }
    }

    fn handle_alias_method(&mut self, call: &ruby_prism::CallNode<'_>) {
        let Some(arguments) = call.arguments() else { return };
        let mut iterator = arguments.arguments().iter();
        let (Some(new_name), Some(old_name)) = (iterator.next(), iterator.next()) else { return };
        let Some(new_name_value) = string_or_symbol_value(&new_name) else { return };
        let Some(old_name_value) = string_or_symbol_value(&old_name) else { return };
        let comments = self.comments(&call.as_node());
        let owner = self.current_owner_name();
        let location = self.location(&new_name.location());
        self.index.add(
            Entry::new(
                new_name_value.clone(),
                self.uri.clone(),
                location,
                comments,
                EntryKind::UnresolvedMethodAlias { new_name: new_name_value, old_name: old_name_value, owner },
            ),
            false,
        );
    }

    fn handle_attribute(&mut self, call: &ruby_prism::CallNode<'_>, reader: bool, writer: bool) {
        let Some(arguments) = call.arguments() else { return };
        if call.receiver().is_some_and(|receiver| receiver.as_self_node().is_none()) {
            return;
        }
        let comments = self.comments(&call.as_node());
        let scope = self.current_visibility_scope();
        let owner = self.current_owner_name();

        for argument in arguments.arguments().iter() {
            let name_and_location = if let Some(symbol) = argument.as_symbol_node() {
                symbol.value_loc().map(|location| (location_slice(&location), location))
            } else {
                argument.as_string_node().map(|string| (location_slice(&string.content_loc()), string.content_loc()))
            };
            let Some((name, location)) = name_and_location else { continue };
            let location = self.location(&location);

            if reader {
                let mut entry = Entry::new(
                    name.clone(),
                    self.uri.clone(),
                    location,
                    comments.clone(),
                    EntryKind::Accessor { owner: owner.clone() },
                );
                entry.visibility = scope.visibility;
                self.index.add(entry, false);
            }

            if !writer {
                continue;
            }

            let mut entry = Entry::new(
                format!("{name}="),
                self.uri.clone(),
                location,
                comments.clone(),
                EntryKind::Accessor { owner: owner.clone() },
            );
            entry.visibility = scope.visibility;
            self.index.add(entry, false);
        }
    }

    fn handle_module_operation(&mut self, call: &ruby_prism::CallNode<'_>, operation: &str) {
        if self.inside_def {
            return;
        }
        let Some(owner) = self.current_owner() else { return };
        let Some(arguments) = call.arguments() else { return };
        let owner_name = self.index.entry(owner).name.clone();

        for argument in arguments.arguments().iter() {
            let is_constant = argument.as_constant_read_node().is_some() || argument.as_constant_path_node().is_some();
            let is_self = argument.as_self_node().is_some();
            if !(is_constant || (is_self && operation == "extend")) {
                continue;
            }

            if is_self {
                let singleton = self.index.existing_or_new_singleton_class(&owner_name);
                push_mixin(self.index, singleton, ModuleOperation::Include(owner_name.clone()));
                continue;
            }

            let Ok(full_name) = constant_full_name(&argument) else { continue };
            match operation {
                "include" => push_mixin(self.index, owner, ModuleOperation::Include(full_name)),
                "prepend" => push_mixin(self.index, owner, ModuleOperation::Prepend(full_name)),
                _ => {
                    let singleton = self.index.existing_or_new_singleton_class(&owner_name);
                    push_mixin(self.index, singleton, ModuleOperation::Include(full_name));
                }
            }
        }
    }

    fn handle_module_function(&mut self, call: &ruby_prism::CallNode<'_>) {
        let Some(owner) = self.current_owner() else { return };
        if !self.index.entry(owner).is_module() {
            return;
        }

        let Some(arguments) = call.arguments() else {
            self.visibility_stack.push(VisibilityScope::module_function_scope());
            return;
        };

        let owner_name = self.index.entry(owner).name.clone();
        for argument in arguments.arguments().iter() {
            let Some(method_name) = string_or_symbol_value(&argument) else { continue };
            let Some(entries) = self.index.resolve_method(&method_name, &owner_name, false) else { continue };

            for entry_id in entries {
                let Some(entry_owner_name) = self.index.entry(entry_id).owner().map(str::to_string) else { continue };
                self.index.entry_mut(entry_id).visibility = Visibility::Private;

                let singleton = self.index.existing_or_new_singleton_class(&entry_owner_name);
                let singleton_name = self.index.entry(singleton).name.clone();
                let location = self.location(&argument.location());
                let comments = match self.comments(&call.as_node()) {
                    Some(mut comments) => {
                        comments.push_str(&self.index.comments_for(entry_id));
                        Some(comments)
                    }
                    None => None,
                };
                let signatures = self.index.entry(entry_id).signatures();
                self.add_method_entry(
                    method_name.clone(),
                    location,
                    location,
                    comments,
                    signatures,
                    Visibility::Public,
                    Some(singleton_name),
                );
            }
        }
    }

    fn handle_private_class_method(&mut self, call: &ruby_prism::CallNode<'_>) {
        let Some(arguments) = call.arguments() else { return };
        let arguments: Vec<Node<'_>> = arguments.arguments().iter().collect();

        if arguments.first().is_some_and(|argument| argument.as_def_node().is_some()) {
            self.visibility_stack.push(VisibilityScope::with_visibility(Visibility::Private));
            return;
        }

        let Some(owner_name) = self.current_owner_name() else { return };

        let mut others: Vec<Node<'_>> = Vec::new();
        let mut arrays: Vec<Node<'_>> = Vec::new();
        for argument in arguments {
            if argument.as_array_node().is_some() {
                arrays.push(argument);
            } else {
                others.push(argument);
            }
        }
        for array in &arrays {
            if let Some(array) = array.as_array_node() {
                others.extend(array.elements().iter());
            }
        }

        let names: Vec<String> = others
            .iter()
            .filter_map(|argument| {
                if let Some(string) = argument.as_string_node() {
                    Some(bytes_to_string(string.unescaped()))
                } else {
                    argument.as_symbol_node().and_then(|symbol| symbol.value_loc().map(|location| location_slice(&location)))
                }
            })
            .collect();

        for name in names {
            let singleton = self.index.existing_or_new_singleton_class(&owner_name);
            let singleton_name = self.index.entry(singleton).name.clone();
            let Some(entries) = self.index.resolve_method(&name, &singleton_name, false) else { continue };
            for id in entries {
                self.index.entry_mut(id).visibility = Visibility::Private;
            }
        }
    }

    fn handle_visibility_change(&mut self, call: &ruby_prism::CallNode<'_>, visibility: Visibility) {
        let Some(owner_name) = self.current_owner_name() else { return };
        let method_names: Vec<String> = match call.arguments() {
            Some(arguments) => arguments.arguments().iter().filter_map(|argument| string_or_symbol_value(&argument)).collect(),
            None => Vec::new(),
        };

        if method_names.is_empty() {
            self.visibility_stack.push(VisibilityScope::with_visibility(visibility));
            return;
        }

        for method_name in method_names {
            let Some(entries) = self.index.resolve_method(&method_name, &owner_name, false) else { continue };
            for id in entries {
                self.index.entry_mut(id).visibility = visibility;
            }
        }
    }

    fn on_call_node_enter(&mut self, call: &ruby_prism::CallNode<'_>) {
        let message = constant_id_name(&call.name());
        match message.as_str() {
            "private_constant" => self.handle_private_constant(call),
            "attr_reader" => self.handle_attribute(call, true, false),
            "attr_writer" => self.handle_attribute(call, false, true),
            "attr_accessor" => self.handle_attribute(call, true, true),
            "attr" => {
                let has_writer = call
                    .arguments()
                    .and_then(|arguments| arguments.arguments().last())
                    .is_some_and(|argument| argument.as_true_node().is_some());
                self.handle_attribute(call, true, has_writer);
            }
            "alias_method" => self.handle_alias_method(call),
            "include" | "prepend" | "extend" => self.handle_module_operation(call, &message),
            "public" => self.handle_visibility_change(call, Visibility::Public),
            "protected" => self.handle_visibility_change(call, Visibility::Protected),
            "private" => self.handle_visibility_change(call, Visibility::Private),
            "module_function" => self.handle_module_function(call),
            "private_class_method" => self.handle_private_class_method(call),
            _ => {}
        }
    }

    fn on_call_node_leave(&mut self, call: &ruby_prism::CallNode<'_>) {
        let message = constant_id_name(&call.name());
        if matches!(message.as_str(), "public" | "protected" | "private" | "private_class_method")
            && call
                .arguments()
                .and_then(|arguments| arguments.arguments().iter().next())
                .is_some_and(|argument| argument.as_def_node().is_some())
        {
            self.visibility_stack.pop();
        }
    }

    fn on_def_node_enter(&mut self, def: &ruby_prism::DefNode<'_>) {
        let Some(owner) = self.current_owner() else { return };
        self.inside_def = true;
        let method_name = constant_id_name(&def.name());
        let comments = self.comments(&def.as_node());
        let scope = self.current_visibility_scope();
        let owner_name = self.index.entry(owner).name.clone();
        let location = self.location(&def.location());
        let name_location = self.location(&def.name_loc());

        match def.receiver() {
            None => {
                let signatures = vec![Signature::new(list_parameters(def.parameters()))];
                self.add_method_entry(
                    method_name.clone(),
                    location,
                    name_location,
                    comments.clone(),
                    signatures.clone(),
                    scope.visibility,
                    Some(owner_name.clone()),
                );

                if scope.module_function {
                    let singleton = self.index.existing_or_new_singleton_class(&owner_name);
                    let singleton_name = self.index.entry(singleton).name.clone();
                    self.add_method_entry(
                        method_name,
                        location,
                        name_location,
                        comments,
                        signatures,
                        Visibility::Public,
                        Some(singleton_name),
                    );
                }
            }
            Some(receiver) if receiver.as_self_node().is_some() => {
                let singleton = self.index.existing_or_new_singleton_class(&owner_name);
                let singleton_name = self.index.entry(singleton).name.clone();
                self.add_method_entry(
                    method_name,
                    location,
                    name_location,
                    comments,
                    vec![Signature::new(list_parameters(def.parameters()))],
                    scope.visibility,
                    Some(singleton_name),
                );
                self.owner_stack.push(singleton);
            }
            Some(_) => {}
        }
    }

    fn on_def_node_leave(&mut self, def: &ruby_prism::DefNode<'_>) {
        self.inside_def = false;
        if def.receiver().is_some_and(|receiver| receiver.as_self_node().is_some()) {
            self.owner_stack.pop();
        }
    }

    fn on_singleton_class_node_enter(&mut self, node: &ruby_prism::SingletonClassNode<'_>) {
        self.visibility_stack.push(VisibilityScope::public_scope());

        if self.current_owner().is_none() {
            return;
        }

        let expression = node.expression();
        let name = if expression.as_self_node().is_some() {
            format!("<Class:{}>", self.last_name_in_stack().unwrap_or_default())
        } else {
            format!("<Class:{}>", node_slice(&expression))
        };
        let real_nesting = actual_nesting(&self.stack, Some(&name));
        let comments = self.comments(&node.as_node());
        let location = self.location(&node.location());
        let name_location = self.location(&expression.location());

        let existing = self.index.get(&real_nesting.join("::")).and_then(|ids| ids.first().copied());
        let id = match existing {
            Some(id) => {
                let entry = self.index.entry_mut(id);
                entry.location = location;
                if let Some(namespace) = entry.namespace_mut() {
                    namespace.name_location = name_location;
                }
                if let Some(comments) = comments {
                    entry.comments.get_or_insert_with(String::new).push_str(&comments);
                }
                id
            }
            None => {
                let entry = Entry::new(
                    real_nesting.join("::"),
                    self.uri.clone(),
                    location,
                    comments,
                    EntryKind::Namespace(Namespace {
                        kind: NamespaceKind::SingletonClass,
                        nesting: real_nesting,
                        name_location,
                        mixin_operations: Vec::new(),
                        parent_class: None,
                    }),
                );
                self.index.add(entry, true)
            }
        };

        self.owner_stack.push(id);
        self.stack.push(name);
    }

    fn on_multi_write_node_enter(&mut self, node: &ruby_prism::MultiWriteNode<'_>) {
        let value = node.value();
        let mut values: Vec<Node<'_>> = match value.as_array_node() {
            Some(array) if array.opening_loc().is_some() => array.elements().iter().collect(),
            _ => Vec::new(),
        };

        let mut targets: Vec<Node<'_>> = node.lefts().iter().collect();
        targets.extend(node.rest());
        targets.extend(node.rights().iter());

        for (index, target) in targets.iter().enumerate() {
            let current_value = values.get(index).map(crate::prism::NodeExt::clone_node);
            if current_value.as_ref().is_some_and(|value| value.as_splat_node().is_some()) {
                values.clear();
            }

            if let Some(constant) = target.as_constant_target_node() {
                let name = self.fully_qualify_name(&constant_id_name(&constant.name()));
                self.add_constant(target, name, current_value);
            } else if target.as_constant_path_target_node().is_some() {
                let name = self.fully_qualify_name(&node_slice(target));
                self.add_constant(target, name, current_value);
            }
        }
    }
}

fn push_mixin(index: &mut Index, id: EntryId, operation: ModuleOperation) {
    if let Some(namespace) = index.entry_mut(id).namespace_mut() {
        namespace.mixin_operations.push(operation);
    }
}

fn split_lines(source: &[u8]) -> Vec<&str> {
    let text = std::str::from_utf8(source).unwrap_or("");
    text.split_inclusive('\n').collect()
}

fn string_or_symbol_value(node: &Node<'_>) -> Option<String> {
    if let Some(string) = node.as_string_node() {
        return Some(location_slice(&string.content_loc()));
    }
    let symbol = node.as_symbol_node()?;
    symbol.value_loc().map(|location| location_slice(&location))
}

fn parameter_name(node: &Node<'_>) -> Option<String> {
    if let Some(parameter) = node.as_required_parameter_node() {
        return Some(constant_id_name(&parameter.name()));
    }
    if let Some(parameter) = node.as_optional_parameter_node() {
        return Some(constant_id_name(&parameter.name()));
    }
    if let Some(parameter) = node.as_required_keyword_parameter_node() {
        return Some(constant_id_name(&parameter.name()));
    }
    if let Some(parameter) = node.as_optional_keyword_parameter_node() {
        return Some(constant_id_name(&parameter.name()));
    }
    if let Some(parameter) = node.as_rest_parameter_node() {
        return parameter.name().map(|name| constant_id_name(&name));
    }
    if let Some(parameter) = node.as_keyword_rest_parameter_node() {
        return parameter.name().map(|name| constant_id_name(&name));
    }
    let multi_target = node.as_multi_target_node()?;
    let mut names: Vec<Option<String>> = multi_target.lefts().iter().map(|left| parameter_name(&left)).collect();

    if let Some(rest) = multi_target.rest() {
        if let Some(splat) = rest.as_splat_node() {
            let name = splat.expression().map(|expression| node_slice(&expression));
            let operator = location_slice(&splat.operator_loc());
            names.push(if operator == "*" { Some(format!("*{}", name.unwrap_or_default())) } else { name });
        }
        if rest.as_implicit_rest_node().is_some() {
            names.push(None);
        }
    }

    names.extend(multi_target.rights().iter().map(|right| parameter_name(&right)));
    let joined = names.into_iter().map(Option::unwrap_or_default).collect::<Vec<_>>().join(", ");
    Some(format!("({joined})"))
}

fn list_parameters(parameters: Option<ruby_prism::ParametersNode<'_>>) -> Vec<Parameter> {
    let Some(parameters_node) = parameters else { return Vec::new() };
    let mut parameters = Vec::new();

    for required in parameters_node.requireds().iter() {
        if let Some(name) = parameter_name(&required) {
            parameters.push(Parameter::Required(name));
        }
    }

    for optional in parameters_node.optionals().iter() {
        if let Some(name) = parameter_name(&optional) {
            parameters.push(Parameter::Optional(name));
        }
    }

    if let Some(rest) = parameters_node.rest()
        && let Some(rest) = rest.as_rest_parameter_node()
    {
        let name = rest.name().map(|name| constant_id_name(&name)).unwrap_or_else(|| REST_PARAMETER_DEFAULT_NAME.to_string());
        parameters.push(Parameter::Rest(name));
    }

    for keyword in parameters_node.keywords().iter() {
        let Some(name) = parameter_name(&keyword) else { continue };
        if keyword.as_required_keyword_parameter_node().is_some() {
            parameters.push(Parameter::Keyword(name));
        } else if keyword.as_optional_keyword_parameter_node().is_some() {
            parameters.push(Parameter::OptionalKeyword(name));
        }
    }

    if let Some(keyword_rest) = parameters_node.keyword_rest() {
        if keyword_rest.as_keyword_rest_parameter_node().is_some() {
            let name = parameter_name(&keyword_rest).unwrap_or_else(|| KEYWORD_REST_PARAMETER_DEFAULT_NAME.to_string());
            parameters.push(Parameter::KeywordRest(name));
        } else if keyword_rest.as_forwarding_parameter_node().is_some() {
            parameters.push(Parameter::Forwarding);
        }
    }

    for post in parameters_node.posts().iter() {
        if let Some(name) = parameter_name(&post) {
            parameters.push(Parameter::Required(name));
        }
    }

    if let Some(block) = parameters_node.block() {
        let name = block.name().map(|name| constant_id_name(&name)).unwrap_or_else(|| BLOCK_PARAMETER_DEFAULT_NAME.to_string());
        parameters.push(Parameter::Block(name));
    }

    parameters
}

macro_rules! constant_write_visitors {
    ($($method:ident, $default:ident, $node:ident;)*) => {
        $(
            fn $method(&mut self, node: &ruby_prism::$node<'pr>) {
                let name = self.fully_qualify_name(&constant_id_name(&node.name()));
                self.add_constant(&node.as_node(), name, Some(node.value()));
                ruby_prism::$default(self, node);
            }
        )*
    };
}

macro_rules! constant_path_write_visitors {
    ($($method:ident, $default:ident, $node:ident;)*) => {
        $(
            fn $method(&mut self, node: &ruby_prism::$node<'pr>) {
                self.handle_constant_path_write(&node.as_node(), &node.target(), node.value());
                ruby_prism::$default(self, node);
            }
        )*
    };
}

macro_rules! variable_write_visitors {
    ($handler:ident; $($method:ident, $default:ident, $node:ident, $location:ident;)*) => {
        $(
            fn $method(&mut self, node: &ruby_prism::$node<'pr>) {
                let name = constant_id_name(&node.name());
                self.$handler(&node.as_node(), name, &node.$location());
                ruby_prism::$default(self, node);
            }
        )*
    };
}

impl<'pr> Visit<'pr> for DeclarationListener<'_, '_> {
    fn visit_class_node(&mut self, node: &ruby_prism::ClassNode<'pr>) {
        let constant_path = node.constant_path();
        let nesting = actual_nesting(&self.stack, Some(&node_slice(&constant_path)));
        let parent_class = match node.superclass() {
            Some(superclass) if superclass.as_constant_read_node().is_some() || superclass.as_constant_path_node().is_some() => {
                Some(node_slice(&superclass))
            }
            _ => {
                if nesting == ["Object"] {
                    Some("::BasicObject".to_string())
                } else if nesting == ["BasicObject"] {
                    None
                } else {
                    Some("::Object".to_string())
                }
            }
        };
        let comments = self.comments(&node.as_node());
        let short_name = nesting.last().cloned().unwrap_or_default();
        let entry = self.add_namespace_entry(
            nesting,
            &node.location(),
            &constant_path.location(),
            comments,
            NamespaceKind::Class,
            parent_class,
        );
        self.advance_namespace_stack(short_name, entry);

        ruby_prism::visit_class_node(self, node);

        self.pop_namespace_stack();
    }

    fn visit_module_node(&mut self, node: &ruby_prism::ModuleNode<'pr>) {
        let constant_path = node.constant_path();
        let name = node_slice(&constant_path);
        let comments = self.comments(&node.as_node());
        let nesting = actual_nesting(&self.stack, Some(&name));
        let entry = self.add_namespace_entry(
            nesting,
            &node.location(),
            &constant_path.location(),
            comments,
            NamespaceKind::Module,
            None,
        );
        self.advance_namespace_stack(name, entry);

        ruby_prism::visit_module_node(self, node);

        self.pop_namespace_stack();
    }

    fn visit_singleton_class_node(&mut self, node: &ruby_prism::SingletonClassNode<'pr>) {
        self.on_singleton_class_node_enter(node);
        ruby_prism::visit_singleton_class_node(self, node);
        self.pop_namespace_stack();
    }

    fn visit_def_node(&mut self, node: &ruby_prism::DefNode<'pr>) {
        self.on_def_node_enter(node);
        ruby_prism::visit_def_node(self, node);
        self.on_def_node_leave(node);
    }

    fn visit_call_node(&mut self, node: &ruby_prism::CallNode<'pr>) {
        self.on_call_node_enter(node);
        ruby_prism::visit_call_node(self, node);
        self.on_call_node_leave(node);
    }

    fn visit_multi_write_node(&mut self, node: &ruby_prism::MultiWriteNode<'pr>) {
        self.on_multi_write_node_enter(node);
        ruby_prism::visit_multi_write_node(self, node);
    }

    fn visit_alias_method_node(&mut self, node: &ruby_prism::AliasMethodNode<'pr>) {
        let new_name = node.new_name();
        let method_name = node_slice(&new_name);
        let comments = self.comments(&node.as_node());
        let owner = self.current_owner_name();
        let location = self.location(&new_name.location());
        self.index.add(
            Entry::new(
                method_name.clone(),
                self.uri.clone(),
                location,
                comments,
                EntryKind::UnresolvedMethodAlias { new_name: method_name, old_name: node_slice(&node.old_name()), owner },
            ),
            false,
        );
        ruby_prism::visit_alias_method_node(self, node);
    }

    constant_write_visitors! {
        visit_constant_write_node, visit_constant_write_node, ConstantWriteNode;
        visit_constant_or_write_node, visit_constant_or_write_node, ConstantOrWriteNode;
        visit_constant_and_write_node, visit_constant_and_write_node, ConstantAndWriteNode;
        visit_constant_operator_write_node, visit_constant_operator_write_node, ConstantOperatorWriteNode;
    }

    constant_path_write_visitors! {
        visit_constant_path_write_node, visit_constant_path_write_node, ConstantPathWriteNode;
        visit_constant_path_or_write_node, visit_constant_path_or_write_node, ConstantPathOrWriteNode;
        visit_constant_path_operator_write_node, visit_constant_path_operator_write_node, ConstantPathOperatorWriteNode;
        visit_constant_path_and_write_node, visit_constant_path_and_write_node, ConstantPathAndWriteNode;
    }

    variable_write_visitors! {
        handle_global_variable;
        visit_global_variable_and_write_node, visit_global_variable_and_write_node, GlobalVariableAndWriteNode, name_loc;
        visit_global_variable_operator_write_node, visit_global_variable_operator_write_node, GlobalVariableOperatorWriteNode, name_loc;
        visit_global_variable_or_write_node, visit_global_variable_or_write_node, GlobalVariableOrWriteNode, name_loc;
        visit_global_variable_write_node, visit_global_variable_write_node, GlobalVariableWriteNode, name_loc;
    }

    variable_write_visitors! {
        handle_instance_variable;
        visit_instance_variable_write_node, visit_instance_variable_write_node, InstanceVariableWriteNode, name_loc;
        visit_instance_variable_and_write_node, visit_instance_variable_and_write_node, InstanceVariableAndWriteNode, name_loc;
        visit_instance_variable_operator_write_node, visit_instance_variable_operator_write_node, InstanceVariableOperatorWriteNode, name_loc;
        visit_instance_variable_or_write_node, visit_instance_variable_or_write_node, InstanceVariableOrWriteNode, name_loc;
    }

    variable_write_visitors! {
        handle_class_variable;
        visit_class_variable_and_write_node, visit_class_variable_and_write_node, ClassVariableAndWriteNode, name_loc;
        visit_class_variable_operator_write_node, visit_class_variable_operator_write_node, ClassVariableOperatorWriteNode, name_loc;
        visit_class_variable_or_write_node, visit_class_variable_or_write_node, ClassVariableOrWriteNode, name_loc;
        visit_class_variable_write_node, visit_class_variable_write_node, ClassVariableWriteNode, name_loc;
    }

    fn visit_global_variable_target_node(&mut self, node: &ruby_prism::GlobalVariableTargetNode<'pr>) {
        let name = constant_id_name(&node.name());
        self.handle_global_variable(&node.as_node(), name, &node.location());
    }

    fn visit_instance_variable_target_node(&mut self, node: &ruby_prism::InstanceVariableTargetNode<'pr>) {
        let name = constant_id_name(&node.name());
        self.handle_instance_variable(&node.as_node(), name, &node.location());
    }

    fn visit_class_variable_target_node(&mut self, node: &ruby_prism::ClassVariableTargetNode<'pr>) {
        let name = constant_id_name(&node.name());
        self.handle_class_variable(&node.as_node(), name, &node.location());
    }
}
