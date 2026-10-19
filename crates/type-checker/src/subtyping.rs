use rbs_environment::{Environment, Literal, Type};

use crate::type_utilities::{array_of, hash_of, literal_class_name};

/// Result of a subtype check. Only `No` may lead to a diagnostic, so anything not decidable is `Unknown`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Certainty {
    Yes,
    No,
    Unknown,
}

impl Certainty {
    fn all(results: impl IntoIterator<Item = Certainty>) -> Certainty {
        let mut combined = Certainty::Yes;
        for result in results {
            match result {
                Certainty::No => return Certainty::No,
                Certainty::Unknown => combined = Certainty::Unknown,
                Certainty::Yes => {}
            }
        }
        combined
    }

    fn any(results: impl IntoIterator<Item = Certainty>) -> Certainty {
        let mut combined = Certainty::No;
        for result in results {
            match result {
                Certainty::Yes => return Certainty::Yes,
                Certainty::Unknown => combined = Certainty::Unknown,
                Certainty::No => {}
            }
        }
        combined
    }
}

const MAXIMUM_DEPTH: usize = 24;

pub(crate) fn is_subtype(environment: &Environment, subtype: &Type, supertype: &Type) -> Certainty {
    let rules = Rules {
        strict_collections: false,
    };
    check(environment, rules, subtype, supertype, 0)
}

/// Like `is_subtype`, but `Array` and `Hash` are certainly not tuples or records, as in Steep. The default leaves that
/// undecided because this checker gives a literal the tuple or record type Steep would infer from the expected type
/// in fewer places than Steep does
pub(crate) fn is_subtype_with_strict_collections(
    environment: &Environment,
    subtype: &Type,
    supertype: &Type,
) -> Certainty {
    let rules = Rules {
        strict_collections: true,
    };
    check(environment, rules, subtype, supertype, 0)
}

#[derive(Clone, Copy)]
struct Rules {
    strict_collections: bool,
}

pub(crate) fn is_fully_known_class(environment: &Environment, name: &str) -> bool {
    let Some(declared_name) = environment.resolve_class_name(name) else {
        return false;
    };
    if !environment.classes.contains_key(declared_name) {
        return false;
    }
    environment.ancestors(declared_name).iter().all(|ancestor| {
        environment
            .resolve_class_name(&ancestor.name)
            .is_some_and(|ancestor_name| environment.classes.contains_key(ancestor_name))
    })
}

