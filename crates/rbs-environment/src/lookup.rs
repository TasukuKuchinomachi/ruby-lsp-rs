use std::collections::{HashMap, HashSet};
use std::sync::RwLock;

use crate::environment::{Ancestor, ClassKind, Environment, MethodDefinition, ResolvedMethod};
use crate::types::{MethodType, NameKind, Substitution, Type, TypeName, TypeParameter};

#[derive(Debug, Default)]
pub(crate) struct AncestorCache {
    pub(crate) instance: RwLock<HashMap<TypeName, Vec<Ancestor>>>,
}

/// One step of the method lookup for `Foo.method`
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SingletonAncestor {
    /// Singleton methods declared with `def self.x` in this class or module
    Singleton(TypeName),
    /// Instance methods of a module given to `extend`, or of `Class`/`Module` themselves
    Instance(Ancestor),
}

const MAXIMUM_ALIAS_DEPTH: usize = 16;

impl Environment {
    /// Follows `class Foo = Bar` aliases and returns the name of the declaration
    pub fn resolve_class_name<'a>(&'a self, name: &'a str) -> Option<&'a str> {
        let mut current = name.strip_prefix("::").unwrap_or(name);
        for _ in 0..MAXIMUM_ALIAS_DEPTH {
            if let Some((declared_name, _)) = self.classes.get_key_value(current) {
                return Some(declared_name.as_str());
            }
            current = self.class_aliases.get(current)?.as_str();
        }
        None
    }

    fn resolve_declaration_name<'a>(&'a self, name: &'a str) -> Option<&'a str> {
        self.resolve_class_name(name).or_else(|| {
            let name = name.strip_prefix("::").unwrap_or(name);
            self.interfaces
                .get_key_value(name)
                .map(|(declared_name, _)| declared_name.as_str())
        })
    }

    fn type_parameters_of(&self, name: &str) -> &[TypeParameter] {
        if let Some(declaration) = self.classes.get(name) {
            &declaration.type_parameters
        } else if let Some(declaration) = self.interfaces.get(name) {
            &declaration.type_parameters
        } else {
            &[]
        }
    }

    /// Maps the type parameters of the declaration `name` to `arguments`, falling back to the parameter default and
    /// then `untyped` when an argument is missing
    fn substitution_for(&self, name: &str, arguments: &[Type]) -> Substitution {
        let variables = self
            .type_parameters_of(name)
            .iter()
            .enumerate()
            .filter_map(|(index, type_parameter)| {
                let argument = arguments.get(index).cloned().or_else(|| type_parameter.default_type.clone()).unwrap_or(Type::Untyped);
                let is_identity = matches!(&argument, Type::Variable(variable) if *variable == type_parameter.name);
                (!is_identity).then(|| (type_parameter.name.clone(), argument))
            })
            .collect();
        Substitution {
            variables,
            ..Substitution::default()
        }
    }

    /// Instance ancestors of `name` in method resolution order, starting with `name` itself (prepends come first).
    /// Type arguments are expressed in terms of the type parameters of `name`. Classes end with
    /// `Object, Kernel, BasicObject`
    pub fn ancestors(&self, name: &str) -> Vec<Ancestor> {
        let Some(name) = self.resolve_declaration_name(name) else {
            return Vec::new();
        };
        self.cached_ancestors(name, &mut Vec::new())
    }

    fn cached_ancestors(&self, name: &str, visiting: &mut Vec<String>) -> Vec<Ancestor> {
        let cache = &self.ancestor_cache.instance;
        if let Some(cached) = cache
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(name)
        {
            return cached.clone();
        }
        if visiting.iter().any(|visiting_name| visiting_name == name) {
            return Vec::new();
        }
        visiting.push(name.to_string());
        let computed = self.compute_ancestors(name, visiting);
        visiting.pop();
        cache
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(name.to_string(), computed.clone());
        computed
    }

