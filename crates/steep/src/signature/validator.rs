use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex, OnceLock},
};

use rbs_definition::{
    ancestor_builder::{Ancestor, AncestorSource, check_type_application},
    ast::{self, Declaration, Member, TypeName, TypeNameKind, TypeParameter, VariableKind},
    collections::{HashMap, IndexSet},
    definition_builder::{Definition, DefinitionBuilder},
    environment::{
        ClassOrModuleDeclaration, Entry, Environment, NormalizedModuleName, TypeNameResolver,
        declaration_location,
    },
    errors::RbsError,
    location::Location,
    type_alias_dependency::TypeAliasDependency,
    type_alias_regularity::TypeAliasRegularity,
    variance_calculator::VarianceCalculator,
};
use steep_interface::{
    builder::Builder,
    substitution::Substitution,
    subtyping::{
        check::Check, constraints::Constraints, relation::Relation, result::SubtypingResult,
    },
    types::Type,
};

use crate::{
    annotations_helper,
    ruby_exception::{self, RubyException},
    signature::diagnostic::{self as diagnostics, Diagnostic, from_rbs_error},
};

#[derive(Clone, Default)]
struct ValidationContext {
    self_type: Option<Type>,
    class_type: Option<Type>,
    instance_type: Option<Type>,
}

#[derive(Default)]
pub struct ValidatorCache {
    type_name_resolver: RefCell<Option<Rc<TypeNameResolver>>>,
    type_alias_dependency: RefCell<Option<Rc<TypeAliasDependency>>>,
    type_alias_regularity: RefCell<Option<Rc<TypeAliasRegularity>>>,
}

pub struct Validator<'builder> {
    builder: &'builder Builder,
    check: Check<'builder>,
    errors: RefCell<Vec<Diagnostic>>,
    contexts: RefCell<Vec<ValidationContext>>,
    cache: Rc<ValidatorCache>,
}

fn upper_bounds(builder: &Builder, parameters: &[TypeParameter]) -> Vec<(String, Option<Type>)> {
    parameters
        .iter()
        .map(|parameter| {
            (
                parameter.name.clone(),
                parameter
                    .upper_bound
                    .as_ref()
                    .map(|type_| builder.factory.type_(type_)),
            )
        })
        .collect()
}

fn ancestor_to_type(ancestor: &Ancestor, builder: &Builder) -> Option<Type> {
    let Ancestor::Instance { name, args, .. } = ancestor else {
        return None;
    };
    let args = args.iter().map(|arg| builder.factory.type_(arg)).collect();
    Some(if name.kind() == TypeNameKind::Interface {
        Type::Interface {
            name: name.clone(),
            args,
        }
    } else {
        Type::NameInstance {
            name: name.clone(),
            args,
        }
    })
}

fn collect_type_names(declaration: &Declaration, names: &mut IndexSet<TypeName>) {
    let name = match declaration {
        Declaration::Class(class) => {
            for member in &class.members {
                if let Member::Declaration(inner) = member {
                    collect_type_names(inner, names);
                }
            }
            &class.name
        }
        Declaration::Module(module) => {
            for member in &module.members {
                if let Member::Declaration(inner) = member {
                    collect_type_names(inner, names);
                }
            }
            &module.name
        }
        Declaration::Interface(interface) => &interface.name,
        Declaration::TypeAlias(alias) => &alias.name,
        Declaration::ClassAlias(alias) => &alias.new_name,
        Declaration::Constant(_) | Declaration::Global(_) => return,
    };
    names.insert(name.clone());
}

type TypeNameValidation = Arc<OnceLock<Result<Vec<Diagnostic>, RubyException>>>;

pub struct SignatureValidation {
    type_names: HashMap<String, IndexSet<TypeName>>,
    constant_names: HashMap<String, Vec<TypeName>>,
    global_names: HashMap<String, Vec<String>>,
    type_name_validations: Mutex<HashMap<TypeName, TypeNameValidation>>,
}

