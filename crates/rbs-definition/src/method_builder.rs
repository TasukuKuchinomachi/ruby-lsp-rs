use std::sync::Arc;

use crate::{
    ast::{
        Alias, Attribute, AttributeKind, Member, MemberKind, MethodDefinition, MethodKind,
        Overload, TypeName, Visibility,
    },
    collections::{HashMap, HashSet},
    environment::Environment,
    errors::RbsError,
    location::Location,
    once_map::OnceMap,
};

#[derive(Clone, Debug)]
pub enum Original {
    MethodDefinition(Arc<MethodDefinition>),
    Attribute(Arc<Attribute>),
    Alias(Arc<Alias>),
}

#[derive(Clone, Debug)]
pub struct MethodEntry {
    pub name: String,
    pub originals: Vec<Original>,
    pub overloads: Vec<Arc<MethodDefinition>>,
    pub accessibilities: Vec<Visibility>,
}

impl Original {
    pub fn location(&self) -> Option<&Location> {
        match self {
            Original::MethodDefinition(definition) => definition.location.as_ref(),
            Original::Attribute(attribute) => attribute.location.as_ref(),
            Original::Alias(alias) => alias.location.as_ref(),
        }
    }
}

impl MethodEntry {
    pub fn original(&self) -> Option<&Original> {
        self.originals.first()
    }

    pub fn accessibility(&self) -> Visibility {
        self.accessibilities[0]
    }
}

#[derive(Clone, Debug, Default)]
pub struct Methods {
    entries: Vec<MethodEntry>,
    indexes: HashMap<String, usize>,
}

impl Methods {
    fn entry(&mut self, name: &str) -> &mut MethodEntry {
        let index = *self.indexes.entry(name.to_string()).or_insert_with(|| {
            self.entries.push(MethodEntry {
                name: name.to_string(),
                originals: Vec::new(),
                overloads: Vec::new(),
                accessibilities: Vec::new(),
            });
            self.entries.len() - 1
        });
        &mut self.entries[index]
    }

    fn validate(self, type_name: &TypeName, singleton: bool) -> Result<Methods, RbsError> {
        for entry in &self.entries {
            if entry.originals.len() > 1 {
                return Err(RbsError::DuplicatedMethodDefinition {
                    type_name: type_name.clone(),
                    singleton,
                    method_name: entry.name.clone(),
                    locations: entry
                        .originals
                        .iter()
                        .map(|original| original.location().cloned())
                        .collect(),
                });
            }
        }
        Ok(self)
    }

    pub fn entries(&self) -> &[MethodEntry] {
        &self.entries
    }

    pub fn sorted(&self, type_name: &TypeName) -> Result<Vec<&MethodEntry>, RbsError> {
        let mut sorted = Vec::new();
        let mut visited = HashSet::default();
        let mut visiting: Vec<&MethodEntry> = Vec::new();
        for entry in &self.entries {
            self.visit(type_name, entry, &mut visited, &mut visiting, &mut sorted)?;
        }
        Ok(sorted)
    }

    fn visit<'a>(
        &'a self,
        type_name: &TypeName,
        entry: &'a MethodEntry,
        visited: &mut HashSet<String>,
        visiting: &mut Vec<&'a MethodEntry>,
        sorted: &mut Vec<&'a MethodEntry>,
    ) -> Result<(), RbsError> {
        if visited.contains(&entry.name) {
            return Ok(());
        }
        if let Some(index) = visiting
            .iter()
            .position(|visiting| visiting.name == entry.name)
        {
            let cycle = &visiting[index..];
            return Err(RbsError::RecursiveAliasDefinition {
                type_name: type_name.clone(),
                names: cycle.iter().map(|entry| entry.name.clone()).collect(),
                location: cycle[0].original().and_then(Original::location).cloned(),
            });
        }
        visiting.push(entry);
        if let Some(Original::Alias(alias)) = entry.original()
            && let Some(&index) = self.indexes.get(&alias.old_name)
        {
            self.visit(type_name, &self.entries[index], visited, visiting, sorted)?;
        }
        visiting.pop();
        visited.insert(entry.name.clone());
        sorted.push(entry);
        Ok(())
    }

    fn build_method(&mut self, member: Arc<MethodDefinition>, accessibility: Visibility) {
        let entry = self.entry(&member.name);
        if member.overloading {
            entry.overloads.push(member);
        } else {
            entry.accessibilities.push(accessibility);
            entry.originals.push(Original::MethodDefinition(member));
        }
    }

    fn build_attribute(&mut self, member: Arc<Attribute>, accessibility: Visibility) {
        if matches!(
            member.attribute_kind,
            AttributeKind::Reader | AttributeKind::Accessor
        ) {
            let entry = self.entry(&member.name);
            entry.accessibilities.push(accessibility);
            entry.originals.push(Original::Attribute(member.clone()));
        }
        if matches!(
            member.attribute_kind,
            AttributeKind::Writer | AttributeKind::Accessor
        ) {
            let entry = self.entry(&format!("{}=", member.name));
            entry.accessibilities.push(accessibility);
            entry.originals.push(Original::Attribute(member));
        }
    }

    fn build_alias(&mut self, member: Arc<Alias>) {
        let entry = self.entry(&member.new_name);
        entry.originals.push(Original::Alias(member));
    }
}

