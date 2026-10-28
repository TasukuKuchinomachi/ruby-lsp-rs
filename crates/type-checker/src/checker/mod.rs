mod arguments;
mod blocks;
mod calls;
mod constants;
mod control_flow;
mod definitions;
mod expressions;
mod hints;
mod narrowing;
mod variables;

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use rbs_environment::{Block, Environment, Location, MethodType, Type};
use ruby_prism::Node;

use crate::annotations::{AnnotationTarget, Annotations, DynamicKind};
use crate::file_index::{FileIndex, InstanceVariableKey, ScopeKind, ScopeRange};
use crate::source::{LineIndex, Span};
use crate::{
    Analysis, AnalysisOptions, DefinitionEntry, HoverEntry, HoverKind, ReceiverEntry, Severity, TypeDiagnostic,
    TypeHint, TypeHintKind,
};

pub(crate) type Locals = HashMap<String, Type>;

const MAXIMUM_SILENT_DEPTH: usize = 24;

pub(crate) enum CacheState {
    InProgress,
    Done(Type),
}

/// What `break` and `next` may carry in the innermost block or loop
pub(crate) struct JumpExpectation {
    pub(crate) break_type: Option<Type>,
    pub(crate) next_type: Option<Type>,
}

pub(crate) struct MethodContext {
    pub(crate) name: String,
    pub(crate) owner: String,
    pub(crate) singleton: bool,
    /// Present only when the RBS declaration has exactly one overload, so that a mismatch is certain
    pub(crate) declared_return_type: Option<Type>,
    pub(crate) block: Option<Block>,
    /// RBS or `@type method` gives the method a type, so that `yield` is checked against its block
    pub(crate) has_method_type: bool,
    /// Where the `def` starts, the scope of the annotations written in it
    pub(crate) scope_start: usize,
    /// The name of the `&block` parameter, which `block_given?` narrows
    pub(crate) block_parameter_name: Option<String>,
}

#[derive(Clone)]
pub(crate) struct Context {
    pub(crate) self_type: Type,
    pub(crate) nesting: Rc<Vec<String>>,
    pub(crate) method: Option<Rc<MethodContext>>,
    pub(crate) declared_locals: Rc<HashMap<String, Type>>,
}

impl Context {
    pub(crate) fn namespace(&self) -> &str {
        self.nesting.last().map(String::as_str).unwrap_or("")
    }

    pub(crate) fn instance_variable_owner(&self) -> Option<(String, bool)> {
        match &self.self_type {
            Type::ClassInstance { name, .. } => Some((name.clone(), false)),
            Type::ClassSingleton { name } => Some((name.clone(), true)),
            _ => None,
        }
    }
}

struct ResolvedAnnotation {
    offset: usize,
    target: AnnotationTarget,
    annotated_type: Type,
    variable_scope_start: usize,
    self_scope_start: usize,
    /// The kind of the innermost scope, the one Steep attaches the annotation to
    attached_kind: ScopeKind,
}

/// Steep attaches an annotation to the innermost `def`, `class`, `module` or block around it, and reads the
/// annotations of a block together with those of the scope around it
pub(crate) struct ScopedAnnotations {
    /// `@type method`: scope start, method name, method type
    pub(crate) method_types: Vec<(usize, String, MethodType)>,
    /// `@implements`: scope start to the name as written; the last one in a scope wins
    pub(crate) implements: HashMap<usize, String>,
    /// `@dynamic`: scope start to the names
    pub(crate) dynamics: HashMap<usize, Vec<(String, DynamicKind)>>,
}

pub(crate) struct Checker<'environment, 'pr> {
    pub(crate) environment: &'environment Environment,
    pub(crate) options: AnalysisOptions,
    pub(crate) file_index: FileIndex<'pr>,
    pub(crate) line_index: LineIndex,
    pub(crate) analysis: Analysis,
    annotations: Vec<ResolvedAnnotation>,
    pub(crate) scoped_annotations: ScopedAnnotations,
    inline_assertions: HashMap<usize, Type>,
    silent_depth: usize,
    expression_depth: usize,
    method_return_cache: HashMap<usize, CacheState>,
    constant_cache: HashMap<String, CacheState>,
    instance_variable_cache: HashMap<InstanceVariableKey, Type>,
    inferred_instance_variable_writes: HashMap<InstanceVariableKey, Vec<Type>>,
    instance_variable_write_frames: Vec<Vec<(InstanceVariableKey, Type)>>,
    method_return_frames: Vec<Vec<Type>>,
    block_next_frames: Vec<Vec<Type>>,
    jump_expectations: Vec<JumpExpectation>,
    /// The whole assignment being checked; Steep reports `IncompatibleAssignment` on the assignment node
    pub(crate) assignment_span: Option<Span>,
    fresh_variable_count: usize,
    fully_known_cache: HashMap<String, bool>,
    repetition_depth: usize,
    /// Diagnostics Steep reports a second time, kept apart from the removal of duplicates
    repeated_diagnostics: Vec<TypeDiagnostic>,
}

