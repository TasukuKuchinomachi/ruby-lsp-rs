use std::sync::Mutex;
use std::{
    fmt,
    hash::{Hash, Hasher},
    sync::Arc,
};

use crate::{
    ast::{ClassOrModule, Member, Mixin, MixinKind, Type, TypeName, TypeNameKind, TypeParameter},
    collections::HashMap,
    environment::{ClassEntry, ClassOrModuleDeclaration, Environment},
    errors::RbsError,
    location::{Location, TypeLocation},
    once_map::OnceMap,
    substitution::{Substitution, normalize_args},
};

/// Where an ancestor comes from. Like the rbs gem, ancestors from different sources are equal.
#[derive(Clone, Debug, Default)]
pub enum AncestorSource {
    #[default]
    Nothing,
    Super,
    Member(Option<Location>),
}

impl AncestorSource {
    pub fn location(&self) -> Option<&Location> {
        match self {
            AncestorSource::Member(location) => location.as_ref(),
            AncestorSource::Nothing | AncestorSource::Super => None,
        }
    }
}

impl PartialEq for AncestorSource {
    fn eq(&self, _: &AncestorSource) -> bool {
        true
    }
}

impl Eq for AncestorSource {}

impl Hash for AncestorSource {
    fn hash<H: Hasher>(&self, _: &mut H) {}
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ancestor {
    Instance {
        name: TypeName,
        args: Vec<Type>,
        source: AncestorSource,
    },
    Singleton {
        name: TypeName,
    },
}

impl Ancestor {
    pub fn name(&self) -> &TypeName {
        match self {
            Ancestor::Instance { name, .. } | Ancestor::Singleton { name } => name,
        }
    }

    fn substitute(&self, substitution: &Substitution) -> Ancestor {
        match self {
            Ancestor::Instance { name, args, source } if !args.is_empty() => Ancestor::Instance {
                name: name.clone(),
                args: args
                    .iter()
                    .map(|arg| arg.substitute(substitution))
                    .collect(),
                source: source.clone(),
            },
            _ => self.clone(),
        }
    }
}

impl fmt::Display for Ancestor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Ancestor::Instance { name, args, .. } if args.is_empty() => {
                write!(formatter, "{name}")
            }
            Ancestor::Instance { name, args, .. } => {
                let args: Vec<String> = args.iter().map(ToString::to_string).collect();
                write!(formatter, "{name}[{}]", args.join(", "))
            }
            Ancestor::Singleton { name } => write!(formatter, "singleton({name})"),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct OneAncestors {
    pub type_name: Option<TypeName>,
    pub params: Vec<String>,
    pub super_class: Option<Ancestor>,
    pub self_types: Option<Vec<Ancestor>>,
    pub included_modules: Option<Vec<Ancestor>>,
    pub included_interfaces: Option<Vec<Ancestor>>,
    pub prepended_modules: Option<Vec<Ancestor>>,
    pub extended_modules: Option<Vec<Ancestor>>,
    pub extended_interfaces: Option<Vec<Ancestor>>,
}

impl OneAncestors {
    pub fn each_ancestor(&self) -> impl Iterator<Item = &Ancestor> {
        self.super_class
            .iter()
            .chain(self.self_types.iter().flatten())
            .chain(self.included_modules.iter().flatten())
            .chain(self.included_interfaces.iter().flatten())
            .chain(self.prepended_modules.iter().flatten())
            .chain(self.extended_modules.iter().flatten())
            .chain(self.extended_interfaces.iter().flatten())
    }
}

#[derive(Clone, Debug)]
pub struct Ancestors {
    pub type_name: TypeName,
    pub params: Vec<String>,
    pub ancestors: Vec<Ancestor>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum OneAncestorsKind {
    Instance,
    Singleton,
    Interface,
}

#[derive(Default)]
pub struct OneAncestorsCache(
    OnceMap<(OneAncestorsKind, TypeName), Result<Arc<OneAncestors>, RbsError>>,
);

pub struct AncestorBuilder<'environment> {
    environment: &'environment Environment,
    one_ancestors_cache: OneAncestorsCache,
    instance_ancestors_cache: Mutex<HashMap<TypeName, Arc<Ancestors>>>,
    singleton_ancestors_cache: Mutex<HashMap<TypeName, Arc<Ancestors>>>,
    interface_ancestors_cache: Mutex<HashMap<TypeName, Arc<Ancestors>>>,
}

fn object_name() -> TypeName {
    TypeName::parse("::Object")
}

fn basic_object_name() -> TypeName {
    TypeName::parse("::BasicObject")
}

#[derive(Clone, Copy)]
struct MixinTargets {
    included_modules: bool,
    included_interfaces: bool,
    prepended_modules: bool,
    extended_modules: bool,
    extended_interfaces: bool,
}

pub fn type_parameters_of<'environment>(
    environment: &'environment Environment,
    type_name: &TypeName,
) -> Result<&'environment [TypeParameter], RbsError> {
    match type_name.kind() {
        TypeNameKind::Class => environment
            .module_class_entry(type_name)
            .map(ClassEntry::type_parameters)
            .ok_or_else(|| RbsError::RuntimeError(format!("Unknown module name: {type_name}"))),
        TypeNameKind::Interface => environment
            .interface_declarations
            .get(type_name)
            .map(|entry| entry.declaration.type_parameters.as_slice())
            .ok_or_else(|| RbsError::RuntimeError(format!("Unknown interface name: {type_name}"))),
        TypeNameKind::Alias => environment
            .type_alias_declarations
            .get(type_name)
            .map(|entry| entry.declaration.type_parameters.as_slice())
            .ok_or_else(|| RbsError::RuntimeError(format!("Unknown alias name: {type_name}"))),
    }
}

