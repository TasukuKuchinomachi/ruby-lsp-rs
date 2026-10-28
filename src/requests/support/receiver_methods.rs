use std::collections::HashSet;

use rbs_environment::{
    Environment, Literal, MethodDefinition, MethodType, SingletonAncestor, Substitution, Type, Visibility,
};

use crate::type_checking;

const MAXIMUM_ALIAS_DEPTH: usize = 16;

/// A method declared in RBS for one receiver type, with the type arguments of the receiver substituted
pub struct ReceiverMethod<'a> {
    pub definition: &'a MethodDefinition,
    pub overloads: Vec<MethodType>,
}

impl ReceiverMethod<'_> {
    pub fn is_private(&self) -> bool {
        self.definition.visibility == Visibility::Private
    }
}

/// The class instances, class singletons and interfaces whose methods can be called on `receiver_type`. The `nil` of
/// an optional type is left out even without `&.`, since `user.` on `User?` is about `User` far more often than about
/// `NilClass`
pub fn receiver_members(environment: &Environment, receiver_type: &Type) -> Vec<Type> {
    let mut members = Vec::new();
    collect_members(environment, receiver_type, 0, &mut members);
    if members.len() > 1 {
        members.retain(|member| !matches!(member, Type::ClassInstance { name, .. } if name == "NilClass"));
    }
    members
}

pub fn without_nil(receiver_type: &Type) -> Option<Type> {
    match receiver_type {
        Type::Nil => None,
        Type::Union(members) => {
            let members: Vec<Type> = members.iter().filter_map(without_nil).collect();
            (!members.is_empty()).then(|| Type::union(members))
        }
        other => Some(other.clone()),
    }
}

fn collect_members(environment: &Environment, collected_type: &Type, depth: usize, members: &mut Vec<Type>) {
    let mut push = |member: Type| {
        if !members.contains(&member) {
            members.push(member);
        }
    };
    let class = |name: &str| Type::instance(name);
    match collected_type {
        Type::ClassInstance { name, arguments } => {
            push(Type::ClassInstance { name: without_root_prefix(name), arguments: arguments.clone() })
        }
        Type::ClassSingleton { name } => push(Type::ClassSingleton { name: without_root_prefix(name) }),
        Type::Interface { name, arguments } => {
            push(Type::Interface { name: without_root_prefix(name), arguments: arguments.clone() })
        }
        Type::Alias { name, arguments } => {
            if depth < MAXIMUM_ALIAS_DEPTH
                && let Some(expanded) = environment.expand_alias(name, arguments)
            {
                collect_members(environment, &expanded, depth + 1, members);
            }
        }
        Type::Nil => push(class("NilClass")),
        Type::Bool => {
            push(class("TrueClass"));
            push(class("FalseClass"));
        }
        Type::Literal(literal) => push(class(match literal {
            Literal::Integer(_) => "Integer",
            Literal::String(_) => "String",
            Literal::Symbol(_) => "Symbol",
            Literal::True => "TrueClass",
            Literal::False => "FalseClass",
        })),
        Type::Tuple(elements) => push(Type::generic("Array", vec![union_or_untyped(elements.clone())])),
        Type::Record(fields) => push(Type::generic(
            "Hash",
            vec![
                Type::instance("Symbol"),
                union_or_untyped(fields.iter().map(|field| field.field_type.clone()).collect()),
            ],
        )),
        Type::Proc(_) => push(class("Proc")),
        Type::Union(union_members) | Type::Intersection(union_members) => {
            for member in union_members {
                collect_members(environment, member, depth, members);
            }
        }
        _ => {}
    }
}

fn union_or_untyped(types: Vec<Type>) -> Type {
    if types.is_empty() { Type::Untyped } else { Type::union(types) }
}

fn without_root_prefix(name: &str) -> String {
    name.strip_prefix("::").unwrap_or(name).to_string()
}

/// Name of the class whose methods the Ruby index lists for `member`, e.g. `Foo::<Class:Foo>` for `singleton(Foo)`
pub fn index_receiver_name(member: &Type) -> Option<String> {
    match member {
        Type::ClassInstance { name, .. } => Some(name.clone()),
        Type::ClassSingleton { name } => {
            let unqualified_name = name.rsplit("::").next().unwrap_or(name);
            Some(format!("{name}::<Class:{unqualified_name}>"))
        }
        _ => None,
    }
}

/// Every method declared in RBS for `member`, including those of its ancestors, sorted by name
pub fn methods<'a>(environment: &'a Environment, member: &Type) -> Vec<(String, ReceiverMethod<'a>)> {
    method_names(environment, member)
        .into_iter()
        .filter_map(|name| {
            let method = find_method(environment, member, &name)?;
            Some((name, method))
        })
        .collect()
}

