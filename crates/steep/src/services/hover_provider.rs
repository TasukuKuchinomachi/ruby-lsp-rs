use std::{path::Path, sync::Arc};

use rbs_definition::{
    ast::{Declaration, Type as RbsType, TypeName, TypeNameKind, UseClause},
    definition_builder::{DefinitionMember, Method},
    environment::{ClassOrModuleDeclaration, ConstantEntry, Environment},
};
use steep_interface::{method_type::MethodType, types::Type};

use crate::{
    locator::{self, RubyResult},
    method_name::MethodName,
    rbs_locator::{self, Component, Key},
    ruby::node::SourceRange,
    services::type_check_service::{TypeCheckService, TypeCheckedSource},
    type_inference::{
        constant_env::nesting_names,
        context::{Context, MethodContext},
        method_call::{MethodCall, MethodCallKind, MethodDeclaration},
    },
};

#[derive(Clone, Debug)]
pub enum CallKind {
    Typed { actual_method_type: MethodType },
    Special { actual_method_type: MethodType },
    Error,
}

#[derive(Clone, Debug)]
pub struct HoverMethodCall {
    pub kind: CallKind,
    pub method_name: String,
    pub receiver_type: Type,
    pub return_type: Type,
    pub method_decls: Vec<MethodDeclaration>,
}

#[derive(Clone, Debug)]
pub enum HoverContent {
    Type {
        location: SourceRange,
        type_: Type,
    },
    Variable {
        location: SourceRange,
        name: String,
        type_: Type,
    },
    TypeAssertion {
        location: SourceRange,
        original_type: Type,
        asserted_type: Type,
    },
    MethodCall {
        location: SourceRange,
        method_call: HoverMethodCall,
    },
    Definition {
        location: SourceRange,
        method_name: MethodName,
        method_type: MethodType,
        definition: Arc<Method>,
    },
    Constant {
        location: SourceRange,
        full_name: TypeName,
        type_: Type,
        declaration: Declaration,
        comments: Vec<String>,
    },
    TypeAlias {
        location: SourceRange,
        declaration: Declaration,
    },
    InterfaceType {
        location: SourceRange,
        declaration: Declaration,
    },
    ClassType {
        location: SourceRange,
        declaration: Declaration,
    },
}

impl HoverContent {
    pub fn location(&self) -> SourceRange {
        match self {
            HoverContent::Type { location, .. }
            | HoverContent::Variable { location, .. }
            | HoverContent::TypeAssertion { location, .. }
            | HoverContent::MethodCall { location, .. }
            | HoverContent::Definition { location, .. }
            | HoverContent::Constant { location, .. }
            | HoverContent::TypeAlias { location, .. }
            | HoverContent::InterfaceType { location, .. }
            | HoverContent::ClassType { location, .. } => *location,
        }
    }
}

pub fn declaration_comment(declaration: &Declaration) -> Option<&str> {
    match declaration {
        Declaration::Class(class) => class.comment.as_deref(),
        Declaration::Module(module) => module.comment.as_deref(),
        Declaration::Interface(interface) => interface.comment.as_deref(),
        Declaration::TypeAlias(alias) => alias.comment.as_deref(),
        Declaration::Constant(constant) => constant.comment.as_deref(),
        Declaration::Global(global) => global.comment.as_deref(),
        Declaration::ClassAlias(alias) => alias.comment.as_deref(),
    }
}

pub fn class_declaration(declaration: &ClassOrModuleDeclaration) -> Declaration {
    match declaration {
        ClassOrModuleDeclaration::Class(class) => Declaration::Class(class.clone()),
        ClassOrModuleDeclaration::Module(module) => Declaration::Module(module.clone()),
    }
}

pub fn constant_declaration(entry: ConstantEntry) -> (Declaration, Vec<String>) {
    match entry {
        ConstantEntry::Class(entry) => (
            class_declaration(entry.primary_declaration()),
            entry
                .declarations()
                .filter_map(|declaration| {
                    declaration_comment(&class_declaration(declaration)).map(str::to_string)
                })
                .collect(),
        ),
        ConstantEntry::ClassAlias(entry) => (
            Declaration::ClassAlias(entry.declaration.clone()),
            entry.declaration.comment.iter().cloned().collect(),
        ),
        ConstantEntry::Constant(entry) => (
            Declaration::Constant(entry.declaration.clone()),
            entry.declaration.comment.iter().cloned().collect(),
        ),
    }
}

pub fn type_name_content(
    environment: &Environment,
    type_name: &TypeName,
    location: SourceRange,
) -> Option<HoverContent> {
    match type_name.kind() {
        TypeNameKind::Class => {
            let declaration = match environment.constant_entry(type_name)? {
                ConstantEntry::Class(entry) => class_declaration(entry.primary_declaration()),
                ConstantEntry::ClassAlias(entry) => {
                    Declaration::ClassAlias(entry.declaration.clone())
                }
                ConstantEntry::Constant(_) => return None,
            };
            Some(HoverContent::ClassType {
                location,
                declaration,
            })
        }
        TypeNameKind::Interface => environment
            .interface_declarations
            .get(type_name)
            .map(|entry| HoverContent::InterfaceType {
                location,
                declaration: Declaration::Interface(entry.declaration.clone()),
            }),
        TypeNameKind::Alias => environment
            .type_alias_declarations
            .get(type_name)
            .map(|entry| HoverContent::TypeAlias {
                location,
                declaration: Declaration::TypeAlias(entry.declaration.clone()),
            }),
    }
}

