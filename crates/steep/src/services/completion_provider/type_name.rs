use std::sync::LazyLock;

use rbs_definition::{
    ast::{Directive, Namespace, TypeName},
    collections::{HashMap, HashSet},
    environment::{Environment, NormalizedModuleName, TypeNameResolver, UseMap},
};
use regex::Regex;

use crate::type_inference::constant_env::{ResolverContext, nesting_names};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Prefix {
    RawIdent {
        ident: String,
    },
    NamespacedIdent {
        namespace: Namespace,
        ident: String,
        size: usize,
    },
    Namespace {
        namespace: Namespace,
        size: usize,
    },
}

static NAMESPACE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\A((?:::(?-u:\w)+[A-Z])+(?:::)?)").expect("valid"));
static ROOT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\A::").expect("valid"));
static NAMESPACED_IDENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\A((?-u:\w)*[A-Za-z_])((?:::(?-u:\w)+[A-Z])+(?:::)?)").expect("valid")
});
static IDENT_IN_ROOT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\A((?-u:\w)*[A-Za-z_])::").expect("valid"));
static RAW_IDENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\A((?-u:\w)*[A-Za-z_])").expect("valid"));

fn reversed(text: &str) -> String {
    text.chars().rev().collect()
}

pub fn parse_namespace(string: &str) -> Namespace {
    let absolute = string.starts_with("::");
    let path = string
        .trim_start_matches("::")
        .split("::")
        .filter(|component| !component.is_empty())
        .map(str::to_string)
        .collect();
    Namespace { absolute, path }
}

impl Prefix {
    pub fn parse(text_before: &str) -> Option<Prefix> {
        let prefix = reversed(text_before);
        if let Some(captures) = NAMESPACE.captures(&prefix) {
            let namespace = &captures[1];
            return Some(Prefix::Namespace {
                namespace: parse_namespace(&reversed(namespace)),
                size: namespace.chars().count(),
            });
        }
        if ROOT.is_match(&prefix) {
            return Some(Prefix::Namespace {
                namespace: Namespace::root(),
                size: 2,
            });
        }
        if let Some(captures) = NAMESPACED_IDENT.captures(&prefix) {
            let ident = &captures[1];
            let namespace = &captures[2];
            return Some(Prefix::NamespacedIdent {
                namespace: parse_namespace(&reversed(namespace)),
                ident: reversed(ident),
                size: ident.chars().count() + namespace.chars().count(),
            });
        }
        if let Some(captures) = IDENT_IN_ROOT.captures(&prefix) {
            let ident = &captures[1];
            return Some(Prefix::NamespacedIdent {
                namespace: Namespace::root(),
                ident: reversed(ident),
                size: ident.chars().count() + 2,
            });
        }
        RAW_IDENT
            .captures(&prefix)
            .map(|captures| Prefix::RawIdent {
                ident: reversed(&captures[1]),
            })
    }

    pub fn size(&self) -> usize {
        match self {
            Prefix::RawIdent { ident } => ident.chars().count(),
            Prefix::NamespacedIdent { size, .. } | Prefix::Namespace { size, .. } => *size,
        }
    }
}

pub struct TypeNameCompletion<'environment> {
    pub environment: &'environment Environment,
    pub context: ResolverContext,
    map: UseMap,
    resolver: TypeNameResolver,
}