fn check(
    environment: &Environment,
    rules: Rules,
    subtype: &Type,
    supertype: &Type,
    depth: usize,
) -> Certainty {
    if depth > MAXIMUM_DEPTH {
        return Certainty::Unknown;
    }
    if subtype == supertype {
        return Certainty::Yes;
    }
    let next = depth + 1;
    match (subtype, supertype) {
        (_, Type::Untyped | Type::Top | Type::Void) => Certainty::Yes,
        (Type::Untyped | Type::Bottom, _) => Certainty::Yes,
        (Type::Alias { name, arguments }, _) => match environment.expand_alias(name, arguments) {
            Some(expanded) => check(environment, rules, &expanded, supertype, next),
            None => Certainty::Unknown,
        },
        (_, Type::Alias { name, arguments }) => match environment.expand_alias(name, arguments) {
            Some(expanded) => check(environment, rules, subtype, &expanded, next),
            None => Certainty::Unknown,
        },
        (Type::Union(members), _) => Certainty::all(
            members
                .iter()
                .map(|member| check(environment, rules, member, supertype, next)),
        ),
        (_, Type::Union(members)) => Certainty::any(
            members
                .iter()
                .map(|member| check(environment, rules, subtype, member, next)),
        ),
        (_, Type::Intersection(members)) => Certainty::all(
            members
                .iter()
                .map(|member| check(environment, rules, subtype, member, next)),
        ),
        (Type::Intersection(members), _) => {
            if members
                .iter()
                .any(|member| check(environment, rules, member, supertype, next) == Certainty::Yes)
            {
                Certainty::Yes
            } else {
                Certainty::Unknown
            }
        }
        (Type::Interface { name, .. }, Type::Interface { name: supertype_name, .. })
            if environment.interfaces.contains_key(name) && name != supertype_name =>
        {
            let mut method_names = Vec::new();
            collect_interface_method_names(environment, name, &mut method_names, 0);
            let mut required_names = Vec::new();
            collect_interface_method_names(environment, supertype_name, &mut required_names, 0);
            if required_names.is_empty() {
                Certainty::Unknown
            } else if required_names.iter().all(|required| method_names.contains(required)) {
                Certainty::Yes
            } else {
                Certainty::No
            }
        }
        // Steep finds no super type of an interface but other interfaces, not even `Object`
        (Type::Interface { name, .. }, Type::ClassInstance { .. } | Type::ClassSingleton { .. })
            if environment.interfaces.contains_key(name) =>
        {
            Certainty::No
        }
        (
            Type::Void
            | Type::Top
            | Type::Variable(_)
            | Type::SelfType
            | Type::Instance
            | Type::Class
            | Type::Interface { .. },
            _,
        ) => Certainty::Unknown,
        (_, Type::Bottom) => Certainty::Unknown,
        (_, Type::Variable(_) | Type::SelfType | Type::Instance | Type::Class) => {
            Certainty::Unknown
        }
        (_, Type::Interface { name, .. }) => interface_check(environment, subtype, name),
        (Type::Literal(left), Type::Literal(right)) => {
            if std::mem::discriminant(left) == std::mem::discriminant(right)
                || is_boolean_literal(left) && is_boolean_literal(right)
            {
                if left == right {
                    Certainty::Yes
                } else {
                    Certainty::No
                }
            } else {
                Certainty::No
            }
        }
        (_, Type::Nil) => match subtype {
            Type::ClassInstance { name, .. } if name == "NilClass" => Certainty::Yes,
            Type::ClassInstance { name, .. }
                if is_fully_known_class(environment, name) && !is_module(environment, name) =>
            {
                Certainty::No
            }
            Type::Literal(_)
            | Type::Bool
            | Type::Tuple(_)
            | Type::Record(_)
            | Type::ClassSingleton { .. }
            | Type::Proc(_) => Certainty::No,
            _ => Certainty::Unknown,
        },
        (Type::Nil, Type::Literal(_)) => Certainty::No,
        (_, Type::Literal(_)) => match subtype {
            Type::ClassInstance { name, .. } if is_fully_known_class(environment, name) => {
                Certainty::No
            }
            Type::Bool
            | Type::Tuple(_)
            | Type::Record(_)
            | Type::Proc(_)
            | Type::ClassSingleton { .. } => Certainty::No,
            _ => Certainty::Unknown,
        },
        (Type::Literal(literal), _) => check(
            environment,
            rules,
            &Type::instance(literal_class_name(literal)),
            supertype,
            next,
        ),
        (Type::Bool, Type::Bool) => Certainty::Yes,
        (Type::Nil, Type::Bool) => Certainty::No,
        (_, Type::Bool) => match subtype {
            Type::ClassInstance { name, .. } if name == "TrueClass" || name == "FalseClass" => {
                Certainty::Yes
            }
            Type::ClassInstance { name, .. }
                if is_fully_known_class(environment, name) && !is_module(environment, name) =>
            {
                Certainty::No
            }
            Type::ClassSingleton { .. } | Type::Tuple(_) | Type::Record(_) => Certainty::No,
            _ => Certainty::Unknown,
        },
        (Type::Bool, _) => Certainty::all([
            check(
                environment,
                rules,
                &Type::instance("TrueClass"),
                supertype,
                next,
            ),
            check(
                environment,
                rules,
                &Type::instance("FalseClass"),
                supertype,
                next,
            ),
        ]),
        (Type::Nil, _) => check(
            environment,
            rules,
            &Type::instance("NilClass"),
            supertype,
            next,
        ),
        (Type::Tuple(left), Type::Tuple(right)) => {
            if left.len() != right.len() {
                Certainty::No
            } else {
                Certainty::all(
                    left.iter()
                        .zip(right)
                        .map(|(left, right)| check(environment, rules, left, right, next)),
                )
            }
        }
        (Type::Tuple(elements), _) => check(
            environment,
            rules,
            &array_of(Type::union(elements.clone())),
            supertype,
            next,
        ),
        (Type::Record(fields), _) => {
            let value_type = Type::union(
                fields
                    .iter()
                    .map(|field| field.field_type.clone())
                    .collect(),
            );
            check(
                environment,
                rules,
                &hash_of(Type::instance("Symbol"), value_type),
                supertype,
                next,
            )
        }
        (Type::Proc(subtype_proc), Type::Proc(supertype_proc)) => proc_check(
            environment,
            rules,
            &subtype_proc.function,
            &supertype_proc.function,
            next,
        ),
        (Type::Proc(_), _) => check(environment, rules, &Type::instance("Proc"), supertype, next),
        (_, Type::Tuple(_) | Type::Record(_) | Type::Proc(_)) => match subtype {
            Type::ClassInstance { name, .. }
                if rules.strict_collections
                    && (name == "Array" && matches!(supertype, Type::Tuple(_))
                        || name == "Hash" && matches!(supertype, Type::Record(_))) =>
            {
                Certainty::No
            }
            Type::ClassInstance { name, .. }
                if name == "Array" || name == "Hash" || name == "Proc" =>
            {
                Certainty::Unknown
            }
            Type::ClassInstance { name, .. }
                if is_fully_known_class(environment, name) && !is_module(environment, name) =>
            {
                Certainty::No
            }
            _ => Certainty::Unknown,
        },
        (Type::ClassSingleton { name: left }, Type::ClassSingleton { name: right }) => {
            if is_ancestor(environment, left, right) == Some(true) {
                Certainty::Yes
            } else if is_fully_known_class(environment, left) {
                Certainty::No
            } else {
                Certainty::Unknown
            }
        }
        (
            Type::ClassSingleton { name },
            Type::ClassInstance {
                name: supertype_name,
                ..
            },
        ) => {
            let metaclass = if is_module(environment, name) {
                "Module"
            } else {
                "Class"
            };
            nominal(
                environment,
                rules,
                metaclass,
                &[],
                supertype_name,
                &[],
                next,
            )
        }
        (Type::ClassInstance { name, .. }, Type::ClassSingleton { .. }) => {
            if name == "Class" || name == "Module" || !is_fully_known_class(environment, name) {
                Certainty::Unknown
            } else {
                Certainty::No
            }
        }
        (
            Type::ClassInstance { name, arguments },
            Type::ClassInstance {
                name: supertype_name,
                arguments: supertype_arguments,
            },
        ) => nominal(
            environment,
            rules,
            name,
            arguments,
            supertype_name,
            supertype_arguments,
            next,
        ),
    }
}