struct Checkpoint {
    type_hints: usize,
    hovers: usize,
    definitions: usize,
    diagnostics: usize,
    receivers: usize,
}

const MAXIMUM_REPETITION_DEPTH: usize = 3;

impl<'environment, 'pr> Checker<'environment, 'pr> {
    pub(crate) fn new(
        environment: &'environment Environment,
        file_index: FileIndex<'pr>,
        line_index: LineIndex,
        annotations: Annotations,
        options: AnalysisOptions,
    ) -> Checker<'environment, 'pr> {
        let resolved_annotations = annotations
            .type_annotations
            .into_iter()
            .map(|annotation| {
                let variable_scope_start = file_index
                    .innermost_scope(annotation.offset, |kind| !matches!(kind, ScopeKind::Block | ScopeKind::Branch))
                    .map(|scope| scope.span.start)
                    .unwrap_or(0);
                let attached_scope = file_index.innermost_scope(annotation.offset, |_| true);
                ResolvedAnnotation {
                    offset: annotation.offset,
                    target: annotation.target,
                    annotated_type: annotation.annotated_type,
                    variable_scope_start,
                    self_scope_start: attached_scope.map(|scope| scope.span.start).unwrap_or(0),
                    attached_kind: attached_scope.map(|scope| scope.kind).unwrap_or(ScopeKind::Program),
                }
            })
            .collect();
        let attached_scope_start = |offset: usize| {
            file_index
                .innermost_scope(offset, |_| true)
                .map(|scope| scope.span.start)
                .unwrap_or(0)
        };
        let mut scoped_annotations = ScopedAnnotations {
            method_types: annotations
                .method_type_annotations
                .into_iter()
                .map(|annotation| {
                    (
                        attached_scope_start(annotation.offset),
                        annotation.name,
                        annotation.method_type,
                    )
                })
                .collect(),
            implements: HashMap::new(),
            dynamics: HashMap::new(),
        };
        for annotation in annotations.implements_annotations {
            scoped_annotations
                .implements
                .insert(attached_scope_start(annotation.offset), annotation.name);
        }
        for annotation in annotations.dynamic_annotations {
            scoped_annotations
                .dynamics
                .entry(attached_scope_start(annotation.offset))
                .or_default()
                .extend(annotation.names);
        }
        let inline_assertions = annotations
            .inline_assertions
            .into_iter()
            .map(|(line, assertion)| (line, assertion.asserted_type))
            .collect();
        Checker {
            environment,
            options,
            file_index,
            line_index,
            analysis: Analysis::default(),
            annotations: resolved_annotations,
            scoped_annotations,
            inline_assertions,
            silent_depth: 0,
            expression_depth: 0,
            method_return_cache: HashMap::new(),
            constant_cache: HashMap::new(),
            instance_variable_cache: HashMap::new(),
            inferred_instance_variable_writes: HashMap::new(),
            instance_variable_write_frames: Vec::new(),
            method_return_frames: Vec::new(),
            block_next_frames: Vec::new(),
            jump_expectations: Vec::new(),
            assignment_span: None,
            fresh_variable_count: 0,
            fully_known_cache: HashMap::new(),
            repetition_depth: 0,
            repeated_diagnostics: Vec::new(),
        }
    }

    pub(crate) fn check_program(&mut self, root: &Node<'pr>) {
        let Some(program) = root.as_program_node() else {
            return;
        };
        let scope_start = crate::file_index::PROGRAM_SCOPE_START;
        let nesting: Rc<Vec<String>> = Rc::new(Vec::new());
        let context = Context {
            self_type: self
                .annotated_self_type(scope_start, &nesting)
                .unwrap_or_else(|| Type::instance("Object")),
            declared_locals: Rc::new(self.local_variable_annotations(scope_start, &nesting)),
            nesting,
            method: None,
        };
        let mut locals = Locals::new();
        self.infer_statements(&program.statements(), None, &context, &mut locals);
        let mut seen = HashSet::new();
        self.analysis.diagnostics.retain(|diagnostic| {
            seen.insert((
                diagnostic.start,
                diagnostic.end,
                diagnostic.code,
                diagnostic.message.clone(),
            ))
        });
        let mut repeated_seen = HashSet::new();
        let repeated: Vec<TypeDiagnostic> = std::mem::take(&mut self.repeated_diagnostics)
            .into_iter()
            .filter(|diagnostic| {
                let key = (diagnostic.start, diagnostic.end, diagnostic.code, diagnostic.message.clone());
                repeated_seen.insert(key.clone()) && seen.contains(&key)
            })
            .collect();
        self.analysis.diagnostics.extend(repeated);
    }

