use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::environment::{ClassKind, Environment, MethodKind};
use crate::loader::is_interface_name;
use crate::signature_validation::{
    AliasOwner, DUPLICATED_METHOD_DEFINITION, FileValidation, MethodAliasRecord, SignatureDiagnostic, absolute_name,
};
use crate::types::{Function, MethodType, Type, Variance};

pub const RECURSIVE_ANCESTOR: &str = "RBS::RecursiveAncestor";
pub const RECURSIVE_TYPE_ALIAS: &str = "RBS::RecursiveTypeAlias";
pub const NONREGULAR_TYPE_ALIAS: &str = "RBS::NonregularTypeAlias";
pub const RECURSIVE_ALIAS: &str = "RBS::RecursiveAlias";
pub const INVALID_VARIANCE_ANNOTATION: &str = "RBS::InvalidVarianceAnnotation";
pub const INSTANCE_VARIABLE_DUPLICATION: &str = "RBS::InstanceVariableDuplicationError";
pub const CLASS_INSTANCE_VARIABLE_DUPLICATION: &str = "RBS::ClassInstanceVariableDuplicationError";

/// The errors `RBS::DefinitionBuilder` and `RBS::Validator` raise while building the definitions of the classes, modules,
/// interfaces and type aliases declared in a signature file
pub(crate) fn validate_definitions(environment: &Environment, validation: &FileValidation) -> Vec<SignatureDiagnostic> {
    let mut diagnostics = Vec::new();
    let mut validated_names = HashSet::new();
    for declaration in &validation.declarations {
        if !validated_names.insert(declaration.name.as_str()) {
            continue;
        }
        let path = declaration.location.path.as_path();
        if environment.classes.contains_key(&declaration.name) {
            diagnostics.extend(recursive_ancestor(environment, &declaration.name, path));
        } else if environment.type_aliases.contains_key(&declaration.name) {
            diagnostics.extend(type_alias_errors(environment, &declaration.name, path));
        }
    }
    diagnostics.extend(invalid_variance_annotations(environment, validation));
    diagnostics.extend(recursive_method_aliases(environment, &validation.method_aliases));
    diagnostics.extend(duplicated_variables(validation));
    diagnostics.extend(methods_defined_by_included_interfaces(environment, validation));
    diagnostics
}

fn ancestor_text(environment: &Environment, name: &str) -> String {
    let parameters = environment
        .classes
        .get(name)
        .map(|declaration| {
            declaration
                .type_parameters
                .iter()
                .map(|parameter| parameter.name.clone())
                .collect::<Vec<String>>()
        })
        .unwrap_or_default();
    if parameters.is_empty() {
        absolute_name(name)
    } else {
        format!("{}[{}]", absolute_name(name), parameters.join(", "))
    }
}

/// `RBS::DefinitionBuilder::AncestorBuilder#instance_ancestors`, which raises `RBS::RecursiveAncestorError` when it
/// enters a class or module it is already building
fn recursive_ancestor(environment: &Environment, name: &str, path: &Path) -> Option<SignatureDiagnostic> {
    let mut building = Vec::new();
    let (repeated, chain) = find_recursive_ancestor(environment, name, &mut building, &mut HashSet::new())?;
    let location = environment.classes.get(&repeated)?.locations.first()?;
    if location.path.as_path() != path {
        return None;
    }
    let names: Vec<String> = chain
        .iter()
        .map(|ancestor| ancestor_text(environment, ancestor))
        .collect();
    Some(SignatureDiagnostic::new(
        location,
        RECURSIVE_ANCESTOR,
        format!("Circular inheritance/mix-in is detected: {}", names.join(" <: ")),
    ))
}