impl<'environment> TypeNameCompletion<'environment> {
    pub fn new(
        environment: &'environment Environment,
        context: ResolverContext,
        directives: &[Directive],
    ) -> TypeNameCompletion<'environment> {
        TypeNameCompletion {
            environment,
            context,
            map: environment.use_map(directives).unwrap_or_default(),
            resolver: TypeNameResolver::new(environment),
        }
    }

    fn context_names(&self) -> Vec<TypeName> {
        nesting_names(&self.context)
    }

    pub fn each_outer_module(&self) -> Vec<Namespace> {
        let mut namespaces = vec![Namespace::root()];
        let mut namespace = Namespace::root();
        for name in self.context_names() {
            namespace = name.with_prefix(&namespace).to_namespace();
            namespaces.push(namespace.clone());
        }
        namespaces
    }

    pub fn each_type_name(&self) -> Vec<TypeName> {
        let environment = self.environment;
        let mut names = Vec::new();
        let mut table: HashMap<Namespace, Vec<TypeName>> = HashMap::default();
        let keys = environment
            .class_declarations
            .keys()
            .chain(environment.type_alias_declarations.keys())
            .chain(environment.interface_declarations.keys())
            .chain(environment.class_alias_declarations.keys());
        for type_name in keys {
            names.push(type_name.clone());
            table
                .entry(type_name.namespace.clone())
                .or_default()
                .push(type_name.clone());
        }
        for alias_name in environment.class_alias_declarations.keys() {
            if let NormalizedModuleName::Found(normalized) =
                environment.normalize_module_name_checked(alias_name)
            {
                self.each_type_name_under(alias_name, &normalized, &table, &mut names);
            }
        }
        let mut pairs = Vec::new();
        for name in self.map.names() {
            let relative_name = TypeName::new(
                Namespace {
                    absolute: false,
                    path: Vec::new(),
                },
                name,
            );
            if let Some(absolute_name) = self.map.resolve(&relative_name)
                && environment.is_type_name(&absolute_name)
            {
                pairs.push((relative_name, absolute_name));
            }
        }
        for (use_name, absolute_name) in pairs {
            names.push(use_name.clone());
            self.each_type_name_under(&use_name, &absolute_name, &table, &mut names);
        }
        names
    }

    fn each_type_name_under(
        &self,
        module_name: &TypeName,
        normalized_name: &TypeName,
        table: &HashMap<Namespace, Vec<TypeName>>,
        names: &mut Vec<TypeName>,
    ) {
        let Some(children) = table.get(&normalized_name.to_namespace()) else {
            return;
        };
        let module_namespace = module_name.to_namespace();
        for normalized_child_name in children {
            let child_name = TypeName::new(module_namespace.clone(), &normalized_child_name.name);
            names.push(child_name.clone());
            if normalized_child_name.is_class() {
                let normalized = self
                    .environment
                    .normalize_module_name(normalized_child_name);
                self.each_type_name_under(&child_name, &normalized, table, names);
            }
        }
    }

    fn resolve_used_name(&self, name: &TypeName) -> Option<TypeName> {
        if name.is_absolute() {
            return None;
        }
        if let Some(resolved) = self.map.resolve(name) {
            return Some(resolved);
        }
        if name.namespace.is_empty() {
            return None;
        }
        let resolved_parent = self.resolve_used_name(&name.namespace.to_type_name())?;
        let resolved_name = TypeName::new(resolved_parent.to_namespace(), &name.name);
        self.environment
            .normalize_type_name(&resolved_name)
            .map(|_| resolved_name)
    }

    pub fn resolve_name_in_context(&self, name: &TypeName) -> Option<(TypeName, TypeName)> {
        if let Some(resolved_name) = self.resolve_used_name(name) {
            return Some((resolved_name, name.clone()));
        }
        if !name.is_absolute() {
            return None;
        }
        let normalized_name = self.environment.normalize_type_name(name)?;
        let context = self.context_names();
        let mut relative_name = TypeName::new(
            Namespace {
                absolute: false,
                path: Vec::new(),
            },
            &name.name,
        );
        for component in name.namespace.path.iter().rev() {
            if self.resolver.resolve(&relative_name, &context).as_ref() == Some(name) {
                return Some((normalized_name, relative_name));
            }
            let mut path = vec![component.clone()];
            path.extend(relative_name.namespace.path.iter().cloned());
            relative_name = TypeName::new(
                Namespace {
                    absolute: false,
                    path,
                },
                &name.name,
            );
        }
        let relative = name.relative();
        if self.resolver.resolve(&relative, &context).as_ref() == Some(name)
            && self.resolve_used_name(&relative).is_none()
        {
            Some((normalized_name, relative))
        } else {
            Some((normalized_name, name.clone()))
        }
    }

    // Why not resolve the root namespace to itself: Namespace#to_type_name of the rbs
    // gem raises for the root, so completing after a leading `::` raises in Steep.
    fn resolve_namespace(&self, namespace: &Namespace) -> Result<Namespace, String> {
        if namespace.is_empty() {
            return Err("RuntimeError".to_string());
        }
        Ok(self
            .resolver
            .resolve(&namespace.to_type_name(), &self.context_names())
            .map(|name| name.to_namespace())
            .unwrap_or_else(|| namespace.clone()))
    }

    pub fn find_type_names(&self, prefix: Option<&Prefix>) -> Result<Vec<TypeName>, String> {
        Ok(match prefix {
            Some(Prefix::RawIdent { ident }) => {
                let ident = ident.to_lowercase();
                self.each_type_name()
                    .into_iter()
                    .filter(|type_name| {
                        type_name
                            .namespace
                            .path
                            .iter()
                            .chain(std::iter::once(&type_name.name))
                            .any(|component| component.to_lowercase().contains(&ident))
                    })
                    .collect()
            }
            Some(Prefix::NamespacedIdent {
                namespace, ident, ..
            }) => {
                let absolute_namespace = if namespace.is_empty() {
                    Namespace::root()
                } else {
                    self.resolve_namespace(namespace)?
                };
                let ident = ident.to_lowercase();
                self.each_type_name()
                    .into_iter()
                    .filter(|name| {
                        name.namespace == absolute_namespace
                            && name.name.to_lowercase().contains(&ident)
                    })
                    .collect()
            }
            Some(Prefix::Namespace { namespace, .. }) => {
                let absolute_namespace = self.resolve_namespace(namespace)?;
                self.each_type_name()
                    .into_iter()
                    .filter(|name| name.namespace == absolute_namespace)
                    .collect()
            }
            None => {
                let namespaces: HashSet<Namespace> = self.each_outer_module().into_iter().collect();
                self.each_type_name()
                    .into_iter()
                    .filter(|name| namespaces.contains(&name.namespace) || !name.is_absolute())
                    .collect()
            }
        })
    }
}