impl SignatureValidation {
    pub fn new(environment: &Environment) -> SignatureValidation {
        let mut type_names: HashMap<String, IndexSet<TypeName>> = HashMap::default();
        for source in &environment.sources {
            for declaration in &source.declarations {
                if let Some(location) = declaration_location(declaration) {
                    collect_type_names(
                        declaration,
                        type_names.entry(location.name().to_string()).or_default(),
                    );
                }
            }
        }
        let mut constant_names: HashMap<String, Vec<TypeName>> = HashMap::default();
        for (name, entry) in &environment.constant_declarations {
            if let Some(location) = &entry.declaration.location {
                constant_names
                    .entry(location.name().to_string())
                    .or_default()
                    .push(name.clone());
            }
        }
        let mut global_names: HashMap<String, Vec<String>> = HashMap::default();
        for (name, declaration) in &environment.global_declarations {
            if let Some(location) = &declaration.location {
                global_names
                    .entry(location.name().to_string())
                    .or_default()
                    .push(name.clone());
            }
        }
        SignatureValidation {
            type_names,
            constant_names,
            global_names,
            type_name_validations: Mutex::default(),
        }
    }

    fn type_name_validation(&self, type_name: &TypeName) -> TypeNameValidation {
        self.type_name_validations
            .lock()
            .expect("no thread panics while holding the validations")
            .entry(type_name.clone())
            .or_default()
            .clone()
    }

    // Why not validate each type name per path as Steep does: a module reopened in
    // N files would be validated N times, each time over all of its N declarations.
    // The validation of a type name does not depend on the path, so its result is
    // shared and filtered by the path afterwards, as Steep filters it.
    pub fn validate_path(
        &self,
        builder: &Builder,
        cache: Rc<ValidatorCache>,
        path: &str,
    ) -> Vec<Diagnostic> {
        let validator = Validator::with_cache(builder, cache.clone());
        let environment = validator.environment();
        for type_name in self.type_names.get(path).into_iter().flatten() {
            let validation = self.type_name_validation(type_name);
            let result = validation.get_or_init(|| {
                ruby_exception::rescue(|| {
                    let validator = Validator::with_cache(builder, cache.clone());
                    match type_name.kind() {
                        TypeNameKind::Class => validator.validate_one_class(type_name),
                        TypeNameKind::Interface => validator.validate_one_interface(type_name),
                        TypeNameKind::Alias => validator.validate_one_alias(type_name),
                    }
                    validator.into_errors()
                })
            });
            match result {
                Ok(diagnostics) => validator
                    .errors
                    .borrow_mut()
                    .extend(diagnostics.iter().cloned()),
                Err(exception) => ruby_exception::raise(exception.clone()),
            }
        }
        for name in self.constant_names.get(path).into_iter().flatten() {
            validator.validate_one_constant(name, &environment.constant_declarations[name]);
        }
        for name in self.global_names.get(path).into_iter().flatten() {
            validator.validate_one_global(&environment.global_declarations[name]);
        }
        validator
            .errors
            .into_inner()
            .into_iter()
            .filter(|diagnostic| diagnostic.path() == Some(path))
            .collect()
    }
}

impl<'builder> Validator<'builder> {
    pub fn new(builder: &'builder Builder) -> Validator<'builder> {
        Validator::with_cache(builder, Rc::default())
    }

    pub fn with_cache(
        builder: &'builder Builder,
        cache: Rc<ValidatorCache>,
    ) -> Validator<'builder> {
        Validator {
            builder,
            check: builder.subtyping(),
            errors: RefCell::default(),
            contexts: RefCell::default(),
            cache,
        }
    }