pub struct MethodBuilder<'environment> {
    environment: &'environment Environment,
    instance_methods: OnceMap<TypeName, Result<Arc<Methods>, RbsError>>,
    singleton_methods: OnceMap<TypeName, Result<Arc<Methods>, RbsError>>,
    interface_methods: OnceMap<TypeName, Result<Arc<Methods>, RbsError>>,
}

fn cached(
    cache: &OnceMap<TypeName, Result<Arc<Methods>, RbsError>>,
    type_name: &TypeName,
    build: impl FnOnce() -> Result<Methods, RbsError>,
) -> Result<Arc<Methods>, RbsError> {
    cache.get_or_init(type_name, || build().map(Arc::new))
}

impl<'environment> MethodBuilder<'environment> {
    pub fn new(environment: &'environment Environment) -> MethodBuilder<'environment> {
        MethodBuilder {
            environment,
            instance_methods: OnceMap::default(),
            singleton_methods: OnceMap::default(),
            interface_methods: OnceMap::default(),
        }
    }

    pub fn build_instance(&self, type_name: &TypeName) -> Result<Arc<Methods>, RbsError> {
        cached(&self.instance_methods, type_name, || {
            let entry = self
                .environment
                .class_declarations
                .get(type_name)
                .ok_or_else(|| format!("unknown class or module: {type_name}"))?;
            let mut methods = Methods::default();
            for declaration in entry.declarations() {
                let substitution = entry.align_parameters(declaration);
                let mut accessibility = Visibility::Public;
                for member in declaration.members() {
                    match member {
                        Member::Visibility(visibility) => accessibility = *visibility,
                        Member::MethodDefinition(definition) => {
                            let definition = match &substitution {
                                Some(substitution) => MethodDefinition {
                                    overloads: definition
                                        .overloads
                                        .iter()
                                        .map(|overload| Overload {
                                            method_type: Arc::new(
                                                overload.method_type.substitute(substitution),
                                            ),
                                            annotations: overload.annotations.clone(),
                                        })
                                        .collect(),
                                    ..definition.clone()
                                },
                                None => definition.clone(),
                            };
                            match definition.kind {
                                MethodKind::Instance => {
                                    let visibility = definition.visibility.unwrap_or(accessibility);
                                    methods.build_method(Arc::new(definition), visibility);
                                }
                                MethodKind::SingletonInstance => {
                                    methods.build_method(Arc::new(definition), Visibility::Private);
                                }
                                MethodKind::Singleton => {}
                            }
                        }
                        Member::Attribute(attribute) if attribute.kind == MemberKind::Instance => {
                            let mut attribute = attribute.clone();
                            if let Some(substitution) = &substitution {
                                attribute.type_ = attribute.type_.substitute(substitution);
                            }
                            let visibility = attribute.visibility.unwrap_or(accessibility);
                            methods.build_attribute(Arc::new(attribute), visibility);
                        }
                        Member::Alias(alias) if alias.kind == MemberKind::Instance => {
                            methods.build_alias(Arc::new(alias.clone()));
                        }
                        _ => {}
                    }
                }
            }
            methods.validate(type_name, false)
        })
    }

    pub fn build_singleton(&self, type_name: &TypeName) -> Result<Arc<Methods>, RbsError> {
        cached(&self.singleton_methods, type_name, || {
            let entry = self
                .environment
                .class_declarations
                .get(type_name)
                .ok_or_else(|| format!("unknown class or module: {type_name}"))?;
            let mut methods = Methods::default();
            for declaration in entry.declarations() {
                for member in declaration.members() {
                    match member {
                        Member::MethodDefinition(definition)
                            if definition.kind != MethodKind::Instance =>
                        {
                            let visibility = definition.visibility.unwrap_or(Visibility::Public);
                            methods.build_method(Arc::new(definition.clone()), visibility);
                        }
                        Member::Attribute(attribute) if attribute.kind == MemberKind::Singleton => {
                            let visibility = attribute.visibility.unwrap_or(Visibility::Public);
                            methods.build_attribute(Arc::new(attribute.clone()), visibility);
                        }
                        Member::Alias(alias) if alias.kind == MemberKind::Singleton => {
                            methods.build_alias(Arc::new(alias.clone()));
                        }
                        _ => {}
                    }
                }
            }
            methods.validate(type_name, true)
        })
    }

    pub fn build_interface(&self, type_name: &TypeName) -> Result<Arc<Methods>, RbsError> {
        cached(&self.interface_methods, type_name, || {
            let entry = self
                .environment
                .interface_declarations
                .get(type_name)
                .ok_or_else(|| format!("unknown interface: {type_name}"))?;
            let mut methods = Methods::default();
            for member in &entry.declaration.members {
                match member {
                    Member::MethodDefinition(definition) => {
                        methods.build_method(Arc::new(definition.clone()), Visibility::Public);
                    }
                    Member::Alias(alias) => methods.build_alias(Arc::new(alias.clone())),
                    _ => {}
                }
            }
            methods.validate(type_name, false)
        })
    }
}
