use std::{path::PathBuf, sync::Arc};

use crate::ast::{
    AliasDeclaration, Block, Callable, ClassDeclaration, ClassOrModule, ConstantDeclaration,
    Declaration, Directive, Function, GlobalDeclaration, InterfaceDeclaration, Member,
    MethodDefinition, MethodType, Mixin, ModuleDeclaration, ModuleSelf, Namespace, Overload,
    Parameter, RecordField, Super, Type, TypeAliasDeclaration, TypeName, TypeNameKind,
    TypeParameter, UseClause,
};
use crate::{
    collections::{HashMap, HashSet, IndexMap},
    errors::RbsError,
    location::Location,
    parallel::map_in_parallel,
    substitution::Substitution,
};

pub type Context = Vec<TypeName>;

#[derive(Clone, Debug)]
pub struct Source {
    pub path: PathBuf,
    pub directives: Vec<Directive>,
    pub declarations: Vec<Declaration>,
}

#[derive(Clone, Debug)]
pub enum ClassOrModuleDeclaration {
    Class(Arc<ClassDeclaration>),
    Module(Arc<ModuleDeclaration>),
}

impl ClassOrModuleDeclaration {
    pub fn name(&self) -> &TypeName {
        match self {
            ClassOrModuleDeclaration::Class(class) => &class.name,
            ClassOrModuleDeclaration::Module(module) => &module.name,
        }
    }

    pub fn type_parameters(&self) -> &[TypeParameter] {
        match self {
            ClassOrModuleDeclaration::Class(class) => &class.type_parameters,
            ClassOrModuleDeclaration::Module(module) => &module.type_parameters,
        }
    }

    pub fn members(&self) -> &[Member] {
        match self {
            ClassOrModuleDeclaration::Class(class) => &class.members,
            ClassOrModuleDeclaration::Module(module) => &module.members,
        }
    }

    pub fn declared_in_ruby(&self) -> bool {
        match self {
            ClassOrModuleDeclaration::Class(class) => class.declared_in_ruby,
            ClassOrModuleDeclaration::Module(module) => module.declared_in_ruby,
        }
    }

    pub fn super_class(&self) -> Option<&Super> {
        match self {
            ClassOrModuleDeclaration::Class(class) => class.super_class.as_ref(),
            ClassOrModuleDeclaration::Module(_) => None,
        }
    }

    pub fn self_types(&self) -> &[ModuleSelf] {
        match self {
            ClassOrModuleDeclaration::Class(_) => &[],
            ClassOrModuleDeclaration::Module(module) => &module.self_types,
        }
    }

    pub fn location(&self) -> Option<&Location> {
        match self {
            ClassOrModuleDeclaration::Class(class) => class.location.as_ref(),
            ClassOrModuleDeclaration::Module(module) => module.location.as_ref(),
        }
    }

    pub fn name_location(&self) -> Option<&Location> {
        match self {
            ClassOrModuleDeclaration::Class(class) => class.name_location.as_ref(),
            ClassOrModuleDeclaration::Module(module) => module.name_location.as_ref(),
        }
    }

