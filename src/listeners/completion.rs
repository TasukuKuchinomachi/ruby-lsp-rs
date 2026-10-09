use std::collections::HashSet;
use std::path::Path;

use rbs_environment::{Environment, Type};
use ruby_prism::{CallNode, Node};
use serde::Serialize;
use serde_json::{Value, json};
use type_checker::Analysis;

use crate::document::ParsedDocument;
use crate::indexer::{EntryId, EntryKind, Index};
use crate::listeners::definition::variable_name;
use crate::node_context::NodeContext;
use crate::position::{Position, Range};
use crate::prism::{ConstantNameError, NodeExt, constant_full_name, constant_name, location_slice, node_slice};
use crate::requests::support::common::self_receiver;
use crate::requests::support::receiver_methods::{self, ReceiverMethod};
use crate::requests::support::sorbet_level::SorbetLevel;
use crate::type_inferrer::TypeInferrer;

pub const KEYWORDS: &[&str] = &[
    "alias",
    "and",
    "begin",
    "BEGIN",
    "break",
    "case",
    "class",
    "def",
    "defined?",
    "do",
    "else",
    "elsif",
    "end",
    "END",
    "ensure",
    "false",
    "for",
    "if",
    "in",
    "module",
    "next",
    "nil",
    "not",
    "or",
    "redo",
    "rescue",
    "retry",
    "return",
    "self",
    "super",
    "then",
    "true",
    "undef",
    "unless",
    "until",
    "when",
    "while",
    "yield",
    "__ENCODING__",
    "__FILE__",
    "__LINE__",
];