    fn checkpoint(&self) -> Checkpoint {
        Checkpoint {
            type_hints: self.analysis.type_hints.len(),
            hovers: self.analysis.hovers.len(),
            definitions: self.analysis.definitions.len(),
            diagnostics: self.analysis.diagnostics.len(),
            receivers: self.analysis.receivers.len(),
        }
    }

    fn rollback(&mut self, checkpoint: Checkpoint) {
        self.analysis.type_hints.truncate(checkpoint.type_hints);
        self.analysis.hovers.truncate(checkpoint.hovers);
        self.analysis.definitions.truncate(checkpoint.definitions);
        self.analysis.diagnostics.truncate(checkpoint.diagnostics);
        self.analysis.receivers.truncate(checkpoint.receivers);
    }

    /// Infers a loop or block body that may run many times. When a variable from outside gets a new type inside the
    /// body, the body is inferred again starting from the widened types and the first pass is discarded, so that
    /// `last = nil; items.each { |item| last&.size; last = item }` sees `last` as `String?` on every iteration
    /// `fresh_names` are block parameters, which start over on every iteration and are never widened. `relaxed_names`
    /// entered as `untyped` in place of `nil`; when the body assigns them they are widened back to `nil | assigned`
    pub(crate) fn infer_repeatedly<T>(
        &mut self,
        entry: &Locals,
        fresh_names: &[String],
        relaxed_names: &[String],
        mut iteration: impl FnMut(&mut Self, &mut Locals) -> T,
    ) -> (Locals, T) {
        let checkpoint = self.checkpoint();
        let mut iteration_locals = entry.clone();
        let result = iteration(self, &mut iteration_locals);
        if self.repetition_depth >= MAXIMUM_REPETITION_DEPTH {
            return (iteration_locals, result);
        }
        let mut widened = entry.clone();
        let mut changed = false;
        for (name, entry_type) in widened.iter_mut() {
            if fresh_names.contains(name) {
                continue;
            }
            let Some(end_type) = iteration_locals.get(name) else {
                continue;
            };
            if end_type == entry_type {
                continue;
            }
            if relaxed_names.contains(name) {
                *entry_type = Type::optional(end_type.clone());
                changed = true;
                continue;
            }
            let union = Type::union(vec![entry_type.clone(), end_type.clone()]);
            if union != *entry_type {
                *entry_type = union;
                changed = true;
            }
        }
        if !changed {
            return (iteration_locals, result);
        }
        self.rollback(checkpoint);
        self.repetition_depth += 1;
        let mut second_locals = widened;
        let result = iteration(self, &mut second_locals);
        self.repetition_depth -= 1;
        (second_locals, result)
    }

    pub(crate) fn is_silent(&self) -> bool {
        self.silent_depth > 0
    }

    pub(crate) fn record_hint(
        &mut self,
        kind: TypeHintKind,
        offset: usize,
        hint_type: &Type,
        is_obvious: bool,
    ) {
        if self.is_silent() {
            return;
        }
        self.analysis.type_hints.push(TypeHint {
            kind,
            offset,
            hint_type: hint_type.clone(),
            is_obvious,
        });
    }

    pub(crate) fn record_hover(&mut self, span: Span, kind: HoverKind) {
        if self.is_silent() || span.start >= span.end {
            return;
        }
        self.analysis.hovers.push(HoverEntry {
            start: span.start,
            end: span.end,
            kind,
        });
    }

    pub(crate) fn record_type_hover(&mut self, span: Span, hover_type: &Type) {
        if self.is_silent() {
            return;
        }
        self.record_hover(span, HoverKind::Type(hover_type.clone()));
    }

    pub(crate) fn record_receiver(&mut self, span: Span, receiver_type: &Type) {
        if self.is_silent() {
            return;
        }
        self.analysis.receivers.push(ReceiverEntry {
            start: span.start,
            end: span.end,
            receiver_type: receiver_type.clone(),
        });
    }