fn hover_method_call(call: &MethodCall) -> Option<HoverMethodCall> {
    let kind = match &call.kind {
        MethodCallKind::Typed { actual_method_type } => CallKind::Typed {
            actual_method_type: actual_method_type.clone(),
        },
        MethodCallKind::Special { actual_method_type } => CallKind::Special {
            actual_method_type: actual_method_type.clone(),
        },
        MethodCallKind::Error { .. } => CallKind::Error,
        MethodCallKind::Untyped | MethodCallKind::NoMethodError { .. } => return None,
    };
    if call.method_decls.is_empty() {
        return None;
    }
    Some(HoverMethodCall {
        kind,
        method_name: call.method_name.clone(),
        receiver_type: call.receiver_type.clone(),
        return_type: call.return_type.clone(),
        method_decls: call.method_decls.clone(),
    })
}

fn method_name_from_method(
    checked: &TypeCheckedSource,
    method_context: &MethodContext,
    method: &Method,
) -> Option<MethodName> {
    let defined_in = method.defined_in();
    let method_name = method_context.name.clone()?;
    let definition_builder = &checked.checker.builder.definition_builder;
    if defined_in.is_class() {
        if definition_builder
            .build_instance(defined_in)
            .ok()?
            .methods
            .contains_key(&method_name)
        {
            Some(MethodName::instance(defined_in.clone(), method_name))
        } else if definition_builder
            .build_singleton(defined_in)
            .ok()?
            .methods
            .contains_key(&method_name)
        {
            Some(MethodName::singleton(defined_in.clone(), method_name))
        } else {
            None
        }
    } else {
        Some(MethodName::instance(defined_in.clone(), method_name))
    }
}

pub fn type_variable_names(context: &Context) -> Vec<String> {
    context.variable_context.table.keys().cloned().collect()
}

fn ruby_content(checked: &TypeCheckedSource, position: usize) -> Option<HoverContent> {
    let typing = &checked.typing;
    let locator = locator::Ruby::new(&checked.source);
    let cursor_context = typing.cursor_context();
    match locator.find(position)? {
        RubyResult::Node(result) => {
            let node = &result.node;
            let context = cursor_context.context()?;
            match node.type_ {
                "lvar" => {
                    let name = node.child_symbol(0).unwrap_or_default();
                    let type_ = context.type_env.get(name).unwrap_or(Type::Any);
                    return Some(HoverContent::Variable {
                        location: node.location.name?,
                        name: name.to_string(),
                        type_,
                    });
                }
                "lvasgn" => {
                    let name = node.child_symbol(0).unwrap_or_default();
                    let type_ = match context.type_env.get(name) {
                        Some(type_) => type_,
                        None => typing.type_of(node.id)?,
                    };
                    return Some(HoverContent::Variable {
                        location: node.location.name?,
                        name: name.to_string(),
                        type_,
                    });
                }
                "send" | "csend" => {
                    let result_node = match result.parents.first() {
                        Some(parent)
                            if matches!(parent.type_, "block" | "numblock" | "itblock")
                                && parent.child_node(0).is_some_and(|child| child == node) =>
                        {
                            parent
                        }
                        _ => node,
                    };
                    let call = typing.call_of(result_node.id)?;
                    if let Some(method_call) = hover_method_call(&call) {
                        return Some(HoverContent::MethodCall {
                            location: node.location.selector?,
                            method_call,
                        });
                    }
                }
                "def" | "defs" => {
                    if let Some(method_context) = &context.method_context
                        && let Some(method) = &method_context.method
                        && let Some(method_type) = &method_context.method_type
                    {
                        let method_name = method_name_from_method(checked, method_context, method)?;
                        return Some(HoverContent::Definition {
                            location: node.location.name?,
                            method_name,
                            method_type: method_type.clone(),
                            definition: method.clone(),
                        });
                    }
                }
                "const" | "casgn" => {
                    let type_ = typing.type_of(node.id)?;
                    let constant_name = typing.source_index().reference(node.id).cloned();
                    if let Some(constant_name) = constant_name {
                        let environment = checked.checker.environment();
                        let entry = environment.constant_entry(&constant_name)?;
                        let (declaration, comments) = constant_declaration(entry);
                        return Some(HoverContent::Constant {
                            location: node.location.name?,
                            full_name: constant_name,
                            type_,
                            declaration,
                            comments,
                        });
                    }
                }
                _ => {}
            }
            Some(HoverContent::Type {
                location: node.expression(),
                type_: typing.type_of(node.id)?,
            })
        }
        RubyResult::TypeAssertion { assertion, node } => {
            let context = cursor_context.context()?;
            let nesting = nesting_names(&context.module_context.nesting);
            let rbs_type = assertion
                .rbs_type(&nesting, &checked.checker)
                .ok()
                .flatten();
            if let Some(rbs_type) = rbs_type
                && let Some((name, location)) = locator::type_name_at(position, &rbs_type)
                && let Some(content) =
                    type_name_content(checked.checker.environment(), &name, location)
            {
                return Some(content);
            }
            let assertion_node = &node.node;
            let original_node = assertion_node.child_node(0)?;
            let original_type = typing.type_of(original_node.id)?;
            let asserted_type = typing.type_of(assertion_node.id)?;
            if original_type != asserted_type {
                Some(HoverContent::TypeAssertion {
                    location: assertion_node.expression(),
                    original_type,
                    asserted_type,
                })
            } else {
                Some(HoverContent::Type {
                    location: assertion_node.expression(),
                    type_: asserted_type,
                })
            }
        }
        RubyResult::TypeApplication { application, .. } => {
            let context = cursor_context.context()?;
            let nesting = nesting_names(&context.module_context.nesting);
            for rbs_type in application.rbs_types(&nesting, &checked.checker).ok()? {
                if rbs_type.location.begin_pos <= position && position <= rbs_type.location.end_pos
                {
                    let (name, location) = locator::type_name_at(position, &rbs_type)?;
                    return type_name_content(checked.checker.environment(), &name, location);
                }
            }
            None
        }
        RubyResult::Annotation { .. } | RubyResult::Comment { .. } => None,
    }
}

