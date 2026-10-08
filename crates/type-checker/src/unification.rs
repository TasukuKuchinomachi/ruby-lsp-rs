use std::collections::HashMap;

use rbs_environment::{Environment, Function, MethodType, Substitution, Type};

use crate::type_utilities::{array_of, literal_class_name, members_of, remove_nil};

const MAXIMUM_DEPTH: usize = 16;

pub(crate) struct Bindings {
    variable_names: Vec<String>,
    bound: HashMap<String, Vec<Type>>,
}

impl Bindings {
    pub(crate) fn new(variable_names: Vec<String>) -> Bindings {
        Bindings {
            variable_names,
            bound: HashMap::new(),
        }
    }

    pub(crate) fn variable_names(&self) -> &[String] {
        &self.variable_names
    }

    fn bind(&mut self, name: &str, argument: &Type) {
        if matches!(argument, Type::Bottom | Type::Untyped) {
            return;
        }
        let candidates = self.bound.entry(name.to_string()).or_default();
        if !candidates.contains(argument) {
            candidates.push(argument.clone());
        }
    }

    fn is_variable(&self, target: &Type) -> Option<String> {
        match target {
            Type::Variable(name) if self.variable_names.contains(name) => Some(name.clone()),
            _ => None,
        }
    }

    /// Substitution for the variables bound so far. Unbound variables become `untyped` when `close_unbound` is set,
    /// otherwise they stay as variables so a later step such as the block body can still bind them
    pub(crate) fn substitution(&self, close_unbound: bool) -> Substitution {
        let mut substitution = Substitution::default();
        for name in &self.variable_names {
            match self.bound.get(name) {
                Some(candidates) if !candidates.is_empty() => {
                    substitution
                        .variables
                        .insert(name.clone(), Type::union(candidates.clone()));
                }
                _ if close_unbound => {
                    substitution.variables.insert(name.clone(), Type::Untyped);
                }
                _ => {}
            }
        }
        substitution
    }

    pub(crate) fn unify(&mut self, environment: &Environment, parameter: &Type, argument: &Type) {
        self.unify_at(environment, parameter, argument, 0);
    }

    fn unify_at(
        &mut self,
        environment: &Environment,
        parameter: &Type,
        argument: &Type,
        depth: usize,
    ) {
        if depth > MAXIMUM_DEPTH || argument.is_untyped() {
            return;
        }
        let next = depth + 1;
        if let Some(name) = self.is_variable(parameter) {
            self.bind(&name, argument);
            return;
        }
        match parameter {
            Type::Optional(inner) => self.unify_at(environment, inner, &remove_nil(argument), next),
            Type::Union(parameter_members) => {
                let variable_members: Vec<String> = parameter_members
                    .iter()
                    .filter_map(|member| self.is_variable(member))
                    .collect();
                for argument_member in members_of(argument) {
                    let matching_member = parameter_members.iter().find(|parameter_member| {
                        self.is_variable(parameter_member).is_none()
                            && same_head(parameter_member, &argument_member)
                    });
                    match matching_member {
                        Some(parameter_member) => {
                            self.unify_at(environment, parameter_member, &argument_member, next)
                        }
                        None => {
                            if variable_members.len() == 1 {
                                self.bind(&variable_members[0], &argument_member);
                            }
                        }
                    }
                }
            }
            Type::ClassInstance {
                name,
                arguments: parameter_arguments,
            } => match argument {
                Type::ClassInstance {
                    name: argument_name,
                    arguments: argument_arguments,
                } => {
                    if name == argument_name {
                        self.unify_all(environment, parameter_arguments, argument_arguments, next);
                    } else if let Some(ancestor_arguments) =
                        ancestor_arguments(environment, argument_name, argument_arguments, name)
                    {
                        self.unify_all(environment, parameter_arguments, &ancestor_arguments, next);
                    }
                }
                Type::Tuple(elements) => {
                    let as_array = array_of(Type::union(elements.clone()));
                    self.unify_at(environment, parameter, &as_array, next);
                }
                Type::Optional(_) | Type::Union(_) => {
                    for argument_member in members_of(argument) {
                        if same_head(parameter, &argument_member) {
                            self.unify_at(environment, parameter, &argument_member, next);
                        }
                    }
                }
                _ => {}
            },
            Type::Tuple(parameter_elements) => {
                if let Type::Tuple(argument_elements) = argument {
                    self.unify_all(environment, parameter_elements, argument_elements, next);
                }
            }
            Type::Proc(parameter_proc) => {
                if let Type::Proc(argument_proc) = argument {
                    self.unify_function(
                        environment,
                        &parameter_proc.function,
                        &argument_proc.function,
                        next,
                    );
                }
            }
            Type::Interface { name, arguments } => {
                self.unify_interface(environment, name, arguments, argument, next)
            }
            _ => {}
        }
    }

    fn unify_all(
        &mut self,
        environment: &Environment,
        parameters: &[Type],
        arguments: &[Type],
        depth: usize,
    ) {
        for (parameter, argument) in parameters.iter().zip(arguments) {
            self.unify_at(environment, parameter, argument, depth);
        }
    }