fn find_recursive_ancestor(
    environment: &Environment,
    name: &str,
    building: &mut Vec<String>,
    finished: &mut HashSet<String>,
) -> Option<(String, Vec<String>)> {
    let name = environment.resolve_class_name(name)?.to_string();
    if finished.contains(&name) {
        return None;
    }
    if building.contains(&name) {
        let mut chain = building.clone();
        chain.push(name.clone());
        return Some((name, chain));
    }
    let declaration = environment.classes.get(&name)?;
    building.push(name.clone());
    let super_class = match declaration.kind {
        ClassKind::Class => declaration.super_class.iter().collect::<Vec<_>>(),
        ClassKind::Module => Vec::new(),
    };
    let self_types = declaration
        .module_self_types
        .iter()
        .filter(|self_type| !is_interface_name(&self_type.name));
    let mixins = declaration
        .includes
        .iter()
        .chain(&declaration.prepends)
        .filter(|mixin| !is_interface_name(&mixin.name));
    for ancestor in super_class.into_iter().chain(self_types).chain(mixins) {
        if let Some(found) = find_recursive_ancestor(environment, &ancestor.name, building, finished) {
            return Some(found);
        }
    }
    building.pop();
    finished.insert(name);
    None
}

/// `RBS::Validator#validate_type_alias`: a type alias that expands to itself through unions and intersections only,
/// then a generic type alias that refers to itself with other type arguments
fn type_alias_errors(environment: &Environment, name: &str, path: &Path) -> Option<SignatureDiagnostic> {
    let declaration = environment.type_aliases.get(name)?;
    let location = declaration.location.as_ref()?;
    if location.path.as_path() != path {
        return None;
    }
    if is_circular_type_alias(environment, name) {
        return Some(SignatureDiagnostic::new(
            location,
            RECURSIVE_TYPE_ALIAS,
            format!("Type aliases cannot be *directly recursive*: {}", absolute_name(name)),
        ));
    }
    let nonregular_type = nonregular_type(environment, name)?;
    Some(SignatureDiagnostic::new(
        location,
        NONREGULAR_TYPE_ALIAS,
        format!(
            "Type alias {} is defined *non-regular*: {}",
            absolute_name(name),
            nonregular_type.steep_display()
        ),
    ))
}

/// `RBS::TypeAliasDependency#direct_dependency`
fn direct_alias_dependencies(target: &Type, dependencies: &mut Vec<String>) {
    match target {
        Type::Union(members) | Type::Intersection(members) => members
            .iter()
            .for_each(|member| direct_alias_dependencies(member, dependencies)),
        Type::Alias { name, .. } => dependencies.push(name.trim_start_matches("::").to_string()),
        _ => {}
    }
}

fn is_circular_type_alias(environment: &Environment, name: &str) -> bool {
    let mut visited = HashSet::new();
    let mut pending = vec![name.to_string()];
    while let Some(current) = pending.pop() {
        let Some(declaration) = environment.type_aliases.get(&current) else {
            continue;
        };
        let mut dependencies = Vec::new();
        direct_alias_dependencies(&declaration.aliased_type, &mut dependencies);
        for dependency in dependencies {
            if dependency == name {
                return true;
            }
            if visited.insert(dependency.clone()) {
                pending.push(dependency);
            }
        }
    }
    false
}

fn alias_types(target: &Type) -> Vec<Type> {
    let mut aliases = Vec::new();
    target.walk(&mut |inner| {
        if matches!(inner, Type::Alias { .. }) {
            aliases.push(inner.clone());
        }
    });
    aliases
}

/// Aliases that refer back to `name`, the strongly connected component `RBS::TypeAliasRegularity` checks together
fn mutually_recursive_aliases(environment: &Environment, name: &str) -> HashSet<String> {
    let reachable_from = |start: &str| {
        let mut reached = HashSet::new();
        let mut pending = vec![start.to_string()];
        while let Some(current) = pending.pop() {
            let Some(declaration) = environment.type_aliases.get(&current) else {
                continue;
            };
            for alias in alias_types(&declaration.aliased_type) {
                if let Type::Alias { name, .. } = alias {
                    let name = name.trim_start_matches("::").to_string();
                    if reached.insert(name.clone()) {
                        pending.push(name);
                    }
                }
            }
        }
        reached
    };
    let mut names: HashSet<String> = reachable_from(name)
        .into_iter()
        .filter(|other| reachable_from(other).contains(name))
        .collect();
    names.insert(name.to_string());
    names
}