    fn compute_ancestors(&self, name: &str, visiting: &mut Vec<String>) -> Vec<Ancestor> {
        let own_arguments = self
            .type_parameters_of(name)
            .iter()
            .map(|type_parameter| Type::Variable(type_parameter.name.clone()));
        let own = Ancestor {
            name: name.to_string(),
            arguments: own_arguments.collect(),
        };
        if let Some(declaration) = self.classes.get(name) {
            let super_ancestors = match &declaration.super_class {
                Some(super_class) => self.instantiated_ancestors(super_class, visiting),
                None => Vec::new(),
            };
            let mut result = Vec::new();
            for prepend in declaration.prepends.iter().rev() {
                result.extend(self.instantiated_ancestors(prepend, visiting));
            }
            result.push(own);
            for include in declaration.includes.iter().rev() {
                result.extend(self.instantiated_ancestors(include, visiting));
            }
            result.extend(super_ancestors);
            deduplicate_ancestors(result)
        } else if let Some(declaration) = self.interfaces.get(name) {
            let mut result = vec![own];
            for include in declaration.includes.iter().rev() {
                result.extend(self.instantiated_ancestors(include, visiting));
            }
            deduplicate_ancestors(result)
        } else {
            Vec::new()
        }
    }

    /// Ancestors of a reference such as `include Enumerable[Elem]`, with the referenced declaration's type
    /// parameters replaced by the reference's arguments
    fn instantiated_ancestors(
        &self,
        reference: &Ancestor,
        visiting: &mut Vec<String>,
    ) -> Vec<Ancestor> {
        let Some(name) = self.resolve_declaration_name(&reference.name) else {
            return Vec::new();
        };
        let substitution = self.substitution_for(name, &reference.arguments);
        let ancestors = self.cached_ancestors(name, visiting);
        if substitution.is_empty() {
            return ancestors;
        }
        ancestors
            .into_iter()
            .map(|ancestor| Ancestor {
                name: ancestor.name,
                arguments: ancestor
                    .arguments
                    .iter()
                    .map(|argument| argument.substitute(&substitution))
                    .collect(),
            })
            .collect()
    }

    /// Instance ancestors followed, for modules, by the ancestors of the module self types and of `Object`, since a
    /// module's instance is always some object
    pub(crate) fn instance_lookup_chain(&self, name: &str) -> Vec<Ancestor> {
        let mut chain = self.ancestors(name);
        let module = self
            .resolve_class_name(name)
            .and_then(|resolved_name| self.classes.get(resolved_name))
            .filter(|declaration| declaration.kind == ClassKind::Module);
        if let Some(module) = module {
            for self_type in &module.module_self_types {
                chain.extend(self.instantiated_ancestors(self_type, &mut Vec::new()));
            }
            chain.extend(self.ancestors("Object"));
            chain = deduplicate_ancestors(chain);
        }
        chain
    }

    /// Lookup order for singleton methods: `singleton(Foo)`, its extends, the singleton of the superclass chain, and
    /// finally the instance ancestors of `Class` (or `Module` for modules)
    pub fn singleton_ancestors(&self, name: &str) -> Vec<SingletonAncestor> {
        let Some(start) = self.resolve_class_name(name) else {
            return Vec::new();
        };
        let mut result = Vec::new();
        let mut visited = HashSet::new();
        let mut current = Some(start);
        while let Some(class_name) = current {
            if !visited.insert(class_name) {
                break;
            }
            let Some(declaration) = self.classes.get(class_name) else {
                break;
            };
            result.push(SingletonAncestor::Singleton(class_name.to_string()));
            for extend in declaration.extends.iter().rev() {
                for ancestor in self.instantiated_ancestors(extend, &mut Vec::new()) {
                    result.push(SingletonAncestor::Instance(ancestor));
                }
            }
            current = declaration
                .super_class
                .as_ref()
                .and_then(|super_class| self.resolve_class_name(&super_class.name));
        }
        let metaclass_name = match self.classes.get(start).map(|declaration| declaration.kind) {
            Some(ClassKind::Module) => "Module",
            _ => "Class",
        };
        result.extend(
            self.ancestors(metaclass_name)
                .into_iter()
                .map(SingletonAncestor::Instance),
        );

        let mut seen_instance_ancestors = HashSet::new();
        result.retain(|step| match step {
            SingletonAncestor::Singleton(_) => true,
            SingletonAncestor::Instance(ancestor) => {
                seen_instance_ancestors.insert(ancestor.name.clone())
            }
        });
        result
    }

