use std::{cell::RefCell, sync::Arc};

use rbs_definition::{
    ast::{self, TypeName, TypeNameKind},
    environment::Environment,
    substitution::normalize_args,
};
use rbs_definition::{collections::HashMap, errors::RbsError};

use crate::{
    method_type::{Block, Function, KeywordParams, MethodType, Params, TypeParam},
    substitution::Substitution,
    types::{RecordElement, Type},
};

pub struct Factory {
    environment: Arc<Environment>,
    type_cache: RefCell<HashMap<ast::Type, Type>>,
}

fn sorted_keywords(keywords: &[(String, ast::Parameter)]) -> Vec<(String, ast::Parameter)> {
    let mut keywords: Vec<(String, ast::Parameter)> = keywords
        .iter()
        .map(|(name, parameter)| (name.clone(), cache_parameter(parameter)))
        .collect();
    keywords.sort_by(|left, right| left.0.cmp(&right.0));
    keywords
}

fn cache_parameter(parameter: &ast::Parameter) -> ast::Parameter {
    ast::Parameter {
        type_: cache_key(&parameter.type_),
        name: parameter.name.clone(),
    }
}

fn cache_callable(callable: &ast::Callable) -> ast::Callable {
    match callable {
        ast::Callable::Function(function) => {
            let parameters = |parameters: &[ast::Parameter]| -> Vec<ast::Parameter> {
                parameters.iter().map(cache_parameter).collect()
            };
            ast::Callable::Function(Box::new(ast::Function {
                required_positionals: parameters(&function.required_positionals),
                optional_positionals: parameters(&function.optional_positionals),
                rest_positionals: function.rest_positionals.as_ref().map(cache_parameter),
                trailing_positionals: parameters(&function.trailing_positionals),
                required_keywords: sorted_keywords(&function.required_keywords),
                optional_keywords: sorted_keywords(&function.optional_keywords),
                rest_keywords: function.rest_keywords.as_ref().map(cache_parameter),
                return_type: cache_key(&function.return_type),
            }))
        }
        ast::Callable::Untyped { return_type } => ast::Callable::Untyped {
            return_type: cache_key(return_type),
        },
    }
}

fn cache_key(type_: &ast::Type) -> ast::Type {
    let all = |types: &[ast::Type]| -> Vec<ast::Type> { types.iter().map(cache_key).collect() };
    match type_ {
        ast::Type::ClassInstance {
            name,
            args,
            location,
        } => ast::Type::ClassInstance {
            location: location.clone(),
            name: name.clone(),
            args: all(args),
        },
        ast::Type::ClassSingleton {
            name,
            args,
            location,
        } => ast::Type::ClassSingleton {
            location: location.clone(),
            name: name.clone(),
            args: all(args),
        },
        ast::Type::Interface {
            name,
            args,
            location,
        } => ast::Type::Interface {
            location: location.clone(),
            name: name.clone(),
            args: all(args),
        },
        ast::Type::Alias {
            name,
            args,
            location,
        } => ast::Type::Alias {
            location: location.clone(),
            name: name.clone(),
            args: all(args),
        },
        ast::Type::Tuple(types) => ast::Type::Tuple(all(types)),
        ast::Type::Union(types) => ast::Type::Union(all(types)),
        ast::Type::Intersection(types) => ast::Type::Intersection(all(types)),
        ast::Type::Optional(type_) => ast::Type::Optional(Box::new(cache_key(type_))),
        ast::Type::Record(fields) => {
            let mut fields: Vec<ast::RecordField> = fields
                .iter()
                .map(|field| ast::RecordField {
                    key: field.key.clone(),
                    type_: cache_key(&field.type_),
                    required: field.required,
                })
                .collect();
            fields.sort_by_key(|field| (field.required, field.key.inspect()));
            ast::Type::Record(fields)
        }
        ast::Type::Proc {
            function,
            block,
            self_type,
        } => ast::Type::Proc {
            function: Box::new(cache_callable(function)),
            block: block.as_ref().map(|block| {
                Box::new(ast::Block {
                    type_: cache_callable(&block.type_),
                    required: block.required,
                    self_type: block.self_type.as_ref().map(cache_key),
                })
            }),
            self_type: self_type
                .as_ref()
                .map(|self_type| Box::new(cache_key(self_type))),
        },
        _ => type_.clone(),
    }
}