    fn unify_function(
        &mut self,
        environment: &Environment,
        parameter: &Function,
        argument: &Function,
        depth: usize,
    ) {
        let parameter_positionals: Vec<&Type> = parameter
            .required_positionals
            .iter()
            .map(|each| &each.parameter_type)
            .collect();
        let argument_positionals: Vec<&Type> = argument
            .required_positionals
            .iter()
            .map(|each| &each.parameter_type)
            .collect();
        for (parameter_type, argument_type) in
            parameter_positionals.into_iter().zip(argument_positionals)
        {
            self.unify_at(environment, parameter_type, argument_type, depth);
        }
        self.unify_at(
            environment,
            &parameter.return_type,
            &argument.return_type,
            depth,
        );
    }

    fn unify_method_type(
        &mut self,
        environment: &Environment,
        parameter: &MethodType,
        argument: &MethodType,
        depth: usize,
    ) {
        self.unify_function(environment, &parameter.function, &argument.function, depth);
        if let (Some(parameter_block), Some(argument_block)) = (&parameter.block, &argument.block) {
            self.unify_function(
                environment,
                &parameter_block.function,
                &argument_block.function,
                depth,
            );
        }
    }

    fn unify_interface(
        &mut self,
        environment: &Environment,
        interface_name: &str,
        interface_arguments: &[Type],
        argument: &Type,
        depth: usize,
    ) {
        let (class_name, class_arguments) = match argument {
            Type::ClassInstance { name, arguments } => (name.clone(), arguments.clone()),
            Type::Tuple(elements) => ("Array".to_string(), vec![Type::union(elements.clone())]),
            Type::Literal(literal) => (literal_class_name(literal).to_string(), Vec::new()),
            _ => return,
        };
        let Some(interface) = environment.interfaces.get(interface_name) else {
            return;
        };
        let mut interface_substitution = Substitution::default();
        for (type_parameter, interface_argument) in
            interface.type_parameters.iter().zip(interface_arguments)
        {
            interface_substitution
                .variables
                .insert(type_parameter.name.clone(), interface_argument.clone());
        }
        let class_substitution =
            class_substitution(environment, &class_name, &class_arguments, argument);
        let mut method_names: Vec<&String> = interface.methods.keys().collect();
        method_names.sort();
        for method_name in method_names {
            let Some(interface_method) =
                environment.find_interface_method(interface_name, method_name)
            else {
                continue;
            };
            let Some(class_method) = environment.find_instance_method(&class_name, method_name)
            else {
                continue;
            };
            let Some(interface_overload) = interface_method.overloads.first() else {
                continue;
            };
            let interface_overload = interface_overload.substitute(&interface_substitution);
            let wants_block = interface_overload.block.is_some();
            let Some(class_overload) = class_method
                .overloads
                .iter()
                .find(|overload| overload.block.is_some() == wants_block)
            else {
                continue;
            };
            let class_overload = class_overload.substitute(&class_substitution);
            self.unify_method_type(environment, &interface_overload, &class_overload, depth);
        }
    }
}

pub(crate) fn class_substitution(
    environment: &Environment,
    class_name: &str,
    class_arguments: &[Type],
    self_type: &Type,
) -> Substitution {
    let mut substitution = Substitution {
        self_type: Some(self_type.clone()),
        instance_type: Some(self_type.clone()),
        class_type: Some(Type::ClassSingleton {
            name: class_name.to_string(),
        }),
        ..Substitution::default()
    };
    if let Some(declaration) = environment.classes.get(class_name) {
        for (index, type_parameter) in declaration.type_parameters.iter().enumerate() {
            let argument = class_arguments.get(index).cloned().unwrap_or(Type::Untyped);
            substitution
                .variables
                .insert(type_parameter.name.clone(), argument);
        }
    }
    substitution
}

fn ancestor_arguments(
    environment: &Environment,
    class_name: &str,
    class_arguments: &[Type],
    ancestor_name: &str,
) -> Option<Vec<Type>> {
    let declared_name = environment.resolve_class_name(class_name)?;
    let ancestor = environment
        .ancestors(declared_name)
        .into_iter()
        .find(|ancestor| ancestor.name == ancestor_name)?;
    let substitution = class_substitution(
        environment,
        declared_name,
        class_arguments,
        &Type::instance(declared_name),
    );
    Some(
        ancestor
            .arguments
            .iter()
            .map(|argument| argument.substitute(&substitution))
            .collect(),
    )
}

fn same_head(parameter: &Type, argument: &Type) -> bool {
    match (parameter, argument) {
        (
            Type::ClassInstance { name, .. },
            Type::ClassInstance {
                name: argument_name,
                ..
            },
        ) => name == argument_name,
        (Type::ClassInstance { name, .. }, Type::Tuple(_)) => name == "Array",
        (Type::Tuple(_), Type::Tuple(_))
        | (Type::Proc(_), Type::Proc(_))
        | (Type::Nil, Type::Nil) => true,
        (
            Type::ClassSingleton { name },
            Type::ClassSingleton {
                name: argument_name,
            },
        ) => name == argument_name,
        _ => false,
    }
}