pub fn check_type_application(
    type_name: &TypeName,
    args: &[Type],
    params: &[TypeParameter],
    location: Option<&Location>,
) -> Result<(), RbsError> {
    let minimum = params
        .iter()
        .filter(|parameter| parameter.default_type.is_none())
        .count();
    if minimum <= args.len() && args.len() <= params.len() {
        return Ok(());
    }
    Err(RbsError::InvalidTypeApplication {
        type_name: type_name.clone(),
        args: args.into(),
        params: params.into(),
        location: location.cloned(),
    })
}

pub fn check_type_application_in(
    environment: &Environment,
    type_name: &TypeName,
    args: &[Type],
    location: Option<&Location>,
) -> Result<(), RbsError> {
    let params = type_parameters_of(environment, type_name)?;
    check_type_application(type_name, args, params, location)
}

impl<'environment> AncestorBuilder<'environment> {
    pub fn new(environment: &'environment Environment) -> AncestorBuilder<'environment> {
        AncestorBuilder::with_one_ancestors_cache(environment, OneAncestorsCache::default())
    }

    pub fn with_one_ancestors_cache(
        environment: &'environment Environment,
        one_ancestors_cache: OneAncestorsCache,
    ) -> AncestorBuilder<'environment> {
        AncestorBuilder {
            environment,
            one_ancestors_cache,
            instance_ancestors_cache: Mutex::default(),
            singleton_ancestors_cache: Mutex::default(),
            interface_ancestors_cache: Mutex::default(),
        }
    }