impl Factory {
    pub fn new(environment: Arc<Environment>) -> Factory {
        Factory {
            environment,
            type_cache: RefCell::default(),
        }
    }

    fn normalize_args(&self, name: &TypeName, args: &[ast::Type]) -> Vec<ast::Type> {
        let parameters = match name.kind() {
            TypeNameKind::Class => self
                .environment
                .class_declarations
                .get(&self.environment.normalize_module_name(name))
                .map(|entry| entry.type_parameters()),
            TypeNameKind::Interface => self
                .environment
                .interface_declarations
                .get(name)
                .map(|entry| entry.declaration.type_parameters.as_slice()),
            TypeNameKind::Alias => self
                .environment
                .type_alias_declarations
                .get(name)
                .map(|entry| entry.declaration.type_parameters.as_slice()),
        };
        match parameters {
            Some(parameters) if !parameters.is_empty() => normalize_args(parameters, args),
            _ => args.to_vec(),
        }
    }

    fn types(&self, name: &TypeName, args: &[ast::Type]) -> Vec<Type> {
        self.normalize_args(name, args)
            .iter()
            .map(|arg| self.type_(arg))
            .collect()
    }

    pub fn type_(&self, type_: &ast::Type) -> Type {
        let key = cache_key(type_);
        if let Some(cached) = self.type_cache.borrow().get(&key) {
            return cached.clone();
        }
        let converted = self.convert_type(type_);
        self.type_cache.borrow_mut().insert(key, converted.clone());
        converted
    }

    fn convert_type(&self, type_: &ast::Type) -> Type {
        match type_ {
            ast::Type::Any => Type::Any,
            ast::Type::Class => Type::Class,
            ast::Type::Instance => Type::Instance,
            ast::Type::SelfType => Type::SelfType,
            ast::Type::Top => Type::Top,
            ast::Type::Bottom => Type::Bot,
            ast::Type::Bool => Type::Boolean,
            ast::Type::Void => Type::Void,
            ast::Type::Nil => Type::Nil,
            ast::Type::Variable(name) => Type::Var(name.clone()),
            ast::Type::ClassSingleton { name, .. } => Type::Singleton(name.clone()),
            ast::Type::ClassInstance { name, args, .. } => Type::NameInstance {
                name: name.clone(),
                args: self.types(name, args),
            },
            ast::Type::Interface { name, args, .. } => Type::Interface {
                name: name.clone(),
                args: self.types(name, args),
            },
            ast::Type::Alias { name, args, .. } => Type::Alias {
                name: name.clone(),
                args: self.types(name, args),
            },
            ast::Type::Union(types) => {
                Type::union(types.iter().map(|type_| self.type_(type_)).collect())
            }
            ast::Type::Intersection(types) => {
                Type::intersection(types.iter().map(|type_| self.type_(type_)).collect())
            }
            ast::Type::Optional(inner) => Type::union(vec![self.type_(inner), Type::Nil]),
            ast::Type::Literal(literal) => Type::Literal(literal.clone()),
            ast::Type::Tuple(types) => {
                Type::Tuple(types.iter().map(|type_| self.type_(type_)).collect())
            }
            ast::Type::Record(fields) => Type::Record(
                fields
                    .iter()
                    .map(|field| RecordElement {
                        key: field.key.clone(),
                        type_: self.type_(&field.type_),
                        required: field.required,
                    })
                    .collect(),
            ),
            ast::Type::Proc {
                function,
                block,
                self_type,
            } => Type::Proc {
                function: Box::new(self.function(function)),
                block: block.as_ref().map(|block| Box::new(self.block(block))),
                self_type: self_type
                    .as_ref()
                    .map(|self_type| Box::new(self.type_(self_type))),
            },
        }
    }

