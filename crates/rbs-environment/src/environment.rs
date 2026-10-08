use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use crate::types::{MethodType, Type, TypeName, TypeParameter};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct SourcePosition {
    /// 0-based
    pub line: u32,
    pub byte_column: u32,
    pub utf16_column: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Location {
    pub path: Arc<PathBuf>,
    pub start: SourcePosition,
    pub end: SourcePosition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassKind {
    Class,
    Module,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visibility {
    Public,
    Private,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MethodKind {
    Instance,
    Singleton,
}

/// A reference to a module or class together with type arguments, e.g. `include Enumerable[Elem]` or `< Array[String]`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ancestor {
    pub name: TypeName,
    pub arguments: Vec<Type>,
}

#[derive(Debug, Clone)]
pub struct MethodDefinition {
    pub name: String,
    pub owner: TypeName,
    pub kind: MethodKind,
    pub overloads: Vec<MethodType>,
    pub visibility: Visibility,
    /// `alias new_name old_name` is stored under `new_name` with `alias_of: Some(old_name)` and no overloads
    pub alias_of: Option<String>,
    pub location: Option<Location>,
    pub comment: Option<String>,
    /// Contents of `%a{...}` on the definition and on each overload, e.g. `pure`
    pub annotations: Vec<String>,
    /// Defined by `attr_reader` or `attr_accessor`
    pub is_attribute_reader: bool,
}

#[derive(Debug, Clone)]
pub struct ClassDeclaration {
    pub name: TypeName,
    pub kind: ClassKind,
    pub type_parameters: Vec<TypeParameter>,
    /// `None` for modules, `BasicObject` and classes without an explicit superclass (the loader fills `Object` for the latter)
    pub super_class: Option<Ancestor>,
    pub includes: Vec<Ancestor>,
    pub prepends: Vec<Ancestor>,
    pub extends: Vec<Ancestor>,
    /// `module Foo : Bar`
    pub module_self_types: Vec<Ancestor>,
    pub instance_methods: HashMap<String, MethodDefinition>,
    pub singleton_methods: HashMap<String, MethodDefinition>,
    pub instance_variables: HashMap<String, Type>,
    pub class_instance_variables: HashMap<String, Type>,
    pub class_variables: HashMap<String, Type>,
    pub locations: Vec<Location>,
    pub comment: Option<String>,
}

#[derive(Debug, Clone)]
pub struct InterfaceDeclaration {
    pub name: TypeName,
    pub type_parameters: Vec<TypeParameter>,
    pub includes: Vec<Ancestor>,
    pub methods: HashMap<String, MethodDefinition>,
    pub location: Option<Location>,
    pub comment: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TypeAliasDeclaration {
    pub name: TypeName,
    pub type_parameters: Vec<TypeParameter>,
    pub aliased_type: Type,
    pub location: Option<Location>,
}

#[derive(Debug, Clone)]
pub struct ConstantDeclaration {
    pub name: TypeName,
    pub constant_type: Type,
    pub location: Option<Location>,
    pub comment: Option<String>,
}

#[derive(Debug, Clone)]
pub struct GlobalDeclaration {
    pub name: String,
    pub global_type: Type,
    pub location: Option<Location>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadError {
    pub path: PathBuf,
    pub message: String,
}

/// A method found by lookup. `overloads` are expressed in terms of the type parameters of the class the lookup
/// started from: type arguments given through `include`/`<` along the ancestor chain are already substituted, while
/// `self`, `instance` and `class` are left for the caller
#[derive(Debug, Clone)]
pub struct ResolvedMethod<'a> {
    pub definition: &'a MethodDefinition,
    pub overloads: Vec<MethodType>,
}

/// Everything known from RBS. Shared between threads behind `Arc`, so any cache must be internally synchronized
#[derive(Debug, Default)]
pub struct Environment {
    pub classes: HashMap<TypeName, ClassDeclaration>,
    pub interfaces: HashMap<TypeName, InterfaceDeclaration>,
    pub type_aliases: HashMap<TypeName, TypeAliasDeclaration>,
    /// `class Foo = Bar` and `module Foo = Bar`, new name to old name
    pub class_aliases: HashMap<TypeName, TypeName>,
    pub constants: HashMap<TypeName, ConstantDeclaration>,
    pub globals: HashMap<String, GlobalDeclaration>,
    pub errors: Vec<LoadError>,
    /// Problems in the project signature files, sorted by path and position
    pub signature_diagnostics: Vec<crate::signature_validation::SignatureDiagnostic>,
    pub(crate) ancestor_cache: crate::lookup::AncestorCache,
}