fn is_boolean_literal(literal: &Literal) -> bool {
    matches!(literal, Literal::True | Literal::False)
}

fn is_module(environment: &Environment, name: &str) -> bool {
    environment
        .resolve_class_name(name)
        .and_then(|declared_name| environment.classes.get(declared_name))
        .is_some_and(|declaration| declaration.kind == rbs_environment::ClassKind::Module)
}

fn is_ancestor(environment: &Environment, name: &str, ancestor_name: &str) -> Option<bool> {
    let declared_name = environment.resolve_class_name(name)?;
    let declared_ancestor_name = environment
        .resolve_class_name(ancestor_name)
        .unwrap_or(ancestor_name);
    if declared_name == declared_ancestor_name {
        return Some(true);
    }
    Some(
        environment
            .ancestors(declared_name)
            .iter()
            .any(|ancestor| ancestor.name == declared_ancestor_name),
    )
}

fn nominal(
    environment: &Environment,
    rules: Rules,
    name: &str,
    arguments: &[Type],
    supertype_name: &str,
    supertype_arguments: &[Type],
    depth: usize,
) -> Certainty {
    let declared_name = environment.resolve_class_name(name).unwrap_or(name);
    let declared_supertype_name = environment
        .resolve_class_name(supertype_name)
        .unwrap_or(supertype_name);
    if declared_name == declared_supertype_name {
        if arguments.is_empty()
            || supertype_arguments.is_empty()
            || arguments.len() != supertype_arguments.len()
        {
            return Certainty::Yes;
        }
        let argument_results: Vec<Certainty> = arguments
            .iter()
            .zip(supertype_arguments)
            .map(|(argument, supertype_argument)| {
                check(environment, rules, argument, supertype_argument, depth)
            })
            .collect();
        return Certainty::all(argument_results);
    }
    if !environment.classes.contains_key(declared_name)
        || !environment.classes.contains_key(declared_supertype_name)
    {
        return Certainty::Unknown;
    }
    if environment
        .ancestors(declared_name)
        .iter()
        .any(|ancestor| ancestor.name == declared_supertype_name)
    {
        return Certainty::Yes;
    }
    if is_module(environment, declared_name) || !is_fully_known_class(environment, declared_name) {
        return Certainty::Unknown;
    }
    Certainty::No
}

