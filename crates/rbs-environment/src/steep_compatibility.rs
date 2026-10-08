use std::collections::{HashMap, HashSet};

use crate::environment::{Ancestor, ClassKind, Environment, Location, MethodKind};
use crate::signature_validation::{
    DUPLICATED_METHOD_DEFINITION, INVALID_METHOD_OVERLOAD, INVALID_TYPE_APPLICATION,
    SignatureDiagnostic, UNKNOWN_METHOD_ALIAS, UNKNOWN_TYPE_NAME, ValidationMember,
    ValidationOwner, ValidationScope,
};
use crate::types::{MethodType, Type, TypeName};

/// A method defined more than once without `...`. RBS rejects it when it builds the definition of the class
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicatedMethodDefinition {
    pub type_name: TypeName,
    pub kind: MethodKind,
    pub method_name: String,
    /// In load order, so the first is where RBS reports the error
    pub locations: Vec<Location>,
}

/// Why `RBS::DefinitionBuilder` fails to build the definition of a class. Steep reports the failure as
/// `Ruby::UnexpectedError` on the Ruby node that needed the definition and gives up typing that node
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DefinitionError {
    DuplicatedMethodDefinition(DuplicatedMethodDefinition),
    NoTypeFound {
        type_name: TypeName,
        location: Option<Location>,
    },
}

impl DefinitionError {
    pub fn message(&self) -> String {
        match self {
            DefinitionError::DuplicatedMethodDefinition(duplicated) => {
                let separator = match duplicated.kind {
                    MethodKind::Instance => "#",
                    MethodKind::Singleton => ".",
                };
                let mut locations = duplicated.locations.iter().map(location_text);
                let first = locations.next().unwrap_or_default();
                let others: Vec<String> = locations.collect();
                let mut message = format!(
                    "{first}: ::{}{separator}{} has duplicated definitions",
                    duplicated.type_name, duplicated.method_name
                );
                if !others.is_empty() {
                    message.push_str(&format!(" in {}", others.join(", ")));
                }
                format!("{message}(RBS::DuplicatedMethodDefinitionError)")
            }
            DefinitionError::NoTypeFound {
                type_name,
                location,
            } => {
                let location = location.as_ref().map(location_text).unwrap_or_default();
                format!("{location}: Could not find {type_name}(RBS::NoTypeFoundError)")
            }
        }
    }
}

/// `RBS::Location#to_s`: 1-based lines and 0-based columns
fn location_text(location: &Location) -> String {
    format!(
        "{}:{}:{}...{}:{}",
        location.path.display(),
        location.start.line + 1,
        location.start.utf16_column,
        location.end.line + 1,
        location.end.utf16_column
    )
}

#[derive(Debug, Default)]
pub(crate) struct DefinitionErrors {
    instance: HashMap<TypeName, DefinitionError>,
    singleton: HashMap<TypeName, DefinitionError>,
}

/// Codes that `Steep::Signature::Validator#rescue_validation_errors` stops at, so only the first one of each declaration
/// is reported. The others come from loading the environment or building ancestors, before the validator runs
const FIRST_ERROR_ONLY_CODES: &[&str] = &[
    UNKNOWN_TYPE_NAME,
    INVALID_TYPE_APPLICATION,
    INVALID_METHOD_OVERLOAD,
    UNKNOWN_METHOD_ALIAS,
    DUPLICATED_METHOD_DEFINITION,
];

impl Environment {
    /// Why Steep cannot build the instance definition of `name` (`RBS::DefinitionBuilder#build_instance`), needed to
    /// call a method on an instance of it or to type a class body
    pub fn instance_definition_error(&self, name: &str) -> Option<&DefinitionError> {
        let name = self.resolve_class_name(name)?;
        self.definition_errors().instance.get(name)
    }

    /// Why Steep cannot build the singleton definition of `name` (`RBS::DefinitionBuilder#build_singleton`), which
    /// includes building the instance definition
    pub fn singleton_definition_error(&self, name: &str) -> Option<&DefinitionError> {
        let name = self.resolve_class_name(name)?;
        self.definition_errors().singleton.get(name)
    }

    fn definition_errors(&self) -> &DefinitionErrors {
        self.definition_errors
            .get_or_init(|| DefinitionErrorCalculator::new(self).calculate())
    }