pub mod completion_item_kind {
    pub const METHOD: u32 = 2;
    pub const FIELD: u32 = 5;
    pub const VARIABLE: u32 = 6;
    pub const CLASS: u32 = 7;
    pub const MODULE: u32 = 9;
    pub const KEYWORD: u32 = 14;
    pub const FILE: u32 = 17;
    pub const REFERENCE: u32 = 18;
    pub const CONSTANT: u32 = 21;
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CompletionItemLabelDetails {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TextEdit {
    pub range: Range,
    pub new_text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletionItem {
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label_details: Option<CompletionItemLabelDetails>,
    pub kind: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filter_text: Option<String>,
    pub text_edit: TextEdit,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

pub struct Completion<'a, 'pr> {
    response: Vec<CompletionItem>,
    index: &'a mut Index,
    parsed: &'a ParsedDocument<'a>,
    node_context: &'a NodeContext<'pr>,
    sorbet_level: SorbetLevel,
    uri: &'a str,
    trigger_character: Option<&'a str>,
    type_checking: Option<(&'a Analysis, &'a Environment)>,
}

impl<'a, 'pr> Completion<'a, 'pr> {
    pub fn new(
        index: &'a mut Index,
        parsed: &'a ParsedDocument<'a>,
        node_context: &'a NodeContext<'pr>,
        sorbet_level: SorbetLevel,
        uri: &'a str,
        trigger_character: Option<&'a str>,
    ) -> Completion<'a, 'pr> {
        Completion {
            response: Vec::new(),
            index,
            parsed,
            node_context,
            sorbet_level,
            uri,
            trigger_character,
            type_checking: None,
        }
    }

    /// Methods of receivers whose type the type checker knows come from that type instead of the guess of `TypeInferrer`
    pub fn with_type_checking(mut self, analysis: &'a Analysis, environment: &'a Environment) -> Completion<'a, 'pr> {
        self.type_checking = Some((analysis, environment));
        self
    }

    pub fn into_response(self) -> Vec<CompletionItem> {
        self.response
    }

    pub fn dispatch(&mut self, target: &Node<'_>) {
        match target.type_name() {
            "ConstantReadNode" => self.on_constant_read_node_enter(target),
            "ConstantPathNode" => self.on_constant_path_node_enter(target),
            "CallNode" => self.on_call_node_enter(&target.as_call_node().expect("type name was checked")),
            "GlobalVariableAndWriteNode"
            | "GlobalVariableOperatorWriteNode"
            | "GlobalVariableOrWriteNode"
            | "GlobalVariableReadNode"
            | "GlobalVariableTargetNode"
            | "GlobalVariableWriteNode" => {
                if let Some(name) = variable_name(target) {
                    let range = self.variable_name_range(target);
                    self.handle_global_variable_completion(&name, range);
                }
            }
            "InstanceVariableReadNode"
            | "InstanceVariableWriteNode"
            | "InstanceVariableAndWriteNode"
            | "InstanceVariableOperatorWriteNode"
            | "InstanceVariableOrWriteNode"
            | "InstanceVariableTargetNode" => {
                if let Some(name) = variable_name(target) {
                    let range = self.variable_name_range(target);
                    self.handle_instance_variable_completion(&name, range);
                }
            }
            "ClassVariableAndWriteNode"
            | "ClassVariableOperatorWriteNode"
            | "ClassVariableOrWriteNode"
            | "ClassVariableTargetNode"
            | "ClassVariableReadNode"
            | "ClassVariableWriteNode" => {
                if let Some(name) = variable_name(target) {
                    let range = self.variable_name_range(target);
                    self.handle_class_variable_completion(&name, range);
                }
            }
            _ => {}
        }
    }

    fn variable_name_range(&self, node: &Node<'_>) -> Range {
        let name_location = match node {
            Node::GlobalVariableAndWriteNode { .. } => node.as_global_variable_and_write_node().map(|node| node.name_loc()),
            Node::GlobalVariableOperatorWriteNode { .. } => {
                node.as_global_variable_operator_write_node().map(|node| node.name_loc())
            }
            Node::GlobalVariableOrWriteNode { .. } => node.as_global_variable_or_write_node().map(|node| node.name_loc()),
            Node::GlobalVariableWriteNode { .. } => node.as_global_variable_write_node().map(|node| node.name_loc()),
            Node::InstanceVariableWriteNode { .. } => node.as_instance_variable_write_node().map(|node| node.name_loc()),
            Node::InstanceVariableAndWriteNode { .. } => {
                node.as_instance_variable_and_write_node().map(|node| node.name_loc())
            }
            Node::InstanceVariableOperatorWriteNode { .. } => {
                node.as_instance_variable_operator_write_node().map(|node| node.name_loc())
            }
            Node::InstanceVariableOrWriteNode { .. } => node.as_instance_variable_or_write_node().map(|node| node.name_loc()),
            Node::ClassVariableAndWriteNode { .. } => node.as_class_variable_and_write_node().map(|node| node.name_loc()),
            Node::ClassVariableOperatorWriteNode { .. } => {
                node.as_class_variable_operator_write_node().map(|node| node.name_loc())
            }
            Node::ClassVariableOrWriteNode { .. } => node.as_class_variable_or_write_node().map(|node| node.name_loc()),
            Node::ClassVariableWriteNode { .. } => node.as_class_variable_write_node().map(|node| node.name_loc()),
            _ => None,
        };
        match name_location {
            Some(location) => self.parsed.range_from_location(&location),
            None => self.parsed.range_from_node(node),
        }
    }

    fn on_constant_read_node_enter(&mut self, node: &Node<'_>) {
        if !self.sorbet_level.is_ignore() {
            return;
        }
        let Some(name) = constant_name(node) else { return };

        let range = self.parsed.range_from_node(node);
        let candidates = self.index.constant_completion_candidates(&name, &self.node_context.nesting);
        for entries in candidates {
            let complete_name = self.index.entry(entries[0]).name.clone();
            let top_level = self.is_top_level(&complete_name);
            let item = self.build_entry_completion(&complete_name, &name, range, &entries, top_level);
            self.response.push(item);
        }
    }

    fn on_constant_path_node_enter(&mut self, node: &Node<'_>) {
        if !self.sorbet_level.is_ignore() {
            return;
        }
        let name = match constant_full_name(node) {
            Ok(name) => name,
            Err(ConstantNameError::MissingNodes) => node_slice(node),
            Err(ConstantNameError::DynamicParts) => return,
        };
        let range = self.parsed.range_from_node(node);
        self.constant_path_completion(&name, range);
    }

    fn on_call_node_enter(&mut self, node: &CallNode<'_>) {
        if self.sorbet_level.is_ignore()
            && let Some(receiver) = node.receiver()
            && (receiver.as_constant_read_node().is_some() || receiver.as_constant_path_node().is_some())
            && let Some(call_operator_location) = node.call_operator_loc()
            && location_slice(&call_operator_location) == "::"
            && let Some(name) = constant_name(&receiver)
        {
            let range = self.parsed.byte_range(node.as_node().start_offset(), call_operator_location.end_offset());
            self.constant_path_completion(&format!("{name}::"), range);
            return;
        }

        let Some(name) = node.message_loc().map(|location| location_slice(&location)) else { return };

        match name.as_str() {
            "require" => self.complete_require(node),
            "require_relative" => self.complete_require_relative(node),
            _ => self.complete_methods(node, &name),
        }
    }

    fn constant_path_completion(&mut self, name: &str, range: Range) {
        let (name, top_level_reference) = match name.strip_prefix("::") {
            Some(stripped) => (stripped.to_string(), true),
            None => (name.to_string(), false),
        };

        let (aliased_namespace, incomplete_name) = match name.strip_suffix("::") {
            Some(stripped) => (stripped.to_string(), String::new()),
            None => {
                let mut parts = ruby_split(&name);
                let incomplete_name = parts.pop().unwrap_or_default();
                (parts.join("::"), incomplete_name)
            }
        };

        let nesting = self.node_context.nesting.clone();
        let Some(namespace_entries) = self.index.resolve(&aliased_namespace, &nesting) else { return };
        let namespace_name = self.index.entry(namespace_entries[0]).name.clone();
        let real_namespace = self.index.follow_aliased_namespace(&namespace_name).unwrap_or(namespace_name);

        let candidate_nesting: &[String] = if top_level_reference { &[] } else { &nesting };
        let candidates =
            self.index.constant_completion_candidates(&format!("{real_namespace}::{incomplete_name}"), candidate_nesting);

        let nesting_prefix = format!("{}::", ruby_array_inspect(&nesting));
        for entries in candidates {
            let first_entry = self.index.entry(entries[0]);
            if first_entry.is_private() && !first_entry.name.starts_with(&nesting_prefix) {
                continue;
            }

            let entry_name = first_entry.name.clone();
            let full_name = if aliased_namespace != real_namespace {
                let constant_name = entry_name.strip_prefix(&format!("{real_namespace}::")).unwrap_or(&entry_name);
                if aliased_namespace.is_empty() {
                    constant_name.to_string()
                } else {
                    format!("{aliased_namespace}::{constant_name}")
                }
            } else if !entry_name.starts_with(&aliased_namespace) {
                let short_name = ruby_split(&entry_name).pop().unwrap_or_default();
                format!("{aliased_namespace}::{short_name}")
            } else {
                entry_name.clone()
            };

            let top_level = top_level_reference || self.is_top_level(&entry_name);
            let item = self.build_entry_completion(&full_name, &name, range, &entries, top_level);
            self.response.push(item);
        }
    }

    fn handle_global_variable_completion(&mut self, name: &str, range: Range) {
        let candidates = self.index.prefix_search(name, None);
        if candidates.is_empty() {
            return;
        }

        let mut seen_names = std::collections::HashSet::new();
        for id in candidates.into_iter().flatten() {
            let entry = self.index.entry(id);
            if !seen_names.insert(entry.name.clone()) {
                continue;
            }
            let entry_name = entry.name.clone();
            self.response.push(CompletionItem {
                label: entry_name.clone(),
                label_details: Some(CompletionItemLabelDetails { detail: None, description: Some(entry.file_name()) }),
                kind: completion_item_kind::VARIABLE,
                filter_text: Some(entry_name.clone()),
                text_edit: TextEdit { range, new_text: entry_name },
                data: None,
            });
        }
    }

    fn handle_class_variable_completion(&mut self, name: &str, range: Range) {
        let Some(receiver_type) = TypeInferrer::new(self.index).infer_receiver_type(self.node_context) else { return };
        let candidates = self.index.class_variable_completion_candidates(name, &receiver_type.name);
        for id in candidates {
            let item = self.variable_completion_item(id, range);
            self.response.push(item);
        }
    }

    fn handle_instance_variable_completion(&mut self, name: &str, range: Range) {
        if self.sorbet_level.is_strict() {
            return;
        }
        let Some(receiver_type) = TypeInferrer::new(self.index).infer_receiver_type(self.node_context) else { return };
        let candidates = self.index.instance_variable_completion_candidates(name, &receiver_type.name);
        for id in candidates {
            let item = self.variable_completion_item(id, range);
            self.response.push(item);
        }
    }

    fn variable_completion_item(&self, id: EntryId, range: Range) -> CompletionItem {
        let entry = self.index.entry(id);
        CompletionItem {
            label: entry.name.clone(),
            label_details: Some(CompletionItemLabelDetails { detail: None, description: Some(entry.file_name()) }),
            kind: completion_item_kind::FIELD,
            filter_text: None,
            text_edit: TextEdit { range, new_text: entry.name.clone() },
            data: Some(json!({ "owner_name": entry.owner() })),
        }
    }

    fn first_string_argument<'node>(node: &CallNode<'node>) -> Option<ruby_prism::StringNode<'node>> {
        node.arguments()?.arguments().iter().next()?.as_string_node()
    }

    fn complete_require(&mut self, node: &CallNode<'_>) {
        let Some(path_node_to_complete) = Self::first_string_argument(node) else { return };
        let content = location_slice(&path_node_to_complete.content_loc());

        let mut paths: Vec<String> =
            self.index.search_require_paths(&content).into_iter().map(|(require_path, _)| require_path.to_string()).collect();
        paths.sort();
        let range = self.parsed.range_from_location(&path_node_to_complete.content_loc());
        for path in paths {
            self.response.push(build_completion(path, range));
        }
    }

    fn complete_require_relative(&mut self, node: &CallNode<'_>) {
        let Some(path_node_to_complete) = Self::first_string_argument(node) else { return };
        let Some(full_path) = crate::uri::to_path(self.uri) else { return };

        let origin_directory = Path::new(&full_path).parent().map(Path::to_path_buf).unwrap_or_default();
        let content = location_slice(&path_node_to_complete.content_loc());
        let path_queries = if content.ends_with('/') || content.is_empty() {
            vec![format!("{content}**/*.rb")]
        } else {
            vec![format!("{content}*/**/*.rb"), format!("**/{content}*.rb")]
        };

        let mut paths: Vec<String> = path_queries.iter().flat_map(|query| glob(&origin_directory, query)).collect();
        paths.sort();
        let range = self.parsed.range_from_location(&path_node_to_complete.content_loc());
        for path in paths {
            let label = path.strip_suffix(".rb").unwrap_or(&path).to_string();
            self.response.push(build_completion(label, range));
        }
    }

    fn complete_methods(&mut self, node: &CallNode<'_>, name: &str) {
        if !self.sorbet_level.is_true_or_higher() && node.receiver().is_none() {
            self.add_local_completions(node, name);
            self.add_keyword_completions(node, name);
        }

        if self.sorbet_level.is_true_or_higher() && self_receiver(node) {
            return;
        }

        let method_name = if self.trigger_character == Some(".") { None } else { Some(name) };

        if self.complete_methods_of_known_receiver_type(node, method_name) {
            return;
        }

        let Some(receiver_type) = TypeInferrer::new(self.index).infer_receiver_type(self.node_context) else { return };
        let Some(range) = self.method_name_range(node, method_name) else { return };

        let guessed_type = if receiver_type.guessed { json!(receiver_type.name) } else { json!(false) };
        let external_references = self.node_context.fully_qualified_name() != receiver_type.name;

        let candidates = self.index.method_completion_candidates(method_name, &receiver_type.name);
        for id in candidates {
            let entry = self.index.entry(id);
            if !entry.is_public() && external_references {
                continue;
            }

            let entry_name = entry.name.clone();
            let new_text = match entry_name.strip_suffix('=') {
                Some(writer_name) if node.receiver().is_none() => format!("self.{writer_name} = "),
                Some(writer_name) => format!("{writer_name} = "),
                None => entry_name.clone(),
            };

            self.response.push(CompletionItem {
                label: entry_name.clone(),
                label_details: Some(CompletionItemLabelDetails {
                    detail: Some(entry.decorated_parameters()),
                    description: Some(entry.file_name()),
                }),
                kind: completion_item_kind::METHOD,
                filter_text: Some(entry_name),
                text_edit: TextEdit { range, new_text },
                data: Some(json!({ "owner_name": entry.owner(), "guessed_type": guessed_type })),
            });
        }
    }

    fn method_name_range(&self, node: &CallNode<'_>, method_name: Option<&str>) -> Option<Range> {
        match method_name {
            Some(_) => node.message_loc().map(|location| self.parsed.range_from_location(&location)),
            None => {
                let location = node.call_operator_loc()?;
                let start = self.parsed.byte_position(location.start_offset());
                let position = Position::new(start.line, start.character + 1);
                Some(Range::new(position, position))
            }
        }
    }

    /// Returns false when the type checker doesn't know the receiver, so that the methods are guessed as before. The
    /// methods declared in RBS come first and the Ruby index adds the methods that only have a Ruby implementation
    fn complete_methods_of_known_receiver_type(&mut self, node: &CallNode<'_>, method_name: Option<&str>) -> bool {
        let Some((analysis, environment)) = self.type_checking else { return false };
        let Some(receiver) = node.receiver() else { return false };
        let Some(receiver_type) = analysis.receiver_type(receiver.start_offset(), receiver.end_offset()) else {
            return false;
        };
        let receiver_type = if node.is_safe_navigation() {
            match receiver_methods::without_nil(receiver_type) {
                Some(receiver_type) => receiver_type,
                None => return false,
            }
        } else {
            receiver_type.clone()
        };
        let members = receiver_methods::receiver_members(environment, &receiver_type);
        if members.is_empty() {
            return false;
        }
        let Some(range) = self.method_name_range(node, method_name) else { return true };
        let prefix = method_name.unwrap_or_default();
        let includes_private = receiver.as_self_node().is_some();

        let mut seen_names = HashSet::new();
        for member in &members {
            for (name, method) in receiver_methods::methods(environment, member) {
                if !name.starts_with(prefix) || (method.is_private() && !includes_private) {
                    continue;
                }
                if seen_names.insert(name.clone()) {
                    let item = receiver_method_completion_item(&name, &method, member, range);
                    self.response.push(item);
                }
            }
            let Some(index_receiver_name) = receiver_methods::index_receiver_name(member) else { continue };
            for id in self.index.method_completion_candidates(method_name, &index_receiver_name) {
                let entry = self.index.entry(id);
                if (!entry.is_public() && !includes_private) || !seen_names.insert(entry.name.clone()) {
                    continue;
                }
                let entry_name = entry.name.clone();
                self.response.push(CompletionItem {
                    label: entry_name.clone(),
                    label_details: Some(CompletionItemLabelDetails {
                        detail: Some(entry.decorated_parameters()),
                        description: Some(entry.file_name()),
                    }),
                    kind: completion_item_kind::METHOD,
                    filter_text: Some(entry_name.clone()),
                    text_edit: TextEdit { range, new_text: method_insertion_text(&entry_name) },
                    data: Some(json!({
                        "owner_name": entry.owner(),
                        "guessed_type": false,
                        "receiver_type": member.to_string(),
                    })),
                });
            }
        }
        true
    }

    fn add_local_completions(&mut self, node: &CallNode<'_>, name: &str) {
        let Some(message_location) = node.message_loc() else { return };
        let range = self.parsed.range_from_location(&message_location);

        for local_name in self.node_context.locals_for_scope() {
            if !local_name.starts_with(name) {
                continue;
            }
            self.response.push(CompletionItem {
                label: local_name.clone(),
                label_details: None,
                kind: completion_item_kind::VARIABLE,
                filter_text: Some(local_name.clone()),
                text_edit: TextEdit { range, new_text: local_name },
                data: Some(json!({ "skip_resolve": true })),
            });
        }
    }

    fn add_keyword_completions(&mut self, node: &CallNode<'_>, name: &str) {
        let Some(message_location) = node.message_loc() else { return };
        let range = self.parsed.range_from_location(&message_location);

        for keyword in KEYWORDS {
            if !keyword.starts_with(name) {
                continue;
            }
            self.response.push(CompletionItem {
                label: keyword.to_string(),
                label_details: None,
                kind: completion_item_kind::KEYWORD,
                filter_text: None,
                text_edit: TextEdit { range, new_text: keyword.to_string() },
                data: Some(json!({ "keyword": true })),
            });
        }
    }

    fn build_entry_completion(
        &self,
        real_name: &str,
        incomplete_name: &str,
        range: Range,
        entries: &[EntryId],
        top_level: bool,
    ) -> CompletionItem {
        let first_entry = self.index.entry(entries[0]);
        let kind = if first_entry.is_class() {
            completion_item_kind::CLASS
        } else if first_entry.is_module() {
            completion_item_kind::MODULE
        } else if matches!(first_entry.kind, EntryKind::Constant) {
            completion_item_kind::CONSTANT
        } else {
            completion_item_kind::REFERENCE
        };

        let mut insertion_text = real_name.to_string();
        let mut filter_text = real_name.to_string();
        if top_level {
            insertion_text.insert_str(0, "::");
            filter_text.insert_str(0, "::");
        }

        let fully_qualified_name = self.node_context.fully_qualified_name();
        if !fully_qualified_name.starts_with(incomplete_name) {
            for namespace in &self.node_context.nesting {
                let prefix = format!("{namespace}::");
                let shortened_name = insertion_text.strip_prefix(&prefix).unwrap_or(&insertion_text).to_string();

                let conflict_name = format!("{fully_qualified_name}::{shortened_name}");
                if real_name != conflict_name && self.index.get(&conflict_name).is_some() {
                    break;
                }

                insertion_text = shortened_name;
                if !incomplete_name.starts_with(&prefix)
                    && let Some(stripped) = filter_text.strip_prefix(&prefix)
                {
                    filter_text = stripped.to_string();
                }
            }
        }

        let description = entries.iter().map(|id| self.index.entry(*id).file_name()).collect::<Vec<_>>().join(",");

        CompletionItem {
            label: real_name.to_string(),
            label_details: Some(CompletionItemLabelDetails { detail: None, description: Some(description) }),
            kind,
            filter_text: Some(filter_text),
            text_edit: TextEdit { range, new_text: insertion_text },
            data: None,
        }
    }

    fn is_top_level(&self, entry_name: &str) -> bool {
        let nesting = &self.node_context.nesting;
        for i in (0..=nesting.len()).rev() {
            let prefix = nesting[..i].join("::");
            let full_name = if prefix.is_empty() { entry_name.to_string() } else { format!("{prefix}::{entry_name}") };
            if full_name == entry_name {
                continue;
            }
            if self.index.get(&full_name).is_some() {
                return true;
            }
        }
        false
    }
}

fn receiver_method_completion_item(name: &str, method: &ReceiverMethod<'_>, member: &Type, range: Range) -> CompletionItem {
    let owner = method.definition.owner.as_str();
    CompletionItem {
        label: name.to_string(),
        label_details: Some(CompletionItemLabelDetails {
            detail: method.overloads.first().map(ToString::to_string),
            description: Some(owner.to_string()),
        }),
        kind: completion_item_kind::METHOD,
        filter_text: Some(name.to_string()),
        text_edit: TextEdit { range, new_text: method_insertion_text(name) },
        data: Some(json!({ "owner_name": owner, "guessed_type": false, "receiver_type": member.to_string() })),
    }
}

/// Only for calls with an explicit receiver, so a writer never needs `self.`
fn method_insertion_text(name: &str) -> String {
    match name.strip_suffix('=') {
        Some(writer_name) if is_writer_name(writer_name) => format!("{writer_name} = "),
        _ => name.to_string(),
    }
}

fn is_writer_name(name: &str) -> bool {
    name.chars().last().is_some_and(|character| character.is_alphanumeric() || character == '_')
}

fn build_completion(label: String, range: Range) -> CompletionItem {
    CompletionItem {
        label: label.clone(),
        label_details: None,
        kind: completion_item_kind::FILE,
        filter_text: None,
        text_edit: TextEdit { range, new_text: label },
        data: None,
    }
}

/// Ruby's `String#split` drops trailing empty strings, unlike `str::split`
fn ruby_split(text: &str) -> Vec<String> {
    let mut parts: Vec<String> = text.split("::").map(str::to_string).collect();
    while parts.last().is_some_and(String::is_empty) {
        parts.pop();
    }
    parts
}

/// Ruby LSP interpolates the nesting array (`"#{nesting}::"`), which produces its `inspect` representation
fn ruby_array_inspect(parts: &[String]) -> String {
    let inspected: Vec<String> = parts.iter().map(|part| format!("{part:?}")).collect();
    format!("[{}]", inspected.join(", "))
}

/// Mirrors `Dir.glob(pattern, File::FNM_PATHNAME, base: base)` for the patterns used by `require_relative` completion:
/// `*` and `?` never match `/` nor leading dots, `**/` matches any number of non hidden directories and the results keep
/// the pattern's literal prefix
pub fn glob(base: &Path, pattern: &str) -> Vec<String> {
    let (start, components): (std::path::PathBuf, Vec<&str>) = match pattern.strip_prefix('/') {
        Some(stripped) => (std::path::PathBuf::from("/"), stripped.split('/').filter(|part| !part.is_empty()).collect()),
        None => (base.to_path_buf(), pattern.split('/').filter(|part| !part.is_empty()).collect()),
    };
    let prefix = if pattern.starts_with('/') { "/".to_string() } else { String::new() };
    let mut results = Vec::new();
    glob_components(&start, &prefix, &components, &mut results);
    results
}

fn glob_components(directory: &Path, relative: &str, components: &[&str], results: &mut Vec<String>) {
    let Some((component, rest)) = components.split_first() else { return };
    let join = |name: &str| if relative.is_empty() || relative.ends_with('/') { format!("{relative}{name}") } else { format!("{relative}/{name}") };

    if *component == "**" {
        glob_components(directory, relative, rest, results);
        for (name, path) in directory_entries(directory) {
            if name.starts_with('.') || !is_directory_without_following_links(&path) {
                continue;
            }
            glob_components(&path, &join(&name), components, results);
        }
        return;
    }

    if !has_wildcard(component) {
        let path = directory.join(component);
        let relative_path = join(component);
        if rest.is_empty() {
            if path.symlink_metadata().is_ok() {
                results.push(relative_path);
            }
        } else if path.is_dir() {
            glob_components(&path, &relative_path, rest, results);
        }
        return;
    }

    let mut entries = directory_entries(directory);
    entries.sort();
    for (name, path) in entries {
        if name.starts_with('.') && !component.starts_with('.') {
            continue;
        }
        if !wildcard_matches(component.as_bytes(), name.as_bytes()) {
            continue;
        }
        if rest.is_empty() {
            results.push(join(&name));
        } else if path.is_dir() {
            glob_components(&path, &join(&name), rest, results);
        }
    }
}

fn directory_entries(directory: &Path) -> Vec<(String, std::path::PathBuf)> {
    let Ok(read_directory) = std::fs::read_dir(directory) else { return Vec::new() };
    read_directory
        .filter_map(Result::ok)
        .map(|entry| (entry.file_name().to_string_lossy().into_owned(), entry.path()))
        .collect()
}

fn is_directory_without_following_links(path: &Path) -> bool {
    path.symlink_metadata().is_ok_and(|metadata| metadata.is_dir())
}

fn has_wildcard(component: &str) -> bool {
    component.contains(['*', '?'])
}

fn wildcard_matches(pattern: &[u8], text: &[u8]) -> bool {
    match pattern.split_first() {
        None => text.is_empty(),
        Some((b'*', rest)) => (0..=text.len()).any(|skip| wildcard_matches(rest, &text[skip..])),
        Some((b'?', rest)) => !text.is_empty() && wildcard_matches(rest, &text[1..]),
        Some((character, rest)) => text.first() == Some(character) && wildcard_matches(rest, &text[1..]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_wildcards() {
        assert!(wildcard_matches(b"*.rb", b"bar.rb"));
        assert!(wildcard_matches(b"b*", b"baz"));
        assert!(!wildcard_matches(b"b*.rb", b"quux.rb"));
        assert!(wildcard_matches(b"b?r", b"bar"));
    }

    #[test]
    fn inspects_arrays_like_ruby() {
        assert_eq!(ruby_array_inspect(&["Foo".to_string(), "Bar".to_string()]), "[\"Foo\", \"Bar\"]");
        assert_eq!(ruby_array_inspect(&[]), "[]");
    }

    #[test]
    fn splits_like_ruby() {
        assert_eq!(ruby_split("Foo::Bar::"), vec!["Foo".to_string(), "Bar".to_string()]);
        assert_eq!(ruby_split("::Foo"), vec![String::new(), "Foo".to_string()]);
    }
}