fn rbs_content(
    service: &TypeCheckService,
    target_name: &str,
    path: &Path,
    position: usize,
) -> Option<HoverContent> {
    let environment = service
        .signature_services
        .get(target_name)?
        .latest_environment()?;
    let absolute_path = service.project.absolute_path(path);
    let source = environment
        .sources
        .iter()
        .find(|source| source.path == absolute_path)?;
    let locator = rbs_locator::Locator::new(&source.directives, &source.declarations);
    let (key, components) = locator.find(position);
    let head = components.first()?;
    let location = |location: Option<rbs_definition::location::Location>| {
        location.map(|location| SourceRange::new(location.start(), location.end()))
    };
    match head {
        Component::Type(type_) => match type_ {
            RbsType::Alias {
                name,
                location: type_location,
                ..
            } => type_name_content(
                environment,
                name,
                location(type_location.location().cloned())?,
            ),
            RbsType::ClassInstance {
                name,
                location: type_location,
                ..
            }
            | RbsType::ClassSingleton {
                name,
                location: type_location,
                ..
            } => {
                if key == Some(Key::Name) {
                    type_name_content(environment, name, location(type_location.name().cloned())?)
                } else {
                    None
                }
            }
            RbsType::Interface {
                name,
                location: type_location,
                ..
            } => type_name_content(environment, name, location(type_location.name().cloned())?),
            _ => None,
        },
        Component::Declaration(Declaration::ClassAlias(alias)) if key == Some(Key::OldName) => {
            type_name_content(
                environment,
                &alias.old_name,
                location(alias.old_name_location.clone())?,
            )
        }
        Component::UseClause(UseClause::Single {
            type_name,
            type_name_location,
            ..
        }) if key == Some(Key::TypeName) => type_name_content(
            environment,
            &type_name.absolute(),
            location(type_name_location.location())?,
        ),
        Component::UseClause(UseClause::Wildcard {
            namespace,
            namespace_location,
            ..
        }) if key == Some(Key::Namespace) => {
            let type_name = rbs_definition::ast::Namespace {
                absolute: true,
                path: namespace.path.clone(),
            }
            .to_type_name();
            type_name_content(
                environment,
                &type_name,
                location(namespace_location.location())?,
            )
        }
        _ => None,
    }
}

pub fn definition_member_location(
    member: &DefinitionMember,
) -> Option<&rbs_definition::location::Location> {
    match member {
        DefinitionMember::MethodDefinition(definition) => definition.location.as_ref(),
        DefinitionMember::Attribute(attribute) => attribute.location.as_ref(),
    }
}

pub fn content_for(
    service: &TypeCheckService,
    path: &Path,
    position: usize,
) -> Option<HoverContent> {
    let project = &service.project;
    if project.target_for_inline_source_path(path).is_some() {
        if let Some((type_name, location)) = service.inline_type_name_at(path, position)
            && let Some(environment) = service
                .project
                .target_for_inline_source_path(path)
                .and_then(|target| service.signature_services.get(&target.name))
                .and_then(|signature| signature.latest_environment())
            && let Some(content) = type_name_content(environment, &type_name, location)
        {
            return Some(content);
        }
        let checked = service.type_check_at(path, position)?;
        return ruby_content(&checked, position);
    }
    if project.target_for_source_path(path).is_some() {
        let checked = service.type_check_at(path, position)?;
        return ruby_content(&checked, position);
    }
    if let Some(target) = project.target_for_signature_path(path) {
        return rbs_content(service, &target.name, path, position);
    }
    if service.source_target(path).is_some() {
        let checked = service.type_check_at(path, position)?;
        return ruby_content(&checked, position);
    }
    None
}