/// `RBS::TypeAliasRegularity#validate_alias_type`, started from `name` with its own type parameters
fn nonregular_type(environment: &Environment, name: &str) -> Option<Type> {
    let declaration = environment.type_aliases.get(name)?;
    if declaration.type_parameters.is_empty() {
        return None;
    }
    let names = mutually_recursive_aliases(environment, name);
    let start = Type::Alias {
        name: name.to_string(),
        arguments: declaration
            .type_parameters
            .iter()
            .map(|parameter| Type::Variable(parameter.name.clone()))
            .collect(),
    };
    let mut seen: HashMap<String, Vec<Type>> = HashMap::new();
    let mut found = HashMap::new();
    validate_alias_type(environment, &start, &names, &mut seen, &mut found);
    found.remove(name)
}

fn validate_alias_type(
    environment: &Environment,
    alias_type: &Type,
    names: &HashSet<String>,
    seen: &mut HashMap<String, Vec<Type>>,
    found: &mut HashMap<String, Type>,
) {
    let Type::Alias { name, arguments } = alias_type else {
        return;
    };
    let name = name.trim_start_matches("::");
    if !names.contains(name) {
        return;
    }
    if let Some(seen_arguments) = seen.get(name) {
        let compatible = seen_arguments.len() == arguments.len()
            && seen_arguments
                .iter()
                .zip(arguments)
                .all(|(left, right)| matches!(left, Type::Untyped) || matches!(right, Type::Untyped) || left == right);
        if !compatible {
            found.entry(name.to_string()).or_insert_with(|| alias_type.clone());
        }
        return;
    }
    seen.insert(name.to_string(), arguments.clone());
    let Some(expanded) = environment.expand_alias(name, arguments) else {
        return;
    };
    for alias in alias_types(&expanded) {
        validate_alias_type(environment, &alias, names, seen, found);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Usage {
    Unused,
    Covariant,
    Contravariant,
    Invariant,
}

/// `RBS::VarianceCalculator::Result`
struct VarianceUsage {
    usages: HashMap<String, Usage>,
}

impl VarianceUsage {
    fn mark(&mut self, name: &str, context: Variance) {
        let Some(usage) = self.usages.get_mut(name) else {
            return;
        };
        *usage = match (context, *usage) {
            (Variance::Invariant, _) => Usage::Invariant,
            (Variance::Covariant, Usage::Unused) => Usage::Covariant,
            (Variance::Covariant, Usage::Contravariant) => Usage::Invariant,
            (Variance::Contravariant, Usage::Unused) => Usage::Contravariant,
            (Variance::Contravariant, Usage::Covariant) => Usage::Invariant,
            (_, unchanged) => unchanged,
        };
    }

    fn compatible(&self, name: &str, annotation: Variance) -> bool {
        matches!(
            (self.usages.get(name).copied().unwrap_or(Usage::Unused), annotation),
            (Usage::Unused, _)
                | (_, Variance::Invariant)
                | (Usage::Covariant, Variance::Covariant)
                | (Usage::Contravariant, Variance::Contravariant)
        )
    }
}

fn negate(variance: Variance) -> Variance {
    match variance {
        Variance::Invariant => Variance::Invariant,
        Variance::Covariant => Variance::Contravariant,
        Variance::Contravariant => Variance::Covariant,
    }
}

fn type_parameter_variances(environment: &Environment, target: &Type) -> Option<Vec<Variance>> {
    let parameters = match target {
        Type::ClassInstance { name, .. } => {
            &environment
                .classes
                .get(environment.resolve_class_name(name)?)?
                .type_parameters
        }
        Type::Interface { name, .. } => {
            &environment
                .interfaces
                .get(name.trim_start_matches("::"))?
                .type_parameters
        }
        Type::Alias { name, .. } => {
            &environment
                .type_aliases
                .get(name.trim_start_matches("::"))?
                .type_parameters
        }
        _ => return None,
    };
    Some(parameters.iter().map(|parameter| parameter.variance).collect())
}

/// `RBS::VarianceCalculator#type`
fn mark_type(environment: &Environment, target: &Type, usage: &mut VarianceUsage, context: Variance) {
    match target {
        Type::Variable(name) => usage.mark(name, context),
        Type::ClassInstance { arguments, .. } | Type::Interface { arguments, .. } | Type::Alias { arguments, .. } => {
            let Some(variances) = type_parameter_variances(environment, target) else {
                return;
            };
            for (argument, variance) in arguments.iter().zip(variances) {
                let argument_context = match variance {
                    Variance::Invariant => Variance::Invariant,
                    Variance::Covariant => context,
                    Variance::Contravariant => negate(context),
                };
                mark_type(environment, argument, usage, argument_context);
            }
        }
        Type::Proc(proc_type) => mark_function(environment, &proc_type.function, usage, context),
        Type::Union(members) | Type::Intersection(members) | Type::Tuple(members) => members
            .iter()
            .for_each(|member| mark_type(environment, member, usage, context)),
        Type::Record(fields) => fields
            .iter()
            .for_each(|field| mark_type(environment, &field.field_type, usage, context)),
        _ => {}
    }
}

/// `RBS::VarianceCalculator#function`
fn mark_function(environment: &Environment, function: &Function, usage: &mut VarianceUsage, context: Variance) {
    for parameter in function.parameters() {
        mark_type(environment, &parameter.parameter_type, usage, negate(context));
    }
    mark_type(environment, &function.return_type, usage, context);
}

/// `RBS::DefinitionBuilder#validate_type_params` on the method types of a class or an interface
fn invalid_variance_annotations(environment: &Environment, validation: &FileValidation) -> Vec<SignatureDiagnostic> {
    validation
        .method_types
        .iter()
        .filter_map(|record| {
            let owner_parameters = match environment.classes.get(&record.owner_name) {
                Some(declaration) => &declaration.type_parameters,
                None => &environment.interfaces.get(&record.owner_name)?.type_parameters,
            };
            let parameter = incompatible_parameter(environment, owner_parameters, &record.method_type)?;
            let variance = match parameter.variance {
                Variance::Covariant => "covariant",
                Variance::Contravariant => "contravariant",
                Variance::Invariant => "invariant",
            };
            Some(SignatureDiagnostic::new(
                &record.location,
                INVALID_VARIANCE_ANNOTATION,
                format!(
                    "The variance of type parameter `{}` is {variance}, but used in incompatible position here",
                    parameter.name
                ),
            ))
        })
        .collect()
}

fn incompatible_parameter<'a>(
    environment: &Environment,
    owner_parameters: &'a [crate::types::TypeParameter],
    method_type: &'a MethodType,
) -> Option<&'a crate::types::TypeParameter> {
    let method_parameter_names: Vec<&str> = method_type
        .type_parameters
        .iter()
        .map(|parameter| parameter.name.as_str())
        .collect();
    let mut usage = VarianceUsage {
        usages: owner_parameters
            .iter()
            .map(|parameter| (parameter.name.clone(), Usage::Unused))
            .collect(),
    };
    mark_function(environment, &method_type.function, &mut usage, Variance::Covariant);
    if let Some(block) = &method_type.block {
        mark_function(environment, &block.function, &mut usage, Variance::Contravariant);
    }
    owner_parameters
        .iter()
        .filter(|parameter| !method_parameter_names.contains(&parameter.name.as_str()))
        .chain(&method_type.type_parameters)
        .find(|parameter| !parameter.unchecked && !usage.compatible(&parameter.name, parameter.variance))
}

