use crate::ast::{
    Block, Callable, Function, MethodType, Parameter, RecordField, Type, TypeParameter,
};
use crate::collections::HashMap;

#[derive(Clone, Debug, Default)]
pub struct Substitution {
    pub mapping: HashMap<String, Type>,
    pub instance_type: Option<Type>,
}

impl Substitution {
    pub fn build(variables: &[String], types: &[Type]) -> Substitution {
        assert_eq!(
            variables.len(),
            types.len(),
            "broken substitution: variables={variables:?}, types={types:?}"
        );
        Substitution {
            mapping: variables
                .iter()
                .cloned()
                .zip(types.iter().cloned())
                .collect(),
            instance_type: None,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.mapping.is_empty() && self.instance_type.is_none()
    }

    pub fn add(&mut self, from: &str, to: Type) {
        self.mapping.insert(from.to_string(), to);
    }

    pub fn compose(&self, other: &Substitution) -> Substitution {
        if other.is_empty() {
            return self.clone();
        }
        if self.is_empty() {
            return other.clone();
        }
        let mut mapping = self.mapping.clone();
        for (variable, type_) in &other.mapping {
            let type_ = if self.mapping.contains_key(variable) {
                self.apply(type_).unwrap_or_else(|| type_.clone())
            } else {
                type_.clone()
            };
            mapping.insert(variable.clone(), type_);
        }
        Substitution {
            mapping,
            instance_type: None,
        }
    }

    fn without(&self, variables: &[String]) -> Substitution {
        let mut substitution = self.clone();
        for variable in variables {
            substitution.mapping.remove(variable);
        }
        substitution
    }

    fn apply(&self, type_: &Type) -> Option<Type> {
        match type_ {
            Type::Variable(name) => self.mapping.get(name).cloned(),
            Type::Instance => self.instance_type.clone(),
            _ => None,
        }
    }
}

impl Type {
    pub fn substitute(&self, substitution: &Substitution) -> Type {
        if let Some(type_) = substitution.apply(self) {
            return type_;
        }
        let all = |types: &[Type]| -> Vec<Type> {
            types
                .iter()
                .map(|type_| type_.substitute(substitution))
                .collect()
        };
        match self {
            Type::ClassInstance {
                name,
                args,
                location,
            } => Type::ClassInstance {
                name: name.clone(),
                args: all(args),
                location: location.clone(),
            },
            Type::ClassSingleton {
                name,
                args,
                location,
            } => Type::ClassSingleton {
                name: name.clone(),
                args: all(args),
                location: location.clone(),
            },
            Type::Interface {
                name,
                args,
                location,
            } => Type::Interface {
                name: name.clone(),
                args: all(args),
                location: location.clone(),
            },
            Type::Alias {
                name,
                args,
                location,
            } => Type::Alias {
                name: name.clone(),
                args: all(args),
                location: location.clone(),
            },
            Type::Tuple(types) => Type::Tuple(all(types)),
            Type::Record(fields) => Type::Record(
                fields
                    .iter()
                    .map(|field| RecordField {
                        key: field.key.clone(),
                        type_: field.type_.substitute(substitution),
                        required: field.required,
                    })
                    .collect(),
            ),
            Type::Optional(inner) => Type::Optional(Box::new(inner.substitute(substitution))),
            Type::Union(types) => Type::Union(all(types)),
            Type::Intersection(types) => Type::Intersection(all(types)),
            Type::Proc {
                function,
                block,
                self_type,
            } => Type::Proc {
                function: Box::new(function.substitute(substitution)),
                block: block
                    .as_ref()
                    .map(|block| Box::new(block.substitute(substitution))),
                self_type: self_type
                    .as_ref()
                    .map(|self_type| Box::new(self_type.substitute(substitution))),
            },
            Type::Bool
            | Type::Void
            | Type::Any
            | Type::Nil
            | Type::Top
            | Type::Bottom
            | Type::SelfType
            | Type::Instance
            | Type::Class
            | Type::Variable(_)
            | Type::Literal(_) => self.clone(),
        }
    }
}

impl Parameter {
    pub fn substitute(&self, substitution: &Substitution) -> Parameter {
        Parameter {
            type_: self.type_.substitute(substitution),
            name: self.name.clone(),
        }
    }
}

impl Callable {
    pub fn substitute(&self, substitution: &Substitution) -> Callable {
        match self {
            Callable::Function(function) => {
                let parameters = |parameters: &[Parameter]| -> Vec<Parameter> {
                    parameters
                        .iter()
                        .map(|parameter| parameter.substitute(substitution))
                        .collect()
                };
                let keywords = |keywords: &[(String, Parameter)]| -> Vec<(String, Parameter)> {
                    keywords
                        .iter()
                        .map(|(name, parameter)| (name.clone(), parameter.substitute(substitution)))
                        .collect()
                };
                Callable::Function(Box::new(Function {
                    required_positionals: parameters(&function.required_positionals),
                    optional_positionals: parameters(&function.optional_positionals),
                    rest_positionals: function
                        .rest_positionals
                        .as_ref()
                        .map(|parameter| parameter.substitute(substitution)),
                    trailing_positionals: parameters(&function.trailing_positionals),
                    required_keywords: keywords(&function.required_keywords),
                    optional_keywords: keywords(&function.optional_keywords),
                    rest_keywords: function
                        .rest_keywords
                        .as_ref()
                        .map(|parameter| parameter.substitute(substitution)),
                    return_type: function.return_type.substitute(substitution),
                }))
            }
            Callable::Untyped { return_type } => Callable::Untyped {
                return_type: return_type.substitute(substitution),
            },
        }
    }
}

impl Block {
    pub fn substitute(&self, substitution: &Substitution) -> Block {
        Block {
            type_: self.type_.substitute(substitution),
            required: self.required,
            self_type: self
                .self_type
                .as_ref()
                .map(|self_type| self_type.substitute(substitution)),
        }
    }
}

impl TypeParameter {
    pub fn substitute(&self, substitution: &Substitution) -> TypeParameter {
        TypeParameter {
            upper_bound: self
                .upper_bound
                .as_ref()
                .map(|type_| type_.substitute(substitution)),
            lower_bound: self
                .lower_bound
                .as_ref()
                .map(|type_| type_.substitute(substitution)),
            default_type: self
                .default_type
                .as_ref()
                .map(|type_| type_.substitute(substitution)),
            ..self.clone()
        }
    }
}

impl MethodType {
    pub fn substitute(&self, substitution: &Substitution) -> MethodType {
        let bound: Vec<String> = self
            .type_parameters
            .iter()
            .map(|parameter| parameter.name.clone())
            .collect();
        let substitution = substitution.without(&bound);
        MethodType {
            type_parameters: self
                .type_parameters
                .iter()
                .map(|parameter| parameter.substitute(&substitution))
                .collect(),
            type_: self.type_.substitute(&substitution),
            block: self
                .block
                .as_ref()
                .map(|block| block.substitute(&substitution)),
            location: self.location.clone(),
        }
    }
}

pub fn normalize_args(parameters: &[TypeParameter], args: &[Type]) -> Vec<Type> {
    if parameters.is_empty() {
        return args.to_vec();
    }
    let required_count = parameters
        .iter()
        .filter(|parameter| parameter.default_type.is_none())
        .count();
    if !(required_count <= args.len() && args.len() <= parameters.len()) {
        return args.to_vec();
    }
    let application = application(parameters, args).unwrap_or_default();
    parameters
        .iter()
        .enumerate()
        .map(|(index, parameter)| match args.get(index) {
            Some(arg) => arg.clone(),
            None => match &parameter.default_type {
                Some(default_type) => default_type.substitute(&application),
                None => Type::Any,
            },
        })
        .collect()
}

pub fn application(parameters: &[TypeParameter], args: &[Type]) -> Option<Substitution> {
    if parameters.is_empty() {
        return None;
    }
    let (optional, required): (Vec<&TypeParameter>, Vec<&TypeParameter>) = parameters
        .iter()
        .partition(|parameter| parameter.default_type.is_some());
    let mut parameter_substitution = Substitution::default();
    let mut application_substitution = Substitution::default();
    for (index, parameter) in required.iter().enumerate() {
        let arg = args.get(index).cloned().unwrap_or(Type::Any);
        parameter_substitution.add(&parameter.name, arg.clone());
        application_substitution.add(&parameter.name, arg);
    }
    for parameter in &optional {
        parameter_substitution.add(&parameter.name, Type::Any);
    }
    for (index, parameter) in optional.iter().enumerate() {
        match args.get(required.len() + index) {
            Some(arg) => application_substitution.add(&parameter.name, arg.clone()),
            None => {
                let default_type = parameter
                    .default_type
                    .as_ref()
                    .expect("an optional parameter has a default type");
                application_substitution.add(
                    &parameter.name,
                    default_type.substitute(&parameter_substitution),
                );
            }
        }
    }
    Some(application_substitution)
}