    pub(crate) fn record_definition(&mut self, span: Span, targets: Vec<Location>) {
        if self.is_silent() || targets.is_empty() || span.start >= span.end {
            return;
        }
        self.analysis.definitions.push(DefinitionEntry {
            start: span.start,
            end: span.end,
            targets,
        });
    }

    pub(crate) fn record_diagnostic(
        &mut self,
        span: Span,
        severity: Severity,
        code: &'static str,
        message: String,
    ) {
        if self.is_silent() {
            return;
        }
        self.analysis.diagnostics.push(TypeDiagnostic {
            start: span.start,
            end: span.end,
            severity,
            code,
            message,
        });
    }

    pub(crate) fn nesting_of_class(name: &str) -> Vec<String> {
        let mut nesting = Vec::new();
        let mut prefix = String::new();
        for segment in name.split("::") {
            if !prefix.is_empty() {
                prefix.push_str("::");
            }
            prefix.push_str(segment);
            nesting.push(prefix.clone());
        }
        nesting
    }

    fn resolve_annotation_type(&self, annotated_type: &Type, nesting: &[String]) -> Type {
        let mut resolved = annotated_type.clone();
        self.environment.resolve_type_names(&mut resolved, nesting);
        resolved
    }

    pub(crate) fn local_variable_annotations(&self, scope_start: usize, nesting: &[String]) -> HashMap<String, Type> {
        self.annotations
            .iter()
            .filter(|annotation| annotation.variable_scope_start == scope_start && annotation.attached_kind != ScopeKind::Branch)
            .filter_map(|annotation| match &annotation.target {
                AnnotationTarget::LocalVariable(name) => Some((
                    name.clone(),
                    self.resolve_annotation_type(&annotation.annotated_type, nesting),
                )),
                _ => None,
            })
            .collect()
    }

    /// The type annotations attached to the scope starting at `scope_start`, in source order
    pub(crate) fn type_annotations_in(&self, scope_start: usize) -> impl Iterator<Item = (&AnnotationTarget, &Type)> {
        self.annotations
            .iter()
            .filter(move |annotation| {
                annotation.self_scope_start == scope_start && annotation.attached_kind != ScopeKind::Branch
            })
            .map(|annotation| (&annotation.target, &annotation.annotated_type))
    }

    /// Steep's `for_branch`: a variable annotated in a branch body has the annotated type there, which must be a
    /// subtype of the type it had (`IncompatibleAnnotation` otherwise). `body_span` is the branch body, where Steep
    /// reports the incompatibility
    pub(crate) fn branch_context(&mut self, body_span: Span, context: &Context, locals: &mut Locals) -> Context {
        let Some(branch_scope) = self.file_index.branch_scopes.get(&body_span.start).copied() else {
            return context.clone();
        };
        let annotated: Vec<(String, Type)> = self
            .annotations
            .iter()
            .filter(|annotation| {
                annotation.attached_kind == ScopeKind::Branch && annotation.self_scope_start == branch_scope.span.start
            })
            .filter_map(|annotation| match &annotation.target {
                AnnotationTarget::LocalVariable(name) => Some((
                    name.clone(),
                    self.resolve_annotation_type(&annotation.annotated_type, &context.nesting),
                )),
                _ => None,
            })
            .collect();
        if annotated.is_empty() {
            return context.clone();
        }
        let mut declared_locals = (*context.declared_locals).clone();
        for (name, annotated_type) in annotated {
            let outer_type = self.local_variable_type(&name, context, locals);
            if !matches!(outer_type, Type::Variable(_) | Type::Untyped)
                && !matches!(annotated_type, Type::Variable(_))
                && !crate::steep_relation::is_subtype(self.environment, &annotated_type, &outer_type)
            {
                let message = crate::steep_relation::with_detail_lines(
                    format!(
                        "Type annotation about `{name}` is incompatible since {} <: {} doesn't hold",
                        annotated_type.steep_display(),
                        outer_type.steep_display()
                    ),
                    self.environment,
                    &annotated_type,
                    &outer_type,
                );
                self.record_diagnostic(body_span, Severity::Error, "Ruby::IncompatibleAnnotation", message);
            }
            locals.insert(name.clone(), annotated_type.clone());
            declared_locals.insert(name, annotated_type);
        }
        let mut branch_context = context.clone();
        branch_context.declared_locals = Rc::new(declared_locals);
        branch_context
    }

    pub(crate) fn annotated_self_type(&self, scope_start: usize, nesting: &[String]) -> Option<Type> {
        self.annotations
            .iter()
            .find(|annotation| {
                annotation.self_scope_start == scope_start
                    && annotation.attached_kind != ScopeKind::Branch
                    && annotation.target == AnnotationTarget::SelfType
            })
            .map(|annotation| self.resolve_annotation_type(&annotation.annotated_type, nesting))
    }

