use crate::{
    ast::{Type, TypeName},
    collections::{HashMap, HashSet, IndexSet},
    environment::Environment,
};

pub struct TypeAliasDependency {
    direct_dependencies: HashMap<TypeName, IndexSet<TypeName>>,
    dependencies: HashMap<TypeName, HashSet<TypeName>>,
}

fn direct_dependency(environment: &Environment, type_: &Type, result: &mut IndexSet<TypeName>) {
    match type_ {
        Type::Union(_) | Type::Intersection(_) | Type::Optional(_) => {
            for nested in type_.each_type() {
                direct_dependency(environment, nested, result);
            }
        }
        Type::Alias { name, .. } => {
            result.insert(
                environment
                    .normalize_type_name(name)
                    .unwrap_or_else(|| name.clone()),
            );
        }
        _ => {}
    }
}

impl TypeAliasDependency {
    pub fn new(environment: &Environment) -> TypeAliasDependency {
        let mut direct_dependencies = HashMap::default();
        for (name, entry) in &environment.type_alias_declarations {
            let mut result = IndexSet::default();
            direct_dependency(environment, &entry.declaration.type_, &mut result);
            direct_dependencies.insert(name.clone(), result);
        }
        let mut dependency = TypeAliasDependency {
            direct_dependencies,
            dependencies: HashMap::default(),
        };
        let names: Vec<TypeName> = environment
            .type_alias_declarations
            .keys()
            .cloned()
            .collect();
        for name in &names {
            dependency
                .dependencies
                .insert(name.clone(), HashSet::default());
        }
        for name in &names {
            dependency.dependency(name, name, false);
        }
        dependency
    }

    fn dependency(&mut self, start: &TypeName, vertex: &TypeName, nested: bool) {
        if start == vertex {
            if self.direct_dependencies[start].contains(vertex) || nested {
                self.dependencies
                    .get_mut(start)
                    .expect("a dependency set")
                    .insert(vertex.clone());
            }
        } else {
            self.dependencies
                .get_mut(start)
                .expect("a dependency set")
                .insert(vertex.clone());
        }
        let Some(children) = self.direct_dependencies.get(vertex).cloned() else {
            return;
        };
        for type_name in children {
            if !self.dependencies[start].contains(&type_name) {
                self.dependency(start, &type_name, *start == type_name);
            }
        }
    }

    pub fn is_circular_definition(&self, environment: &Environment, alias_name: &TypeName) -> bool {
        let name = environment
            .normalize_type_name(alias_name)
            .unwrap_or_else(|| alias_name.clone());
        self.dependencies
            .get(&name)
            .is_some_and(|dependencies| dependencies.contains(&name))
    }
}
