use std::{
    sync::Arc,
    sync::atomic::{AtomicUsize, Ordering},
};

use crate::{
    ancestor_builder::{Ancestor, AncestorBuilder, AncestorSource, Ancestors, OneAncestors},
    ast::{
        Alias, Attribute, AttributeInstanceVariable, Callable, ClassOrModule, Function, Member,
        MemberKind, MethodDefinition, MethodKind, MethodType, Namespace, Parameter, Type, TypeName,
        TypeNameKind, TypeParameter, VariableKind, Visibility,
    },
    collections::{HashSet, IndexMap},
    environment::Environment,
    errors::RbsError,
    location::{AstLocation, Location, TypeLocation},
    method_builder::{MethodBuilder, MethodEntry, Methods, Original},
    once_map::OnceMap,
    substitution::{Substitution, application},
    variance_calculator::VarianceCalculator,
};

#[derive(Clone, Debug)]
pub enum DefinitionMember {
    MethodDefinition(Arc<MethodDefinition>),
    Attribute(Arc<Attribute>),
}

#[derive(Clone, Debug)]
pub struct TypeDefinition {
    pub type_: Arc<MethodType>,
    pub member: DefinitionMember,
    pub defined_in: TypeName,
    pub implemented_in: Option<TypeName>,
    pub member_annotations: Vec<String>,
    pub overload_annotations: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct Method {
    pub super_method: Option<Arc<Method>>,
    pub defs: Vec<TypeDefinition>,
    pub accessibility: Visibility,
    pub annotations: Vec<String>,
    pub alias_of: Option<Arc<Method>>,
    pub alias_member: Option<Arc<Alias>>,
}

impl Method {
    pub fn method_types(&self) -> impl Iterator<Item = &MethodType> {
        self.defs.iter().map(|definition| &*definition.type_)
    }

    pub fn defined_in(&self) -> &TypeName {
        &self
            .defs
            .last()
            .expect("a method has a definition")
            .defined_in
    }

    pub fn implemented_in(&self) -> Option<&TypeName> {
        self.defs
            .last()
            .expect("a method has a definition")
            .implemented_in
            .as_ref()
    }

    fn member_locations(&self) -> Vec<Option<Location>> {
        let mut locations: Vec<Option<Location>> = Vec::new();
        let mut members: Vec<*const ()> = Vec::new();
        for definition in &self.defs {
            let (pointer, location) = match &definition.member {
                DefinitionMember::MethodDefinition(member) => {
                    (Arc::as_ptr(member) as *const (), member.location.clone())
                }
                DefinitionMember::Attribute(member) => {
                    (Arc::as_ptr(member) as *const (), member.location.clone())
                }
            };
            if !members.contains(&pointer) {
                members.push(pointer);
                locations.push(location);
            }
        }
        locations
    }

