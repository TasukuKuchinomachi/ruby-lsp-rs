use std::rc::Rc;

use rbs_environment::{Location, Type};
use ruby_prism::Node;

use super::{CacheState, Checker, Context, Locals};
use crate::source::{Span, constant_text, qualify};
use crate::{HoverKind, Severity};

enum ConstantKind {
    RbsClass,
    RbsConstant,
    FileClass,
    FileConstant,
}

struct ResolvedConstant {
    name: String,
    kind: ConstantKind,
}

impl ResolvedConstant {
    /// Steep only knows constants from signatures, so one defined only in Ruby code is still unknown to it
    fn is_declared_in_signature(&self) -> bool {
        matches!(
            self.kind,
            ConstantKind::RbsClass | ConstantKind::RbsConstant
        )
    }
}

impl<'pr> Checker<'_, 'pr> {
    fn classify_constant(&self, name: &str) -> Option<ResolvedConstant> {
        if let Some(declared) = self.environment.resolve_class_name(name)
            && self.environment.classes.contains_key(declared)
        {
            return Some(ResolvedConstant {
                name: declared.to_string(),
                kind: ConstantKind::RbsClass,
            });
        }
        if self.environment.constants.contains_key(name) {
            return Some(ResolvedConstant {
                name: name.to_string(),
                kind: ConstantKind::RbsConstant,
            });
        }
        if self.file_index.classes.contains_key(name) {
            return Some(ResolvedConstant {
                name: name.to_string(),
                kind: ConstantKind::FileClass,
            });
        }
        if self.file_index.constants.contains_key(name) {
            return Some(ResolvedConstant {
                name: name.to_string(),
                kind: ConstantKind::FileConstant,
            });
        }
        None
    }

    /// Lexical lookup. When both RBS and this file know a name, the one declared in the more deeply nested scope wins,
    /// as it does in Ruby
    fn resolve_relative_constant(
        &self,
        nesting: &[String],
        relative_name: &str,
    ) -> Option<ResolvedConstant> {
        let from_rbs = self
            .environment
            .resolve_constant_name(nesting, relative_name);
        let from_file = self.file_index.resolve_constant(nesting, relative_name);
        let chosen = match (from_rbs, from_file) {
            (Some(from_rbs), Some(from_file)) => {
                if from_file.matches("::").count() > from_rbs.matches("::").count() {
                    from_file
                } else {
                    from_rbs
                }
            }
            (Some(from_rbs), None) => from_rbs,
            (None, Some(from_file)) => from_file,
            (None, None) => return None,
        };
        self.classify_constant(&chosen)
    }

    fn constant_type(&mut self, resolved: &ResolvedConstant) -> Type {
        match resolved.kind {
            ConstantKind::RbsClass | ConstantKind::FileClass => Type::ClassSingleton {
                name: resolved.name.clone(),
            },
            ConstantKind::RbsConstant => self
                .environment
                .constants
                .get(&resolved.name)
                .map(|constant| constant.constant_type.clone())
                .unwrap_or(Type::Untyped),
            ConstantKind::FileConstant => self.file_constant_type(&resolved.name),
        }
    }

    fn file_constant_type(&mut self, name: &str) -> Type {
        match self.constant_cache.get(name) {
            Some(CacheState::Done(constant_type)) => return constant_type.clone(),
            Some(CacheState::InProgress) => return Type::Untyped,
            None => {}
        }
        let Some(constant) = self.file_index.constants.get(name) else {
            return Type::Untyped;
        };
        let value = prism_child_nodes::copy_node(&constant.value);
        let nesting = Rc::new(constant.nesting.clone());
        let self_type = match nesting.last() {
            Some(namespace) => Type::ClassSingleton {
                name: namespace.clone(),
            },
            None => Type::instance("Object"),
        };
        let context = Context {
            self_type,
            nesting,
            method: None,
            declared_locals: Rc::default(),
        };
        self.constant_cache
            .insert(name.to_string(), CacheState::InProgress);
        let inferred = self
            .silently(|checker| checker.infer(&value, &context, &mut Locals::new()))
            .unwrap_or(Type::Untyped);
        self.constant_cache
            .insert(name.to_string(), CacheState::Done(inferred.clone()));
        inferred
    }

    fn constant_documentation(
        &self,
        resolved: &ResolvedConstant,
    ) -> (Option<String>, Vec<Location>) {
        match resolved.kind {
            ConstantKind::RbsClass => match self.environment.classes.get(&resolved.name) {
                Some(declaration) => (declaration.comment.clone(), declaration.locations.clone()),
                None => (None, Vec::new()),
            },
            ConstantKind::RbsConstant => match self.environment.constants.get(&resolved.name) {
                Some(constant) => (
                    constant.comment.clone(),
                    constant.location.iter().cloned().collect(),
                ),
                None => (None, Vec::new()),
            },
            ConstantKind::FileClass | ConstantKind::FileConstant => (None, Vec::new()),
        }
    }

    fn record_constant_reference(
        &mut self,
        span: Span,
        resolved: &ResolvedConstant,
        constant_type: &Type,
    ) {
        if self.is_silent() {
            return;
        }
        let (comment, locations) = self.constant_documentation(resolved);
        self.record_hover(
            span,
            HoverKind::Constant {
                name: resolved.name.clone(),
                constant_type: constant_type.clone(),
                comment,
            },
        );
        self.record_definition(span, locations);
    }

    fn report_unknown_constant(&mut self, span: Span, name: &str, _nesting: &[String]) {
        if self.is_silent() {
            return;
        }
        self.record_diagnostic(
            span,
            Severity::Warning,
            "Ruby::UnknownConstant",
            format!("Cannot find the declaration of constant: `{name}`"),
        );
    }

    pub(crate) fn infer_constant_reference(
        &mut self,
        node: &Node<'pr>,
        context: &Context,
        locals: &mut Locals,
    ) -> Type {
        match node {
            Node::ConstantReadNode { .. } => {
                let Some(read) = node.as_constant_read_node() else {
                    return Type::Untyped;
                };
                let name = constant_text(&read.name());
                let span = Span::of_node(node);
                match self.resolve_relative_constant(&context.nesting, &name) {
                    Some(resolved) => {
                        if !resolved.is_declared_in_signature() {
                            self.report_unknown_constant(span, &name, &context.nesting);
                        }
                        let constant_type = self.constant_type(&resolved);
                        self.record_constant_reference(span, &resolved, &constant_type);
                        constant_type
                    }
                    None => {
                        self.report_unknown_constant(span, &name, &context.nesting);
                        Type::Untyped
                    }
                }
            }
            Node::ConstantPathNode { .. } => {
                let Some(path) = node.as_constant_path_node() else {
                    return Type::Untyped;
                };
                let Some(name) = path.name().map(|name| constant_text(&name)) else {
                    return Type::Untyped;
                };
                let name_span = Span::of_location(&path.name_loc());
                let parent_name = match path.parent() {
                    None => Some(String::new()),
                    Some(parent) => {
                        let parent_type = self.infer(&parent, context, locals);
                        match parent_type {
                            Type::ClassSingleton { name } => Some(name),
                            _ => None,
                        }
                    }
                };
                let Some(parent_name) = parent_name else {
                    return Type::Untyped;
                };
                let full_name = self
                    .constant_in_ancestors(&parent_name, &name)
                    .unwrap_or_else(|| qualify(&parent_name, &name));
                match self.classify_constant(&full_name) {
                    Some(resolved) => {
                        if !resolved.is_declared_in_signature() {
                            self.report_unknown_constant(name_span, &full_name, &[]);
                        }
                        let constant_type = self.constant_type(&resolved);
                        self.record_constant_reference(name_span, &resolved, &constant_type);
                        constant_type
                    }
                    None => {
                        let parent_is_known = parent_name.is_empty()
                            || self.environment.classes.contains_key(&parent_name);
                        if parent_is_known {
                            let lookup_nesting = if parent_name.is_empty() {
                                Vec::new()
                            } else {
                                vec![parent_name.clone()]
                            };
                            self.report_unknown_constant(name_span, &full_name, &lookup_nesting);
                        }
                        Type::Untyped
                    }
                }
            }
            _ => Type::Untyped,
        }
    }

    /// `Parent::NAME` also finds a constant declared in an ancestor of `Parent`, as Ruby and Steep do
    fn constant_in_ancestors(&self, parent_name: &str, name: &str) -> Option<String> {
        if parent_name.is_empty() {
            return None;
        }
        let direct = qualify(parent_name, name);
        if self.classify_constant(&direct).is_some() {
            return Some(direct);
        }
        let declared_parent = self.environment.resolve_class_name(parent_name)?;
        self.environment
            .ancestors(declared_parent)
            .into_iter()
            .map(|ancestor| qualify(&ancestor.name, name))
            .find(|candidate| self.classify_constant(candidate).is_some())
    }

    pub(crate) fn infer_constant_write(
        &mut self,
        node: &Node<'pr>,
        context: &Context,
        locals: &mut Locals,
    ) -> Type {
        match node {
            Node::ConstantWriteNode { .. } => {
                let Some(write) = node.as_constant_write_node() else {
                    return Type::Untyped;
                };
                let value_type = self.infer(&write.value(), context, locals);
                let name = qualify(context.namespace(), &constant_text(&write.name()));
                if !self.constant_cache.contains_key(&name) {
                    self.constant_cache
                        .insert(name.clone(), CacheState::Done(value_type.clone()));
                }
                let resolved = self.classify_constant(&name).unwrap_or(ResolvedConstant {
                    name,
                    kind: ConstantKind::FileConstant,
                });
                if matches!(resolved.kind, ConstantKind::RbsConstant)
                    && let Some(declared) = self
                        .environment
                        .constants
                        .get(&resolved.name)
                        .map(|constant| constant.constant_type.clone())
                {
                    let snapshot = locals.clone();
                    self.assignment_span = Some(Span::of_node(node));
                    self.report_incompatible_assignment(
                        Span::of_location(&write.name_loc()),
                        &declared,
                        &value_type,
                        Some(&write.value()),
                        "a constant",
                        context,
                        &snapshot,
                    );
                    self.assignment_span = None;
                }
                self.record_constant_reference(
                    Span::of_location(&write.name_loc()),
                    &resolved,
                    &value_type,
                );
                value_type
            }
            _ => {
                let mut value_type = Type::Untyped;
                for child in prism_child_nodes::child_nodes(node) {
                    if matches!(child, Node::ConstantPathNode { .. }) {
                        continue;
                    }
                    value_type = self.infer(&child, context, locals);
                }
                value_type
            }
        }
    }
}
