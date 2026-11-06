use crate::{
    ast::{Type, TypeName},
    collections::{HashMap, IndexMap, IndexSet},
    environment::Environment,
    errors::RbsError,
    location::TypeLocation,
    substitution::Substitution,
};

#[derive(Clone, Debug)]
pub struct NonregularDiagnostic {
    pub type_name: TypeName,
    pub nonregular_type: Type,
}

pub struct TypeAliasRegularity {
    diagnostics: HashMap<TypeName, NonregularDiagnostic>,
}

pub fn expand_alias(
    environment: &Environment,
    type_name: &TypeName,
    args: &[Type],
) -> Result<Type, RbsError> {
    let type_name = environment
        .normalize_type_name(type_name)
        .unwrap_or_else(|| type_name.clone());
    let entry = environment
        .type_alias_declarations
        .get(&type_name)
        .ok_or_else(|| RbsError::RuntimeError(format!("Unknown alias name: {type_name}")))?;
    environment.ensure_namespace(&type_name.namespace, entry.declaration.location.as_ref())?;
    let params: Vec<String> = entry
        .declaration
        .type_parameters
        .iter()
        .map(|parameter| parameter.name.clone())
        .collect();
    if params.len() != args.len() {
        return Err(RbsError::RuntimeError(format!(
            "Invalid type application: type = {type_name}"
        )));
    }
    if params.is_empty() {
        return Ok(entry.declaration.type_.clone());
    }
    Ok(entry
        .declaration
        .type_
        .substitute(&Substitution::build(&params, args)))
}

fn each_alias_type<'type_>(type_: &'type_ Type, types: &mut Vec<&'type_ Type>) {
    if let Type::Alias { .. } = type_ {
        types.push(type_);
    }
    for child in type_.each_type() {
        each_alias_type(child, types);
    }
}

fn alias_types(type_: &Type) -> Vec<&Type> {
    let mut types = Vec::new();
    each_alias_type(type_, &mut types);
    types
}

fn compatible_args(left: &[Type], right: &[Type]) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(left, right)| {
            matches!(left, Type::Any) || matches!(right, Type::Any) || left == right
        })
}

struct Tarjan<'graph> {
    graph: &'graph IndexMap<TypeName, Vec<TypeName>>,
    index: usize,
    indexes: HashMap<TypeName, usize>,
    lowlinks: HashMap<TypeName, usize>,
    stack: Vec<TypeName>,
    on_stack: IndexSet<TypeName>,
    components: Vec<Vec<TypeName>>,
}

impl Tarjan<'_> {
    fn visit(&mut self, node: &TypeName) {
        self.indexes.insert(node.clone(), self.index);
        self.lowlinks.insert(node.clone(), self.index);
        self.index += 1;
        self.stack.push(node.clone());
        self.on_stack.insert(node.clone());
        for child in self.graph.get(node).cloned().unwrap_or_default() {
            if !self.indexes.contains_key(&child) {
                self.visit(&child);
                let low = self.lowlinks[node].min(self.lowlinks[&child]);
                self.lowlinks.insert(node.clone(), low);
            } else if self.on_stack.contains(&child) {
                let low = self.lowlinks[node].min(self.indexes[&child]);
                self.lowlinks.insert(node.clone(), low);
            }
        }
        if self.lowlinks[node] == self.indexes[node] {
            let mut component = Vec::new();
            while let Some(member) = self.stack.pop() {
                self.on_stack.shift_remove(&member);
                let done = member == *node;
                component.push(member);
                if done {
                    break;
                }
            }
            component.reverse();
            self.components.push(component);
        }
    }
}

impl TypeAliasRegularity {
    pub fn validate(environment: &Environment) -> Result<TypeAliasRegularity, RbsError> {
        let mut graph: IndexMap<TypeName, Vec<TypeName>> = IndexMap::default();
        for entry in environment.type_alias_declarations.values() {
            let Some(name) = environment.normalize_type_name(&entry.name) else {
                continue;
            };
            let arguments: Vec<Type> = entry
                .declaration
                .type_parameters
                .iter()
                .map(|_| Type::Any)
                .collect();
            let expanded = expand_alias(environment, &name, &arguments)?;
            let children = alias_types(&expanded)
                .into_iter()
                .filter_map(|type_| match type_ {
                    Type::Alias { name, .. } => environment.normalize_type_name(name),
                    _ => None,
                })
                .collect();
            graph.insert(name, children);
        }
        let mut tarjan = Tarjan {
            graph: &graph,
            index: 0,
            indexes: HashMap::default(),
            lowlinks: HashMap::default(),
            stack: Vec::new(),
            on_stack: IndexSet::default(),
            components: Vec::new(),
        };
        for node in graph.keys() {
            if !tarjan.indexes.contains_key(node) {
                tarjan.visit(node);
            }
        }
        let components = tarjan.components;
        let mut regularity = TypeAliasRegularity {
            diagnostics: HashMap::default(),
        };
        for names in components {
            for name in &names {
                let entry = &environment.type_alias_declarations[name];
                if entry.declaration.type_parameters.is_empty() {
                    continue;
                }
                let alias_type = Type::Alias {
                    name: name.clone(),
                    args: entry
                        .declaration
                        .type_parameters
                        .iter()
                        .map(|parameter| Type::Variable(parameter.name.clone()))
                        .collect(),
                    location: TypeLocation::default(),
                };
                regularity.validate_alias_type(
                    environment,
                    &alias_type,
                    &names,
                    &mut HashMap::default(),
                )?;
                break;
            }
        }
        Ok(regularity)
    }

    fn validate_alias_type(
        &mut self,
        environment: &Environment,
        alias_type: &Type,
        names: &[TypeName],
        types: &mut HashMap<TypeName, Type>,
    ) -> Result<(), RbsError> {
        let Type::Alias { name, args, .. } = alias_type else {
            return Ok(());
        };
        let Some(alias_name) = environment.normalize_type_name(name) else {
            return Ok(());
        };
        if !names.contains(&alias_name) {
            return Ok(());
        }
        if let Some(Type::Alias {
            args: existing_args,
            ..
        }) = types.get(&alias_name)
        {
            if !compatible_args(existing_args, args) {
                self.diagnostics
                    .entry(alias_name)
                    .or_insert_with(|| NonregularDiagnostic {
                        type_name: name.clone(),
                        nonregular_type: alias_type.clone(),
                    });
            }
            return Ok(());
        }
        types.insert(name.clone(), alias_type.clone());
        let expanded = expand_alias(environment, &alias_name, args)?;
        for type_ in alias_types(&expanded) {
            self.validate_alias_type(environment, type_, names, types)?;
        }
        Ok(())
    }

    pub fn nonregular(
        &self,
        environment: &Environment,
        type_name: &TypeName,
    ) -> Option<&NonregularDiagnostic> {
        let name = environment
            .normalize_type_name(type_name)
            .unwrap_or_else(|| type_name.clone());
        self.diagnostics.get(&name)
    }
}
