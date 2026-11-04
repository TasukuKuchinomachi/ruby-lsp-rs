use rbs_definition::{
    ast::{
        AttributeKind, ConstantDeclaration, Declaration, GlobalDeclaration, Member, MemberKind,
        MethodKind, TypeName,
    },
    collections::IndexMap,
    environment::Environment,
    location::Location,
};
use std::sync::Arc;

use crate::method_name::MethodName;

#[derive(Clone, Debug)]
pub enum MethodDeclaration {
    MethodDefinition(rbs_definition::ast::MethodDefinition),
    Alias(rbs_definition::ast::Alias),
    Attribute(rbs_definition::ast::Attribute),
}

impl MethodDeclaration {
    pub fn location(&self) -> Option<&Location> {
        match self {
            MethodDeclaration::MethodDefinition(definition) => definition.location.as_ref(),
            MethodDeclaration::Alias(alias) => alias.location.as_ref(),
            MethodDeclaration::Attribute(attribute) => attribute.location.as_ref(),
        }
    }

    pub fn name_location(&self) -> Option<&Location> {
        match self {
            MethodDeclaration::MethodDefinition(definition) => definition.name_location.as_ref(),
            MethodDeclaration::Alias(alias) => alias.new_name_location.as_ref(),
            MethodDeclaration::Attribute(attribute) => attribute.name_location.as_ref(),
        }
    }
}

#[derive(Default)]
pub struct RbsIndex {
    pub type_index: IndexMap<TypeName, Vec<Declaration>>,
    pub method_index: IndexMap<MethodName, Vec<MethodDeclaration>>,
    pub constant_index: IndexMap<TypeName, Vec<Arc<ConstantDeclaration>>>,
    pub global_index: IndexMap<String, Vec<Arc<GlobalDeclaration>>>,
}

fn same_location(left: Option<&Location>, right: Option<&Location>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => left.same_range(right),
        _ => false,
    }
}

impl RbsIndex {
    pub fn build(environment: &Environment) -> RbsIndex {
        let mut index = RbsIndex::default();
        for (name, entry) in &environment.class_declarations {
            for declaration in entry.declarations() {
                let (declaration_value, members) = match declaration {
                    rbs_definition::environment::ClassOrModuleDeclaration::Class(class) => {
                        (Declaration::Class(class.clone()), &class.members)
                    }
                    rbs_definition::environment::ClassOrModuleDeclaration::Module(module) => {
                        (Declaration::Module(module.clone()), &module.members)
                    }
                };
                index.add_type_declaration(name, declaration_value);
                let declared_in_ruby = declaration.declared_in_ruby();
                for member in members {
                    index.member(name, member, declared_in_ruby);
                }
            }
        }
        for (name, entry) in &environment.class_alias_declarations {
            index.add_type_declaration(name, Declaration::ClassAlias(entry.declaration.clone()));
        }
        for (name, entry) in &environment.interface_declarations {
            index.add_type_declaration(name, Declaration::Interface(entry.declaration.clone()));
            for member in &entry.declaration.members {
                index.member(name, member, false);
            }
        }
        for (name, entry) in &environment.type_alias_declarations {
            index.add_type_declaration(name, Declaration::TypeAlias(entry.declaration.clone()));
        }
        for (name, entry) in &environment.constant_declarations {
            index
                .constant_index
                .entry(name.clone())
                .or_default()
                .push(entry.declaration.clone());
        }
        for (name, declaration) in &environment.global_declarations {
            index
                .global_index
                .entry(name.clone())
                .or_default()
                .push(declaration.clone());
        }
        index
    }

    fn add_type_declaration(&mut self, name: &TypeName, declaration: Declaration) {
        self.type_index
            .entry(name.clone())
            .or_default()
            .push(declaration);
    }

    fn add_method_declaration(&mut self, name: MethodName, declaration: MethodDeclaration) {
        let declarations = self.method_index.entry(name).or_default();
        if !declarations
            .iter()
            .any(|existing| same_location(existing.location(), declaration.location()))
        {
            declarations.push(declaration);
        }
    }

    fn member(&mut self, type_name: &TypeName, member: &Member, declared_in_ruby: bool) {
        match member {
            Member::MethodDefinition(definition) if declared_in_ruby => {
                self.add_method_declaration(
                    MethodName::instance(type_name.clone(), definition.name.clone()),
                    MethodDeclaration::MethodDefinition(definition.clone()),
                );
            }
            _ if declared_in_ruby => {}
            Member::MethodDefinition(definition) => {
                if matches!(
                    definition.kind,
                    MethodKind::Instance | MethodKind::SingletonInstance
                ) {
                    self.add_method_declaration(
                        MethodName::instance(type_name.clone(), definition.name.clone()),
                        MethodDeclaration::MethodDefinition(definition.clone()),
                    );
                }
                if matches!(
                    definition.kind,
                    MethodKind::Singleton | MethodKind::SingletonInstance
                ) {
                    self.add_method_declaration(
                        MethodName::singleton(type_name.clone(), definition.name.clone()),
                        MethodDeclaration::MethodDefinition(definition.clone()),
                    );
                }
            }
            Member::Attribute(attribute) => {
                let method_name = |name: String| match attribute.kind {
                    MemberKind::Instance => MethodName::instance(type_name.clone(), name),
                    MemberKind::Singleton => MethodName::singleton(type_name.clone(), name),
                };
                if matches!(
                    attribute.attribute_kind,
                    AttributeKind::Reader | AttributeKind::Accessor
                ) {
                    self.add_method_declaration(
                        method_name(attribute.name.clone()),
                        MethodDeclaration::Attribute(attribute.clone()),
                    );
                }
                if matches!(
                    attribute.attribute_kind,
                    AttributeKind::Writer | AttributeKind::Accessor
                ) {
                    self.add_method_declaration(
                        method_name(format!("{}=", attribute.name)),
                        MethodDeclaration::Attribute(attribute.clone()),
                    );
                }
            }
            Member::Alias(alias) => {
                let method_name = match alias.kind {
                    MemberKind::Instance => {
                        MethodName::instance(type_name.clone(), alias.new_name.clone())
                    }
                    MemberKind::Singleton => {
                        MethodName::singleton(type_name.clone(), alias.new_name.clone())
                    }
                };
                self.add_method_declaration(method_name, MethodDeclaration::Alias(alias.clone()));
            }
            _ => {}
        }
    }

    pub fn type_declarations(&self, type_name: &TypeName) -> &[Declaration] {
        self.type_index
            .get(type_name)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    pub fn method_declarations(&self, method_name: &MethodName) -> &[MethodDeclaration] {
        self.method_index
            .get(method_name)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }
}
