use rbs_definition::errors::RbsError;
use std::{
    cell::Cell,
    fmt,
    hash::{DefaultHasher, Hash, Hasher},
};

use rbs_definition::ast::{Literal, TypeName};

use crate::{
    method_type::{Block, Function},
    substitution::Substitution,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Logic {
    Not,
    ReceiverIsNil,
    ReceiverIsNotNil,
    ReceiverIsArg,
    ArgIsReceiver,
    ArgEqualsReceiver,
    ArgIsAncestor,
}

impl Logic {
    fn class_name(self) -> &'static str {
        match self {
            Logic::Not => "Steep::AST::Types::Logic::Not",
            Logic::ReceiverIsNil => "Steep::AST::Types::Logic::ReceiverIsNil",
            Logic::ReceiverIsNotNil => "Steep::AST::Types::Logic::ReceiverIsNotNil",
            Logic::ReceiverIsArg => "Steep::AST::Types::Logic::ReceiverIsArg",
            Logic::ArgIsReceiver => "Steep::AST::Types::Logic::ArgIsReceiver",
            Logic::ArgEqualsReceiver => "Steep::AST::Types::Logic::ArgEqualsReceiver",
            Logic::ArgIsAncestor => "Steep::AST::Types::Logic::ArgIsAncestor",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RecordElement {
    pub key: Literal,
    pub type_: Type,
    pub required: bool,
}

#[derive(Clone, Debug)]
pub enum Type {
    Any,
    Class,
    Instance,
    SelfType,
    Top,
    Bot,
    Boolean,
    Void,
    Nil,
    Var(String),
    Singleton(TypeName),
    NameInstance {
        name: TypeName,
        args: Vec<Type>,
    },
    Interface {
        name: TypeName,
        args: Vec<Type>,
    },
    Alias {
        name: TypeName,
        args: Vec<Type>,
    },
    Union(Vec<Type>),
    Intersection(Vec<Type>),
    Literal(Literal),
    Tuple(Vec<Type>),
    Record(Vec<RecordElement>),
    Proc {
        function: Box<Function>,
        block: Option<Box<Block>>,
        self_type: Option<Box<Type>>,
    },
    Logic(Logic),
}

fn same_members(left: &[Type], right: &[Type]) -> bool {
    left.iter().all(|type_| right.contains(type_)) && right.iter().all(|type_| left.contains(type_))
}

impl PartialEq for Type {
    fn eq(&self, other: &Type) -> bool {
        match (self, other) {
            (Type::Any, Type::Any)
            | (Type::Class, Type::Class)
            | (Type::Instance, Type::Instance)
            | (Type::SelfType, Type::SelfType)
            | (Type::Top, Type::Top)
            | (Type::Bot, Type::Bot)
            | (Type::Boolean, Type::Boolean)
            | (Type::Void, Type::Void)
            | (Type::Nil, Type::Nil) => true,
            (Type::Var(left), Type::Var(right)) => left == right,
            (Type::Singleton(left), Type::Singleton(right)) => left == right,
            (
                Type::NameInstance { name, args },
                Type::NameInstance {
                    name: other_name,
                    args: other_args,
                },
            )
            | (
                Type::Interface { name, args },
                Type::Interface {
                    name: other_name,
                    args: other_args,
                },
            )
            | (
                Type::Alias { name, args },
                Type::Alias {
                    name: other_name,
                    args: other_args,
                },
            ) => name == other_name && args == other_args,
            (Type::Union(left), Type::Union(right)) => same_members(left, right),
            (Type::Intersection(left), Type::Intersection(right)) => left == right,
            (Type::Literal(left), Type::Literal(right)) => left.inspect() == right.inspect(),
            (Type::Tuple(left), Type::Tuple(right)) => left == right,
            (Type::Record(left), Type::Record(right)) => {
                left.len() == right.len()
                    && left.iter().all(|element| {
                        right.iter().any(|other| {
                            element.key.inspect() == other.key.inspect()
                                && element.type_ == other.type_
                                && element.required == other.required
                        })
                    })
            }
            (
                Type::Proc {
                    function,
                    block,
                    self_type,
                },
                Type::Proc {
                    function: other_function,
                    block: other_block,
                    self_type: other_self_type,
                },
            ) => function == other_function && block == other_block && self_type == other_self_type,
            (Type::Logic(left), Type::Logic(right)) => left == right,
            _ => false,
        }
    }
}

impl Type {
    pub fn union(types: Vec<Type>) -> Type {
        if types.is_empty() {
            return Type::Bot;
        }
        if types.len() == 1 {
            return types.into_iter().next().unwrap();
        }
        let mut flattened: Vec<Type> = Vec::new();
        for type_ in types.into_iter().flat_map(|type_| match type_ {
            Type::Union(types) => types,
            type_ => vec![type_],
        }) {
            match type_ {
                Type::Any => return Type::Any,
                Type::Top => return Type::Top,
                Type::Bot => {}
                type_ => {
                    if !flattened.contains(&type_) {
                        flattened.push(type_);
                    }
                }
            }
        }
        match flattened.len() {
            0 => Type::Bot,
            1 => flattened.pop().unwrap(),
            _ => Type::Union(flattened),
        }
    }

    pub fn intersection(types: Vec<Type>) -> Type {
        let mut flattened: Vec<Type> = Vec::new();
        for type_ in types.into_iter().flat_map(|type_| match type_ {
            Type::Intersection(types) => types,
            type_ => vec![type_],
        }) {
            match type_ {
                Type::Any => return Type::Any,
                Type::Bot => return Type::Bot,
                Type::Top => {}
                type_ => {
                    if !flattened.contains(&type_) {
                        flattened.push(type_);
                    }
                }
            }
        }
        match flattened.len() {
            0 => Type::Top,
            1 => flattened.pop().unwrap(),
            _ => Type::Intersection(flattened),
        }
    }

    pub fn substitute(&self, substitution: &Substitution) -> Type {
        let all = |types: &[Type]| -> Vec<Type> {
            types
                .iter()
                .map(|type_| type_.substitute(substitution))
                .collect()
        };
        match self {
            Type::SelfType => substitution
                .self_type
                .clone()
                .unwrap_or_else(|| self.clone()),
            Type::Instance => substitution
                .instance_type
                .clone()
                .unwrap_or_else(|| self.clone()),
            Type::Class => substitution
                .module_type
                .clone()
                .unwrap_or_else(|| self.clone()),
            Type::Var(name) => substitution
                .dictionary
                .get(name)
                .cloned()
                .unwrap_or_else(|| self.clone()),
            Type::NameInstance { name, args } => Type::NameInstance {
                name: name.clone(),
                args: all(args),
            },
            Type::Interface { name, args } => Type::Interface {
                name: name.clone(),
                args: all(args),
            },
            Type::Alias { name, args } => Type::Alias {
                name: name.clone(),
                args: all(args),
            },
            Type::Union(types) => Type::union(all(types)),
            Type::Intersection(types) => Type::intersection(all(types)),
            Type::Tuple(types) => Type::Tuple(all(types)),
            Type::Record(elements) => Type::Record(
                elements
                    .iter()
                    .map(|element| RecordElement {
                        type_: element.type_.substitute(substitution),
                        ..element.clone()
                    })
                    .collect(),
            ),
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
            Type::Any
            | Type::Top
            | Type::Bot
            | Type::Boolean
            | Type::Void
            | Type::Nil
            | Type::Singleton(_)
            | Type::Literal(_)
            | Type::Logic(_) => self.clone(),
        }
    }
}

thread_local! {
    static FRESH_VARIABLE_COUNT: Cell<usize> = const { Cell::new(0) };
}

pub fn fresh_variable_name(name: &str) -> String {
    let count = FRESH_VARIABLE_COUNT.with(|count| {
        count.set(count.get() + 1);
        count.get()
    });
    format!("{name}({count})")
}

pub fn reset_fresh_variable_count() {
    FRESH_VARIABLE_COUNT.with(|count| count.set(0));
}

pub(crate) fn hash_of(value: &impl Hash) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

pub(crate) fn unordered_hash(hashes: impl Iterator<Item = u64>, state: &mut impl Hasher) {
    let mut hashes: Vec<u64> = hashes.collect();
    hashes.sort_unstable();
    hashes.dedup();
    hashes.hash(state);
}

impl Hash for Type {
    fn hash<HashState: Hasher>(&self, state: &mut HashState) {
        std::mem::discriminant(self).hash(state);
        match self {
            Type::Var(name) => name.hash(state),
            Type::Singleton(name) => name.hash(state),
            Type::NameInstance { name, args }
            | Type::Interface { name, args }
            | Type::Alias { name, args } => {
                name.hash(state);
                args.hash(state);
            }
            Type::Union(types) => unordered_hash(types.iter().map(hash_of), state),
            Type::Intersection(types) | Type::Tuple(types) => types.hash(state),
            Type::Literal(literal) => literal.inspect().hash(state),
            Type::Record(elements) => unordered_hash(
                elements.iter().map(|element| {
                    hash_of(&(element.key.inspect(), &element.type_, element.required))
                }),
                state,
            ),
            Type::Proc {
                function,
                block,
                self_type,
            } => {
                function.hash(state);
                block.hash(state);
                self_type.hash(state);
            }
            Type::Logic(logic) => logic.hash(state),
            Type::Any
            | Type::Class
            | Type::Instance
            | Type::SelfType
            | Type::Top
            | Type::Bot
            | Type::Boolean
            | Type::Void
            | Type::Nil => {}
        }
    }
}

impl Eq for Type {}

fn level_of_children(levels: Vec<Vec<usize>>) -> Vec<usize> {
    let mut summed: Vec<usize> = Vec::new();
    for level in levels {
        if level.len() > summed.len() {
            summed.resize(level.len(), 0);
        }
        for (index, value) in level.into_iter().enumerate() {
            summed[index] += value;
        }
    }
    summed
}

fn level_with_children(children: Vec<&Type>) -> Result<Vec<usize>, RbsError> {
    let levels = children
        .into_iter()
        .map(Type::level)
        .collect::<Result<Vec<_>, RbsError>>()?;
    let mut level = vec![0];
    level.extend(level_of_children(levels));
    Ok(level)
}

impl Type {
    pub fn free_variables(&self) -> Vec<String> {
        let mut variables = Vec::new();
        self.collect_free_variables(&mut variables);
        variables
    }

    pub(crate) fn collect_free_variables(&self, variables: &mut Vec<String>) {
        match self {
            Type::Var(name) => {
                if !variables.contains(name) {
                    variables.push(name.clone());
                }
            }
            Type::NameInstance { args: types, .. }
            | Type::Interface { args: types, .. }
            | Type::Alias { args: types, .. }
            | Type::Union(types)
            | Type::Intersection(types)
            | Type::Tuple(types) => {
                for type_ in types {
                    type_.collect_free_variables(variables);
                }
            }
            Type::Record(elements) => {
                for element in elements {
                    element.type_.collect_free_variables(variables);
                }
            }
            Type::Proc {
                function,
                block,
                self_type,
            } => {
                function.collect_free_variables(variables);
                if let Some(block) = block {
                    block.collect_free_variables(variables);
                }
                if let Some(self_type) = self_type {
                    self_type.collect_free_variables(variables);
                }
            }
            _ => {}
        }
    }

    pub fn map_type(&self, transform: &dyn Fn(&Type) -> Type) -> Type {
        let all = |types: &[Type]| -> Vec<Type> { types.iter().map(transform).collect() };
        match self {
            Type::NameInstance { name, args } => Type::NameInstance {
                name: name.clone(),
                args: all(args),
            },
            Type::Interface { name, args } => Type::Interface {
                name: name.clone(),
                args: all(args),
            },
            Type::Alias { name, args } => Type::Alias {
                name: name.clone(),
                args: all(args),
            },
            Type::Union(types) => Type::union(all(types)),
            Type::Intersection(types) => Type::intersection(all(types)),
            Type::Tuple(types) => Type::Tuple(all(types)),
            Type::Record(elements) => Type::Record(
                elements
                    .iter()
                    .map(|element| RecordElement {
                        type_: transform(&element.type_),
                        ..element.clone()
                    })
                    .collect(),
            ),
            Type::Proc {
                function,
                block,
                self_type,
            } => Type::Proc {
                function: Box::new(function.map_type(transform)),
                block: block
                    .as_ref()
                    .map(|block| Box::new(block.map_type(transform))),
                self_type: self_type
                    .as_ref()
                    .map(|self_type| Box::new(transform(self_type))),
            },
            _ => self.clone(),
        }
    }

    pub fn each_child(&self) -> Vec<&Type> {
        match self {
            Type::NameInstance { args: types, .. }
            | Type::Interface { args: types, .. }
            | Type::Alias { args: types, .. }
            | Type::Union(types)
            | Type::Intersection(types)
            | Type::Tuple(types) => types.iter().collect(),
            Type::Record(elements) => elements.iter().map(|element| &element.type_).collect(),
            Type::Proc {
                function,
                block,
                self_type,
            } => {
                let mut children = function.each_type();
                if let Some(block) = block {
                    children.extend(block.type_.each_type());
                }
                if let Some(self_type) = self_type {
                    children.extend(self_type.each_child());
                }
                children
            }
            _ => Vec::new(),
        }
    }

    pub fn level(&self) -> Result<Vec<usize>, RbsError> {
        match self {
            Type::Any => Ok(vec![1]),
            Type::Top | Type::Bot => Ok(vec![2]),
            Type::NameInstance { args: types, .. }
            | Type::Interface { args: types, .. }
            | Type::Alias { args: types, .. }
            | Type::Union(types)
            | Type::Intersection(types)
            | Type::Tuple(types) => level_with_children(types.iter().collect()),
            Type::Record(elements) => {
                level_with_children(elements.iter().map(|element| &element.type_).collect())
            }
            Type::Proc {
                function,
                block,
                self_type,
            } => {
                let Some(params) = &function.params else {
                    return Err(format!("undefined method `+' for nil: {self}").into());
                };
                let mut children: Vec<&Type> = params.each_type().collect();
                children.push(&function.return_type);
                if let Some(block) = block {
                    if let Some(params) = &block.type_.params {
                        children.extend(params.each_type());
                    }
                    children.push(&block.type_.return_type);
                }
                if let Some(self_type) = self_type {
                    children.push(self_type);
                }
                level_with_children(children)
            }
            _ => Ok(vec![0]),
        }
    }
}

pub fn literal_back_type(literal: &Literal) -> Type {
    let name = match literal {
        Literal::Integer(_) => "::Integer",
        Literal::String(_) => "::String",
        Literal::Symbol(_) => "::Symbol",
        Literal::Bool(true) => "::TrueClass",
        Literal::Bool(false) => "::FalseClass",
    };
    Type::NameInstance {
        name: TypeName::parse(name),
        args: Vec::new(),
    }
}

fn literal_to_string(literal: &Literal) -> String {
    match literal {
        Literal::Integer(_) => literal.inspect(),
        Literal::String(string) | Literal::Symbol(string) => string.clone(),
        Literal::Bool(value) => value.to_string(),
    }
}

fn join(types: &[Type], separator: &str) -> String {
    types
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(separator)
}

fn application(formatter: &mut fmt::Formatter<'_>, name: &TypeName, args: &[Type]) -> fmt::Result {
    if args.is_empty() {
        write!(formatter, "{name}")
    } else {
        write!(formatter, "{name}[{}]", join(args, ", "))
    }
}

impl fmt::Display for Type {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Type::Any => formatter.write_str("untyped"),
            Type::Class => formatter.write_str("class"),
            Type::Instance => formatter.write_str("instance"),
            Type::SelfType => formatter.write_str("self"),
            Type::Top => formatter.write_str("top"),
            Type::Bot => formatter.write_str("bot"),
            Type::Boolean => formatter.write_str("bool"),
            Type::Void => formatter.write_str("void"),
            Type::Nil => formatter.write_str("nil"),
            Type::Var(name) => formatter.write_str(name),
            Type::Singleton(name) => write!(formatter, "singleton({name})"),
            Type::NameInstance { name, args }
            | Type::Interface { name, args }
            | Type::Alias { name, args } => application(formatter, name, args),
            Type::Union(types) => write!(formatter, "({})", join(types, " | ")),
            Type::Intersection(types) => write!(formatter, "({})", join(types, " & ")),
            Type::Literal(literal) => formatter.write_str(&literal.inspect()),
            Type::Tuple(types) => write!(formatter, "[{}]", join(types, ", ")),
            Type::Record(elements) => {
                let mut sorted: Vec<&RecordElement> = elements.iter().collect();
                sorted.sort_by_key(|element| literal_to_string(&element.key));
                let elements: Vec<String> = sorted
                    .into_iter()
                    .map(|element| {
                        let optional = if element.required { "" } else { "?" };
                        format!("{optional}{} => {}", element.key.inspect(), element.type_)
                    })
                    .collect();
                write!(formatter, "{{ {} }}", elements.join(", "))
            }
            Type::Proc {
                function,
                block,
                self_type,
            } => {
                let self_binding = self_type
                    .as_ref()
                    .map(|self_type| format!("[self: {self_type}] "))
                    .unwrap_or_default();
                let parameters = function.parameters_to_string();
                match block {
                    Some(block) => write!(
                        formatter,
                        "^{parameters} {self_binding}{block} -> {}",
                        function.return_type
                    ),
                    None => write!(
                        formatter,
                        "^{parameters} {self_binding}-> {}",
                        function.return_type
                    ),
                }
            }
            Type::Logic(logic) => write!(formatter, "<% {} %>", logic.class_name()),
        }
    }
}