    fn method_at(
        &self,
        step: &SingletonAncestor,
        method_name: &str,
    ) -> Option<(&MethodDefinition, Substitution)> {
        match step {
            SingletonAncestor::Singleton(class_name) => {
                let definition = self
                    .classes
                    .get(class_name)?
                    .singleton_methods
                    .get(method_name)?;
                Some((definition, Substitution::default()))
            }
            SingletonAncestor::Instance(ancestor) => {
                let definition = match self.classes.get(&ancestor.name) {
                    Some(declaration) => declaration.instance_methods.get(method_name)?,
                    None => self
                        .interfaces
                        .get(&ancestor.name)?
                        .methods
                        .get(method_name)?,
                };
                Some((
                    definition,
                    self.substitution_for(&ancestor.name, &ancestor.arguments),
                ))
            }
        }
    }

    /// Finds `method_name` in the first step of `chain` that defines it. An alias is looked up again from the step
    /// that declares it, because the aliased method may come from that step's own ancestors
    pub(crate) fn find_in_chain(
        &self,
        chain: &[SingletonAncestor],
        method_name: &str,
        alias_depth: usize,
    ) -> Option<ResolvedMethod<'_>> {
        for (index, step) in chain.iter().enumerate() {
            let Some((definition, substitution)) = self.method_at(step, method_name) else {
                continue;
            };
            if let Some(original_name) = &definition.alias_of {
                let overloads = if alias_depth < MAXIMUM_ALIAS_DEPTH {
                    self.find_in_chain(&chain[index..], original_name, alias_depth + 1)
                        .map(|original| original.overloads)
                        .unwrap_or_default()
                } else {
                    Vec::new()
                };
                return Some(ResolvedMethod {
                    definition,
                    overloads,
                });
            }
            let overloads = definition
                .overloads
                .iter()
                .map(|overload| substitute_method_type(overload, &substitution))
                .collect();
            return Some(ResolvedMethod {
                definition,
                overloads,
            });
        }
        None
    }

    pub fn find_instance_method(
        &self,
        class_name: &str,
        method_name: &str,
    ) -> Option<ResolvedMethod<'_>> {
        if self.resolve_class_name(class_name).is_none() {
            return self.find_interface_method(class_name, method_name);
        }
        let chain: Vec<SingletonAncestor> = self
            .instance_lookup_chain(class_name)
            .into_iter()
            .map(SingletonAncestor::Instance)
            .collect();
        self.find_in_chain(&chain, method_name, 0)
    }

    pub fn find_singleton_method(
        &self,
        class_name: &str,
        method_name: &str,
    ) -> Option<ResolvedMethod<'_>> {
        self.find_in_chain(&self.singleton_ancestors(class_name), method_name, 0)
    }

    pub fn find_interface_method(
        &self,
        interface_name: &str,
        method_name: &str,
    ) -> Option<ResolvedMethod<'_>> {
        let chain: Vec<SingletonAncestor> = self
            .ancestors(interface_name)
            .into_iter()
            .map(SingletonAncestor::Instance)
            .collect();
        self.find_in_chain(&chain, method_name, 0)
    }

    /// Names of every instance method visible on `class_name`, for completion and diagnostics
    pub fn instance_method_names(&self, class_name: &str) -> Vec<String> {
        let mut names = HashSet::new();
        for ancestor in self.instance_lookup_chain(class_name) {
            if let Some(declaration) = self.classes.get(&ancestor.name) {
                names.extend(declaration.instance_methods.keys().cloned());
            } else if let Some(declaration) = self.interfaces.get(&ancestor.name) {
                names.extend(declaration.methods.keys().cloned());
            }
        }
        let mut names: Vec<String> = names.into_iter().collect();
        names.sort();
        names
    }

    /// Expands `type foo[T] = ...` with the given arguments
    pub fn expand_alias(&self, alias_name: &str, arguments: &[Type]) -> Option<Type> {
        let declaration = self
            .type_aliases
            .get(alias_name.strip_prefix("::").unwrap_or(alias_name))?;
        let variables = declaration
            .type_parameters
            .iter()
            .enumerate()
            .map(|(index, type_parameter)| {
                let argument = arguments
                    .get(index)
                    .cloned()
                    .or_else(|| type_parameter.default_type.clone())
                    .unwrap_or(Type::Untyped);
                (type_parameter.name.clone(), argument)
            })
            .collect();
        Some(declaration.aliased_type.substitute(&Substitution {
            variables,
            ..Substitution::default()
        }))
    }

    /// Looks up `@name` (including the `@`) in the class and its ancestors
    pub fn instance_variable_type(&self, class_name: &str, variable_name: &str) -> Option<Type> {
        self.ancestors(class_name).iter().find_map(|ancestor| {
            let variable_type = self
                .classes
                .get(&ancestor.name)?
                .instance_variables
                .get(variable_name)?;
            Some(
                variable_type
                    .substitute(&self.substitution_for(&ancestor.name, &ancestor.arguments)),
            )
        })
    }

    fn constant_exists(&self, name: &str) -> bool {
        self.classes.contains_key(name)
            || self.class_aliases.contains_key(name)
            || self.constants.contains_key(name)
    }

    /// Returns `name` when it is declared, or the same name with its namespace rewritten through class aliases
    fn declared_constant_name(&self, name: &str) -> Option<TypeName> {
        if self.constant_exists(name) {
            return Some(name.to_string());
        }
        let (namespace, last_segment) = name.rsplit_once("::")?;
        let namespace = self.declared_constant_name(namespace)?;
        let resolved_namespace = self.resolve_class_name(&namespace)?;
        let candidate = format!("{resolved_namespace}::{last_segment}");
        self.constant_exists(&candidate).then_some(candidate)
    }

    /// Resolves a constant written as `relative_name` inside the nesting `nesting` (innermost last, fully qualified),
    /// returning its fully qualified name. Matches classes, modules, class aliases and constants
    pub fn resolve_constant_name(
        &self,
        nesting: &[String],
        relative_name: &str,
    ) -> Option<TypeName> {
        if let Some(absolute_name) = relative_name.strip_prefix("::") {
            return self.declared_constant_name(absolute_name);
        }
        for prefix in nesting.iter().rev() {
            if let Some(name) = self.declared_constant_name(&format!("{prefix}::{relative_name}")) {
                return Some(name);
            }
        }
        if let Some(innermost) = nesting.last() {
            for ancestor in self.ancestors(innermost).iter().skip(1) {
                if let Some(name) =
                    self.declared_constant_name(&format!("{}::{relative_name}", ancestor.name))
                {
                    return Some(name);
                }
            }
        }
        self.declared_constant_name(relative_name)
    }

    fn type_name_exists(&self, kind: NameKind, name: &str) -> bool {
        match kind {
            NameKind::Class => {
                self.classes.contains_key(name) || self.class_aliases.contains_key(name)
            }
            NameKind::Interface => self.interfaces.contains_key(name),
            NameKind::Alias => self.type_aliases.contains_key(name),
        }
    }

    fn resolve_type_name(
        &self,
        kind: NameKind,
        nesting: &[String],
        written_name: &str,
    ) -> TypeName {
        if let Some(absolute_name) = written_name.strip_prefix("::") {
            return absolute_name.to_string();
        }
        for prefix in nesting.iter().rev() {
            let candidate = format!("{prefix}::{written_name}");
            if self.type_name_exists(kind, &candidate) {
                return candidate;
            }
        }
        written_name.to_string()
    }

    /// Resolves names inside a type parsed with `parse_type`, using the same rules as `resolve_constant_name`
    pub fn resolve_type_names(&self, parsed_type: &mut Type, nesting: &[String]) {
        parsed_type.map_names(&mut |kind, name| self.resolve_type_name(kind, nesting, name));
    }
}

fn deduplicate_ancestors(ancestors: Vec<Ancestor>) -> Vec<Ancestor> {
    let mut seen = HashSet::new();
    ancestors
        .into_iter()
        .filter(|ancestor| seen.insert(ancestor.name.clone()))
        .collect()
}

/// Method type parameters shadow class type parameters of the same name
fn substitute_method_type(method_type: &MethodType, substitution: &Substitution) -> MethodType {
    if substitution.is_empty() {
        return method_type.clone();
    }
    let shadows = method_type
        .type_parameters
        .iter()
        .any(|type_parameter| substitution.variables.contains_key(&type_parameter.name));
    if !shadows {
        return method_type.substitute(substitution);
    }
    let mut narrowed = substitution.clone();
    for type_parameter in &method_type.type_parameters {
        narrowed.variables.remove(&type_parameter.name);
    }
    method_type.substitute(&narrowed)
}