    pub(crate) fn annotated_break_type(&self, scope_start: usize, nesting: &[String]) -> Option<Type> {
        self.annotations
            .iter()
            .find(|annotation| {
                annotation.self_scope_start == scope_start
                    && annotation.attached_kind != ScopeKind::Branch
                    && annotation.target == AnnotationTarget::Break
            })
            .map(|annotation| self.resolve_annotation_type(&annotation.annotated_type, nesting))
    }

    pub(crate) fn instance_variable_annotation(&self, class_name: &str, variable_name: &str) -> Option<Type> {
        self.annotations.iter().find_map(|annotation| {
            let AnnotationTarget::InstanceVariable(name) = &annotation.target else {
                return None;
            };
            if name != variable_name {
                return None;
            }
            let annotated_class = match self.file_index.innermost_scope(annotation.offset, |kind| kind == ScopeKind::Class) {
                Some(class_scope) => self.file_index.class_names_by_offset.get(&class_scope.span.start)?.as_str(),
                None => "Object",
            };
            if annotated_class != class_name {
                return None;
            }
            Some(self.resolve_annotation_type(
                &annotation.annotated_type,
                &Checker::nesting_of_class(class_name),
            ))
        })
    }

    pub(crate) fn inline_assertion_for(
        &self,
        value_end_offset: usize,
        nesting: &[String],
    ) -> Option<Type> {
        let line = self.line_index.line_of(value_end_offset);
        self.inline_assertions
            .get(&line)
            .map(|asserted_type| self.resolve_annotation_type(asserted_type, nesting))
    }

    /// The scopes whose annotations a node spanning `node_span` reads, innermost first: the blocks around it and the
    /// first scope that is not a block. The node's own scope is not one of them
    pub(crate) fn annotation_scopes(&self, node_span: Span) -> Vec<ScopeRange> {
        let mut enclosing: Vec<ScopeRange> = self
            .file_index
            .scopes
            .iter()
            .filter(|scope| {
                scope.kind != ScopeKind::Branch
                    && scope.span.start <= node_span.start
                    && node_span.start < scope.span.end
                    && (scope.span.start, scope.span.end) != (node_span.start, node_span.end)
            })
            .copied()
            .collect();
        enclosing.sort_by_key(|scope| scope.span.end - scope.span.start);
        let first_non_block = enclosing.iter().position(|scope| scope.kind != ScopeKind::Block);
        enclosing.truncate(first_non_block.map_or(enclosing.len(), |index| index + 1));
        enclosing
    }

    /// `@type method name: ...` that a `def` spanning `definition_span` reads
    pub(crate) fn annotated_method_type(
        &self,
        definition_span: Span,
        method_name: &str,
        nesting: &[String],
    ) -> Option<MethodType> {
        self.annotation_scopes(definition_span).iter().find_map(|scope| {
            self.scoped_annotations
                .method_types
                .iter()
                .rev()
                .find(|(scope_start, name, _)| *scope_start == scope.span.start && name == method_name)
                .map(|(_, _, method_type)| {
                    let mut resolved = method_type.clone();
                    self.environment.resolve_method_type_names(&mut resolved, nesting);
                    resolved
                })
        })
    }

    /// The module that `@implements` names in the class or module body starting at `scope_start`, when RBS declares
    /// it, as Steep's `implement_module` resolves it
    pub(crate) fn implemented_module(&self, scope_start: usize, nesting: &[String]) -> Option<String> {
        let written = self.scoped_annotations.implements.get(&scope_start)?;
        let name = self.environment.resolve_constant_name(nesting, written)?;
        self.environment.classes.contains_key(&name).then_some(name)
    }

    pub(crate) fn is_fully_known(&mut self, class_name: &str) -> bool {
        if let Some(known) = self.fully_known_cache.get(class_name) {
            return *known;
        }
        let known = crate::subtyping::is_fully_known_class(self.environment, class_name);
        self.fully_known_cache.insert(class_name.to_string(), known);
        known
    }

    pub(crate) fn silently<T>(&mut self, operation: impl FnOnce(&mut Self) -> T) -> Option<T> {
        if self.silent_depth >= MAXIMUM_SILENT_DEPTH {
            return None;
        }
        self.silent_depth += 1;
        let saved_expression_depth = self.expression_depth;
        let result = operation(self);
        self.expression_depth = saved_expression_depth;
        self.silent_depth -= 1;
        Some(result)
    }
}