    /// `signature_diagnostics` as `steep check` reports them: the validator stops a declaration at its first error,
    /// and RBS reports a duplicated method at its first definition only, which is dropped when it is in a library
    pub fn steep_compatible_signature_diagnostics(&self) -> Vec<&SignatureDiagnostic> {
        let mut kept = vec![true; self.signature_diagnostics.len()];
        let mut first_of_owner: HashMap<&ValidationOwner, (ValidationOrder, usize)> =
            HashMap::new();
        for (index, diagnostic) in self.signature_diagnostics.iter().enumerate() {
            if diagnostic.code == DUPLICATED_METHOD_DEFINITION
                && !self.is_first_duplicated_definition(diagnostic)
            {
                kept[index] = false;
                continue;
            }
            if !FIRST_ERROR_ONLY_CODES.contains(&diagnostic.code) {
                continue;
            }
            let Some(scope) = self.innermost_validation_scope(diagnostic) else {
                continue;
            };
            let order = ValidationOrder {
                phase: validation_phase(scope.member, diagnostic.code),
                position: (
                    diagnostic.path.clone(),
                    diagnostic.start.line,
                    diagnostic.start.byte_column,
                ),
            };
            match first_of_owner.get(&scope.owner) {
                Some((first_order, _)) if *first_order <= order => kept[index] = false,
                Some((_, first_index)) => {
                    kept[*first_index] = false;
                    first_of_owner.insert(&scope.owner, (order, index));
                }
                None => {
                    first_of_owner.insert(&scope.owner, (order, index));
                }
            }
        }
        self.signature_diagnostics
            .iter()
            .zip(kept)
            .filter_map(|(diagnostic, kept)| kept.then_some(diagnostic))
            .collect()
    }

    fn is_first_duplicated_definition(&self, diagnostic: &SignatureDiagnostic) -> bool {
        let is_at = |location: &Location| {
            location.path.as_ref() == &diagnostic.path && location.start == diagnostic.start
        };
        self.duplicated_method_definitions
            .iter()
            .find(|duplicated| duplicated.locations.iter().any(is_at))
            .is_none_or(|duplicated| duplicated.locations.first().is_some_and(is_at))
    }