    fn environment(&self) -> &'builder Environment {
        self.builder.definition_builder.environment()
    }

    fn definition_builder(&self) -> &DefinitionBuilder {
        &self.builder.definition_builder
    }

    pub fn has_error(&self) -> bool {
        !self.errors.borrow().is_empty()
    }

    pub fn into_errors(self) -> Vec<Diagnostic> {
        self.errors.into_inner()
    }

    fn push(&self, diagnostic: Diagnostic) {
        self.errors.borrow_mut().push(diagnostic);
    }

    pub fn rescue_validation_errors(&self, validation: impl FnOnce(&Self) -> Result<(), RbsError>) {
        if let Err(error) = validation(self) {
            match from_rbs_error(&error, Some(&self.builder.factory)) {
                Ok(diagnostic) => self.push(diagnostic),
                Err(_) => ruby_exception::raise(error),
            }
        }
    }

    fn with_context<Output>(
        &self,
        context: ValidationContext,
        body: impl FnOnce() -> Output,
    ) -> Output {
        self.contexts.borrow_mut().push(context);
        let result = body();
        self.contexts.borrow_mut().pop();
        result
    }

    fn latest_context(&self) -> ValidationContext {
        self.contexts.borrow().last().cloned().unwrap_or_default()
    }

    fn check(
        &self,
        sub_type: Type,
        super_type: Type,
        context: ValidationContext,
    ) -> Result<Rc<SubtypingResult>, RbsError> {
        self.check.check(
            Relation::new(sub_type, super_type),
            Constraints::empty().shared(),
            context.self_type,
            context.instance_type,
            context.class_type,
        )
    }

    fn self_instance_class_context() -> ValidationContext {
        ValidationContext {
            self_type: Some(Type::SelfType),
            class_type: Some(Type::Class),
            instance_type: Some(Type::Instance),
        }
    }

    fn with_bounds<Output>(
        &self,
        bounds: Vec<(String, Option<Type>)>,
        body: impl FnOnce() -> Output,
    ) -> Output {
        self.check.push_variable_bounds(bounds, body)
    }

    fn absolute_type_name(&self, name: &TypeName) -> TypeName {
        let resolver = self
            .cache
            .type_name_resolver
            .borrow_mut()
            .get_or_insert_with(|| Rc::new(TypeNameResolver::new(self.environment())))
            .clone();
        resolver.resolve(name, &[]).unwrap_or_else(|| name.clone())
    }

    fn declared_type_parameters(&self, type_: &ast::Type) -> Option<&'builder [TypeParameter]> {
        let environment = self.environment();
        let name = match type_ {
            ast::Type::ClassInstance { name, .. }
            | ast::Type::Interface { name, .. }
            | ast::Type::Alias { name, .. } => name,
            _ => return None,
        };
        let normalized = environment.normalize_type_name(name)?;
        match normalized.kind() {
            TypeNameKind::Class => environment
                .class_declarations
                .get(&normalized)
                .map(|entry| entry.type_parameters()),
            TypeNameKind::Interface => environment
                .interface_declarations
                .get(&normalized)
                .map(|entry| entry.declaration.type_parameters.as_slice()),
            TypeNameKind::Alias => environment
                .type_alias_declarations
                .get(&normalized)
                .map(|entry| entry.declaration.type_parameters.as_slice()),
        }
    }

    fn validate_type_with_rbs_validator(&self, type_: &ast::Type) -> Result<(), RbsError> {
        match type_ {
            ast::Type::ClassInstance { name, args, .. }
            | ast::Type::Interface { name, args, .. }
            | ast::Type::Alias { name, args, .. } => {
                let name = self.absolute_type_name(name);
                self.definition_builder()
                    .validate_type_name(&name, type_.location())?;
                if self.environment().normalize_type_name(&name).is_some() {
                    let parameters = self
                        .declared_type_parameters(type_)
                        .ok_or_else(|| RbsError::RuntimeError(format!("Unknown type: {name}")))?;
                    check_type_application(&name, args, parameters, type_.location())?;
                }
            }
            ast::Type::ClassSingleton { name, .. } => {
                let name = self.absolute_type_name(name);
                self.definition_builder()
                    .validate_type_name(&name, type_.location())?;
            }
            _ => {}
        }
        for child in type_.each_type() {
            self.validate_type_with_rbs_validator(child)?;
        }
        Ok(())
    }

    pub fn validate_type(&self, type_: &ast::Type) -> Result<(), RbsError> {
        self.validate_type_with_rbs_validator(type_)?;
        self.validate_type_0(type_)
    }

    fn validate_type_0(&self, type_: &ast::Type) -> Result<(), RbsError> {
        self.validate_type_application(type_)?;
        if let ast::Type::ClassInstance { name, location, .. }
        | ast::Type::Interface { name, location, .. }
        | ast::Type::ClassSingleton { name, location, .. }
        | ast::Type::Alias { name, location, .. } = type_
            && let Some(name_location) = location.name()
        {
            self.validate_type_name_deprecation(name, Some(name_location.clone()));
        }
        for child in type_.each_type() {
            self.validate_type_0(child)?;
        }
        Ok(())
    }

    fn validate_type_application(&self, type_: &ast::Type) -> Result<(), RbsError> {
        let (name, parameters, args): (TypeName, Vec<TypeParameter>, &[ast::Type]) = match type_ {
            ast::Type::ClassInstance { name, args, .. } => (
                name.clone(),
                self.definition_builder()
                    .build_instance(name)?
                    .type_parameters
                    .clone(),
                args,
            ),
            ast::Type::Interface { name, args, .. } => (
                name.clone(),
                self.definition_builder()
                    .build_interface(name)?
                    .type_parameters
                    .clone(),
                args,
            ),
            ast::Type::Alias { name, args, .. } => {
                let Some(normalized) = self.environment().normalize_type_name(name) else {
                    return Ok(());
                };
                let entry = &self.environment().type_alias_declarations[&normalized];
                (normalized, entry.declaration.type_parameters.clone(), args)
            }
            _ => return Ok(()),
        };
        if !parameters.is_empty() && !args.is_empty() {
            self.validate_type_application_constraints(
                &name,
                &parameters,
                args,
                type_.location().cloned(),
            )?;
        }
        Ok(())
    }

    fn validate_type_application_constraints(
        &self,
        type_name: &TypeName,
        parameters: &[TypeParameter],
        args: &[ast::Type],
        location: Option<Location>,
    ) -> Result<(), RbsError> {
        if parameters.len() != args.len() {
            return Ok(());
        }
        let factory = &self.builder.factory;
        let names: Vec<String> = parameters
            .iter()
            .map(|parameter| parameter.name.clone())
            .collect();
        let types: Vec<Type> = args.iter().map(|arg| factory.type_(arg)).collect();
        let substitution = Substitution::build(&names, &types, None, None, None);
        for (parameter, arg) in parameters.iter().zip(args) {
            let Some(upper_bound) = &parameter.upper_bound else {
                continue;
            };
            let upper_bound = factory.type_(upper_bound).substitute(&substitution);
            let arg_type = factory.type_(arg);
            let result =
                self.check(arg_type.clone(), upper_bound.clone(), self.latest_context())?;
            if result.is_failure() {
                self.push(diagnostics::unsatisfiable_type_application(
                    type_name,
                    &arg_type,
                    &upper_bound,
                    &result,
                    location.clone(),
                ));
            }
        }
        Ok(())
    }

    fn validate_type_name_deprecation(&self, type_name: &TypeName, location: Option<Location>) {
        if let Some(message) =
            annotations_helper::deprecated_type_name(type_name, self.environment())
        {
            self.push(diagnostics::deprecated_type_name(
                type_name,
                message.as_deref(),
                location,
            ));
        }
    }

    fn mixin_constraints(
        &self,
        definition: &Definition,
        mixin_ancestors: &[Ancestor],
        immediate_self_types: Option<&Vec<Ancestor>>,
    ) -> Result<Vec<(Type, Type, Ancestor)>, RbsError> {
        let mut self_type = self.builder.factory.type_(&definition.self_type);
        if let Some(self_types) = immediate_self_types
            && !self_types.is_empty()
        {
            let mut types: Vec<Type> = self_types
                .iter()
                .filter_map(|ancestor| ancestor_to_type(ancestor, self.builder))
                .collect();
            types.push(self_type);
            self_type = Type::intersection(types);
        }
        let mut relations = Vec::new();
        for ancestor in mixin_ancestors {
            let Ancestor::Instance { name, args, .. } = ancestor else {
                continue;
            };
            let args: Vec<Type> = args
                .iter()
                .map(|arg| self.builder.factory.type_(arg))
                .collect();
            let ancestor_ancestors = self
                .definition_builder()
                .ancestor_builder
                .one_instance_ancestors(name)?;
            for self_ancestor in ancestor_ancestors.self_types.iter().flatten() {
                let substitution =
                    Substitution::build(&ancestor_ancestors.params, &args, None, None, None);
                if let Some(constraint) = ancestor_to_type(self_ancestor, self.builder) {
                    relations.push((
                        self_type.clone(),
                        constraint.substitute(&substitution),
                        ancestor.clone(),
                    ));
                }
            }
        }
        Ok(relations)
    }

    fn validate_definition_type(&self, definition: &Definition) -> Result<(), RbsError> {
        for method in definition.methods.values() {
            if *method.defined_in() != definition.type_name {
                continue;
            }
            for method_type in method.method_types() {
                let bounds = upper_bounds(self.builder, &method_type.type_parameters);
                self.with_bounds(bounds, || {
                    for type_ in method_type.each_type() {
                        self.validate_type(type_)?;
                    }
                    Ok::<(), RbsError>(())
                })?;
            }
        }
        for variable in definition
            .instance_variables
            .values()
            .chain(definition.class_variables.values())
        {
            if variable.declared_in == definition.type_name {
                self.validate_type(&variable.type_)?;
            }
        }
        Ok(())
    }

    fn validate_type_params(&self, parameters: &[TypeParameter]) -> Result<(), RbsError> {
        let optional_names: Vec<&String> = parameters
            .iter()
            .filter(|parameter| parameter.default_type.is_some())
            .map(|parameter| &parameter.name)
            .collect();
        for parameter in parameters {
            if let Some(default_type) = &parameter.default_type
                && default_type
                    .free_variables()
                    .iter()
                    .any(|variable| optional_names.contains(&variable))
            {
                self.push(diagnostics::type_param_default_reference(
                    &parameter.name,
                    parameter.default_type_location.location(),
                ));
            }
        }
        let bounds = upper_bounds(self.builder, parameters);
        self.with_bounds(bounds, || {
            for parameter in parameters {
                let param = self.builder.factory.type_param(parameter);
                if let Some(upper_bound) = &parameter.upper_bound {
                    self.validate_type(upper_bound)?;
                }
                if let Some(default_type) = &parameter.default_type {
                    self.validate_type(default_type)?;
                }
                let (Some(default_type), Some(upper_bound)) =
                    (param.default_type, param.upper_bound)
                else {
                    continue;
                };
                let result = self.check(default_type, upper_bound, ValidationContext::default())?;
                if result.is_failure() {
                    self.push(diagnostics::unsatisfiable_generics_default_type(
                        &parameter.name,
                        &result,
                        parameter.default_type_location.location(),
                    ));
                }
            }
            Ok(())
        })
    }

    fn check_instance_variables(
        &self,
        definition: &Definition,
        context: ValidationContext,
    ) -> Result<(), RbsError> {
        for variable in definition.instance_variables.values() {
            let Some(parent) = &variable.parent_variable else {
                continue;
            };
            let variable_type = self.builder.factory.type_(&variable.type_);
            let parent_type = self.builder.factory.type_(&parent.type_);
            let forward =
                self.check(variable_type.clone(), parent_type.clone(), context.clone())?;
            let backward =
                self.check(parent_type.clone(), variable_type.clone(), context.clone())?;
            if !(forward.is_success() && backward.is_success()) {
                self.push(diagnostics::instance_variable_type_error(
                    &variable_type,
                    &parent_type,
                    variable.type_location().cloned(),
                ));
            }
        }
        Ok(())
    }

    fn check_mixin_self_types(
        &self,
        name: &TypeName,
        definition: &Definition,
        mixins: &[Ancestor],
        self_types: Option<&Vec<Ancestor>>,
    ) -> Result<(), RbsError> {
        for (sub_type, super_type, ancestor) in
            self.mixin_constraints(definition, mixins, self_types)?
        {
            let result = self.check(sub_type, super_type, Self::self_instance_class_context())?;
            if result.is_failure() {
                let location = match &ancestor {
                    Ancestor::Instance { source, .. } => source.location().cloned(),
                    Ancestor::Singleton { .. } => None,
                };
                self.push(diagnostics::module_self_type_error(name, &result, location));
            }
        }
        Ok(())
    }

    fn validate_ancestor_application(
        &self,
        name: &TypeName,
        ancestor: &Ancestor,
    ) -> Result<(), RbsError> {
        let Ancestor::Instance {
            name: ancestor_name,
            args,
            source,
        } = ancestor
        else {
            return Ok(());
        };
        if args.is_empty() {
            return Ok(());
        }
        let definition = if ancestor_name.kind() == TypeNameKind::Interface {
            self.definition_builder().build_interface(ancestor_name)?
        } else {
            self.definition_builder().build_instance(ancestor_name)?
        };
        let location = match source {
            AncestorSource::Super => {
                let entry = &self.environment().class_declarations[name];
                let primary = entry.primary_declaration();
                match primary.super_class() {
                    Some(super_class) => super_class.location.clone(),
                    None => primary.name_location().cloned(),
                }
            }
            _ => source.location().cloned(),
        };
        self.validate_type_application_constraints(
            ancestor_name,
            &definition.type_parameters,
            args,
            location,
        )?;
        for arg in args {
            self.validate_type(arg)?;
        }
        Ok(())
    }

    fn validate_one_class_declaration(&self, name: &TypeName) -> Result<(), RbsError> {
        let environment = self.environment();
        let entry = &environment.class_declarations[name];
        let class_type = Type::Singleton(name.clone());
        let instance_type = Type::NameInstance {
            name: name.clone(),
            args: entry.type_parameters().iter().map(|_| Type::Any).collect(),
        };
        for declaration in entry.declarations() {
            if annotations_helper::deprecated_annotation(declaration.annotations()).is_none() {
                self.validate_type_name_deprecation(name, declaration.name_location().cloned());
            }
        }

        let definition = self.definition_builder().build_instance(name)?;
        let self_type = Type::NameInstance {
            name: name.clone(),
            args: entry
                .type_parameters()
                .iter()
                .map(|parameter| Type::Var(parameter.name.clone()))
                .collect(),
        };
        let context = ValidationContext {
            self_type: Some(self_type),
            class_type: Some(class_type.clone()),
            instance_type: Some(instance_type.clone()),
        };
        self.with_context(context, || {
            self.with_bounds(
                upper_bounds(self.builder, &definition.type_parameters),
                || {
                    self.check_instance_variables(&definition, ValidationContext::default())?;
                    let ancestors = self
                        .definition_builder()
                        .ancestor_builder
                        .one_instance_ancestors(name)?;
                    self.check_mixin_self_types(
                        name,
                        &definition,
                        ancestors.included_modules.as_deref().unwrap_or_default(),
                        ancestors.self_types.as_ref(),
                    )?;
                    for ancestor in ancestors.each_ancestor() {
                        let Ancestor::Instance {
                            name: ancestor_name,
                            source,
                            ..
                        } = ancestor
                        else {
                            continue;
                        };
                        self.validate_ancestor_application(name, ancestor)?;
                        let location = match source {
                            AncestorSource::Super => match entry.primary_declaration() {
                                ClassOrModuleDeclaration::Class(class) => class
                                    .super_class
                                    .as_ref()
                                    .and_then(|super_class| super_class.location.clone()),
                                ClassOrModuleDeclaration::Module(_) => None,
                            },
                            AncestorSource::Nothing => None,
                            AncestorSource::Member(location) => location.clone(),
                        };
                        if location.is_some() {
                            self.validate_type_name_deprecation(ancestor_name, location);
                        }
                    }
                    self.validate_definition_type(&definition)
                },
            )
        })?;

        let definition = self.definition_builder().build_singleton(name)?;
        let context = ValidationContext {
            self_type: Some(class_type.clone()),
            class_type: Some(class_type),
            instance_type: Some(instance_type),
        };
        self.with_context(context, || {
            self.check_instance_variables(&definition, Self::self_instance_class_context())?;
            for (variable_name, variable) in &definition.class_variables {
                if variable.declared_in != definition.type_name {
                    continue;
                }
                let Some(parent) = &variable.parent_variable else {
                    continue;
                };
                if variable.declared_in == parent.declared_in {
                    continue;
                }
                let member = entry
                    .declarations()
                    .flat_map(|declaration| declaration.members())
                    .find_map(|member| match member {
                        Member::Variable(member)
                            if member.variable_kind == VariableKind::ClassVariable
                                && member.name == *variable_name =>
                        {
                            Some(member)
                        }
                        _ => None,
                    });
                if let Some(member) = member {
                    self.push(diagnostics::class_variable_duplication_error(
                        &definition.type_name,
                        &parent.declared_in,
                        variable_name,
                        member.name_location.clone(),
                    ));
                }
            }
            let ancestors = self
                .definition_builder()
                .ancestor_builder
                .one_singleton_ancestors(name)?;
            self.check_mixin_self_types(
                name,
                &definition,
                ancestors.extended_modules.as_deref().unwrap_or_default(),
                ancestors.self_types.as_ref(),
            )?;
            for ancestor in ancestors.each_ancestor() {
                self.validate_ancestor_application(name, ancestor)?;
            }
            self.validate_definition_type(&definition)
        })?;

        self.validate_type_params(entry.type_parameters())
    }

    pub fn validate_one_class(&self, name: &TypeName) {
        let environment = self.environment();
        if environment.class_declarations.contains_key(name) {
            self.rescue_validation_errors(|validator| {
                validator.validate_one_class_declaration(name)
            });
        } else if let Some(entry) = environment.class_alias_declarations.get(name) {
            self.rescue_validation_errors(|validator| validator.validate_one_class_alias(entry));
        }
    }

    fn validate_one_class_alias(
        &self,
        entry: &Entry<ast::AliasDeclaration>,
    ) -> Result<(), RbsError> {
        let declaration = &entry.declaration;
        match self
            .environment()
            .normalize_module_name_checked(&declaration.new_name)
        {
            NormalizedModuleName::Unknown => {
                return Err(RbsError::NoTypeFound {
                    type_name: declaration.old_name.clone(),
                    location: declaration.old_name_location.clone(),
                });
            }
            NormalizedModuleName::Cyclic => {
                return Err(RbsError::CyclicClassAliasDefinition {
                    new_name: declaration.new_name.clone(),
                    location: declaration.new_name_location.clone(),
                });
            }
            NormalizedModuleName::Found(_) => {}
        }
        let consistent = self
            .environment()
            .class_declarations
            .get(&declaration.old_name)
            .is_some_and(|old| old.kind == declaration.kind);
        if !consistent {
            return Err(RbsError::InconsistentClassModuleAlias {
                kind: declaration.kind,
                new_name: declaration.new_name.clone(),
                old_name: Box::new(declaration.old_name.clone()),
                location: declaration.old_name_location.clone(),
            });
        }
        if declaration.location.is_some() {
            self.validate_type_name_deprecation(
                &declaration.old_name,
                declaration.old_name_location.clone(),
            );
        }
        Ok(())
    }

    fn validate_one_interface_declaration(&self, name: &TypeName) -> Result<(), RbsError> {
        let definition = self.definition_builder().build_interface(name)?;
        self.validate_type_params(&definition.type_parameters)?;
        let self_type = Type::Interface {
            name: name.clone(),
            args: definition
                .type_parameters
                .iter()
                .map(|parameter| Type::Var(parameter.name.clone()))
                .collect(),
        };
        let context = ValidationContext {
            self_type: Some(self_type),
            class_type: None,
            instance_type: None,
        };
        self.with_context(context, || {
            self.with_bounds(
                upper_bounds(self.builder, &definition.type_parameters),
                || {
                    self.validate_definition_type(&definition)?;
                    let ancestors = self
                        .definition_builder()
                        .ancestor_builder
                        .one_interface_ancestors(name)?;
                    for ancestor in ancestors.each_ancestor() {
                        let Ancestor::Instance {
                            name: ancestor_name,
                            args,
                            source,
                        } = ancestor
                        else {
                            continue;
                        };
                        let ancestor_definition =
                            self.definition_builder().build_interface(ancestor_name)?;
                        self.validate_type_application_constraints(
                            ancestor_name,
                            &ancestor_definition.type_parameters,
                            args,
                            source.location().cloned(),
                        )?;
                    }
                    Ok(())
                },
            )
        })
    }

    pub fn validate_one_interface(&self, name: &TypeName) {
        self.rescue_validation_errors(|validator| {
            validator.validate_one_interface_declaration(name)
        });
    }

    fn type_alias_dependency(&self) -> Rc<TypeAliasDependency> {
        self.cache
            .type_alias_dependency
            .borrow_mut()
            .get_or_insert_with(|| Rc::new(TypeAliasDependency::new(self.environment())))
            .clone()
    }

    fn type_alias_regularity(&self) -> Result<Rc<TypeAliasRegularity>, RbsError> {
        if let Some(regularity) = self.cache.type_alias_regularity.borrow().as_ref() {
            return Ok(regularity.clone());
        }
        let regularity = Rc::new(TypeAliasRegularity::validate(self.environment())?);
        *self.cache.type_alias_regularity.borrow_mut() = Some(regularity.clone());
        Ok(regularity)
    }

    fn validate_type_alias(
        &self,
        entry: &Entry<ast::TypeAliasDeclaration>,
    ) -> Result<(), RbsError> {
        let environment = self.environment();
        let declaration = &entry.declaration;
        let type_name = &entry.name;
        if self
            .type_alias_dependency()
            .is_circular_definition(environment, type_name)
        {
            return Err(RbsError::RecursiveTypeAlias {
                alias_names: vec![type_name.clone()],
                location: declaration.location.clone(),
            });
        }
        if let Some(diagnostic) = self
            .type_alias_regularity()?
            .nonregular(environment, type_name)
        {
            return Err(RbsError::NonregularTypeAlias {
                type_name: diagnostic.type_name.clone(),
                nonregular_type: Box::new(diagnostic.nonregular_type.clone()),
                location: declaration.location.clone(),
            });
        }
        if !declaration.type_parameters.is_empty() {
            let result = VarianceCalculator::new(environment).in_type_alias(type_name)?;
            for parameter_name in result.incompatible(&declaration.type_parameters) {
                let parameter = declaration
                    .type_parameters
                    .iter()
                    .find(|parameter| parameter.name == parameter_name)
                    .expect("an incompatible parameter is declared");
                if parameter.unchecked {
                    continue;
                }
                return Err(RbsError::InvalidVarianceAnnotation {
                    type_name: type_name.clone(),
                    param: Box::new(parameter.clone()),
                    location: declaration.type_location.clone(),
                });
            }
            validate_cyclic_type_parameter_bounds(
                type_name,
                &declaration.type_parameters,
                declaration.type_parameters_location.clone(),
            )?;
        }
        Ok(())
    }

    fn validate_one_alias_declaration(
        &self,
        entry: &Entry<ast::TypeAliasDeclaration>,
    ) -> Result<(), RbsError> {
        let declaration = &entry.declaration;
        if !entry.name.namespace.is_empty() {
            self.definition_builder().validate_type_name(
                &entry.name.namespace.to_type_name(),
                declaration.name_location.as_ref(),
            )?;
        }
        self.validate_type_params(&declaration.type_parameters)?;
        self.validate_type_alias(entry)?;
        self.with_bounds(
            upper_bounds(self.builder, &declaration.type_parameters),
            || self.validate_type(&declaration.type_),
        )
    }

    pub fn validate_one_alias(&self, name: &TypeName) {
        let environment = self.environment();
        let Some(entry) = environment.type_alias_declarations.get(name) else {
            return;
        };
        let mut context = ValidationContext::default();
        if let Some(outer) = entry.context.last()
            && let Some(outer_entry) = environment.module_class_entry(outer)
        {
            context.class_type = Some(Type::Singleton(outer_entry.name.clone()));
            context.instance_type = Some(Type::NameInstance {
                name: outer_entry.name.clone(),
                args: outer_entry
                    .type_parameters()
                    .iter()
                    .map(|_| Type::Any)
                    .collect(),
            });
        }
        self.with_context(context, || {
            self.rescue_validation_errors(|validator| {
                validator.validate_one_alias_declaration(entry)
            })
        });
    }

    pub fn validate_one_constant(&self, name: &TypeName, entry: &Entry<ast::ConstantDeclaration>) {
        self.rescue_validation_errors(|validator| {
            validator
                .definition_builder()
                .ensure_namespace(&name.namespace, entry.declaration.location.as_ref())?;
            validator.validate_type(&entry.declaration.type_)
        });
    }

    pub fn validate_one_global(&self, declaration: &ast::GlobalDeclaration) {
        self.rescue_validation_errors(|validator| validator.validate_type(&declaration.type_));
    }
}