/// `RBS::DefinitionBuilder::MethodBuilder::Methods#each`, which raises `RBS::RecursiveAliasDefinitionError` on the
/// first cycle of aliases among the methods of a class, in the order the methods are declared
fn recursive_method_aliases(environment: &Environment, aliases: &[MethodAliasRecord]) -> Vec<SignatureDiagnostic> {
    let mut reported = HashSet::new();
    let mut diagnostics = Vec::new();
    for alias in aliases {
        let AliasOwner::Class(kind) = alias.owner else {
            continue;
        };
        if reported.contains(&(alias.owner_name.as_str(), kind)) {
            continue;
        }
        let owned_aliases: Vec<&MethodAliasRecord> = aliases
            .iter()
            .filter(|other| {
                other.owner_name == alias.owner_name
                    && matches!(other.owner, AliasOwner::Class(other_kind) if other_kind == kind)
            })
            .collect();
        let Some(cycle) = first_alias_cycle(environment, &alias.owner_name, kind, &owned_aliases) else {
            continue;
        };
        reported.insert((alias.owner_name.as_str(), kind));
        let names: Vec<&str> = cycle.iter().map(|record| record.new_name.as_str()).collect();
        diagnostics.push(SignatureDiagnostic::new(
            &cycle[0].location,
            RECURSIVE_ALIAS,
            format!(
                "Circular method alias is detected in `{}`: {}",
                absolute_name(&alias.owner_name),
                names.join(" -> ")
            ),
        ));
    }
    diagnostics
}

