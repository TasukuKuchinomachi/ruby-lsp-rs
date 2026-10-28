use std::collections::HashMap;

use prism_child_nodes::child_nodes;
use rbs_environment::{ClassKind, Environment};
use ruby_prism::Node;

use crate::source::{ConstantPath, LineIndex, Span, bytes_text, constant_text, qualify};

pub(crate) const MAXIMUM_NODE_DEPTH: usize = 1000;

/// Comments before the first statement still belong to the program, so its scope starts at the beginning of the file
pub(crate) const PROGRAM_SCOPE_START: usize = 0;

#[derive(Debug, Clone)]
pub(crate) struct FileClass {
    pub(crate) kind: ClassKind,
    pub(crate) superclass: Option<String>,
    pub(crate) includes: Vec<String>,
    pub(crate) extends: Vec<String>,
    /// Methods may come from `define_method`, `method_missing`, delegation or modules this file cannot see
    pub(crate) is_dynamic: bool,
}

pub(crate) struct DefinitionInfo<'pr> {
    pub(crate) node: Node<'pr>,
    pub(crate) owner: String,
    pub(crate) singleton: bool,
    pub(crate) nesting: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct MethodKey {
    pub(crate) owner: String,
    pub(crate) singleton: bool,
    pub(crate) name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FileMethod {
    Definition(usize),
    AttributeReader(String),
    AttributeWriter(String),
    Alias(String),
}

pub(crate) struct FileConstant<'pr> {
    pub(crate) value: Node<'pr>,
    pub(crate) nesting: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScopeKind {
    Program,
    Class,
    Method,
    Block,
    /// The lines of a branch body of `if`, `unless`, `when`, `while` or `until`, which Steep's
    /// `Source.construct_mapping` gives the annotations written there
    Branch,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ScopeRange {
    pub(crate) span: Span,
    pub(crate) kind: ScopeKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct InstanceVariableKey {
    pub(crate) owner: String,
    pub(crate) singleton: bool,
    pub(crate) name: String,
}

/// Everything the checker needs to know about the file before inferring it in order, so that a method or an instance
/// variable defined further down the file is already known
#[derive(Default)]
pub(crate) struct FileIndex<'pr> {
    pub(crate) classes: HashMap<String, FileClass>,
    pub(crate) class_names_by_offset: HashMap<usize, String>,
    pub(crate) definitions: HashMap<usize, DefinitionInfo<'pr>>,
    pub(crate) methods: HashMap<MethodKey, FileMethod>,
    pub(crate) instance_variable_writers: HashMap<InstanceVariableKey, Vec<usize>>,
    pub(crate) constants: HashMap<String, FileConstant<'pr>>,
    pub(crate) scopes: Vec<ScopeRange>,
    /// The start of a branch body (its statements, or the `elsif` node) to its `ScopeKind::Branch` scope
    pub(crate) branch_scopes: HashMap<usize, ScopeRange>,
}

#[derive(Clone)]
struct WalkState {
    nesting: Vec<String>,
    /// Inside `class << target`: the class whose singleton is opened, or `None` when the target is not a constant
    singleton_class_of: Option<Option<String>>,
    current_definition: Option<(usize, String, bool)>,
}

impl WalkState {
    fn namespace(&self) -> &str {
        self.nesting.last().map(String::as_str).unwrap_or("")
    }

    fn method_owner(&self) -> Option<(String, bool)> {
        match &self.singleton_class_of {
            Some(Some(name)) => Some((name.clone(), true)),
            Some(None) => None,
            None if self.namespace().is_empty() => Some(("Object".to_string(), false)),
            None => Some((self.namespace().to_string(), false)),
        }
    }
}

impl<'pr> FileIndex<'pr> {
    pub(crate) fn build(environment: &Environment, root: &Node<'pr>, line_index: &LineIndex) -> FileIndex<'pr> {
        let mut index = FileIndex::default();
        let mut builder = Builder {
            environment,
            index: &mut index,
            line_index,
        };
        let state = WalkState {
            nesting: Vec::new(),
            singleton_class_of: None,
            current_definition: None,
        };
        builder.visit(root, &state, 0);
        index
    }

    pub(crate) fn find_method(
        &self,
        owner: &str,
        singleton: bool,
        name: &str,
    ) -> Option<&FileMethod> {
        self.methods.get(&MethodKey {
            owner: owner.to_string(),
            singleton,
            name: name.to_string(),
        })
    }

    pub(crate) fn innermost_scope(
        &self,
        offset: usize,
        accept: impl Fn(ScopeKind) -> bool,
    ) -> Option<ScopeRange> {
        self.scopes
            .iter()
            .filter(|scope| {
                accept(scope.kind) && scope.span.start <= offset && offset < scope.span.end
            })
            .min_by_key(|scope| scope.span.end - scope.span.start)
            .copied()
    }

    pub(crate) fn resolve_constant(
        &self,
        nesting: &[String],
        relative_name: &str,
    ) -> Option<String> {
        for prefix in nesting.iter().rev() {
            let candidate = qualify(prefix, relative_name);
            if self.classes.contains_key(&candidate) || self.constants.contains_key(&candidate) {
                return Some(candidate);
            }
        }
        if self.classes.contains_key(relative_name) || self.constants.contains_key(relative_name) {
            return Some(relative_name.to_string());
        }
        None
    }
}

struct Builder<'index, 'environment, 'pr> {
    environment: &'environment Environment,
    index: &'index mut FileIndex<'pr>,
    line_index: &'environment LineIndex,
}

const DYNAMIC_METHOD_DEFINERS: &[&str] = &[
    "define_method",
    "define_singleton_method",
    "delegate",
    "delegate_missing_to",
    "def_delegator",
    "def_delegators",
    "def_instance_delegator",
    "def_instance_delegators",
    "instance_delegate",
    "method_missing",
];

impl<'pr> Builder<'_, '_, 'pr> {
    fn visit(&mut self, node: &Node<'pr>, state: &WalkState, depth: usize) {
        if depth > MAXIMUM_NODE_DEPTH {
            return;
        }
        let next = depth + 1;
        match node {
            Node::ProgramNode { .. } => {
                self.index.scopes.push(ScopeRange {
                    span: Span {
                        start: PROGRAM_SCOPE_START,
                        end: usize::MAX,
                    },
                    kind: ScopeKind::Program,
                });
                self.visit_children(node, state, next);
            }
            Node::ClassNode { .. } => {
                let Some(class_node) = node.as_class_node() else {
                    return;
                };
                if let Some(superclass) = class_node.superclass() {
                    self.visit(&superclass, state, next);
                }
                let name = self.declared_name(state, &class_node.constant_path());
                let superclass = class_node
                    .superclass()
                    .and_then(|superclass| ConstantPath::of_node(&superclass))
                    .map(|path| self.resolve_reference(&state.nesting, &path));
                self.enter_class(
                    node,
                    name,
                    ClassKind::Class,
                    superclass,
                    class_node.body(),
                    state,
                    next,
                );
            }
            Node::ModuleNode { .. } => {
                let Some(module_node) = node.as_module_node() else {
                    return;
                };
                let name = self.declared_name(state, &module_node.constant_path());
                self.enter_class(
                    node,
                    name,
                    ClassKind::Module,
                    None,
                    module_node.body(),
                    state,
                    next,
                );
            }
            Node::SingletonClassNode { .. } => {
                let Some(singleton_class) = node.as_singleton_class_node() else {
                    return;
                };
                self.index.scopes.push(ScopeRange {
                    span: Span::of_node(node),
                    kind: ScopeKind::Class,
                });
                let mut inner_state = state.clone();
                let expression = singleton_class.expression();
                inner_state.singleton_class_of = Some(match &expression {
                    Node::SelfNode { .. } if !state.namespace().is_empty() => {
                        Some(state.namespace().to_string())
                    }
                    _ => ConstantPath::of_node(&expression)
                        .map(|path| self.resolve_reference(&state.nesting, &path)),
                });
                if let Some(body) = singleton_class.body() {
                    self.visit(&body, &inner_state, next);
                }
            }
            Node::DefNode { .. } => self.visit_definition(node, state, next),
            Node::IfNode { .. } | Node::UnlessNode { .. } | Node::WhenNode { .. } | Node::WhileNode { .. } | Node::UntilNode { .. } => {
                self.register_branches(node);
                self.visit_children(node, state, next);
            }
            Node::BlockNode { .. } | Node::LambdaNode { .. } => {
                self.index.scopes.push(ScopeRange {
                    span: Span::of_node(node),
                    kind: ScopeKind::Block,
                });
                self.visit_children(node, state, next);
            }
            Node::InstanceVariableWriteNode { .. }
            | Node::InstanceVariableOrWriteNode { .. }
            | Node::InstanceVariableAndWriteNode { .. }
            | Node::InstanceVariableOperatorWriteNode { .. }
            | Node::InstanceVariableTargetNode { .. } => {
                if let (Some((definition_offset, owner, singleton)), Some(name)) =
                    (&state.current_definition, instance_variable_name(node))
                {
                    let key = InstanceVariableKey {
                        owner: owner.clone(),
                        singleton: *singleton,
                        name,
                    };
                    let writers = self.index.instance_variable_writers.entry(key).or_default();
                    if !writers.contains(definition_offset) {
                        writers.push(*definition_offset);
                    }
                }
                self.visit_children(node, state, next);
            }
            Node::ConstantWriteNode { .. } => {
                if let Some(write) = node.as_constant_write_node() {
                    let name = qualify(state.namespace(), &constant_text(&write.name()));
                    self.index.constants.insert(
                        name,
                        FileConstant {
                            value: write.value(),
                            nesting: state.nesting.clone(),
                        },
                    );
                }
                self.visit_children(node, state, next);
            }
            Node::CallNode { .. } => {
                self.visit_class_body_call(node, state);
                self.visit_children(node, state, next);
            }
            Node::AliasMethodNode { .. } => {
                if let Some(alias) = node.as_alias_method_node()
                    && let (Some(new_name), Some(old_name)) = (
                        symbol_name(&alias.new_name()),
                        symbol_name(&alias.old_name()),
                    )
                {
                    self.register_method(state, new_name, FileMethod::Alias(old_name));
                }
            }
            _ => self.visit_children(node, state, next),
        }
    }

    fn visit_children(&mut self, node: &Node<'pr>, state: &WalkState, depth: usize) {
        for child in child_nodes(node) {
            self.visit(&child, state, depth);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn enter_class(
        &mut self,
        node: &Node<'pr>,
        name: String,
        kind: ClassKind,
        superclass: Option<String>,
        body: Option<Node<'pr>>,
        state: &WalkState,
        depth: usize,
    ) {
        self.index.scopes.push(ScopeRange {
            span: Span::of_node(node),
            kind: ScopeKind::Class,
        });
        self.index
            .class_names_by_offset
            .insert(node.location().start_offset(), name.clone());
        let entry = self.index.classes.entry(name.clone()).or_insert(FileClass {
            kind,
            superclass: None,
            includes: Vec::new(),
            extends: Vec::new(),
            is_dynamic: false,
        });
        if superclass.is_some() {
            entry.superclass = superclass;
        }
        let mut nesting = state.nesting.clone();
        nesting.push(name);
        let inner_state = WalkState {
            nesting,
            singleton_class_of: None,
            current_definition: None,
        };
        if let Some(body) = body {
            self.visit(&body, &inner_state, depth);
        }
    }

    fn last_line(&self, location: &ruby_prism::Location<'_>) -> usize {
        self.line_index
            .line_of(location.end_offset().saturating_sub(1).max(location.start_offset()))
    }

    fn start_line(&self, location: &ruby_prism::Location<'_>) -> usize {
        self.line_index.line_of(location.start_offset())
    }

    /// A branch body takes the annotations on the lines from `start_line` up to, not including, `end_line`
    fn register_branch(&mut self, body_start: usize, start_line: usize, end_line: usize) {
        if start_line >= end_line {
            return;
        }
        let Some(start) = self.line_index.line_start(start_line) else {
            return;
        };
        let end = self.line_index.line_start(end_line).unwrap_or(usize::MAX);
        let scope = ScopeRange {
            span: Span { start, end },
            kind: ScopeKind::Branch,
        };
        self.index.scopes.push(scope);
        self.index.branch_scopes.insert(body_start, scope);
    }

    /// The line ranges of Steep's `Source.construct_mapping` for prefix `if`, `unless`, `while` and `until` and for
    /// `when`
    fn register_branches(&mut self, node: &Node<'pr>) {
        let node_location = node.location();
        match node {
            Node::IfNode { .. } => {
                let Some(if_node) = node.as_if_node() else { return };
                let Some(keyword) = if_node.if_keyword_loc() else {
                    return;
                };
                if keyword.start_offset() != node_location.start_offset() {
                    return;
                }
                let subsequent_line = if_node
                    .subsequent()
                    .map(|subsequent| self.start_line(&subsequent.location()));
                if let Some(statements) = if_node.statements() {
                    let then_start = match if_node.then_keyword_loc() {
                        Some(then_keyword) => self.last_line(&then_keyword),
                        None => self.last_line(&if_node.predicate().location()),
                    };
                    let then_end = subsequent_line.unwrap_or_else(|| self.last_line(&node_location));
                    self.register_branch(statements.location().start_offset(), then_start, then_end);
                }
                match if_node.subsequent() {
                    Some(Node::ElseNode { .. }) => {
                        let Some(else_node) = if_node.subsequent().and_then(|subsequent| subsequent.as_else_node())
                        else {
                            return;
                        };
                        if let Some(statements) = else_node.statements() {
                            let else_start = self.last_line(&else_node.else_keyword_loc());
                            self.register_branch(
                                statements.location().start_offset(),
                                else_start,
                                self.last_line(&node_location),
                            );
                        }
                    }
                    Some(elsif) => {
                        let elsif_location = elsif.location();
                        self.register_branch(
                            elsif_location.start_offset(),
                            self.start_line(&elsif_location),
                            self.last_line(&node_location),
                        );
                    }
                    None => {}
                }
            }
            Node::UnlessNode { .. } => {
                let Some(unless_node) = node.as_unless_node() else {
                    return;
                };
                if unless_node.keyword_loc().start_offset() != node_location.start_offset() {
                    return;
                }
                let else_line = unless_node
                    .else_clause()
                    .map(|else_clause| self.start_line(&else_clause.else_keyword_loc()));
                if let Some(statements) = unless_node.statements() {
                    let start = match unless_node.then_keyword_loc() {
                        Some(then_keyword) => self.last_line(&then_keyword),
                        None => self.last_line(&unless_node.predicate().location()),
                    };
                    let end = else_line.unwrap_or_else(|| self.last_line(&node_location));
                    self.register_branch(statements.location().start_offset(), start, end);
                }
                if let Some(else_clause) = unless_node.else_clause()
                    && let Some(statements) = else_clause.statements()
                {
                    let start = self.last_line(&else_clause.else_keyword_loc());
                    self.register_branch(
                        statements.location().start_offset(),
                        start,
                        self.last_line(&node_location),
                    );
                }
            }
            Node::WhenNode { .. } => {
                let Some(when) = node.as_when_node() else { return };
                let Some(statements) = when.statements() else { return };
                let start = match when.then_keyword_loc() {
                    Some(then_keyword) => self.last_line(&then_keyword),
                    None => match when.conditions().iter().last() {
                        Some(last_condition) => self.last_line(&last_condition.location()) + 1,
                        None => return,
                    },
                };
                let statements_location = statements.location();
                self.register_branch(
                    statements_location.start_offset(),
                    start,
                    self.last_line(&statements_location),
                );
            }
            Node::WhileNode { .. } | Node::UntilNode { .. } => {
                let (keyword, predicate, statements, closing) = match node {
                    Node::WhileNode { .. } => {
                        let Some(while_node) = node.as_while_node() else { return };
                        (
                            while_node.keyword_loc(),
                            while_node.predicate(),
                            while_node.statements(),
                            while_node.closing_loc(),
                        )
                    }
                    _ => {
                        let Some(until_node) = node.as_until_node() else { return };
                        (
                            until_node.keyword_loc(),
                            until_node.predicate(),
                            until_node.statements(),
                            until_node.closing_loc(),
                        )
                    }
                };
                if keyword.start_offset() != node_location.start_offset() {
                    return;
                }
                let (Some(statements), Some(closing)) = (statements, closing) else {
                    return;
                };
                let start = self.last_line(&predicate.location());
                self.register_branch(statements.location().start_offset(), start, self.start_line(&closing));
            }
            _ => {}
        }
    }

    fn visit_definition(&mut self, node: &Node<'pr>, state: &WalkState, depth: usize) {
        let Some(definition) = node.as_def_node() else {
            return;
        };
        let span = Span::of_node(node);
        self.index.scopes.push(ScopeRange {
            span,
            kind: ScopeKind::Method,
        });
        let owner = match definition.receiver() {
            None => state.method_owner(),
            Some(Node::SelfNode { .. }) => match state.method_owner() {
                Some((owner, _)) if owner != "Object" || !state.namespace().is_empty() => {
                    Some((owner, true))
                }
                other => other,
            },
            Some(_) => None,
        };
        let Some((owner, singleton)) = owner else {
            self.visit_children(node, state, depth);
            return;
        };
        let name = constant_text(&definition.name());
        if name == "method_missing"
            && let Some(class) = self.index.classes.get_mut(&owner)
        {
            class.is_dynamic = true;
        }
        self.index.definitions.insert(
            span.start,
            DefinitionInfo {
                node: definition.as_node(),
                owner: owner.clone(),
                singleton,
                nesting: state.nesting.clone(),
            },
        );
        self.index
            .methods
            .entry(MethodKey {
                owner: owner.clone(),
                singleton,
                name,
            })
            .or_insert(FileMethod::Definition(span.start));
        let mut inner_state = state.clone();
        inner_state.current_definition = Some((span.start, owner, singleton));
        self.visit_children(node, &inner_state, depth);
    }

    fn visit_class_body_call(&mut self, node: &Node<'pr>, state: &WalkState) {
        let Some(call) = node.as_call_node() else {
            return;
        };
        if state.namespace().is_empty() || state.current_definition.is_some() {
            return;
        }
        if call
            .receiver()
            .is_some_and(|receiver| !matches!(receiver, Node::SelfNode { .. }))
        {
            return;
        }
        let method_name = constant_text(&call.name());
        let arguments: Vec<Node<'pr>> = call
            .arguments()
            .map(|arguments| arguments.arguments().iter().collect())
            .unwrap_or_default();
        let namespace = state.namespace().to_string();
        match method_name.as_str() {
            "attr_reader" | "attr_writer" | "attr_accessor" => {
                for argument in &arguments {
                    let Some(attribute_name) = symbol_name(argument) else {
                        continue;
                    };
                    let instance_variable = format!("@{attribute_name}");
                    if method_name != "attr_writer" {
                        self.register_method(
                            state,
                            attribute_name.clone(),
                            FileMethod::AttributeReader(instance_variable.clone()),
                        );
                    }
                    if method_name != "attr_reader" {
                        self.register_method(
                            state,
                            format!("{attribute_name}="),
                            FileMethod::AttributeWriter(instance_variable),
                        );
                    }
                }
            }
            "alias_method" => {
                if let [new_name, old_name] = arguments.as_slice()
                    && let (Some(new_name), Some(old_name)) =
                        (symbol_name(new_name), symbol_name(old_name))
                {
                    self.register_method(state, new_name, FileMethod::Alias(old_name));
                }
            }
            "include" | "extend" | "prepend" => {
                let mut resolved_modules = Vec::new();
                let mut has_unknown_module = false;
                for argument in &arguments {
                    match ConstantPath::of_node(argument) {
                        Some(path) => {
                            resolved_modules.push(self.resolve_reference(&state.nesting, &path))
                        }
                        None => has_unknown_module = true,
                    }
                }
                let Some(class) = self.index.classes.get_mut(&namespace) else {
                    return;
                };
                class.is_dynamic |= has_unknown_module;
                if method_name == "extend" {
                    class.extends.extend(resolved_modules);
                } else {
                    class.includes.extend(resolved_modules);
                }
            }
            name if DYNAMIC_METHOD_DEFINERS.contains(&name) => {
                if let Some(class) = self.index.classes.get_mut(&namespace) {
                    class.is_dynamic = true;
                }
            }
            _ => {}
        }
    }

    fn register_method(&mut self, state: &WalkState, name: String, method: FileMethod) {
        let Some((owner, singleton)) = state.method_owner() else {
            return;
        };
        self.index
            .methods
            .entry(MethodKey {
                owner,
                singleton,
                name,
            })
            .or_insert(method);
    }

    fn declared_name(&self, state: &WalkState, constant_path: &Node<'pr>) -> String {
        let Some(path) = ConstantPath::of_node(constant_path) else {
            return qualify(
                state.namespace(),
                &bytes_text(constant_path.location().as_slice()),
            );
        };
        if path.absolute {
            return path.joined();
        }
        if path.segments.len() == 1 {
            return qualify(state.namespace(), &path.segments[0]);
        }
        let first = &path.segments[0];
        let rest = path.segments[1..].join("::");
        let resolved_first = self
            .environment
            .resolve_constant_name(&state.nesting, first)
            .or_else(|| self.index.resolve_constant(&state.nesting, first));
        match resolved_first {
            Some(resolved_first) => qualify(&resolved_first, &rest),
            None => qualify(state.namespace(), &path.joined()),
        }
    }

    fn resolve_reference(&self, nesting: &[String], path: &ConstantPath) -> String {
        let joined = path.joined();
        if path.absolute {
            return joined;
        }
        self.environment
            .resolve_constant_name(nesting, &joined)
            .or_else(|| self.index.resolve_constant(nesting, &joined))
            .unwrap_or(joined)
    }
}

fn instance_variable_name(node: &Node<'_>) -> Option<String> {
    let name = match node {
        Node::InstanceVariableWriteNode { .. } => node.as_instance_variable_write_node()?.name(),
        Node::InstanceVariableOrWriteNode { .. } => {
            node.as_instance_variable_or_write_node()?.name()
        }
        Node::InstanceVariableAndWriteNode { .. } => {
            node.as_instance_variable_and_write_node()?.name()
        }
        Node::InstanceVariableOperatorWriteNode { .. } => {
            node.as_instance_variable_operator_write_node()?.name()
        }
        Node::InstanceVariableTargetNode { .. } => node.as_instance_variable_target_node()?.name(),
        _ => return None,
    };
    Some(constant_text(&name))
}

pub(crate) fn symbol_name(node: &Node<'_>) -> Option<String> {
    match node {
        Node::SymbolNode { .. } => Some(bytes_text(node.as_symbol_node()?.unescaped())),
        Node::StringNode { .. } => Some(bytes_text(node.as_string_node()?.unescaped())),
        _ => None,
    }
}