fn method_names(environment: &Environment, member: &Type) -> Vec<String> {
    match member {
        Type::ClassInstance { name, .. } | Type::Interface { name, .. } => environment.instance_method_names(name),
        Type::ClassSingleton { name } => {
            let mut names = HashSet::new();
            for step in environment.singleton_ancestors(name) {
                match step {
                    SingletonAncestor::Singleton(class_name) => {
                        if let Some(declaration) = environment.classes.get(&class_name) {
                            names.extend(declaration.singleton_methods.keys().cloned());
                        }
                    }
                    SingletonAncestor::Instance(ancestor) => {
                        if let Some(declaration) = environment.classes.get(&ancestor.name) {
                            names.extend(declaration.instance_methods.keys().cloned());
                        } else if let Some(declaration) = environment.interfaces.get(&ancestor.name) {
                            names.extend(declaration.methods.keys().cloned());
                        }
                    }
                }
            }
            let mut names: Vec<String> = names.into_iter().collect();
            names.sort();
            names
        }
        _ => Vec::new(),
    }
}

pub fn find_method<'a>(environment: &'a Environment, member: &Type, method_name: &str) -> Option<ReceiverMethod<'a>> {
    let (resolved, substitution) = match member {
        Type::ClassInstance { name, arguments } => {
            let resolved = environment.find_instance_method(name, method_name)?;
            (resolved, instance_substitution(environment, member, name, arguments))
        }
        Type::Interface { name, arguments } => {
            let resolved = environment.find_interface_method(name, method_name)?;
            let mut substitution = Substitution { self_type: Some(member.clone()), ..Substitution::default() };
            if let Some(declaration) = environment.interfaces.get(name) {
                for (index, type_parameter) in declaration.type_parameters.iter().enumerate() {
                    let argument = arguments.get(index).cloned().unwrap_or(Type::Untyped);
                    substitution.variables.insert(type_parameter.name.clone(), argument);
                }
            }
            (resolved, substitution)
        }
        Type::ClassSingleton { name } => {
            let resolved = environment.find_singleton_method(name, method_name)?;
            let instance_type = Type::ClassInstance {
                name: name.clone(),
                arguments: type_parameter_names(environment, name).into_iter().map(Type::Variable).collect(),
            };
            let substitution = Substitution {
                self_type: Some(member.clone()),
                instance_type: Some(instance_type),
                class_type: Some(member.clone()),
                ..Substitution::default()
            };
            (resolved, substitution)
        }
        _ => return None,
    };
    let overloads = resolved.overloads.iter().map(|overload| overload.substitute(&substitution)).collect();
    Some(ReceiverMethod { definition: resolved.definition, overloads })
}

fn instance_substitution(environment: &Environment, member: &Type, name: &str, arguments: &[Type]) -> Substitution {
    let declared_name = environment.resolve_class_name(name).unwrap_or(name);
    let mut substitution = Substitution {
        self_type: Some(member.clone()),
        instance_type: Some(member.clone()),
        class_type: Some(Type::ClassSingleton { name: declared_name.to_string() }),
        ..Substitution::default()
    };
    for (index, type_parameter_name) in type_parameter_names(environment, declared_name).into_iter().enumerate() {
        let argument = arguments.get(index).cloned().unwrap_or(Type::Untyped);
        substitution.variables.insert(type_parameter_name, argument);
    }
    substitution
}

fn type_parameter_names(environment: &Environment, class_name: &str) -> Vec<String> {
    environment
        .classes
        .get(class_name)
        .map(|declaration| declaration.type_parameters.iter().map(|type_parameter| type_parameter.name.clone()).collect())
        .unwrap_or_default()
}

/// Documentation of the method `method_name` of the receiver type written as RBS, e.g. `Array[Integer]`
pub fn method_markdown(environment: &Environment, receiver_type: &str, method_name: &str) -> Option<String> {
    let member = receiver_members(environment, &rbs_environment::parse_type(receiver_type).ok()?).into_iter().next()?;
    let method = find_method(environment, &member, method_name)?;
    let singleton = matches!(member, Type::ClassSingleton { .. });
    Some(type_checking::method_signature_markdown(
        &method.definition.owner,
        method_name,
        singleton,
        &method.overloads,
        method.definition.comment.as_deref(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_nil_from_optional_and_union_types() {
        let optional = Type::optional(Type::instance("String"));
        assert_eq!(without_nil(&optional), Some(Type::instance("String")));
        assert_eq!(without_nil(&Type::Nil), None);
        let union = Type::Union(vec![Type::instance("Integer"), Type::Nil, Type::instance("String")]);
        assert_eq!(without_nil(&union), Some(Type::union(vec![Type::instance("Integer"), Type::instance("String")])));
    }

    #[test]
    fn collects_the_classes_of_literals_tuples_and_records() {
        let environment = Environment::default();
        let members = receiver_members(
            &environment,
            &Type::Union(vec![Type::Literal(Literal::Integer("1".to_string())), Type::Bool, Type::Nil]),
        );
        assert_eq!(members, vec![Type::instance("Integer"), Type::instance("TrueClass"), Type::instance("FalseClass")]);
        assert_eq!(
            receiver_members(&environment, &Type::Tuple(vec![Type::instance("String")])),
            vec![Type::generic("Array", vec![Type::instance("String")])]
        );
    }

    #[test]
    fn names_singleton_classes_like_the_index() {
        let singleton = Type::ClassSingleton { name: "Foo::Bar".to_string() };
        assert_eq!(index_receiver_name(&singleton).as_deref(), Some("Foo::Bar::<Class:Bar>"));
        assert_eq!(index_receiver_name(&Type::instance("Foo")).as_deref(), Some("Foo"));
    }
}