fn interface_check(environment: &Environment, subtype: &Type, interface_name: &str) -> Certainty {
    let class_name = match subtype {
        Type::ClassInstance { name, .. } => name.as_str(),
        Type::Literal(literal) => literal_class_name(literal),
        Type::Nil => "NilClass",
        Type::Tuple(_) => "Array",
        Type::Record(_) => "Hash",
        _ => return Certainty::Unknown,
    };
    let mut method_names = Vec::new();
    collect_interface_method_names(environment, interface_name, &mut method_names, 0);
    if method_names.is_empty() {
        return Certainty::Unknown;
    }
    if method_names.iter().all(|method_name| {
        environment
            .find_instance_method(class_name, method_name)
            .is_some()
    }) {
        Certainty::Yes
    } else if is_fully_known_class(environment, class_name) {
        Certainty::No
    } else {
        Certainty::Unknown
    }
}

fn collect_interface_method_names(
    environment: &Environment,
    interface_name: &str,
    method_names: &mut Vec<String>,
    depth: usize,
) {
    if depth > 8 {
        return;
    }
    let Some(declaration) = environment.interfaces.get(interface_name) else {
        return;
    };
    method_names.extend(declaration.methods.keys().cloned());
    for included in &declaration.includes {
        collect_interface_method_names(environment, &included.name, method_names, depth + 1);
    }
}

/// `^(A) -> R <: ^(B) -> S`: every call the supertype allows must be accepted by the subtype (parameters are
/// contravariant), and the result must fit (covariant)
fn proc_check(
    environment: &Environment,
    rules: Rules,
    subtype: &rbs_environment::Function,
    supertype: &rbs_environment::Function,
    depth: usize,
) -> Certainty {
    let return_result = check(
        environment,
        rules,
        &subtype.return_type,
        &supertype.return_type,
        depth,
    );
    if subtype.untyped_parameters || supertype.untyped_parameters {
        return return_result;
    }
    let subtype_required = subtype.required_positionals.len() + subtype.trailing_positionals.len();
    let supertype_required =
        supertype.required_positionals.len() + supertype.trailing_positionals.len();
    let subtype_maximum = if subtype.rest_positionals.is_some() {
        usize::MAX
    } else {
        subtype_required + subtype.optional_positionals.len()
    };
    let supertype_maximum = if supertype.rest_positionals.is_some() {
        usize::MAX
    } else {
        supertype_required + supertype.optional_positionals.len()
    };
    if subtype_required > supertype_required || subtype_maximum < supertype_maximum {
        return Certainty::No;
    }
    for (name, _) in supertype
        .required_keywords
        .iter()
        .chain(supertype.optional_keywords.iter())
    {
        let accepted = subtype
            .required_keywords
            .iter()
            .chain(subtype.optional_keywords.iter())
            .any(|(keyword, _)| keyword == name)
            || subtype.rest_keywords.is_some();
        if !accepted {
            return Certainty::No;
        }
    }
    for (name, _) in &subtype.required_keywords {
        if !supertype
            .required_keywords
            .iter()
            .any(|(keyword, _)| keyword == name)
        {
            return Certainty::No;
        }
    }
    let subtype_positionals: Vec<&Type> = subtype
        .required_positionals
        .iter()
        .chain(subtype.optional_positionals.iter())
        .map(|parameter| &parameter.parameter_type)
        .collect();
    let supertype_positionals: Vec<&Type> = supertype
        .required_positionals
        .iter()
        .chain(supertype.optional_positionals.iter())
        .map(|parameter| &parameter.parameter_type)
        .collect();
    let parameter_results: Vec<Certainty> = subtype_positionals
        .iter()
        .zip(supertype_positionals.iter())
        .map(|(subtype_parameter, supertype_parameter)| {
            check(
                environment,
                rules,
                supertype_parameter,
                subtype_parameter,
                depth,
            )
        })
        .collect();
    Certainty::all(
        parameter_results
            .into_iter()
            .chain(std::iter::once(return_result)),
    )
}