    fn substitute(&self, substitution: &Substitution) -> Method {
        if substitution.is_empty() {
            return self.clone();
        }
        Method {
            super_method: self
                .super_method
                .as_ref()
                .map(|method| Arc::new(method.substitute(substitution))),
            defs: self
                .defs
                .iter()
                .map(|definition| TypeDefinition {
                    type_: Arc::new(definition.type_.substitute(substitution)),
                    ..definition.clone()
                })
                .collect(),
            ..self.clone()
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VariableSourceKind {
    Attribute,
    InstanceVariable,
    ClassInstanceVariable,
    ClassVariable,
}

#[derive(Clone, Debug)]
pub struct VariableSource {
    pub kind: VariableSourceKind,
    pub name: String,
    pub location: Option<Location>,
}

#[derive(Clone, Debug)]
pub struct DefinitionVariable {
    pub type_: Type,
    pub declared_in: TypeName,
    pub type_location: Option<Location>,
    pub source: VariableSource,
    pub parent_variable: Option<Box<DefinitionVariable>>,
}

impl DefinitionVariable {
    fn substitute(&self, substitution: &Substitution) -> DefinitionVariable {
        DefinitionVariable {
            type_: self.type_.substitute(substitution),
            ..self.clone()
        }
    }

    pub fn type_location(&self) -> Option<&Location> {
        self.type_.location().or(self.type_location.as_ref())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DefinitionKind {
    Instance,
    Singleton,
    Interface,
}

#[derive(Clone, Debug)]
pub struct Definition {
    pub type_name: TypeName,
    pub kind: DefinitionKind,
    pub self_type: Type,
    pub type_parameters: Vec<TypeParameter>,
    pub ancestors: Arc<Ancestors>,
    pub methods: IndexMap<String, Arc<Method>>,
    pub instance_variables: IndexMap<String, DefinitionVariable>,
    pub class_variables: IndexMap<String, DefinitionVariable>,
}

impl Definition {
    fn substitute(&self, substitution: &Substitution) -> Definition {
        if substitution.is_empty() {
            return self.clone();
        }
        let variables = |variables: &IndexMap<String, DefinitionVariable>| {
            variables
                .iter()
                .map(|(name, variable)| (name.clone(), variable.substitute(substitution)))
                .collect()
        };
        Definition {
            self_type: self.self_type.substitute(substitution),
            methods: self
                .methods
                .iter()
                .map(|(name, method)| (name.clone(), Arc::new(method.substitute(substitution))))
                .collect(),
            instance_variables: variables(&self.instance_variables),
            class_variables: variables(&self.class_variables),
            ..self.clone()
        }
    }
}

fn inherited_ruby_method(
    existing_method: Arc<Method>,
    implemented_in: Option<&TypeName>,
) -> Method {
    Method {
        super_method: Some(existing_method.clone()),
        defs: existing_method
            .defs
            .iter()
            .map(|type_definition| TypeDefinition {
                implemented_in: implemented_in.cloned(),
                ..type_definition.clone()
            })
            .collect(),
        accessibility: Visibility::Public,
        annotations: existing_method.annotations.clone(),
        alias_of: existing_method.alias_of.clone(),
        alias_member: None,
    }
}

fn special_accessibility(is_instance: bool, method_name: &str) -> Option<Visibility> {
    const NAMES: &[&str] = &[
        "initialize",
        "initialize_copy",
        "initialize_clone",
        "initialize_dup",
        "respond_to_missing?",
    ];
    (is_instance && NAMES.contains(&method_name)).then_some(Visibility::Private)
}

static FRESH_VARIABLE_COUNT: AtomicUsize = AtomicUsize::new(0);

fn fresh_variable_name(name: &str) -> String {
    let count = FRESH_VARIABLE_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    format!("{name}@{count}")
}

fn variables(names: &[String]) -> Vec<Type> {
    names.iter().cloned().map(Type::Variable).collect()
}

struct InterfaceMethods {
    interface: Ancestor,
    methods: Arc<Methods>,
}

pub struct DefinitionBuilder {
    pub ancestor_builder: AncestorBuilder,
    method_builder: MethodBuilder,
    instance_cache: OnceMap<TypeName, Result<Arc<Definition>, RbsError>>,
    singleton0_cache: OnceMap<TypeName, Result<Arc<Definition>, RbsError>>,
    singleton_cache: OnceMap<TypeName, Result<Arc<Definition>, RbsError>>,
    interface_cache: OnceMap<TypeName, Result<Arc<Definition>, RbsError>>,
}

fn cached(
    cache: &OnceMap<TypeName, Result<Arc<Definition>, RbsError>>,
    type_name: &TypeName,
    build: impl FnOnce() -> Result<Definition, RbsError>,
) -> Result<Arc<Definition>, RbsError> {
    cache.get_or_init(type_name, || build().map(Arc::new))
}

fn attribute_method_type(attribute: &Attribute, attribute_type: Type, setter: bool) -> MethodType {
    let function = if setter {
        Function {
            required_positionals: vec![Parameter {
                type_: attribute_type.clone(),
                name: Some(attribute.name.clone()),
            }],
            ..empty_function(attribute_type)
        }
    } else {
        empty_function(attribute_type)
    };
    MethodType {
        type_parameters: Vec::new(),
        type_: Callable::Function(Box::new(function)),
        block: None,
        location: AstLocation::new(attribute.location.clone()),
    }
}

impl DefinitionBuilder {
    pub fn new(environment: Arc<Environment>) -> DefinitionBuilder {
        DefinitionBuilder::with_ancestor_builder(AncestorBuilder::new(environment))
    }

    pub fn with_ancestor_builder(ancestor_builder: AncestorBuilder) -> DefinitionBuilder {
        let environment = ancestor_builder.shared_environment().clone();
        DefinitionBuilder {
            ancestor_builder,
            method_builder: MethodBuilder::new(environment),
            instance_cache: OnceMap::default(),
            singleton0_cache: OnceMap::default(),
            singleton_cache: OnceMap::default(),
            interface_cache: OnceMap::default(),
        }
    }

    pub fn environment(&self) -> &Environment {
        self.ancestor_builder.environment()
    }

    pub fn ensure_namespace(
        &self,
        namespace: &Namespace,
        location: Option<&Location>,
    ) -> Result<(), RbsError> {
        self.environment().ensure_namespace(namespace, location)
    }

    pub fn validate_type_name(
        &self,
        name: &TypeName,
        location: Option<&Location>,
    ) -> Result<(), RbsError> {
        let name = name.absolute();
        let normalized = self
            .environment()
            .normalize_type_name(&name)
            .unwrap_or_else(|| name.clone());
        if self.environment().is_type_name(&normalized) {
            return Ok(());
        }
        Err(RbsError::NoTypeFound {
            type_name: name,
            location: location.cloned(),
        })
    }

    pub fn validate_type_presence(&self, type_: &Type) -> Result<(), RbsError> {
        if let Type::ClassInstance { name, .. }
        | Type::ClassSingleton { name, .. }
        | Type::Interface { name, .. }
        | Type::Alias { name, .. } = type_
        {
            self.validate_type_name(name, type_.location())?;
        }
        for child in type_.each_type() {
            self.validate_type_presence(child)?;
        }
        Ok(())
    }

    fn validate_types_presence(&self, types: &[Type]) -> Result<(), RbsError> {
        for type_ in types {
            self.validate_type_presence(type_)?;
        }
        Ok(())
    }

    fn type_application(&self, name: &TypeName, args: &[Type]) -> Result<Substitution, RbsError> {
        let parameters: &[TypeParameter] = match name.kind() {
            TypeNameKind::Interface => {
                &self
                    .environment()
                    .interface_declarations
                    .get(name)
                    .ok_or_else(|| format!("Unknown interface name: {name}"))?
                    .declaration
                    .type_parameters
            }
            TypeNameKind::Alias => {
                &self
                    .environment()
                    .type_alias_declarations
                    .get(name)
                    .ok_or_else(|| format!("Unknown alias name: {name}"))?
                    .declaration
                    .type_parameters
            }
            TypeNameKind::Class => self
                .environment()
                .class_declarations
                .get(name)
                .ok_or_else(|| format!("Unknown module name: {name}"))?
                .type_parameters(),
        };
        Ok(application(parameters, args).unwrap_or_default())
    }

    fn interface_methods(
        &self,
        interfaces: Vec<Ancestor>,
    ) -> Result<Vec<InterfaceMethods>, RbsError> {
        interfaces
            .into_iter()
            .map(|interface| {
                Ok(InterfaceMethods {
                    methods: self.method_builder.build_interface(interface.name())?,
                    interface,
                })
            })
            .collect()
    }

    fn included_interfaces_with_their_ancestors(
        &self,
        interfaces: &[Ancestor],
    ) -> Result<Vec<Ancestor>, RbsError> {
        let mut all = Vec::new();
        for interface in interfaces {
            all.push(interface.clone());
            let ancestors = self
                .ancestor_builder
                .interface_ancestors(interface.name())?;
            all.extend(ancestors.ancestors.iter().skip(1).cloned());
        }
        Ok(all)
    }

    fn validate_type_params(
        &self,
        definition: &Definition,
        ancestors: &OneAncestors,
        methods: &Methods,
        declaration_location: Option<&Location>,
        super_class_location: Option<&Location>,
    ) -> Result<(), RbsError> {
        let type_params = &definition.type_parameters;
        let calculator = VarianceCalculator::new(self.environment());
        let parameter_names: Vec<String> = type_params
            .iter()
            .map(|parameter| parameter.name.clone())
            .collect();
        for ancestor in ancestors.each_ancestor() {
            let Ancestor::Instance { name, args, source } = ancestor else {
                continue;
            };
            let result = calculator.in_inherit(name, args, &parameter_names)?;
            for parameter in type_params {
                if !parameter.unchecked
                    && !result.is_compatible(&parameter.name, parameter.variance)
                {
                    let location = match source {
                        AncestorSource::Nothing => declaration_location.cloned(),
                        AncestorSource::Super => super_class_location.cloned(),
                        AncestorSource::Member(location) => location.clone(),
                    };
                    return Err(RbsError::InvalidVarianceAnnotation {
                        type_name: definition.type_name.clone(),
                        param: Box::new(parameter.clone()),
                        location,
                    });
                }
            }
        }
        for entry in methods.entries() {
            if entry.name == "initialize" {
                continue;
            }
            let method_types: Vec<MethodType> = match entry.original() {
                Some(Original::MethodDefinition(definition)) => definition
                    .overloads
                    .iter()
                    .map(|overload| (*overload.method_type).clone())
                    .collect(),
                Some(Original::Attribute(attribute)) => vec![attribute_method_type(
                    attribute,
                    attribute.type_.clone(),
                    entry.name.ends_with('='),
                )],
                Some(Original::Alias(_)) | None => continue,
            };
            for method_type in method_types {
                let method_parameter_names: Vec<&String> = method_type
                    .type_parameters
                    .iter()
                    .map(|parameter| &parameter.name)
                    .collect();
                let merged: Vec<&TypeParameter> = type_params
                    .iter()
                    .filter(|parameter| !method_parameter_names.contains(&&parameter.name))
                    .chain(method_type.type_parameters.iter())
                    .collect();
                let result = calculator.in_method_type(&method_type, &parameter_names)?;
                for parameter in merged {
                    if !parameter.unchecked
                        && !result.is_compatible(&parameter.name, parameter.variance)
                    {
                        return Err(RbsError::InvalidVarianceAnnotation {
                            type_name: definition.type_name.clone(),
                            param: Box::new(parameter.clone()),
                            location: method_type.location.location(),
                        });
                    }
                }
            }
        }
        Ok(())
    }

    pub fn build_interface(&self, type_name: &TypeName) -> Result<Arc<Definition>, RbsError> {
        cached(&self.interface_cache, type_name, || {
            let entry = self
                .environment()
                .interface_declarations
                .get(type_name)
                .ok_or_else(|| format!("Unknown name for build_interface: {type_name}"))?;
            self.ensure_namespace(&type_name.namespace, entry.declaration.location.as_ref())?;
            let parameters: Vec<String> = entry
                .declaration
                .type_parameters
                .iter()
                .map(|parameter| parameter.name.clone())
                .collect();
            let mut definition = Definition {
                type_name: type_name.clone(),
                kind: DefinitionKind::Interface,
                self_type: Type::Interface {
                    name: type_name.clone(),
                    args: variables(&parameters),
                    location: TypeLocation::default(),
                },
                type_parameters: entry.declaration.type_parameters.clone(),
                ancestors: self.ancestor_builder.interface_ancestors(type_name)?,
                methods: IndexMap::default(),
                instance_variables: IndexMap::default(),
                class_variables: IndexMap::default(),
            };
            let methods = self.method_builder.build_interface(type_name)?;
            let one_ancestors = self.ancestor_builder.one_interface_ancestors(type_name)?;
            self.validate_type_params(
                &definition,
                &one_ancestors,
                &methods,
                entry.declaration.location.as_ref(),
                None,
            )?;
            let substitution = Substitution::build(&parameters, &variables(&parameters));
            self.define_interface(&mut definition, type_name, &substitution)?;
            Ok(definition)
        })
    }

    fn define_interface(
        &self,
        definition: &mut Definition,
        type_name: &TypeName,
        substitution: &Substitution,
    ) -> Result<(), RbsError> {
        let included = self
            .ancestor_builder
            .interface_ancestors(type_name)?
            .ancestors
            .iter()
            .skip(1)
            .cloned()
            .collect();
        let interface_methods = self.interface_methods(included)?;
        let methods = self.method_builder.build_interface(type_name)?;
        self.import_methods(
            definition,
            type_name,
            &methods,
            &interface_methods,
            substitution,
            None,
        )
    }

    fn insert_variable(
        &self,
        variables: &mut IndexMap<String, DefinitionVariable>,
        name: String,
        mut variable: DefinitionVariable,
    ) -> Result<(), RbsError> {
        variable.parent_variable = variables.get(&name).cloned().map(Box::new);
        validate_variable(&variable)?;
        variables.insert(name, variable);
        Ok(())
    }

    fn define_instance(
        &self,
        definition: &mut Definition,
        type_name: &TypeName,
        substitution: &Substitution,
        define_class_variables: bool,
    ) -> Result<(), RbsError> {
        let one_ancestors = self.ancestor_builder.one_instance_ancestors(type_name)?;
        let methods = self.method_builder.build_instance(type_name)?;

        let mut self_type_methods: IndexMap<String, Arc<Method>> = IndexMap::default();
        for self_type in one_ancestors.self_types.iter().flatten() {
            let Ancestor::Instance { name, args, .. } = self_type else {
                continue;
            };
            self.validate_types_presence(args)?;
            let self_type_definition = if name.kind() == TypeNameKind::Interface {
                self.build_interface(name)?
            } else {
                self.build_instance(name)?
            };
            let self_type_substitution = substitution.compose(&self.type_application(name, args)?);
            for (method_name, method) in &self_type_definition.methods {
                self_type_methods.insert(
                    method_name.clone(),
                    Arc::new(method.substitute(&self_type_substitution)),
                );
            }
        }

        for module in one_ancestors.included_modules.iter().flatten() {
            if let Ancestor::Instance { name, args, .. } = module {
                self.validate_types_presence(args)?;
                let module_substitution = substitution.compose(&self.type_application(name, args)?);
                self.define_instance(
                    definition,
                    name,
                    &module_substitution,
                    define_class_variables,
                )?;
            }
        }

        let interfaces = self.included_interfaces_with_their_ancestors(
            one_ancestors
                .included_interfaces
                .as_deref()
                .unwrap_or_default(),
        )?;
        let interface_methods = self.interface_methods(interfaces)?;
        self.import_methods(
            definition,
            type_name,
            &methods,
            &interface_methods,
            substitution,
            Some(&self_type_methods),
        )?;

        for module in one_ancestors.prepended_modules.iter().flatten() {
            if let Ancestor::Instance { name, args, .. } = module {
                self.validate_types_presence(args)?;
                let module_substitution = substitution.compose(&self.type_application(name, args)?);
                self.define_instance(
                    definition,
                    name,
                    &module_substitution,
                    define_class_variables,
                )?;
            }
        }

        let entry = &self.environment().class_declarations[type_name];
        for declaration in entry.declarations() {
            let declaration_substitution = match entry.align_parameters(declaration) {
                Some(align) => substitution.compose(&align),
                None => substitution.clone(),
            };
            for member in declaration.members() {
                match member {
                    Member::Attribute(attribute) if attribute.kind == MemberKind::Instance => {
                        if let Some(name) = attribute_instance_variable_name(attribute) {
                            self.insert_variable(
                                &mut definition.instance_variables,
                                name.clone(),
                                DefinitionVariable {
                                    type_: attribute.type_.substitute(&declaration_substitution),
                                    declared_in: type_name.clone(),
                                    type_location: attribute.type_location.clone(),
                                    source: VariableSource {
                                        kind: VariableSourceKind::Attribute,
                                        name,
                                        location: attribute.location.clone(),
                                    },
                                    parent_variable: None,
                                },
                            )?;
                        }
                    }
                    Member::Variable(variable)
                        if variable.variable_kind == VariableKind::InstanceVariable =>
                    {
                        self.insert_variable(
                            &mut definition.instance_variables,
                            variable.name.clone(),
                            DefinitionVariable {
                                type_: variable.type_.substitute(&declaration_substitution),
                                declared_in: type_name.clone(),
                                type_location: variable.type_location.clone(),
                                source: VariableSource {
                                    kind: VariableSourceKind::InstanceVariable,
                                    name: variable.name.clone(),
                                    location: variable.location.clone(),
                                },
                                parent_variable: None,
                            },
                        )?;
                    }
                    Member::Variable(variable)
                        if variable.variable_kind == VariableKind::ClassVariable
                            && define_class_variables =>
                    {
                        self.insert_variable(
                            &mut definition.class_variables,
                            variable.name.clone(),
                            DefinitionVariable {
                                type_: variable.type_.clone(),
                                declared_in: type_name.clone(),
                                type_location: variable.type_location.clone(),
                                source: VariableSource {
                                    kind: VariableSourceKind::ClassVariable,
                                    name: variable.name.clone(),
                                    location: variable.location.clone(),
                                },
                                parent_variable: None,
                            },
                        )?;
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }

    pub fn build_instance(&self, type_name: &TypeName) -> Result<Arc<Definition>, RbsError> {
        let type_name = self.environment().normalize_module_name(type_name);
        cached(&self.instance_cache, &type_name.clone(), || {
            let entry = self
                .environment()
                .class_declarations
                .get(&type_name)
                .ok_or_else(|| format!("Unknown name for build_instance: {type_name}"))?;
            let primary = entry.primary_declaration();
            self.ensure_namespace(&type_name.namespace, primary.location())?;
            let parameters = entry.type_parameter_names();
            let mut definition = Definition {
                type_name: type_name.clone(),
                kind: DefinitionKind::Instance,
                self_type: Type::ClassInstance {
                    name: type_name.clone(),
                    args: variables(&parameters),
                    location: TypeLocation::default(),
                },
                type_parameters: entry.type_parameters().to_vec(),
                ancestors: self.ancestor_builder.instance_ancestors(&type_name)?,
                methods: IndexMap::default(),
                instance_variables: IndexMap::default(),
                class_variables: IndexMap::default(),
            };
            let one_ancestors = self.ancestor_builder.one_instance_ancestors(&type_name)?;
            let methods = self.method_builder.build_instance(&type_name)?;
            self.validate_type_params(
                &definition,
                &one_ancestors,
                &methods,
                primary.location(),
                primary
                    .super_class()
                    .and_then(|super_class| super_class.location.as_ref()),
            )?;
            match entry.kind {
                ClassOrModule::Class => {
                    if let Some(Ancestor::Instance { name, args, .. }) = &one_ancestors.super_class
                    {
                        let mut super_definition = self.build_instance(name)?;
                        if !args.is_empty() {
                            self.validate_types_presence(args)?;
                            super_definition = Arc::new(
                                super_definition.substitute(&self.type_application(name, args)?),
                            );
                        }
                        definition.methods.extend(super_definition.methods.clone());
                        definition
                            .instance_variables
                            .extend(super_definition.instance_variables.clone());
                        definition
                            .class_variables
                            .extend(super_definition.class_variables.clone());
                    }
                }
                ClassOrModule::Module => {
                    for self_type in one_ancestors.self_types.iter().flatten() {
                        if let Ancestor::Instance { name, args, .. } = self_type {
                            self.validate_types_presence(args)?;
                            let substitution = self.type_application(name, args)?;
                            if name.kind() == TypeNameKind::Interface {
                                self.define_interface(&mut definition, name, &substitution)?;
                            } else {
                                self.define_instance(&mut definition, name, &substitution, true)?;
                            }
                        }
                    }
                }
            }
            self.define_instance(&mut definition, &type_name, &Substitution::default(), true)?;
            Ok(definition)
        })
    }

    fn build_singleton0(&self, type_name: &TypeName) -> Result<Arc<Definition>, RbsError> {
        cached(&self.singleton0_cache, type_name, || {
            let entry = self
                .environment()
                .class_declarations
                .get(type_name)
                .ok_or_else(|| format!("Unknown name for build_singleton0: {type_name}"))?;
            self.ensure_namespace(&type_name.namespace, entry.primary_declaration().location())?;
            let mut definition = Definition {
                type_name: type_name.clone(),
                kind: DefinitionKind::Singleton,
                self_type: Type::ClassSingleton {
                    name: type_name.clone(),
                    args: Vec::new(),
                    location: TypeLocation::default(),
                },
                type_parameters: Vec::new(),
                ancestors: self.ancestor_builder.singleton_ancestors(type_name)?,
                methods: IndexMap::default(),
                instance_variables: IndexMap::default(),
                class_variables: IndexMap::default(),
            };
            let one_ancestors = self.ancestor_builder.one_singleton_ancestors(type_name)?;
            let methods = self.method_builder.build_singleton(type_name)?;

            if let Some(super_class) = &one_ancestors.super_class {
                let super_definition = match super_class {
                    Ancestor::Instance { name, .. } => self.build_instance(name)?,
                    Ancestor::Singleton { name } => self.build_singleton0(name)?,
                };
                definition.methods.extend(super_definition.methods.clone());
                definition
                    .instance_variables
                    .extend(super_definition.instance_variables.clone());
            }

            for module in one_ancestors.extended_modules.iter().flatten() {
                if let Ancestor::Instance { name, args, .. } = module {
                    self.validate_types_presence(args)?;
                    let substitution = self.type_application(name, args)?;
                    self.define_instance(&mut definition, name, &substitution, false)?;
                }
            }

            let interfaces = self.included_interfaces_with_their_ancestors(
                one_ancestors
                    .extended_interfaces
                    .as_deref()
                    .unwrap_or_default(),
            )?;
            let interface_methods = self.interface_methods(interfaces)?;
            self.import_methods(
                &mut definition,
                type_name,
                &methods,
                &interface_methods,
                &Substitution::default(),
                None,
            )?;

            for declaration in entry.declarations() {
                for member in declaration.members() {
                    match member {
                        Member::Attribute(attribute) if attribute.kind == MemberKind::Singleton => {
                            if let Some(name) = attribute_instance_variable_name(attribute) {
                                self.insert_variable(
                                    &mut definition.instance_variables,
                                    name.clone(),
                                    DefinitionVariable {
                                        type_: attribute.type_.clone(),
                                        declared_in: type_name.clone(),
                                        type_location: attribute.type_location.clone(),
                                        source: VariableSource {
                                            kind: VariableSourceKind::Attribute,
                                            name,
                                            location: attribute.location.clone(),
                                        },
                                        parent_variable: None,
                                    },
                                )?;
                            }
                        }
                        Member::Variable(variable)
                            if variable.variable_kind == VariableKind::ClassInstanceVariable =>
                        {
                            self.insert_variable(
                                &mut definition.instance_variables,
                                variable.name.clone(),
                                DefinitionVariable {
                                    type_: variable.type_.clone(),
                                    declared_in: type_name.clone(),
                                    type_location: variable.type_location.clone(),
                                    source: VariableSource {
                                        kind: VariableSourceKind::ClassInstanceVariable,
                                        name: variable.name.clone(),
                                        location: variable.location.clone(),
                                    },
                                    parent_variable: None,
                                },
                            )?;
                        }
                        _ => {}
                    }
                }
            }

            definition.class_variables = self.build_instance(type_name)?.class_variables.clone();
            Ok(definition)
        })
    }

    pub fn build_singleton(&self, type_name: &TypeName) -> Result<Arc<Definition>, RbsError> {
        let type_name = self.environment().normalize_module_name(type_name);
        cached(&self.singleton_cache, &type_name.clone(), || {
            let entry = self
                .environment()
                .class_declarations
                .get(&type_name)
                .ok_or_else(|| format!("Unknown name for build_singleton: {type_name}"))?;
            self.ensure_namespace(&type_name.namespace, entry.primary_declaration().location())?;
            let mut definition = (*self.build_singleton0(&type_name)?).clone();
            if entry.kind != ClassOrModule::Class {
                return Ok(definition);
            }
            let class_name = TypeName::parse("::Class");
            let Some(new_method) = definition.methods.get("new").cloned() else {
                return Ok(definition);
            };
            if !new_method
                .defs
                .iter()
                .all(|type_definition| type_definition.defined_in == class_name)
            {
                return Ok(definition);
            }
            let alias_names: Vec<String> = definition
                .methods
                .iter()
                .filter(|(_, method)| {
                    let mut current = Some(Arc::clone(method));
                    while let Some(method) = current {
                        match &method.alias_of {
                            Some(alias_of) if Arc::ptr_eq(alias_of, &new_method) => return true,
                            alias_of => current = alias_of.clone(),
                        }
                    }
                    false
                })
                .map(|(name, _)| name.clone())
                .collect();

            let instance = self.build_instance(&type_name)?;
            let Some(initialize) = instance.methods.get("initialize") else {
                return Ok(definition);
            };
            let class_parameters = entry.type_parameters();
            let class_parameter_names = entry.type_parameter_names();
            let instance_type = Type::ClassInstance {
                name: type_name.clone(),
                args: variables(&class_parameter_names),
                location: TypeLocation::default(),
            };
            let defs = initialize
                .defs
                .iter()
                .map(|initialize_definition| {
                    let method_type = &initialize_definition.type_;
                    let class_names: HashSet<&String> = class_parameter_names.iter().collect();
                    let mut method_type = if method_type
                        .type_parameters
                        .iter()
                        .any(|parameter| class_names.contains(&parameter.name))
                    {
                        let new_names: Vec<String> = method_type
                            .type_parameters
                            .iter()
                            .map(|parameter| {
                                if class_names.contains(&parameter.name) {
                                    fresh_variable_name(&parameter.name)
                                } else {
                                    parameter.name.clone()
                                }
                            })
                            .collect();
                        let old_names: Vec<String> = method_type
                            .type_parameters
                            .iter()
                            .map(|parameter| parameter.name.clone())
                            .collect();
                        let substitution = Substitution::build(&old_names, &variables(&new_names));
                        let renamed_parameters: Vec<TypeParameter> = method_type
                            .type_parameters
                            .iter()
                            .zip(&new_names)
                            .map(|(parameter, new_name)| TypeParameter {
                                name: new_name.clone(),
                                ..parameter.substitute(&substitution)
                            })
                            .collect();
                        let without_parameters = MethodType {
                            type_parameters: Vec::new(),
                            ..(**method_type).clone()
                        };
                        let mut renamed = without_parameters.substitute(&substitution);
                        renamed.type_parameters = class_parameters
                            .iter()
                            .cloned()
                            .chain(renamed_parameters)
                            .collect();
                        renamed
                    } else {
                        MethodType {
                            type_parameters: class_parameters
                                .iter()
                                .cloned()
                                .chain(method_type.type_parameters.iter().cloned())
                                .collect(),
                            ..(**method_type).clone()
                        }
                    };
                    method_type = map_top_level_types(&method_type, &mut |type_| match type_ {
                        Type::SelfType => instance_type.clone(),
                        type_ => type_.clone(),
                    });
                    method_type.type_ = with_return_type(&method_type.type_, instance_type.clone());
                    TypeDefinition {
                        type_: Arc::new(method_type),
                        member: initialize_definition.member.clone(),
                        defined_in: initialize_definition.defined_in.clone(),
                        implemented_in: initialize_definition.implemented_in.clone(),
                        member_annotations: Vec::new(),
                        overload_annotations: initialize_definition.overload_annotations.clone(),
                    }
                })
                .collect();
            let typed_new = Arc::new(Method {
                super_method: Some(new_method),
                defs,
                accessibility: Visibility::Public,
                annotations: Vec::new(),
                alias_of: None,
                alias_member: None,
            });
            definition
                .methods
                .insert("new".to_string(), typed_new.clone());
            for alias_name in alias_names {
                let method = &definition.methods[&alias_name];
                let updated = Method {
                    alias_of: Some(typed_new.clone()),
                    defs: typed_new.defs.clone(),
                    ..(**method).clone()
                };
                definition.methods.insert(alias_name, Arc::new(updated));
            }
            Ok(definition)
        })
    }

    fn import_methods(
        &self,
        definition: &mut Definition,
        module_name: &TypeName,
        module_methods: &Methods,
        interface_methods: &[InterfaceMethods],
        substitution: &Substitution,
        self_type_methods: Option<&IndexMap<String, Arc<Method>>>,
    ) -> Result<(), RbsError> {
        let mut new_methods: IndexMap<String, Arc<Method>> = IndexMap::default();
        let mut interface_method_names: HashSet<String> = HashSet::default();

        for InterfaceMethods { interface, methods } in interface_methods {
            let interface_substitution = match interface {
                Ancestor::Instance { name, args, .. } if !args.is_empty() => {
                    self.validate_types_presence(args)?;
                    let parameters = &self.environment().interface_declarations[name]
                        .declaration
                        .type_parameters;
                    match application(parameters, args) {
                        Some(application) => substitution.compose(&application),
                        None => substitution.clone(),
                    }
                }
                _ => substitution.clone(),
            };
            for method in methods.sorted(interface.name())? {
                if !interface_method_names.insert(method.name.clone()) {
                    let location = match interface {
                        Ancestor::Instance { source, .. } => source.location().cloned(),
                        Ancestor::Singleton { .. } => None,
                    };
                    return Err(RbsError::DuplicatedInterfaceMethodDefinition {
                        type_name: definition.type_name.clone(),
                        singleton: definition.kind == DefinitionKind::Singleton,
                        method_name: method.name.clone(),
                        location,
                    });
                }
                self.define_method(
                    &mut new_methods,
                    definition,
                    method,
                    &interface_substitution,
                    None,
                    interface.name(),
                    Some(module_name),
                )?;
            }
        }

        let implemented_in = (module_name.kind() != TypeNameKind::Interface).then_some(module_name);
        for method in module_methods.sorted(module_name)? {
            self.define_method(
                &mut new_methods,
                definition,
                method,
                substitution,
                self_type_methods,
                module_name,
                implemented_in,
            )?;
        }

        definition.methods.extend(new_methods);
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn define_method(
        &self,
        methods: &mut IndexMap<String, Arc<Method>>,
        definition: &Definition,
        method: &MethodEntry,
        substitution: &Substitution,
        self_type_methods: Option<&IndexMap<String, Arc<Method>>>,
        defined_in: &TypeName,
        implemented_in: Option<&TypeName>,
    ) -> Result<(), RbsError> {
        let existing_method = methods
            .get(&method.name)
            .or_else(|| definition.methods.get(&method.name))
            .cloned();
        let substitute = |method_type: &Arc<MethodType>| {
            if substitution.is_empty() {
                method_type.clone()
            } else {
                Arc::new(method_type.substitute(substitution))
            }
        };
        let singleton = definition.kind == DefinitionKind::Singleton;
        let duplicated = |locations: Vec<Option<Location>>| RbsError::DuplicatedMethodDefinition {
            type_name: definition.type_name.clone(),
            singleton,
            method_name: method.name.clone(),
            locations,
        };

        let mut method_definition = match method.original() {
            Some(Original::Alias(alias)) => {
                let original_method = methods
                    .get(&alias.old_name)
                    .or_else(|| definition.methods.get(&alias.old_name))
                    .or_else(|| self_type_methods.and_then(|methods| methods.get(&alias.old_name)))
                    .cloned()
                    .ok_or_else(|| RbsError::UnknownMethodAlias {
                        type_name: definition.type_name.clone(),
                        original_name: alias.old_name.as_str().into(),
                        aliased_name: alias.new_name.as_str().into(),
                        location: alias.location.clone(),
                    })?;
                let accessibility =
                    special_accessibility(alias.kind == MemberKind::Instance, &alias.new_name)
                        .unwrap_or(original_method.accessibility);
                Method {
                    super_method: existing_method,
                    defs: original_method
                        .defs
                        .iter()
                        .map(|type_definition| TypeDefinition {
                            defined_in: defined_in.clone(),
                            implemented_in: implemented_in.cloned(),
                            ..type_definition.clone()
                        })
                        .collect(),
                    accessibility,
                    annotations: alias.annotations.clone(),
                    alias_of: Some(original_method),
                    alias_member: Some(alias.clone()),
                }
            }
            Some(Original::MethodDefinition(original))
                if original.unannotated_ruby_definition && existing_method.is_some() =>
            {
                if let Some(duplicated_method) = methods.get(&method.name) {
                    let mut locations = vec![original.location.clone()];
                    locations.extend(duplicated_method.member_locations());
                    return Err(duplicated(locations));
                }
                inherited_ruby_method(existing_method.expect("checked above"), implemented_in)
            }
            Some(Original::MethodDefinition(original)) => {
                if let Some(duplicated_method) = methods.get(&method.name) {
                    let mut locations = vec![original.location.clone()];
                    locations.extend(duplicated_method.member_locations());
                    return Err(duplicated(locations));
                }
                let defs = original
                    .overloads
                    .iter()
                    .map(|overload| TypeDefinition {
                        type_: substitute(&overload.method_type),
                        member: DefinitionMember::MethodDefinition(original.clone()),
                        defined_in: defined_in.clone(),
                        implemented_in: implemented_in.cloned(),
                        member_annotations: Vec::new(),
                        overload_annotations: overload.annotations.clone(),
                    })
                    .collect();
                let is_instance = original.kind != MethodKind::Singleton;
                let accessibility = original
                    .visibility
                    .or_else(|| special_accessibility(is_instance, &method.name))
                    .unwrap_or_else(|| method.accessibility());
                Method {
                    super_method: implemented_in.and(existing_method),
                    defs,
                    accessibility,
                    annotations: original.annotations.clone(),
                    alias_of: None,
                    alias_member: None,
                }
            }
            Some(Original::Attribute(original)) => {
                if let Some(duplicated_method) = methods.get(&method.name) {
                    let mut locations = duplicated_method.member_locations();
                    locations.push(original.location.clone());
                    return Err(duplicated(locations));
                }
                let attribute_type = original.type_.substitute(substitution);
                let accessibility = original
                    .visibility
                    .or_else(|| {
                        special_accessibility(original.kind == MemberKind::Instance, &method.name)
                    })
                    .unwrap_or_else(|| method.accessibility());
                Method {
                    super_method: implemented_in.and(existing_method),
                    defs: vec![TypeDefinition {
                        type_: Arc::new(attribute_method_type(
                            original,
                            attribute_type,
                            method.name.ends_with('='),
                        )),
                        member: DefinitionMember::Attribute(original.clone()),
                        defined_in: defined_in.clone(),
                        implemented_in: implemented_in.cloned(),
                        member_annotations: Vec::new(),
                        overload_annotations: Vec::new(),
                    }],
                    accessibility,
                    annotations: original.annotations.clone(),
                    alias_of: None,
                    alias_member: None,
                }
            }
            None => {
                let super_method = if let Some(method) = methods.get(&method.name) {
                    method.super_method.clone()
                } else if definition.methods.contains_key(&method.name) {
                    existing_method.clone()
                } else {
                    return Err(RbsError::InvalidOverloadMethod {
                        type_name: definition.type_name.clone(),
                        method_name: method.name.clone(),
                        singleton: false,
                        location: method
                            .overloads
                            .first()
                            .and_then(|overload| overload.location.clone()),
                    });
                };
                let existing_method = existing_method.expect("an overloaded method exists");
                Method {
                    super_method,
                    defs: existing_method
                        .defs
                        .iter()
                        .map(|type_definition| TypeDefinition {
                            implemented_in: implemented_in.cloned(),
                            ..type_definition.clone()
                        })
                        .collect(),
                    accessibility: existing_method.accessibility,
                    annotations: existing_method.annotations.clone(),
                    alias_of: existing_method.alias_of.clone(),
                    alias_member: None,
                }
            }
        };

        for overloading in &method.overloads {
            for overload in overloading.overloads.iter().rev() {
                method_definition.defs.insert(
                    0,
                    TypeDefinition {
                        type_: substitute(&overload.method_type),
                        member: DefinitionMember::MethodDefinition(overloading.clone()),
                        defined_in: defined_in.clone(),
                        implemented_in: implemented_in.cloned(),
                        member_annotations: Vec::new(),
                        overload_annotations: overload.annotations.clone(),
                    },
                );
            }
            method_definition
                .annotations
                .extend(overloading.annotations.iter().cloned());
        }
        for type_definition in &mut method_definition.defs {
            type_definition.member_annotations = method_definition.annotations.clone();
        }

        methods.insert(method.name.clone(), Arc::new(method_definition));
        Ok(())
    }
}

fn attribute_instance_variable_name(attribute: &Attribute) -> Option<String> {
    match &attribute.instance_variable {
        AttributeInstanceVariable::Empty => None,
        AttributeInstanceVariable::Name(name) => Some(name.clone()),
        AttributeInstanceVariable::Unspecified => Some(format!("@{}", attribute.name)),
    }
}

fn validate_variable(variable: &DefinitionVariable) -> Result<(), RbsError> {
    if variable.parent_variable.is_none() {
        return Ok(());
    }
    let mut variables: Vec<&DefinitionVariable> = Vec::new();
    let mut current = Some(variable);
    while let Some(variable) = current {
        if variable.source.kind != VariableSourceKind::Attribute {
            variables.push(variable);
        }
        current = variable.parent_variable.as_deref();
    }
    let [left, right] = variables.as_slice() else {
        return Ok(());
    };
    if left.declared_in != right.declared_in || left.source.kind != right.source.kind {
        return Ok(());
    }
    match left.source.kind {
        VariableSourceKind::InstanceVariable => Err(RbsError::InstanceVariableDuplication {
            type_name: left.declared_in.clone(),
            variable_name: left.source.name.clone(),
            location: left.source.location.clone(),
        }),
        VariableSourceKind::ClassInstanceVariable => {
            Err(RbsError::ClassInstanceVariableDuplication {
                type_name: left.declared_in.clone(),
                variable_name: left.source.name.clone(),
                location: left.source.location.clone(),
            })
        }
        VariableSourceKind::Attribute | VariableSourceKind::ClassVariable => Ok(()),
    }
}

fn empty_function(return_type: Type) -> Function {
    Function {
        required_positionals: Vec::new(),
        optional_positionals: Vec::new(),
        rest_positionals: None,
        trailing_positionals: Vec::new(),
        required_keywords: Vec::new(),
        optional_keywords: Vec::new(),
        rest_keywords: None,
        return_type,
    }
}

fn with_return_type(callable: &Callable, return_type: Type) -> Callable {
    match callable {
        Callable::Function(function) => Callable::Function(Box::new(Function {
            return_type,
            ..(**function).clone()
        })),
        Callable::Untyped { .. } => Callable::Untyped { return_type },
    }
}

fn map_callable_top_level(callable: &Callable, map: &mut dyn FnMut(&Type) -> Type) -> Callable {
    match callable {
        Callable::Function(function) => {
            let mut parameters = |parameters: &[Parameter]| -> Vec<Parameter> {
                parameters
                    .iter()
                    .map(|parameter| Parameter {
                        type_: map(&parameter.type_),
                        name: parameter.name.clone(),
                    })
                    .collect()
            };
            let required_positionals = parameters(&function.required_positionals);
            let optional_positionals = parameters(&function.optional_positionals);
            let trailing_positionals = parameters(&function.trailing_positionals);
            let mut keywords = |keywords: &[(String, Parameter)]| -> Vec<(String, Parameter)> {
                keywords
                    .iter()
                    .map(|(name, parameter)| {
                        (
                            name.clone(),
                            Parameter {
                                type_: map(&parameter.type_),
                                name: parameter.name.clone(),
                            },
                        )
                    })
                    .collect()
            };
            let required_keywords = keywords(&function.required_keywords);
            let optional_keywords = keywords(&function.optional_keywords);
            let mut single = |parameter: &Option<Parameter>| {
                parameter.as_ref().map(|parameter| Parameter {
                    type_: map(&parameter.type_),
                    name: parameter.name.clone(),
                })
            };
            let rest_positionals = single(&function.rest_positionals);
            let rest_keywords = single(&function.rest_keywords);
            Callable::Function(Box::new(Function {
                required_positionals,
                optional_positionals,
                rest_positionals,
                trailing_positionals,
                required_keywords,
                optional_keywords,
                rest_keywords,
                return_type: map(&function.return_type),
            }))
        }
        Callable::Untyped { return_type } => Callable::Untyped {
            return_type: map(return_type),
        },
    }
}

fn map_top_level_types(method_type: &MethodType, map: &mut dyn FnMut(&Type) -> Type) -> MethodType {
    MethodType {
        type_parameters: method_type.type_parameters.clone(),
        type_: map_callable_top_level(&method_type.type_, map),
        block: method_type.block.as_ref().map(|block| crate::ast::Block {
            type_: map_callable_top_level(&block.type_, map),
            required: block.required,
            self_type: block.self_type.as_ref().map(&mut *map),
        }),
        location: method_type.location.clone(),
    }
}