fn first_alias_cycle<'a>(
    environment: &Environment,
    owner_name: &str,
    kind: MethodKind,
    aliases: &[&'a MethodAliasRecord],
) -> Option<Vec<&'a MethodAliasRecord>> {
    let defined = |method_name: &str| {
        let declaration = environment.classes.get(owner_name);
        let methods = declaration.map(|declaration| match kind {
            MethodKind::Instance => &declaration.instance_methods,
            MethodKind::Singleton => &declaration.singleton_methods,
        });
        methods.is_some_and(|methods| methods.contains_key(method_name))
    };
    for start in aliases {
        let mut chain = vec![*start];
        let mut current = *start;
        loop {
            if !defined(&current.old_name) {
                break;
            }
            let Some(next) = aliases.iter().find(|alias| alias.new_name == current.old_name) else {
                break;
            };
            if std::ptr::eq(*next, *start) {
                return Some(chain);
            }
            if chain.iter().any(|visited| std::ptr::eq(*visited, *next)) {
                break;
            }
            chain.push(next);
            current = next;
        }
    }
    None
}

/// `RBS::DefinitionBuilder#validate_variable`: an instance variable or a class instance variable declared twice in
/// the same class, reported at the later declaration
fn duplicated_variables(validation: &FileValidation) -> Vec<SignatureDiagnostic> {
    let mut seen = HashSet::new();
    validation
        .variables
        .iter()
        .filter(|variable| !seen.insert((variable.owner_name.as_str(), variable.name.as_str(), variable.kind)))
        .map(|variable| {
            let (code, description) = match variable.kind {
                MethodKind::Instance => (INSTANCE_VARIABLE_DUPLICATION, "instance variable"),
                MethodKind::Singleton => (CLASS_INSTANCE_VARIABLE_DUPLICATION, "class instance variable"),
            };
            SignatureDiagnostic::new(
                &variable.location,
                code,
                format!(
                    "Duplicated {description} name `{}` in `{}`",
                    variable.name,
                    absolute_name(&variable.owner_name)
                ),
            )
        })
        .collect()
}

/// `RBS::DefinitionBuilder#define_method` raises `RBS::DuplicatedMethodDefinitionError` for a `def` of a method that an
/// included interface already defines
fn methods_defined_by_included_interfaces(
    environment: &Environment,
    validation: &FileValidation,
) -> Vec<SignatureDiagnostic> {
    validation
        .method_members
        .iter()
        .filter(|member| !member.overloading)
        .filter(|member| {
            let Some(declaration) = environment.classes.get(&member.owner_name) else {
                return false;
            };
            declaration
                .includes
                .iter()
                .filter(|include| is_interface_name(&include.name))
                .any(|include| environment.find_interface_method(&include.name, &member.name).is_some())
        })
        .map(|member| {
            SignatureDiagnostic::new(
                &member.location,
                DUPLICATED_METHOD_DEFINITION,
                format!(
                    "Non-overloading method definition of `{}` in `{}` cannot be duplicated",
                    member.name,
                    absolute_name(&member.owner_name)
                ),
            )
        })
        .collect()
}