    pub fn annotations(&self) -> &[String] {
        match self {
            ClassOrModuleDeclaration::Class(class) => &class.annotations,
            ClassOrModuleDeclaration::Module(module) => &module.annotations,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ClassEntry {
    pub name: TypeName,
    pub kind: ClassOrModule,
    pub declarations: Vec<(Context, ClassOrModuleDeclaration)>,
}

impl ClassEntry {
    pub fn declarations(&self) -> impl DoubleEndedIterator<Item = &ClassOrModuleDeclaration> {
        self.declarations.iter().map(|(_, declaration)| declaration)
    }

    pub fn primary_declaration(&self) -> &ClassOrModuleDeclaration {
        self.declarations()
            .find(|declaration| declaration.super_class().is_some())
            .or_else(|| self.declarations().next())
            .expect("a class entry has a declaration")
    }

    pub fn type_parameters(&self) -> &[TypeParameter] {
        self.primary_declaration().type_parameters()
    }

    pub fn type_parameter_names(&self) -> Vec<String> {
        self.type_parameters()
            .iter()
            .map(|parameter| parameter.name.clone())
            .collect()
    }

    pub fn align_parameters(&self, declaration: &ClassOrModuleDeclaration) -> Option<Substitution> {
        let entry_names = self.type_parameter_names();
        let declaration_names: Vec<String> = declaration
            .type_parameters()
            .iter()
            .map(|parameter| parameter.name.clone())
            .collect();
        if declaration_names == entry_names {
            return None;
        }
        let variables: Vec<Type> = entry_names.into_iter().map(Type::Variable).collect();
        Some(Substitution::build(&declaration_names, &variables))
    }
}

#[derive(Clone, Debug)]
pub struct Entry<T> {
    pub name: TypeName,
    pub declaration: Arc<T>,
    pub context: Context,
}

#[derive(Default, Debug)]
pub struct Environment {
    pub sources: Vec<Source>,
    pub class_declarations: IndexMap<TypeName, ClassEntry>,
    pub interface_declarations: IndexMap<TypeName, Entry<InterfaceDeclaration>>,
    pub type_alias_declarations: IndexMap<TypeName, Entry<TypeAliasDeclaration>>,
    pub constant_declarations: IndexMap<TypeName, Entry<ConstantDeclaration>>,
    pub global_declarations: IndexMap<String, Arc<GlobalDeclaration>>,
    pub class_alias_declarations: IndexMap<TypeName, Entry<AliasDeclaration>>,
}

#[derive(Clone, Copy)]
pub enum ConstantEntry<'environment> {
    Class(&'environment ClassEntry),
    ClassAlias(&'environment Entry<AliasDeclaration>),
    Constant(&'environment Entry<ConstantDeclaration>),
}

pub enum NormalizedModuleName {
    Found(TypeName),
    Cyclic,
    Unknown,
}

pub fn declaration_location(declaration: &Declaration) -> Option<Location> {
    match declaration {
        Declaration::Class(class) => class.location.clone(),
        Declaration::Module(module) => module.location.clone(),
        Declaration::Interface(interface) => interface.location.clone(),
        Declaration::TypeAlias(alias) => alias.location.clone(),
        Declaration::Constant(constant) => constant.location.clone(),
        Declaration::Global(global) => global.location.clone(),
        Declaration::ClassAlias(alias) => alias.location.clone(),
    }
}

impl Environment {
    pub fn from_sources(sources: Vec<Source>) -> Result<Environment, String> {
        let mut environment = Environment::default();
        for source in sources {
            environment.add_source(source)?;
        }
        Ok(environment)
    }

    pub fn add_source(&mut self, source: Source) -> Result<(), RbsError> {
        let declarations = source.declarations.clone();
        self.sources.push(source);
        for declaration in &declarations {
            self.insert(declaration, &Vec::new(), &Namespace::root())?;
        }
        Ok(())
    }

    fn is_constant_name_taken(&self, name: &TypeName) -> bool {
        self.class_declarations.contains_key(name)
            || self.constant_declarations.contains_key(name)
            || self.class_alias_declarations.contains_key(name)
    }

    fn insert(
        &mut self,
        declaration: &Declaration,
        context: &Context,
        namespace: &Namespace,
    ) -> Result<(), RbsError> {
        let duplicated = |name: &dyn std::fmt::Display| {
            Err(RbsError::DuplicatedDeclaration {
                name: name.to_string(),
                location: declaration_location(declaration),
            })
        };
        match declaration {
            Declaration::Class(_) | Declaration::Module(_) => {
                let (name, kind, entry_declaration, members) = match declaration {
                    Declaration::Class(class) => (
                        class.name.with_prefix(namespace),
                        ClassOrModule::Class,
                        ClassOrModuleDeclaration::Class(class.clone()),
                        &class.members,
                    ),
                    Declaration::Module(module) => (
                        module.name.with_prefix(namespace),
                        ClassOrModule::Module,
                        ClassOrModuleDeclaration::Module(module.clone()),
                        &module.members,
                    ),
                    _ => unreachable!(),
                };
                if self.constant_declarations.contains_key(&name)
                    || self.class_alias_declarations.contains_key(&name)
                {
                    return duplicated(&name);
                }
                let entry = self
                    .class_declarations
                    .entry(name.clone())
                    .or_insert_with(|| ClassEntry {
                        name: name.clone(),
                        kind,
                        declarations: Vec::new(),
                    });
                if entry.kind != kind {
                    return duplicated(&name);
                }
                entry
                    .declarations
                    .push((context.clone(), entry_declaration));

                let mut inner_context = context.clone();
                inner_context.push(name.clone());
                let inner_namespace = name.to_namespace();
                for member in members {
                    if let Member::Declaration(inner) = member {
                        self.insert(inner, &inner_context, &inner_namespace)?;
                    }
                }
            }
            Declaration::Interface(interface) => {
                let name = interface.name.with_prefix(namespace);
                if self.interface_declarations.contains_key(&name) {
                    return duplicated(&name);
                }
                self.interface_declarations.insert(
                    name.clone(),
                    Entry {
                        name,
                        declaration: interface.clone(),
                        context: context.clone(),
                    },
                );
            }
            Declaration::TypeAlias(alias) => {
                let name = alias.name.with_prefix(namespace);
                if self.type_alias_declarations.contains_key(&name) {
                    return duplicated(&name);
                }
                self.type_alias_declarations.insert(
                    name.clone(),
                    Entry {
                        name,
                        declaration: alias.clone(),
                        context: context.clone(),
                    },
                );
            }
            Declaration::Constant(constant) => {
                let name = constant.name.with_prefix(namespace);
                if self.is_constant_name_taken(&name) {
                    return duplicated(&name);
                }
                self.constant_declarations.insert(
                    name.clone(),
                    Entry {
                        name,
                        declaration: constant.clone(),
                        context: context.clone(),
                    },
                );
            }
            Declaration::Global(global) => {
                if self.global_declarations.contains_key(&global.name) {
                    return duplicated(&global.name);
                }
                self.global_declarations
                    .insert(global.name.clone(), global.clone());
            }
            Declaration::ClassAlias(alias) => {
                let name = alias.new_name.with_prefix(namespace);
                if self.is_constant_name_taken(&name) {
                    return duplicated(&name);
                }
                self.class_alias_declarations.insert(
                    name.clone(),
                    Entry {
                        name,
                        declaration: alias.clone(),
                        context: context.clone(),
                    },
                );
            }
        }
        Ok(())
    }

    pub fn validate_type_params(&self) -> Result<(), RbsError> {
        for entry in self.class_declarations.values() {
            let mut declarations = entry.declarations();
            let Some(first) = declarations.next() else {
                continue;
            };
            let first_parameters = first.type_parameters();
            let first_names: Vec<String> = first_parameters
                .iter()
                .map(|parameter| parameter.name.clone())
                .collect();
            for other in declarations {
                let other_parameters = other.type_parameters();
                if first_names.len() != other_parameters.len()
                    || first_parameters != rename_type_parameters(other_parameters, &first_names)
                {
                    return Err(RbsError::GenericParameterMismatch {
                        name: entry.name.clone(),
                        location: other.location().cloned(),
                    });
                }
            }
        }
        Ok(())
    }

    pub fn normalize_module_name(&self, name: &TypeName) -> TypeName {
        let mut name = name.absolute();
        if let Some(alias) = self.class_alias_declarations.get(&name) {
            name = alias.declaration.old_name.clone();
        }
        name
    }

    pub fn normalize_module_name_checked(&self, name: &TypeName) -> NormalizedModuleName {
        let mut current = name.absolute();
        let mut visited: HashSet<TypeName> = HashSet::default();
        let mut resolver: Option<TypeNameResolver> = None;
        loop {
            if self.class_declarations.contains_key(&current) {
                return NormalizedModuleName::Found(current);
            }
            let Some(alias) = self.class_alias_declarations.get(&current) else {
                return NormalizedModuleName::Unknown;
            };
            if !visited.insert(current.clone()) {
                return NormalizedModuleName::Cyclic;
            }
            let old_name = &alias.declaration.old_name;
            current = if old_name.is_absolute() {
                old_name.clone()
            } else {
                let resolver = resolver.get_or_insert_with(|| TypeNameResolver::new(self));
                match resolver.resolve_without_alias_normalization(old_name, &alias.context) {
                    Some(name) => name,
                    None => return NormalizedModuleName::Unknown,
                }
            };
        }
    }

    pub fn normalize_type_name(&self, name: &TypeName) -> Option<TypeName> {
        if name.is_class() {
            return match self.normalize_module_name_checked(name) {
                NormalizedModuleName::Found(name) => Some(name),
                _ => None,
            };
        }
        let type_name = if name.namespace.is_empty() {
            name.clone()
        } else {
            match self.normalize_module_name_checked(&name.namespace.to_type_name()) {
                NormalizedModuleName::Found(parent) => {
                    TypeName::new(parent.to_namespace(), &name.name)
                }
                _ => return None,
            }
        };
        self.is_type_name(&type_name).then_some(type_name)
    }

    pub fn ensure_namespace(
        &self,
        namespace: &Namespace,
        location: Option<&Location>,
    ) -> Result<(), RbsError> {
        let mut path = namespace.path.clone();
        while !path.is_empty() {
            let current = Namespace {
                absolute: namespace.absolute,
                path: path.clone(),
            };
            let type_name = current.to_type_name();
            if !self.is_type_name(&type_name) {
                return Err(RbsError::NoTypeFound {
                    type_name,
                    location: location.cloned(),
                });
            }
            path.pop();
        }
        Ok(())
    }

    pub fn is_type_name(&self, name: &TypeName) -> bool {
        match name.kind() {
            TypeNameKind::Class => self.is_module_name(name),
            TypeNameKind::Interface => self.interface_declarations.contains_key(name),
            TypeNameKind::Alias => self.type_alias_declarations.contains_key(name),
        }
    }

    pub fn is_module_name(&self, name: &TypeName) -> bool {
        self.class_declarations.contains_key(name)
            || self.class_alias_declarations.contains_key(name)
    }

    pub fn constant_entry(&self, name: &TypeName) -> Option<ConstantEntry<'_>> {
        if let Some(entry) = self.class_declarations.get(name) {
            return Some(ConstantEntry::Class(entry));
        }
        if let Some(entry) = self.class_alias_declarations.get(name) {
            return Some(ConstantEntry::ClassAlias(entry));
        }
        self.constant_declarations
            .get(name)
            .map(ConstantEntry::Constant)
    }

    pub fn module_class_entry(&self, name: &TypeName) -> Option<&ClassEntry> {
        match self.normalize_module_name_checked(name) {
            NormalizedModuleName::Found(name) => self.class_declarations.get(&name),
            _ => None,
        }
    }

    pub fn use_map(&self, directives: &[Directive]) -> Result<UseMap, String> {
        let table = UseTable::new(
            self.class_declarations
                .keys()
                .chain(self.class_alias_declarations.keys())
                .chain(self.type_alias_declarations.keys())
                .chain(self.interface_declarations.keys()),
        );
        UseMap::build(&table, directives)
    }

    pub fn resolve_type_names(&self, jobs: usize) -> Result<Environment, String> {
        let resolver = TypeNameResolver::new(self);
        let table = UseTable::new(
            self.class_declarations
                .keys()
                .chain(self.class_alias_declarations.keys())
                .chain(self.type_alias_declarations.keys())
                .chain(self.interface_declarations.keys()),
        );
        let sources = map_in_parallel(&self.sources, jobs, |source| {
            let map = UseMap::build(&table, &source.directives)?;
            let declaration_resolver = DeclarationResolver {
                resolver: &resolver,
                map: &map,
            };
            Ok(Source {
                path: source.path.clone(),
                directives: source.directives.clone(),
                declarations: source
                    .declarations
                    .iter()
                    .map(|declaration| {
                        declaration_resolver.resolve_declaration(
                            declaration,
                            &Vec::new(),
                            &Namespace::root(),
                        )
                    })
                    .collect(),
            })
        })
        .into_iter()
        .collect::<Result<Vec<Source>, String>>()?;
        Environment::from_sources(sources)
    }
}

struct UseTable {
    children: HashMap<Namespace, Vec<TypeName>>,
}

impl UseTable {
    fn new<'name>(known_types: impl Iterator<Item = &'name TypeName>) -> UseTable {
        let mut children: HashMap<Namespace, Vec<TypeName>> = HashMap::default();
        for type_name in known_types {
            if !type_name.namespace.is_empty() {
                children
                    .entry(type_name.namespace.clone())
                    .or_default()
                    .push(type_name.clone());
            }
        }
        UseTable { children }
    }
}

#[derive(Default)]
pub struct UseMap {
    map: HashMap<String, TypeName>,
}

impl UseMap {
    fn build(table: &UseTable, directives: &[Directive]) -> Result<UseMap, String> {
        let mut use_map = UseMap::default();
        for Directive::Use(clauses, _) in directives {
            for clause in clauses {
                match clause {
                    UseClause::Single {
                        type_name,
                        new_name,
                        ..
                    } => {
                        let name = new_name.as_ref().unwrap_or(&type_name.name);
                        use_map.map.insert(name.clone(), type_name.absolute());
                    }
                    UseClause::Wildcard { namespace, .. } => {
                        let namespace = Namespace {
                            absolute: true,
                            path: namespace.path.clone(),
                        };
                        let children = table
                            .children
                            .get(&namespace)
                            .ok_or_else(|| format!("key not found: {namespace}"))?;
                        for child in children {
                            use_map.map.insert(child.name.clone(), child.clone());
                        }
                    }
                }
            }
        }
        Ok(use_map)
    }

    pub fn names(&self) -> impl Iterator<Item = &String> {
        self.map.keys()
    }

    pub fn resolve(&self, type_name: &TypeName) -> Option<TypeName> {
        if type_name.is_absolute() {
            return None;
        }
        let Some((head, tail)) = type_name.namespace.path.split_first() else {
            return self.map.get(&type_name.name).cloned();
        };
        let used = self.map.get(head)?;
        let mut path = used.namespace.path.clone();
        path.push(used.name.clone());
        path.extend(tail.iter().cloned());
        Some(TypeName {
            namespace: Namespace {
                absolute: true,
                path,
            },
            name: type_name.name.clone(),
        })
    }
}

pub fn rename_type_parameters(
    parameters: &[TypeParameter],
    new_names: &[String],
) -> Vec<TypeParameter> {
    let old_names: Vec<String> = parameters
        .iter()
        .map(|parameter| parameter.name.clone())
        .collect();
    let variables: Vec<Type> = new_names.iter().cloned().map(Type::Variable).collect();
    let substitution = Substitution::build(&old_names, &variables);
    parameters
        .iter()
        .zip(new_names)
        .map(|(parameter, new_name)| TypeParameter {
            name: new_name.clone(),
            ..parameter.substitute(&substitution)
        })
        .collect()
}

pub struct TypeNameResolver {
    all_names: HashSet<TypeName>,
    aliases: HashMap<TypeName, (TypeName, Context)>,
}

impl TypeNameResolver {
    pub fn new(environment: &Environment) -> TypeNameResolver {
        let mut all_names = HashSet::default();
        all_names.extend(environment.class_declarations.keys().cloned());
        all_names.extend(environment.interface_declarations.keys().cloned());
        all_names.extend(environment.type_alias_declarations.keys().cloned());
        let aliases = environment
            .class_alias_declarations
            .iter()
            .map(|(name, entry)| {
                (
                    name.clone(),
                    (entry.declaration.old_name.clone(), entry.context.clone()),
                )
            })
            .collect();
        TypeNameResolver { all_names, aliases }
    }

    fn has_type_name(&self, name: &TypeName) -> Option<TypeName> {
        self.all_names.contains(name).then(|| name.clone())
    }

    fn aliased_name(&self, name: &TypeName) -> Option<TypeName> {
        self.aliases.contains_key(name).then(|| name.clone())
    }

    pub fn resolve(&self, type_name: &TypeName, context: &[TypeName]) -> Option<TypeName> {
        if type_name.is_absolute() && self.all_names.contains(type_name) {
            return Some(type_name.clone());
        }
        if type_name.is_class() {
            return self.resolve_namespace(type_name, context, &mut HashSet::default());
        }
        if type_name.namespace.is_empty() {
            return self.resolve_type_name(&type_name.name, context);
        }
        let namespace = self.resolve_namespace(
            &type_name.namespace.to_type_name(),
            context,
            &mut HashSet::default(),
        )?;
        self.has_type_name(&TypeName::new(namespace.to_namespace(), &type_name.name))
    }

    pub fn resolve_without_alias_normalization(
        &self,
        type_name: &TypeName,
        context: &[TypeName],
    ) -> Option<TypeName> {
        if let Some(name) = self.resolve(type_name, context) {
            return Some(name);
        }
        let known =
            |name: &TypeName| self.all_names.contains(name) || self.aliases.contains_key(name);
        if type_name.is_absolute() {
            return known(type_name).then(|| type_name.clone());
        }
        for inner in context.iter().rev() {
            let name = type_name.with_prefix(&inner.to_namespace());
            if known(&name) {
                return Some(name);
            }
        }
        let name = type_name.absolute();
        known(&name).then_some(name)
    }

    fn resolve_type_name(&self, name: &str, context: &[TypeName]) -> Option<TypeName> {
        for inner in context.iter().rev() {
            if let Some(found) = self.has_type_name(&TypeName::new(inner.to_namespace(), name)) {
                return Some(found);
            }
        }
        self.has_type_name(&TypeName::new(Namespace::root(), name))
    }

    fn resolve_head_namespace(&self, head: &str, context: &[TypeName]) -> Option<TypeName> {
        for inner in context.iter().rev() {
            let name = TypeName::new(inner.to_namespace(), head);
            if let Some(found) = self
                .has_type_name(&name)
                .or_else(|| self.aliased_name(&name))
            {
                return Some(found);
            }
        }
        let name = TypeName::new(Namespace::root(), head);
        self.has_type_name(&name)
            .or_else(|| self.aliased_name(&name))
    }

    fn normalize_namespace(
        &self,
        name: &TypeName,
        right_hand_side: &TypeName,
        context: &[TypeName],
        visited: &mut HashSet<TypeName>,
    ) -> Option<TypeName> {
        if !visited.insert(name.clone()) {
            return None;
        }
        let resolved = self.resolve_namespace(right_hand_side, context, visited);
        visited.remove(name);
        resolved
    }

    fn resolve_namespace(
        &self,
        type_name: &TypeName,
        context: &[TypeName],
        visited: &mut HashSet<TypeName>,
    ) -> Option<TypeName> {
        let mut segments = type_name.namespace.path.iter().chain([&type_name.name]);
        let head = segments.next().expect("a type name has a name");
        let mut current = if type_name.is_absolute() {
            let name = TypeName::new(Namespace::root(), head);
            self.has_type_name(&name)
                .or_else(|| self.aliased_name(&name))
        } else {
            self.resolve_head_namespace(head, context)
        }?;
        if let Some((right_hand_side, alias_context)) = self.aliases.get(&current) {
            current =
                self.normalize_namespace(&current, right_hand_side, alias_context, visited)?;
        }
        for segment in segments {
            let name = TypeName::new(current.to_namespace(), segment);
            current = if self.all_names.contains(&name) {
                name
            } else if let Some((right_hand_side, alias_context)) = self.aliases.get(&name) {
                self.normalize_namespace(&name, right_hand_side, alias_context, visited)?
            } else {
                return None;
            };
        }
        Some(current)
    }

    fn absolute_type_name(&self, name: &TypeName, context: &[TypeName]) -> TypeName {
        self.resolve(name, context).unwrap_or_else(|| name.clone())
    }

    pub fn absolute_type(&self, type_: &Type, context: &[TypeName]) -> Type {
        map_type_names(type_, &mut |name| self.absolute_type_name(name, context))
    }
}

struct DeclarationResolver<'resolver> {
    resolver: &'resolver TypeNameResolver,
    map: &'resolver UseMap,
}

impl DeclarationResolver<'_> {
    fn absolute_type_name(&self, name: &TypeName, context: &[TypeName]) -> TypeName {
        let name = self.map.resolve(name).unwrap_or_else(|| name.clone());
        self.resolver.resolve(&name, context).unwrap_or(name)
    }

    fn absolute_type(&self, type_: &Type, context: &[TypeName]) -> Type {
        map_type_names(type_, &mut |name| self.absolute_type_name(name, context))
    }

    fn absolute_types(&self, types: &[Type], context: &[TypeName]) -> Vec<Type> {
        types
            .iter()
            .map(|type_| self.absolute_type(type_, context))
            .collect()
    }

    fn resolve_type_parameters(
        &self,
        parameters: &[TypeParameter],
        context: &[TypeName],
    ) -> Vec<TypeParameter> {
        parameters
            .iter()
            .map(|parameter| {
                map_type_parameter_names(parameter, &mut |name| {
                    self.absolute_type_name(name, context)
                })
            })
            .collect()
    }

    fn resolve_declaration(
        &self,
        declaration: &Declaration,
        context: &Context,
        prefix: &Namespace,
    ) -> Declaration {
        match declaration {
            Declaration::Class(class) => {
                let name = class.name.with_prefix(prefix);
                let mut inner_context = context.clone();
                inner_context.push(name.clone());
                let inner_prefix = prefix.join(&class.name.to_namespace());
                Declaration::Class(Arc::new(ClassDeclaration {
                    type_parameters: self
                        .resolve_type_parameters(&class.type_parameters, &inner_context),
                    super_class: class.super_class.as_ref().map(|super_class| Super {
                        name: self.absolute_type_name(&super_class.name, context),
                        args: self.absolute_types(&super_class.args, context),
                        location: super_class.location.clone(),
                        name_location: super_class.name_location.clone(),
                    }),
                    members: self.resolve_members(&class.members, &inner_context, &inner_prefix),
                    name,
                    ..(**class).clone()
                }))
            }
            Declaration::Module(module) => {
                let name = module.name.with_prefix(prefix);
                let mut inner_context = context.clone();
                inner_context.push(name.clone());
                let inner_prefix = prefix.join(&module.name.to_namespace());
                Declaration::Module(Arc::new(ModuleDeclaration {
                    type_parameters: self
                        .resolve_type_parameters(&module.type_parameters, &inner_context),
                    self_types: module
                        .self_types
                        .iter()
                        .map(|self_type| ModuleSelf {
                            name: self.absolute_type_name(&self_type.name, &inner_context),
                            args: self.absolute_types(&self_type.args, &inner_context),
                            location: self_type.location.clone(),
                            name_location: self_type.name_location.clone(),
                        })
                        .collect(),
                    members: self.resolve_members(&module.members, &inner_context, &inner_prefix),
                    name,
                    ..(**module).clone()
                }))
            }
            Declaration::Interface(interface) => {
                Declaration::Interface(Arc::new(InterfaceDeclaration {
                    name: interface.name.with_prefix(prefix),
                    type_parameters: self
                        .resolve_type_parameters(&interface.type_parameters, context),
                    members: self.resolve_members(&interface.members, context, prefix),
                    ..(**interface).clone()
                }))
            }
            Declaration::TypeAlias(alias) => {
                Declaration::TypeAlias(Arc::new(TypeAliasDeclaration {
                    name: alias.name.with_prefix(prefix),
                    type_parameters: self.resolve_type_parameters(&alias.type_parameters, context),
                    type_: self.absolute_type(&alias.type_, context),
                    ..(**alias).clone()
                }))
            }
            Declaration::Constant(constant) => {
                Declaration::Constant(Arc::new(ConstantDeclaration {
                    name: constant.name.with_prefix(prefix),
                    type_: self.absolute_type(&constant.type_, context),
                    ..(**constant).clone()
                }))
            }
            Declaration::Global(global) => Declaration::Global(Arc::new(GlobalDeclaration {
                type_: self.absolute_type(&global.type_, &[]),
                ..(**global).clone()
            })),
            Declaration::ClassAlias(alias) => Declaration::ClassAlias(Arc::new(AliasDeclaration {
                new_name: alias.new_name.with_prefix(prefix),
                old_name: self.absolute_type_name(&alias.old_name, context),
                ..(**alias).clone()
            })),
        }
    }

    fn resolve_members(
        &self,
        members: &[Member],
        context: &Context,
        prefix: &Namespace,
    ) -> Vec<Member> {
        members
            .iter()
            .map(|member| self.resolve_member(member, context, prefix))
            .collect()
    }

    fn resolve_member(&self, member: &Member, context: &Context, prefix: &Namespace) -> Member {
        match member {
            Member::MethodDefinition(definition) => Member::MethodDefinition(MethodDefinition {
                overloads: definition
                    .overloads
                    .iter()
                    .map(|overload| Overload {
                        method_type: Arc::new(map_method_type_names(
                            &overload.method_type,
                            &mut |name| self.absolute_type_name(name, context),
                        )),
                        annotations: overload.annotations.clone(),
                    })
                    .collect(),
                ..definition.clone()
            }),
            Member::Attribute(attribute) => {
                let mut attribute = attribute.clone();
                attribute.type_ = self.absolute_type(&attribute.type_, context);
                Member::Attribute(attribute)
            }
            Member::Variable(variable) => {
                let mut variable = variable.clone();
                variable.type_ = self.absolute_type(&variable.type_, context);
                Member::Variable(variable)
            }
            Member::Mixin(mixin) => Member::Mixin(Mixin {
                name: self.absolute_type_name(&mixin.name, context),
                args: self.absolute_types(&mixin.args, context),
                ..mixin.clone()
            }),
            Member::Alias(_) | Member::Visibility(_) => member.clone(),
            Member::Declaration(declaration) => {
                Member::Declaration(self.resolve_declaration(declaration, context, prefix))
            }
        }
    }
}

pub fn map_type_names(type_: &Type, map: &mut dyn FnMut(&TypeName) -> TypeName) -> Type {
    let map_all = |types: &[Type], map: &mut dyn FnMut(&TypeName) -> TypeName| -> Vec<Type> {
        types
            .iter()
            .map(|type_| map_type_names(type_, map))
            .collect()
    };
    match type_ {
        Type::ClassInstance {
            name,
            args,
            location,
        } => Type::ClassInstance {
            name: map(name),
            args: map_all(args, map),
            location: location.clone(),
        },
        Type::ClassSingleton {
            name,
            args,
            location,
        } => Type::ClassSingleton {
            name: map(name),
            args: map_all(args, map),
            location: location.clone(),
        },
        Type::Interface {
            name,
            args,
            location,
        } => Type::Interface {
            name: map(name),
            args: map_all(args, map),
            location: location.clone(),
        },
        Type::Alias {
            name,
            args,
            location,
        } => Type::Alias {
            name: map(name),
            args: map_all(args, map),
            location: location.clone(),
        },
        Type::Tuple(types) => Type::Tuple(map_all(types, map)),
        Type::Record(fields) => Type::Record(
            fields
                .iter()
                .map(|field| RecordField {
                    key: field.key.clone(),
                    type_: map_type_names(&field.type_, map),
                    required: field.required,
                })
                .collect(),
        ),
        Type::Optional(inner) => Type::Optional(Box::new(map_type_names(inner, map))),
        Type::Union(types) => Type::Union(map_all(types, map)),
        Type::Intersection(types) => Type::Intersection(map_all(types, map)),
        Type::Proc {
            function,
            block,
            self_type,
        } => Type::Proc {
            function: Box::new(map_callable_names(function, map)),
            block: block
                .as_ref()
                .map(|block| Box::new(map_block_names(block, map))),
            self_type: self_type
                .as_ref()
                .map(|self_type| Box::new(map_type_names(self_type, map))),
        },
        Type::Bool
        | Type::Void
        | Type::Any
        | Type::Nil
        | Type::Top
        | Type::Bottom
        | Type::SelfType
        | Type::Instance
        | Type::Class
        | Type::Variable(_)
        | Type::Literal(_) => type_.clone(),
    }
}

fn map_parameter_names(
    parameter: &Parameter,
    map: &mut dyn FnMut(&TypeName) -> TypeName,
) -> Parameter {
    Parameter {
        type_: map_type_names(&parameter.type_, map),
        name: parameter.name.clone(),
    }
}

fn map_callable_names(callable: &Callable, map: &mut dyn FnMut(&TypeName) -> TypeName) -> Callable {
    match callable {
        Callable::Function(function) => {
            let mut map_parameters = |parameters: &[Parameter]| -> Vec<Parameter> {
                parameters
                    .iter()
                    .map(|parameter| map_parameter_names(parameter, map))
                    .collect()
            };
            let required_positionals = map_parameters(&function.required_positionals);
            let optional_positionals = map_parameters(&function.optional_positionals);
            let trailing_positionals = map_parameters(&function.trailing_positionals);
            let mut map_keywords = |keywords: &[(String, Parameter)]| -> Vec<(String, Parameter)> {
                keywords
                    .iter()
                    .map(|(name, parameter)| (name.clone(), map_parameter_names(parameter, map)))
                    .collect()
            };
            let required_keywords = map_keywords(&function.required_keywords);
            let optional_keywords = map_keywords(&function.optional_keywords);
            Callable::Function(Box::new(Function {
                required_positionals,
                optional_positionals,
                rest_positionals: function
                    .rest_positionals
                    .as_ref()
                    .map(|parameter| map_parameter_names(parameter, map)),
                trailing_positionals,
                required_keywords,
                optional_keywords,
                rest_keywords: function
                    .rest_keywords
                    .as_ref()
                    .map(|parameter| map_parameter_names(parameter, map)),
                return_type: map_type_names(&function.return_type, map),
            }))
        }
        Callable::Untyped { return_type } => Callable::Untyped {
            return_type: map_type_names(return_type, map),
        },
    }
}

fn map_block_names(block: &Block, map: &mut dyn FnMut(&TypeName) -> TypeName) -> Block {
    Block {
        type_: map_callable_names(&block.type_, map),
        required: block.required,
        self_type: block
            .self_type
            .as_ref()
            .map(|self_type| map_type_names(self_type, map)),
    }
}

fn map_type_parameter_names(
    parameter: &TypeParameter,
    map: &mut dyn FnMut(&TypeName) -> TypeName,
) -> TypeParameter {
    TypeParameter {
        upper_bound: parameter
            .upper_bound
            .as_ref()
            .map(|type_| map_type_names(type_, map)),
        lower_bound: parameter
            .lower_bound
            .as_ref()
            .map(|type_| map_type_names(type_, map)),
        default_type: parameter
            .default_type
            .as_ref()
            .map(|type_| map_type_names(type_, map)),
        ..parameter.clone()
    }
}

pub fn map_method_type_names(
    method_type: &MethodType,
    map: &mut dyn FnMut(&TypeName) -> TypeName,
) -> MethodType {
    MethodType {
        type_parameters: method_type
            .type_parameters
            .iter()
            .map(|parameter| map_type_parameter_names(parameter, map))
            .collect(),
        type_: map_callable_names(&method_type.type_, map),
        block: method_type
            .block
            .as_ref()
            .map(|block| map_block_names(block, map)),
        location: method_type.location.clone(),
    }
}