    pub fn environment(&self) -> &'environment Environment {
        self.environment
    }

    fn class_entry(&self, name: &TypeName) -> Result<&'environment ClassEntry, RbsError> {
        self.environment
            .class_declarations
            .get(name)
            .ok_or_else(|| RbsError::RuntimeError(format!("Unknown name for ancestors: {name}")))
    }

    fn validate_super_class(&self, entry: &ClassEntry) -> Result<(), RbsError> {
        let mut super_types: Vec<Type> = Vec::new();
        for declaration in entry.declarations() {
            let Some(super_class) = declaration.super_class() else {
                continue;
            };
            let args = match entry.align_parameters(declaration) {
                Some(substitution) => super_class
                    .args
                    .iter()
                    .map(|arg| arg.substitute(&substitution))
                    .collect(),
                None => super_class.args.clone(),
            };
            let super_type = Type::ClassInstance {
                name: super_class.name.clone(),
                args,
                location: TypeLocation::default(),
            };
            if !super_types.contains(&super_type) {
                super_types.push(super_type);
            }
        }
        if super_types.len() > 1 {
            return Err(RbsError::SuperclassMismatch {
                name: entry.name.clone(),
                location: entry.primary_declaration().location().cloned(),
            });
        }
        Ok(())
    }

    fn cached_one_ancestors(
        &self,
        kind: OneAncestorsKind,
        name: &TypeName,
        build: impl FnOnce() -> Result<OneAncestors, RbsError>,
    ) -> Result<Arc<OneAncestors>, RbsError> {
        self.one_ancestors_cache
            .0
            .get_or_init(&(kind, name.clone()), || build().map(Arc::new))
    }

    pub fn into_one_ancestors_cache(self) -> OneAncestorsCache {
        self.one_ancestors_cache
    }

    fn super_class_name(&self, primary: &ClassOrModuleDeclaration) -> Result<TypeName, RbsError> {
        let super_name = match primary.super_class() {
            Some(super_class) => super_class.name.clone(),
            None => object_name(),
        };
        let normalized = self.environment.normalize_module_name(&super_name);
        if !self.environment.is_module_name(&normalized) {
            return Err(RbsError::NoSuperclassFound {
                type_name: super_name,
                location: primary.location().cloned(),
            });
        }
        let super_name = normalized;
        if let Some(super_class) = primary.super_class() {
            let normalized = self
                .environment
                .normalize_type_name(&super_class.name)
                .unwrap_or_else(|| super_class.name.clone());
            let is_class = self
                .environment
                .class_declarations
                .get(&normalized)
                .is_some_and(|entry| entry.kind == ClassOrModule::Class)
                || self
                    .environment
                    .class_alias_declarations
                    .get(&normalized)
                    .is_some_and(|alias| alias.declaration.kind == ClassOrModule::Class);
            if !is_class {
                return Err(RbsError::InheritModule {
                    super_name: super_class.name.clone(),
                    location: super_class.location.clone(),
                });
            }
        }
        Ok(super_name)
    }

    pub fn one_instance_ancestors(
        &self,
        type_name: &TypeName,
    ) -> Result<Arc<OneAncestors>, RbsError> {
        let type_name = self.environment.normalize_module_name(type_name);
        self.cached_one_ancestors(OneAncestorsKind::Instance, &type_name.clone(), || {
            let entry = self.class_entry(&type_name)?;
            let mut ancestors = OneAncestors {
                type_name: Some(type_name.clone()),
                params: entry.type_parameter_names(),
                included_modules: Some(Vec::new()),
                included_interfaces: Some(Vec::new()),
                prepended_modules: Some(Vec::new()),
                ..OneAncestors::default()
            };
            match entry.kind {
                ClassOrModule::Class => {
                    self.validate_super_class(entry)?;
                    if type_name != basic_object_name() {
                        let primary = entry.primary_declaration();
                        let super_name = self.super_class_name(primary)?;
                        let super_args = match primary.super_class() {
                            Some(super_class) => {
                                check_type_application_in(
                                    self.environment,
                                    &super_class.name,
                                    &super_class.args,
                                    super_class.location.as_ref(),
                                )?;
                                super_class.args.clone()
                            }
                            None => Vec::new(),
                        };
                        let super_entry = self.class_entry(&super_name)?;
                        ancestors.super_class = Some(Ancestor::Instance {
                            name: super_name,
                            args: normalize_args(super_entry.type_parameters(), &super_args),
                            source: AncestorSource::Super,
                        });
                    }
                }
                ClassOrModule::Module => {
                    let mut self_types = Vec::new();
                    let mut module_self_types: Vec<(TypeName, Vec<Type>, Option<Location>)> =
                        Vec::new();
                    for declaration in entry.declarations() {
                        let substitution = entry.align_parameters(declaration);
                        for self_type in declaration.self_types() {
                            let args: Vec<Type> = self_type
                                .args
                                .iter()
                                .map(|arg| match &substitution {
                                    Some(substitution) => arg.substitute(substitution),
                                    None => arg.clone(),
                                })
                                .collect();
                            if !module_self_types.iter().any(|(name, other_args, _)| {
                                *name == self_type.name && *other_args == args
                            }) {
                                module_self_types.push((
                                    self_type.name.clone(),
                                    args,
                                    self_type.location.clone(),
                                ));
                            }
                        }
                    }
                    if module_self_types.is_empty() {
                        self_types.push(Ancestor::Instance {
                            name: object_name(),
                            args: Vec::new(),
                            source: AncestorSource::Nothing,
                        });
                    }
                    for (name, args, location) in module_self_types {
                        let normalized = self
                            .environment
                            .normalize_type_name(&name)
                            .unwrap_or_else(|| name.clone());
                        let found = match normalized.kind() {
                            TypeNameKind::Class => self.environment.is_module_name(&normalized),
                            TypeNameKind::Interface => self
                                .environment
                                .interface_declarations
                                .contains_key(&normalized),
                            TypeNameKind::Alias => false,
                        };
                        if !found {
                            return Err(RbsError::NoSelfTypeFound {
                                type_name: name,
                                location,
                            });
                        }
                        check_type_application_in(
                            self.environment,
                            &name,
                            &args,
                            location.as_ref(),
                        )?;
                        let (name, args) = match name.kind() {
                            TypeNameKind::Class => {
                                let entry = self.environment.module_class_entry(&name).ok_or_else(
                                    || {
                                        RbsError::RuntimeError(format!(
                                            "Unknown module name: {name}"
                                        ))
                                    },
                                )?;
                                let args = normalize_args(entry.type_parameters(), &args);
                                (entry.name.clone(), args)
                            }
                            _ => {
                                let entry = &self.environment.interface_declarations[&name];
                                let args =
                                    normalize_args(&entry.declaration.type_parameters, &args);
                                (name, args)
                            }
                        };
                        self_types.push(Ancestor::Instance {
                            name,
                            args,
                            source: AncestorSource::Member(location),
                        });
                    }
                    ancestors.self_types = Some(self_types);
                }
            }
            self.mixin_ancestors(
                entry,
                &type_name,
                &mut ancestors,
                MixinTargets {
                    included_modules: true,
                    included_interfaces: true,
                    prepended_modules: true,
                    extended_modules: false,
                    extended_interfaces: false,
                },
            )?;
            Ok(ancestors)
        })
    }

    pub fn one_singleton_ancestors(
        &self,
        type_name: &TypeName,
    ) -> Result<Arc<OneAncestors>, RbsError> {
        let type_name = self.environment.normalize_module_name(type_name);
        self.cached_one_ancestors(OneAncestorsKind::Singleton, &type_name.clone(), || {
            let entry = self.class_entry(&type_name)?;
            let super_class = match entry.kind {
                ClassOrModule::Class => {
                    self.validate_super_class(entry)?;
                    if type_name != basic_object_name() {
                        let super_name = self.super_class_name(entry.primary_declaration())?;
                        Ancestor::Singleton { name: super_name }
                    } else {
                        Ancestor::Instance {
                            name: TypeName::parse("::Class"),
                            args: Vec::new(),
                            source: AncestorSource::Super,
                        }
                    }
                }
                ClassOrModule::Module => Ancestor::Instance {
                    name: TypeName::parse("::Module"),
                    args: Vec::new(),
                    source: AncestorSource::Super,
                },
            };
            let mut ancestors = OneAncestors {
                type_name: Some(type_name.clone()),
                super_class: Some(super_class),
                extended_modules: Some(Vec::new()),
                extended_interfaces: Some(Vec::new()),
                ..OneAncestors::default()
            };
            self.mixin_ancestors(
                entry,
                &type_name,
                &mut ancestors,
                MixinTargets {
                    included_modules: false,
                    included_interfaces: false,
                    prepended_modules: false,
                    extended_modules: true,
                    extended_interfaces: true,
                },
            )?;
            Ok(ancestors)
        })
    }

    pub fn one_interface_ancestors(
        &self,
        type_name: &TypeName,
    ) -> Result<Arc<OneAncestors>, RbsError> {
        self.cached_one_ancestors(OneAncestorsKind::Interface, type_name, || {
            let entry = self
                .environment
                .interface_declarations
                .get(type_name)
                .ok_or_else(|| {
                    RbsError::RuntimeError(format!(
                        "Unknown name for one_interface_ancestors: {type_name}"
                    ))
                })?;
            let mut ancestors = OneAncestors {
                type_name: Some(type_name.clone()),
                params: entry
                    .declaration
                    .type_parameters
                    .iter()
                    .map(|parameter| parameter.name.clone())
                    .collect(),
                included_interfaces: Some(Vec::new()),
                ..OneAncestors::default()
            };
            self.mixin_ancestors_of_members(
                &entry.declaration.members,
                type_name,
                None,
                &mut ancestors,
                MixinTargets {
                    included_modules: false,
                    included_interfaces: true,
                    prepended_modules: false,
                    extended_modules: false,
                    extended_interfaces: false,
                },
            )?;
            Ok(ancestors)
        })
    }

    fn mixin_ancestors(
        &self,
        entry: &ClassEntry,
        type_name: &TypeName,
        ancestors: &mut OneAncestors,
        targets: MixinTargets,
    ) -> Result<(), RbsError> {
        for declaration in entry.declarations() {
            let align = entry.align_parameters(declaration);
            self.mixin_ancestors_of_members(
                declaration.members(),
                type_name,
                align.as_ref(),
                ancestors,
                targets,
            )?;
        }
        Ok(())
    }

    fn check_mixin(&self, type_name: &TypeName, mixin: &Mixin) -> Result<(), RbsError> {
        if mixin.name.is_class()
            && self
                .environment
                .class_declarations
                .get(&mixin.name)
                .is_some_and(|entry| entry.kind == ClassOrModule::Class)
        {
            return Err(RbsError::MixinClass {
                type_name: type_name.clone(),
                member_name: Box::new(mixin.name.clone()),
                mixin_kind: mixin.mixin_kind,
                location: mixin.location.clone(),
            });
        }
        self.check_mixin_found(mixin)
    }

    fn check_mixin_found(&self, mixin: &Mixin) -> Result<(), RbsError> {
        let found = self.environment.is_module_name(&mixin.name)
            || self
                .environment
                .interface_declarations
                .contains_key(&mixin.name);
        if found {
            Ok(())
        } else {
            Err(RbsError::NoMixinFound {
                type_name: mixin.name.clone(),
                location: mixin.location.clone(),
            })
        }
    }

    fn module_entry(&self, name: &TypeName) -> Result<&'environment ClassEntry, RbsError> {
        self.environment
            .module_class_entry(name)
            .filter(|entry| entry.kind == ClassOrModule::Module)
            .ok_or_else(|| RbsError::RuntimeError(format!("Unknown module name: {name}")))
    }

    fn mixin_ancestors_of_members(
        &self,
        members: &[Member],
        type_name: &TypeName,
        align: Option<&Substitution>,
        ancestors: &mut OneAncestors,
        targets: MixinTargets,
    ) -> Result<(), RbsError> {
        for member in members {
            let Member::Mixin(mixin) = member else {
                continue;
            };
            let aligned_args = || -> Vec<Type> {
                mixin
                    .args
                    .iter()
                    .map(|arg| match align {
                        Some(substitution) => arg.substitute(substitution),
                        None => arg.clone(),
                    })
                    .collect()
            };
            let push = |list: &mut Option<Vec<Ancestor>>, ancestor: Ancestor| {
                list.as_mut().expect("a mixin target list").push(ancestor);
            };
            let source = AncestorSource::Member(mixin.location.clone());
            match mixin.mixin_kind {
                MixinKind::Include => match mixin.name.kind() {
                    TypeNameKind::Class if targets.included_modules => {
                        self.check_mixin(type_name, mixin)?;
                        let entry = self.module_entry(&mixin.name)?;
                        let args = normalize_args(entry.type_parameters(), &aligned_args());
                        push(
                            &mut ancestors.included_modules,
                            Ancestor::Instance {
                                name: self.environment.normalize_module_name(&mixin.name),
                                args,
                                source,
                            },
                        );
                    }
                    TypeNameKind::Interface if targets.included_interfaces => {
                        self.check_mixin_found(mixin)?;
                        let parameters =
                            &self.environment.interface_declarations[&mixin.name].declaration;
                        let args = normalize_args(&parameters.type_parameters, &aligned_args());
                        push(
                            &mut ancestors.included_interfaces,
                            Ancestor::Instance {
                                name: mixin.name.clone(),
                                args,
                                source,
                            },
                        );
                    }
                    _ => {}
                },
                MixinKind::Prepend if targets.prepended_modules => {
                    self.check_mixin(type_name, mixin)?;
                    let entry = self.module_entry(&mixin.name)?;
                    let args = normalize_args(entry.type_parameters(), &aligned_args());
                    push(
                        &mut ancestors.prepended_modules,
                        Ancestor::Instance {
                            name: entry.name.clone(),
                            args,
                            source,
                        },
                    );
                }
                MixinKind::Extend => match mixin.name.kind() {
                    TypeNameKind::Class if targets.extended_modules => {
                        self.check_mixin(type_name, mixin)?;
                        let entry = self.module_entry(&mixin.name)?;
                        let args = normalize_args(entry.type_parameters(), &mixin.args);
                        push(
                            &mut ancestors.extended_modules,
                            Ancestor::Instance {
                                name: self.environment.normalize_module_name(&mixin.name),
                                args,
                                source,
                            },
                        );
                    }
                    TypeNameKind::Interface if targets.extended_interfaces => {
                        self.check_mixin_found(mixin)?;
                        let parameters =
                            &self.environment.interface_declarations[&mixin.name].declaration;
                        let args = normalize_args(&parameters.type_parameters, &mixin.args);
                        push(
                            &mut ancestors.extended_interfaces,
                            Ancestor::Instance {
                                name: mixin.name.clone(),
                                args,
                                source,
                            },
                        );
                    }
                    _ => {}
                },
                MixinKind::Prepend => {}
            }
        }
        Ok(())
    }

    fn check_recursion(
        ancestor: &Ancestor,
        building: &[Ancestor],
        location: Option<&Location>,
    ) -> Result<(), RbsError> {
        let recursive = match ancestor {
            Ancestor::Instance { name, .. } => building.iter().any(|building| {
                matches!(building, Ancestor::Instance { name: building_name, .. } if building_name == name)
            }),
            Ancestor::Singleton { .. } => building.contains(ancestor),
        };
        if recursive {
            let mut ancestors = building.to_vec();
            ancestors.push(ancestor.clone());
            return Err(RbsError::RecursiveAncestor {
                ancestors,
                location: location.cloned(),
            });
        }
        Ok(())
    }

    fn apply(
        &self,
        ancestors: &Ancestors,
        args: &[Type],
        location: Option<&Location>,
    ) -> Result<Vec<Ancestor>, RbsError> {
        check_type_application_in(self.environment, &ancestors.type_name, args, location)?;
        let substitution = Substitution::build(&ancestors.params, args);
        Ok(ancestors
            .ancestors
            .iter()
            .map(|ancestor| ancestor.substitute(&substitution))
            .collect())
    }

    pub fn instance_ancestors(&self, type_name: &TypeName) -> Result<Arc<Ancestors>, RbsError> {
        self.instance_ancestors_building(type_name, &mut Vec::new())
    }

    fn instance_ancestors_building(
        &self,
        type_name: &TypeName,
        building: &mut Vec<Ancestor>,
    ) -> Result<Arc<Ancestors>, RbsError> {
        if let Some(ancestors) = self
            .instance_ancestors_cache
            .lock()
            .expect("a cache lock is never poisoned")
            .get(type_name)
        {
            return Ok(ancestors.clone());
        }
        let entry = self.class_entry(type_name)?;
        let params = entry.type_parameter_names();
        let self_ancestor = Ancestor::Instance {
            name: type_name.clone(),
            args: params.iter().cloned().map(Type::Variable).collect(),
            source: AncestorSource::Nothing,
        };
        let primary = entry.primary_declaration();
        Self::check_recursion(&self_ancestor, building, primary.location())?;
        building.push(self_ancestor.clone());

        let one_ancestors = self.one_instance_ancestors(type_name)?;
        let mut ancestors: Vec<Ancestor> = Vec::new();

        if entry.kind == ClassOrModule::Class
            && let Some(Ancestor::Instance { name, args, .. }) = &one_ancestors.super_class
        {
            let super_ancestors = self.instance_ancestors_building(name, building)?;
            ancestors = self.apply(
                &super_ancestors,
                args,
                primary
                    .super_class()
                    .and_then(|super_class| super_class.location.as_ref()),
            )?;
        }
        for self_type in one_ancestors.self_types.iter().flatten() {
            if self_type.name().is_class() {
                self.instance_ancestors_building(self_type.name(), building)?;
            }
        }
        for module in one_ancestors.included_modules.iter().flatten() {
            if let Ancestor::Instance { name, args, source } = module {
                let module_ancestors = self.instance_ancestors_building(name, building)?;
                let mut module_ancestors =
                    self.apply(&module_ancestors, args, source.location())?;
                module_ancestors.extend(ancestors);
                ancestors = module_ancestors;
            }
        }
        ancestors.insert(0, self_ancestor);
        for module in one_ancestors.prepended_modules.iter().flatten() {
            if let Ancestor::Instance { name, args, source } = module {
                let module_ancestors = self.instance_ancestors_building(name, building)?;
                let mut module_ancestors =
                    self.apply(&module_ancestors, args, source.location())?;
                module_ancestors.extend(ancestors);
                ancestors = module_ancestors;
            }
        }
        building.pop();

        let ancestors = Arc::new(Ancestors {
            type_name: type_name.clone(),
            params,
            ancestors,
        });
        let ancestors = self
            .instance_ancestors_cache
            .lock()
            .expect("a cache lock is never poisoned")
            .entry(type_name.clone())
            .or_insert(ancestors)
            .clone();
        Ok(ancestors)
    }

    pub fn singleton_ancestors(&self, type_name: &TypeName) -> Result<Arc<Ancestors>, RbsError> {
        self.singleton_ancestors_building(type_name, &mut Vec::new())
    }

    fn singleton_ancestors_building(
        &self,
        type_name: &TypeName,
        building: &mut Vec<Ancestor>,
    ) -> Result<Arc<Ancestors>, RbsError> {
        if let Some(ancestors) = self
            .singleton_ancestors_cache
            .lock()
            .expect("a cache lock is never poisoned")
            .get(type_name)
        {
            return Ok(ancestors.clone());
        }
        let entry = self.class_entry(type_name)?;
        let self_ancestor = Ancestor::Singleton {
            name: type_name.clone(),
        };
        Self::check_recursion(
            &self_ancestor,
            building,
            entry.primary_declaration().location(),
        )?;
        building.push(self_ancestor.clone());

        let one_ancestors = self.one_singleton_ancestors(type_name)?;
        let mut ancestors: Vec<Ancestor> = match &one_ancestors.super_class {
            Some(Ancestor::Instance { name, args, .. }) => {
                let super_ancestors = self.instance_ancestors_building(name, building)?;
                self.apply(&super_ancestors, args, None)?
            }
            Some(Ancestor::Singleton { name }) => self
                .singleton_ancestors_building(name, &mut Vec::new())?
                .ancestors
                .clone(),
            None => Vec::new(),
        };
        for module in one_ancestors.extended_modules.iter().flatten() {
            if let Ancestor::Instance { name, args, source } = module {
                let module_ancestors = self.instance_ancestors_building(name, building)?;
                let mut module_ancestors =
                    self.apply(&module_ancestors, args, source.location())?;
                module_ancestors.extend(ancestors);
                ancestors = module_ancestors;
            }
        }
        ancestors.insert(0, self_ancestor);
        building.pop();

        let ancestors = Arc::new(Ancestors {
            type_name: type_name.clone(),
            params: Vec::new(),
            ancestors,
        });
        let ancestors = self
            .singleton_ancestors_cache
            .lock()
            .expect("a cache lock is never poisoned")
            .entry(type_name.clone())
            .or_insert(ancestors)
            .clone();
        Ok(ancestors)
    }

    pub fn interface_ancestors(&self, type_name: &TypeName) -> Result<Arc<Ancestors>, RbsError> {
        self.interface_ancestors_building(type_name, &mut Vec::new())
    }

    fn interface_ancestors_building(
        &self,
        type_name: &TypeName,
        building: &mut Vec<Ancestor>,
    ) -> Result<Arc<Ancestors>, RbsError> {
        if let Some(ancestors) = self
            .interface_ancestors_cache
            .lock()
            .expect("a cache lock is never poisoned")
            .get(type_name)
        {
            return Ok(ancestors.clone());
        }
        let entry = self
            .environment
            .interface_declarations
            .get(type_name)
            .ok_or_else(|| {
                RbsError::RuntimeError(format!("Unknown name for interface_ancestors: {type_name}"))
            })?;
        let one_ancestors = self.one_interface_ancestors(type_name)?;
        let params = one_ancestors.params.clone();
        let self_ancestor = Ancestor::Instance {
            name: type_name.clone(),
            args: params.iter().cloned().map(Type::Variable).collect(),
            source: AncestorSource::Nothing,
        };
        Self::check_recursion(
            &self_ancestor,
            building,
            entry.declaration.location.as_ref(),
        )?;
        building.push(self_ancestor.clone());

        let mut ancestors: Vec<Ancestor> = Vec::new();
        for interface in one_ancestors.included_interfaces.iter().flatten() {
            if let Ancestor::Instance { name, args, source } = interface {
                let included = self.interface_ancestors_building(name, building)?;
                let mut included = self.apply(&included, args, source.location())?;
                included.extend(ancestors);
                ancestors = included;
            }
        }
        ancestors.insert(0, self_ancestor);
        building.pop();

        let ancestors = Arc::new(Ancestors {
            type_name: type_name.clone(),
            params,
            ancestors,
        });
        let ancestors = self
            .interface_ancestors_cache
            .lock()
            .expect("a cache lock is never poisoned")
            .entry(type_name.clone())
            .or_insert(ancestors)
            .clone();
        Ok(ancestors)
    }
}