fn validate_cyclic_type_parameter_bounds(
    type_name: &TypeName,
    parameters: &[TypeParameter],
    location: Option<Location>,
) -> Result<(), RbsError> {
    let children = |name: &str| -> Vec<String> {
        parameters
            .iter()
            .find(|parameter| parameter.name == name)
            .map(|parameter| {
                parameter
                    .upper_bound
                    .iter()
                    .chain(parameter.lower_bound.iter())
                    .flat_map(ast::Type::free_variables)
                    .collect()
            })
            .unwrap_or_default()
    };
    for parameter in parameters {
        let mut visited: Vec<String> = Vec::new();
        let mut stack = vec![parameter.name.clone()];
        while let Some(name) = stack.pop() {
            for child in children(&name) {
                if child == parameter.name {
                    let mut cycle: Vec<TypeParameter> = parameters
                        .iter()
                        .filter(|parameter| visited.contains(&parameter.name))
                        .cloned()
                        .collect();
                    cycle.push(parameter.clone());
                    if cycle.len() > 1 {
                        return Err(RbsError::CyclicTypeParameterBound {
                            type_name: type_name.clone(),
                            method_name: None,
                            params: cycle,
                            location,
                        });
                    }
                } else if !visited.contains(&child) {
                    visited.push(child.clone());
                    stack.push(child);
                }
            }
        }
    }
    Ok(())
}