    fn params(&self, callable: &ast::Callable) -> Option<Params> {
        match callable {
            ast::Callable::Function(function) => {
                let types = |parameters: &[ast::Parameter]| -> Vec<Type> {
                    parameters
                        .iter()
                        .map(|parameter| self.type_(&parameter.type_))
                        .collect()
                };
                let keywords = |keywords: &[(String, ast::Parameter)]| -> Vec<(String, Type)> {
                    keywords
                        .iter()
                        .map(|(name, parameter)| (name.clone(), self.type_(&parameter.type_)))
                        .collect()
                };
                Some(Params::build(
                    types(&function.required_positionals),
                    types(&function.optional_positionals),
                    function
                        .rest_positionals
                        .as_ref()
                        .map(|parameter| self.type_(&parameter.type_)),
                    KeywordParams {
                        requireds: keywords(&function.required_keywords),
                        optionals: keywords(&function.optional_keywords),
                        rest: function
                            .rest_keywords
                            .as_ref()
                            .map(|parameter| self.type_(&parameter.type_)),
                    },
                ))
            }
            ast::Callable::Untyped { .. } => None,
        }
    }

    fn function(&self, callable: &ast::Callable) -> Function {
        Function {
            params: self.params(callable),
            return_type: self.type_(callable.return_type()),
        }
    }

    fn block(&self, block: &ast::Block) -> Block {
        Block {
            type_: self.function(&block.type_),
            optional: !block.required,
            self_type: block.self_type.as_ref().map(|type_| self.type_(type_)),
        }
    }

    pub fn type_param(&self, parameter: &ast::TypeParameter) -> TypeParam {
        TypeParam {
            name: parameter.name.clone(),
            upper_bound: parameter
                .upper_bound
                .as_ref()
                .map(|type_| self.type_(type_)),
            variance: parameter.variance,
            unchecked: parameter.unchecked,
            default_type: parameter
                .default_type
                .as_ref()
                .map(|type_| self.type_(type_)),
        }
    }

    pub fn method_type(&self, method_type: &ast::MethodType) -> MethodType {
        MethodType {
            type_params: method_type
                .type_parameters
                .iter()
                .map(|parameter| self.type_param(parameter))
                .collect(),
            type_: self.function(&method_type.type_),
            block: method_type.block.as_ref().map(|block| self.block(block)),
        }
    }

    pub fn expand_alias(&self, name: &TypeName, args: &[Type]) -> Result<Type, RbsError> {
        let name = self
            .environment
            .normalize_type_name(name)
            .unwrap_or_else(|| name.clone());
        let entry = self
            .environment
            .type_alias_declarations
            .get(&name)
            .ok_or_else(|| format!("Unknown alias name: {name}"))?;
        self.environment
            .ensure_namespace(&name.namespace, entry.declaration.location.as_ref())
            .map_err(|error| error.to_string())?;
        let parameters: Vec<String> = entry
            .declaration
            .type_parameters
            .iter()
            .map(|parameter| parameter.name.clone())
            .collect();
        if parameters.len() != args.len() {
            return Err(format!("invalid type application: {name}").into());
        }
        let substitution = Substitution::build(&parameters, args, None, None, None);
        Ok(self
            .type_(&entry.declaration.type_)
            .substitute(&substitution))
    }

    pub fn normalize_type(&self, type_: &Type) -> Type {
        let normalize = |type_: &Type| self.normalize_type(type_);
        let all = |types: &[Type]| -> Vec<Type> { types.iter().map(normalize).collect() };
        match type_ {
            Type::NameInstance { name, args } => Type::NameInstance {
                name: self.environment.normalize_module_name(name),
                args: all(args),
            },
            Type::Singleton(name) => Type::Singleton(self.environment.normalize_module_name(name)),
            Type::Intersection(types) => Type::intersection(all(types)),
            Type::Union(types) => Type::union(all(types)),
            Type::Tuple(types) => Type::Tuple(all(types)),
            Type::Record(_) | Type::Proc { .. } => type_.map_type(&normalize),
            Type::Alias { name, args } => Type::Alias {
                name: name.clone(),
                args: all(args),
            },
            Type::Interface { name, args } => Type::Interface {
                name: name.clone(),
                args: all(args),
            },
            _ => type_.clone(),
        }
    }

    pub fn instance_type(&self, name: &TypeName) -> Type {
        let parameter_count = self
            .environment
            .class_declarations
            .get(name)
            .map_or(0, |entry| entry.type_parameters().len());
        Type::NameInstance {
            name: name.clone(),
            args: vec![Type::Any; parameter_count],
        }
    }
}
