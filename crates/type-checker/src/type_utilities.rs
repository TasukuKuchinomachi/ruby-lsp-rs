use rbs_environment::{ClassKind, Environment, Literal, Type};

pub(crate) fn members_of(target: &Type) -> Vec<Type> {
    match target {
        Type::Union(members) => members.iter().flat_map(members_of).collect(),
        other => vec![other.clone()],
    }
}

pub(crate) fn can_be_nil(target: &Type) -> bool {
    match target {
        Type::Nil => true,
        Type::Union(members) => members.iter().any(can_be_nil),
        _ => false,
    }
}

pub(crate) fn remove_nil(target: &Type) -> Type {
    match target {
        Type::Union(members) => Type::union(
            members
                .iter()
                .filter(|member| !matches!(member, Type::Nil))
                .map(remove_nil)
                .collect(),
        ),
        Type::Nil => Type::Bottom,
        other => other.clone(),
    }
}

fn is_falsy_member(member: &Type) -> bool {
    matches!(member, Type::Nil | Type::Literal(Literal::False))
        || matches!(member, Type::ClassInstance { name, .. } if name == "NilClass" || name == "FalseClass")
}

fn may_be_falsy_member(member: &Type) -> bool {
    is_falsy_member(member)
        || matches!(
            member,
            Type::Bool
                | Type::Untyped
                | Type::Top
                | Type::Void
                | Type::Variable(_)
                | Type::Alias { .. }
                | Type::Interface { .. }
        )
        || matches!(member, Type::ClassInstance { name, .. } if name == "Object" || name == "BasicObject")
}

pub(crate) fn truthy_part(target: &Type) -> Type {
    if target.is_untyped() {
        return Type::Untyped;
    }
    Type::union(
        members_of(target)
            .into_iter()
            .filter(|member| !is_falsy_member(member))
            .collect(),
    )
}

pub(crate) fn falsy_part(target: &Type) -> Type {
    if target.is_untyped() {
        return Type::Untyped;
    }
    let members: Vec<Type> = members_of(target)
        .into_iter()
        .filter(may_be_falsy_member)
        .map(|member| {
            if matches!(member, Type::ClassInstance { ref name, .. } if name == "NilClass") {
                Type::Nil
            } else {
                member
            }
        })
        .collect();
    Type::union(members)
}

pub(crate) fn can_be_falsy(target: &Type) -> bool {
    members_of(target).iter().any(may_be_falsy_member)
}

pub(crate) fn contains_uncertain_part(target: &Type) -> bool {
    let mut found = false;
    target.walk(&mut |member| {
        if matches!(
            member,
            Type::Untyped
                | Type::Variable(_)
                | Type::SelfType
                | Type::Instance
                | Type::Class
                | Type::Top
        ) {
            found = true;
        }
    });
    found
}

pub(crate) fn contains_variable_in(target: &Type, names: &[String]) -> bool {
    let mut found = false;
    target.walk(&mut |member| {
        if let Type::Variable(name) = member
            && names.contains(name)
        {
            found = true;
        }
    });
    found
}

pub(crate) fn instance_type_of_class(environment: &Environment, name: &str) -> Type {
    let declared_name = environment.resolve_class_name(name).unwrap_or(name);
    let arguments = environment
        .classes
        .get(declared_name)
        .map(|declaration| {
            declaration
                .type_parameters
                .iter()
                .map(|_| Type::Untyped)
                .collect()
        })
        .unwrap_or_default();
    Type::ClassInstance {
        name: declared_name.to_string(),
        arguments,
    }
}

pub(crate) fn self_instance_type(environment: &Environment, name: &str) -> Type {
    let arguments = environment
        .classes
        .get(name)
        .map(|declaration| {
            declaration
                .type_parameters
                .iter()
                .map(|type_parameter| Type::Variable(type_parameter.name.clone()))
                .collect()
        })
        .unwrap_or_default();
    Type::ClassInstance {
        name: name.to_string(),
        arguments,
    }
}

/// Steep's `instance_type` of a module body: `Object`, the module's self types and the module itself
pub(crate) fn module_instance_self_type(environment: &Environment, name: &str) -> Option<Type> {
    let declaration = environment.classes.get(name)?;
    if declaration.kind != ClassKind::Module {
        return None;
    }
    let mut members = vec![Type::instance("Object")];
    for self_type in &declaration.module_self_types {
        let member = if environment.interfaces.contains_key(&self_type.name) {
            Type::Interface {
                name: self_type.name.clone(),
                arguments: self_type.arguments.clone(),
            }
        } else {
            Type::ClassInstance {
                name: self_type.name.clone(),
                arguments: self_type.arguments.clone(),
            }
        };
        if !members.contains(&member) {
            members.push(member);
        }
    }
    let module_instance = self_instance_type(environment, name);
    if !members.contains(&module_instance) {
        members.push(module_instance);
    }
    Some(Type::Intersection(members))
}

pub(crate) fn instance_type_of_class_value(
    environment: &Environment,
    class_value: &Type,
) -> Option<Type> {
    match class_value {
        Type::ClassSingleton { name } => Some(instance_type_of_class(environment, name)),
        Type::Union(members) => {
            let instances: Option<Vec<Type>> = members
                .iter()
                .map(|member| instance_type_of_class_value(environment, member))
                .collect();
            instances.map(Type::union)
        }
        _ => None,
    }
}

pub(crate) fn literal_class_name(literal: &Literal) -> &'static str {
    match literal {
        Literal::Integer(_) => "Integer",
        Literal::String(_) => "String",
        Literal::Symbol(_) => "Symbol",
        Literal::True => "TrueClass",
        Literal::False => "FalseClass",
    }
}

pub(crate) fn array_of(element: Type) -> Type {
    Type::generic("Array", vec![element])
}

pub(crate) fn hash_of(key: Type, value: Type) -> Type {
    Type::generic("Hash", vec![key, value])
}

pub(crate) fn element_type(target: &Type) -> Type {
    match target {
        Type::ClassInstance { name, arguments }
            if name == "Array" || name == "Set" || name == "Range" || name == "Enumerator" =>
        {
            arguments.first().cloned().unwrap_or(Type::Untyped)
        }
        Type::ClassInstance { name, arguments } if name == "Hash" && arguments.len() == 2 => {
            Type::Tuple(arguments.clone())
        }
        Type::Tuple(elements) => Type::union(elements.clone()),
        Type::Nil => Type::Bottom,
        _ => Type::Untyped,
    }
}

pub(crate) fn is_bottom(target: &Type) -> bool {
    matches!(target, Type::Bottom)
}