    fn innermost_validation_scope(
        &self,
        diagnostic: &SignatureDiagnostic,
    ) -> Option<&ValidationScope> {
        let position = (diagnostic.start.line, diagnostic.start.byte_column);
        self.signature_validation_scopes
            .iter()
            .filter(|scope| {
                scope.location.path.as_ref() == &diagnostic.path
                    && (scope.location.start.line, scope.location.start.byte_column) <= position
                    && position < (scope.location.end.line, scope.location.end.byte_column)
            })
            .max_by_key(|scope| (scope.location.start.line, scope.location.start.byte_column))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ValidationOrder {
    phase: u8,
    position: (std::path::PathBuf, u32, u32),
}

/// The order `Steep::Signature::Validator#validate_one_class_decl` meets errors in: `build_instance` (duplicated
/// instance methods, then the variance check of instance method types), the instance method types, the instance
/// variable types, `build_singleton` (duplicated singleton methods), then the singleton member types
fn validation_phase(member: ValidationMember, code: &str) -> u8 {
    match member {
        ValidationMember::Declaration => 0,
        ValidationMember::InstanceMethod
        | ValidationMember::Initializer
        | ValidationMember::InstanceAlias
            if code == DUPLICATED_METHOD_DEFINITION =>
        {
            0
        }
        ValidationMember::InstanceMethod if code == UNKNOWN_TYPE_NAME => 1,
        ValidationMember::InstanceMethod
        | ValidationMember::Initializer
        | ValidationMember::InstanceAlias => 2,
        ValidationMember::InstanceVariable => 3,
        ValidationMember::SingletonMember if code == DUPLICATED_METHOD_DEFINITION => 4,
        ValidationMember::SingletonMember => 5,
    }
}

/// Follows the steps of `RBS::DefinitionBuilder#build_instance` and `#build_singleton` that raise, without building
/// anything
struct DefinitionErrorCalculator<'environment> {
    environment: &'environment Environment,
    duplicated: HashMap<(&'environment str, MethodKind), &'environment DuplicatedMethodDefinition>,
    instance: HashMap<TypeName, Option<DefinitionError>>,
    singleton: HashMap<TypeName, Option<DefinitionError>>,
    mixin: HashMap<TypeName, Option<DefinitionError>>,
    in_progress: HashSet<(TypeName, u8)>,
}

const INSTANCE_STEP: u8 = 0;
const SINGLETON_STEP: u8 = 1;
const MIXIN_STEP: u8 = 2;

impl<'environment> DefinitionErrorCalculator<'environment> {
    fn new(environment: &'environment Environment) -> DefinitionErrorCalculator<'environment> {
        let mut duplicated = HashMap::new();
        for definition in &environment.duplicated_method_definitions {
            duplicated
                .entry((definition.type_name.as_str(), definition.kind))
                .or_insert(definition);
        }
        DefinitionErrorCalculator {
            environment,
            duplicated,
            instance: HashMap::new(),
            singleton: HashMap::new(),
            mixin: HashMap::new(),
            in_progress: HashSet::new(),
        }
    }

    fn calculate(mut self) -> DefinitionErrors {
        let mut names: Vec<&TypeName> = self.environment.classes.keys().collect();
        names.sort();
        for name in names {
            self.instance_error(name);
            self.singleton_error(name);
        }
        DefinitionErrors {
            instance: self
                .instance
                .into_iter()
                .filter_map(|(name, error)| Some((name, error?)))
                .collect(),
            singleton: self
                .singleton
                .into_iter()
                .filter_map(|(name, error)| Some((name, error?)))
                .collect(),
        }
    }

    fn duplicated_error(&self, name: &str, kind: MethodKind) -> Option<DefinitionError> {
        self.duplicated
            .get(&(name, kind))
            .map(|duplicated| DefinitionError::DuplicatedMethodDefinition((*duplicated).clone()))
    }

    fn memoized(
        &mut self,
        name: &str,
        step: u8,
        calculate: impl FnOnce(&mut Self, &str) -> Option<DefinitionError>,
    ) -> Option<DefinitionError> {
        let cache = match step {
            INSTANCE_STEP => &self.instance,
            SINGLETON_STEP => &self.singleton,
            _ => &self.mixin,
        };
        if let Some(error) = cache.get(name) {
            return error.clone();
        }
        if !self.in_progress.insert((name.to_string(), step)) {
            return None;
        }
        let error = calculate(self, name);
        self.in_progress.remove(&(name.to_string(), step));
        let cache = match step {
            INSTANCE_STEP => &mut self.instance,
            SINGLETON_STEP => &mut self.singleton,
            _ => &mut self.mixin,
        };
        cache.insert(name.to_string(), error.clone());
        error
    }

    fn instance_error(&mut self, name: &str) -> Option<DefinitionError> {
        let name = self.environment.resolve_class_name(name)?.to_string();
        self.memoized(&name, INSTANCE_STEP, |calculator, name| {
            let declaration = calculator.environment.classes.get(name)?;
            if let Some(error) = calculator.duplicated_error(name, MethodKind::Instance) {
                return Some(error);
            }
            if let Some(error) = calculator.variance_error(name) {
                return Some(error);
            }
            if declaration.kind == ClassKind::Class
                && let Some(super_class) = &declaration.super_class
                && let Some(error) = calculator.instance_error(&super_class.name)
            {
                return Some(error);
            }
            for self_type in &declaration.module_self_types {
                if let Some(error) = calculator.instance_error(&self_type.name) {
                    return Some(error);
                }
            }
            calculator
                .mixed_in_modules_error(declaration.includes.iter().chain(&declaration.prepends))
        })
    }

    fn singleton_error(&mut self, name: &str) -> Option<DefinitionError> {
        let name = self.environment.resolve_class_name(name)?.to_string();
        self.memoized(&name, SINGLETON_STEP, |calculator, name| {
            let declaration = calculator.environment.classes.get(name)?;
            if let Some(error) = calculator.duplicated_error(name, MethodKind::Singleton) {
                return Some(error);
            }
            let super_error = match (&declaration.kind, &declaration.super_class) {
                (ClassKind::Class, Some(super_class)) => {
                    calculator.singleton_error(&super_class.name)
                }
                (ClassKind::Class, None) => calculator.instance_error("Class"),
                (ClassKind::Module, _) => calculator.instance_error("Module"),
            };
            if super_error.is_some() {
                return super_error;
            }
            if let Some(error) = calculator.mixed_in_modules_error(declaration.extends.iter()) {
                return Some(error);
            }
            calculator.instance_error(name)
        })
    }

    /// `RBS::DefinitionBuilder#define_instance` on included, prepended or extended modules, which builds their own
    /// methods and, recursively, the modules they include
    fn mixed_in_modules_error<'ancestor>(
        &mut self,
        modules: impl Iterator<Item = &'ancestor Ancestor>,
    ) -> Option<DefinitionError> {
        for module in modules {
            if crate::loader::is_interface_name(&module.name) {
                continue;
            }
            if let Some(error) = self.mixin_error(&module.name) {
                return Some(error);
            }
        }
        None
    }

    fn mixin_error(&mut self, name: &str) -> Option<DefinitionError> {
        let name = self.environment.resolve_class_name(name)?.to_string();
        self.memoized(&name, MIXIN_STEP, |calculator, name| {
            let declaration = calculator.environment.classes.get(name)?;
            if let Some(error) = calculator.duplicated_error(name, MethodKind::Instance) {
                return Some(error);
            }
            for self_type in &declaration.module_self_types {
                if let Some(error) = calculator.instance_error(&self_type.name) {
                    return Some(error);
                }
            }
            calculator
                .mixed_in_modules_error(declaration.includes.iter().chain(&declaration.prepends))
        })
    }

    /// `RBS::DefinitionBuilder#validate_type_params`, whose `RBS::VarianceCalculator` raises on an unknown name in
    /// the type arguments of an ancestor or in the types of an instance method other than `initialize`
    fn variance_error(&self, name: &str) -> Option<DefinitionError> {
        let declaration = self.environment.classes.get(name)?;
        let ancestor_types = declaration
            .super_class
            .iter()
            .chain(&declaration.includes)
            .chain(&declaration.prepends)
            .flat_map(|ancestor| ancestor.arguments.iter());
        for ancestor_type in ancestor_types {
            if let Some(type_name) = self.unknown_type_name(ancestor_type) {
                return Some(DefinitionError::NoTypeFound {
                    type_name,
                    location: declaration.locations.first().cloned(),
                });
            }
        }
        let mut methods: Vec<_> = declaration
            .instance_methods
            .values()
            .filter(|method| method.name != "initialize" && method.alias_of.is_none())
            .collect();
        methods.sort_by_key(|method| {
            method.location.as_ref().map(|location| {
                (
                    location.path.clone(),
                    location.start.line,
                    location.start.byte_column,
                )
            })
        });
        methods.into_iter().find_map(|method| {
            let type_name = method
                .overloads
                .iter()
                .find_map(|overload| self.unknown_type_name_in_method_type(overload))?;
            Some(DefinitionError::NoTypeFound {
                type_name,
                location: method.location.clone(),
            })
        })
    }

    fn unknown_type_name_in_method_type(&self, method_type: &MethodType) -> Option<TypeName> {
        let mut types: Vec<&Type> = method_type
            .function
            .parameters()
            .map(|parameter| &parameter.parameter_type)
            .collect();
        types.push(&method_type.function.return_type);
        if let Some(block) = &method_type.block {
            types.extend(
                block
                    .function
                    .parameters()
                    .map(|parameter| &parameter.parameter_type),
            );
            types.push(&block.function.return_type);
        }
        types
            .into_iter()
            .find_map(|parameter_type| self.unknown_type_name(parameter_type))
    }

    fn unknown_type_name(&self, target: &Type) -> Option<TypeName> {
        let mut unknown = None;
        target.walk(&mut |inner| {
            if unknown.is_some() {
                return;
            }
            let exists = match inner {
                Type::ClassInstance { name, .. } => {
                    self.environment.resolve_class_name(name).is_some()
                }
                Type::Interface { name, .. } => self
                    .environment
                    .interfaces
                    .contains_key(name.trim_start_matches("::")),
                Type::Alias { name, .. } => self
                    .environment
                    .type_aliases
                    .contains_key(name.trim_start_matches("::")),
                _ => true,
            };
            if !exists
                && let Type::ClassInstance { name, .. }
                | Type::Interface { name, .. }
                | Type::Alias { name, .. } = inner
            {
                unknown = Some(name.clone());
            }
        });
        unknown
    }
}
