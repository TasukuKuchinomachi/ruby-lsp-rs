use std::collections::HashMap;
use std::fmt;

/// Fully qualified name without the leading `::`, e.g. `Foo::Bar`, `Foo::_Each`, `Foo::bar` (type alias)
pub type TypeName = String;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Type {
    Untyped,
    Top,
    Bottom,
    Void,
    Nil,
    Bool,
    SelfType,
    Instance,
    Class,
    ClassInstance {
        name: TypeName,
        arguments: Vec<Type>,
    },
    ClassSingleton {
        name: TypeName,
    },
    Interface {
        name: TypeName,
        arguments: Vec<Type>,
    },
    Alias {
        name: TypeName,
        arguments: Vec<Type>,
    },
    Union(Vec<Type>),
    Intersection(Vec<Type>),
    Optional(Box<Type>),
    Tuple(Vec<Type>),
    Record(Vec<RecordField>),
    Proc(Box<ProcType>),
    Literal(Literal),
    Variable(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Literal {
    Integer(String),
    String(String),
    Symbol(String),
    True,
    False,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RecordField {
    pub name: String,
    pub field_type: Type,
    pub required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProcType {
    pub function: Function,
    pub block: Option<Block>,
    pub self_type: Option<Type>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Parameter {
    pub parameter_type: Type,
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Function {
    pub required_positionals: Vec<Parameter>,
    pub optional_positionals: Vec<Parameter>,
    pub rest_positionals: Option<Parameter>,
    pub trailing_positionals: Vec<Parameter>,
    pub required_keywords: Vec<(String, Parameter)>,
    pub optional_keywords: Vec<(String, Parameter)>,
    pub rest_keywords: Option<Parameter>,
    pub return_type: Type,
    /// `(?) -> T`
    pub untyped_parameters: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Block {
    pub function: Function,
    pub required: bool,
    pub self_type: Option<Type>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TypeParameter {
    pub name: String,
    pub upper_bound: Option<Type>,
    pub default_type: Option<Type>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MethodType {
    pub type_parameters: Vec<TypeParameter>,
    pub function: Function,
    pub block: Option<Block>,
}

/// Replacement applied to a type. `self_type`, `instance_type` and `class_type` replace `self`, `instance` and `class`
#[derive(Debug, Clone, Default)]
pub struct Substitution {
    pub variables: HashMap<String, Type>,
    pub self_type: Option<Type>,
    pub instance_type: Option<Type>,
    pub class_type: Option<Type>,
}

impl Substitution {
    pub fn is_empty(&self) -> bool {
        self.variables.is_empty()
            && self.self_type.is_none()
            && self.instance_type.is_none()
            && self.class_type.is_none()
    }
}

impl Type {
    pub fn instance(name: &str) -> Type {
        Type::ClassInstance {
            name: name.to_string(),
            arguments: Vec::new(),
        }
    }

    pub fn generic(name: &str, arguments: Vec<Type>) -> Type {
        Type::ClassInstance {
            name: name.to_string(),
            arguments,
        }
    }

    pub fn is_untyped(&self) -> bool {
        matches!(self, Type::Untyped)
    }

    /// Builds a union, flattening nested unions, removing duplicates and collapsing `T | nil` into `T?`
    pub fn union(types: Vec<Type>) -> Type {
        let mut members: Vec<Type> = Vec::new();
        let mut includes_nil = false;
        let push = |members: &mut Vec<Type>, member: Type| {
            if !members.contains(&member) {
                members.push(member);
            }
        };
        let mut pending = types;
        pending.reverse();
        while let Some(member) = pending.pop() {
            match member {
                Type::Union(inner) => pending.extend(inner.into_iter().rev()),
                Type::Optional(inner) => {
                    includes_nil = true;
                    pending.push(*inner);
                }
                Type::Nil => includes_nil = true,
                Type::Bottom => {}
                Type::Untyped => return Type::Untyped,
                other => push(&mut members, other),
            }
        }
        let body = match members.len() {
            0 => {
                return if includes_nil {
                    Type::Nil
                } else {
                    Type::Bottom
                };
            }
            1 => members.pop().unwrap(),
            _ => Type::Union(members),
        };
        if includes_nil {
            Type::Optional(Box::new(body))
        } else {
            body
        }
    }

    pub fn optional(inner: Type) -> Type {
        Type::union(vec![inner, Type::Nil])
    }

    pub fn substitute(&self, substitution: &Substitution) -> Type {
        if substitution.is_empty() {
            return self.clone();
        }
        match self {
            Type::Variable(name) => substitution
                .variables
                .get(name)
                .cloned()
                .unwrap_or_else(|| self.clone()),
            Type::SelfType => substitution.self_type.clone().unwrap_or(Type::SelfType),
            Type::Instance => substitution.instance_type.clone().unwrap_or(Type::Instance),
            Type::Class => substitution.class_type.clone().unwrap_or(Type::Class),
            Type::ClassInstance { name, arguments } => Type::ClassInstance {
                name: name.clone(),
                arguments: substitute_all(arguments, substitution),
            },
            Type::Interface { name, arguments } => Type::Interface {
                name: name.clone(),
                arguments: substitute_all(arguments, substitution),
            },
            Type::Alias { name, arguments } => Type::Alias {
                name: name.clone(),
                arguments: substitute_all(arguments, substitution),
            },
            Type::Union(types) => Type::union(substitute_all(types, substitution)),
            Type::Intersection(types) => Type::Intersection(substitute_all(types, substitution)),
            Type::Optional(inner) => Type::optional(inner.substitute(substitution)),
            Type::Tuple(types) => Type::Tuple(substitute_all(types, substitution)),
            Type::Record(fields) => Type::Record(
                fields
                    .iter()
                    .map(|field| RecordField {
                        name: field.name.clone(),
                        field_type: field.field_type.substitute(substitution),
                        required: field.required,
                    })
                    .collect(),
            ),
            Type::Proc(proc_type) => Type::Proc(Box::new(ProcType {
                function: proc_type.function.substitute(substitution),
                block: proc_type
                    .block
                    .as_ref()
                    .map(|block| block.substitute(substitution)),
                self_type: proc_type
                    .self_type
                    .as_ref()
                    .map(|self_type| self_type.substitute(substitution)),
            })),
            Type::Untyped
            | Type::Top
            | Type::Bottom
            | Type::Void
            | Type::Nil
            | Type::Bool
            | Type::ClassSingleton { .. }
            | Type::Literal(_) => self.clone(),
        }
    }

    /// Visits every type in the tree, including `self`, depth first
    pub fn walk(&self, visitor: &mut dyn FnMut(&Type)) {
        visitor(self);
        match self {
            Type::ClassInstance { arguments, .. }
            | Type::Interface { arguments, .. }
            | Type::Alias { arguments, .. } => {
                arguments.iter().for_each(|argument| argument.walk(visitor))
            }
            Type::Union(types) | Type::Intersection(types) | Type::Tuple(types) => {
                types.iter().for_each(|member| member.walk(visitor))
            }
            Type::Optional(inner) => inner.walk(visitor),
            Type::Record(fields) => fields
                .iter()
                .for_each(|field| field.field_type.walk(visitor)),
            Type::Proc(proc_type) => {
                proc_type.function.walk(visitor);
                if let Some(block) = &proc_type.block {
                    block.function.walk(visitor);
                }
            }
            _ => {}
        }
    }

    pub fn map_names(&mut self, resolve: &mut dyn FnMut(NameKind, &str) -> String) {
        match self {
            Type::ClassInstance { name, arguments } => {
                *name = resolve(NameKind::Class, name);
                arguments
                    .iter_mut()
                    .for_each(|argument| argument.map_names(resolve));
            }
            Type::ClassSingleton { name } => *name = resolve(NameKind::Class, name),
            Type::Interface { name, arguments } => {
                *name = resolve(NameKind::Interface, name);
                arguments
                    .iter_mut()
                    .for_each(|argument| argument.map_names(resolve));
            }
            Type::Alias { name, arguments } => {
                *name = resolve(NameKind::Alias, name);
                arguments
                    .iter_mut()
                    .for_each(|argument| argument.map_names(resolve));
            }
            Type::Union(types) | Type::Intersection(types) | Type::Tuple(types) => types
                .iter_mut()
                .for_each(|member| member.map_names(resolve)),
            Type::Optional(inner) => inner.map_names(resolve),
            Type::Record(fields) => fields
                .iter_mut()
                .for_each(|field| field.field_type.map_names(resolve)),
            Type::Proc(proc_type) => {
                proc_type.function.map_names(resolve);
                if let Some(block) = &mut proc_type.block {
                    block.map_names(resolve);
                }
                if let Some(self_type) = &mut proc_type.self_type {
                    self_type.map_names(resolve);
                }
            }
            _ => {}
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameKind {
    Class,
    Interface,
    Alias,
}

fn substitute_all(types: &[Type], substitution: &Substitution) -> Vec<Type> {
    types
        .iter()
        .map(|member| member.substitute(substitution))
        .collect()
}

impl Parameter {
    pub fn new(parameter_type: Type) -> Parameter {
        Parameter {
            parameter_type,
            name: None,
        }
    }

    fn substitute(&self, substitution: &Substitution) -> Parameter {
        Parameter {
            parameter_type: self.parameter_type.substitute(substitution),
            name: self.name.clone(),
        }
    }
}

impl Function {
    pub fn returning(return_type: Type) -> Function {
        Function {
            required_positionals: Vec::new(),
            optional_positionals: Vec::new(),
            rest_positionals: None,
            trailing_positionals: Vec::new(),
            required_keywords: Vec::new(),
            optional_keywords: Vec::new(),
            rest_keywords: None,
            return_type,
            untyped_parameters: false,
        }
    }

    pub fn substitute(&self, substitution: &Substitution) -> Function {
        let substitute_list = |parameters: &[Parameter]| {
            parameters
                .iter()
                .map(|parameter| parameter.substitute(substitution))
                .collect()
        };
        let substitute_keywords = |keywords: &[(String, Parameter)]| {
            keywords
                .iter()
                .map(|(name, parameter)| (name.clone(), parameter.substitute(substitution)))
                .collect()
        };
        Function {
            required_positionals: substitute_list(&self.required_positionals),
            optional_positionals: substitute_list(&self.optional_positionals),
            rest_positionals: self
                .rest_positionals
                .as_ref()
                .map(|parameter| parameter.substitute(substitution)),
            trailing_positionals: substitute_list(&self.trailing_positionals),
            required_keywords: substitute_keywords(&self.required_keywords),
            optional_keywords: substitute_keywords(&self.optional_keywords),
            rest_keywords: self
                .rest_keywords
                .as_ref()
                .map(|parameter| parameter.substitute(substitution)),
            return_type: self.return_type.substitute(substitution),
            untyped_parameters: self.untyped_parameters,
        }
    }

    pub fn parameters(&self) -> impl Iterator<Item = &Parameter> {
        self.required_positionals
            .iter()
            .chain(self.optional_positionals.iter())
            .chain(self.rest_positionals.iter())
            .chain(self.trailing_positionals.iter())
            .chain(
                self.required_keywords
                    .iter()
                    .map(|(_, parameter)| parameter),
            )
            .chain(
                self.optional_keywords
                    .iter()
                    .map(|(_, parameter)| parameter),
            )
            .chain(self.rest_keywords.iter())
    }

    fn walk(&self, visitor: &mut dyn FnMut(&Type)) {
        self.parameters()
            .for_each(|parameter| parameter.parameter_type.walk(visitor));
        self.return_type.walk(visitor);
    }

    pub fn map_names(&mut self, resolve: &mut dyn FnMut(NameKind, &str) -> String) {
        for parameter in self
            .required_positionals
            .iter_mut()
            .chain(self.optional_positionals.iter_mut())
            .chain(self.rest_positionals.iter_mut())
            .chain(self.trailing_positionals.iter_mut())
            .chain(
                self.required_keywords
                    .iter_mut()
                    .map(|(_, parameter)| parameter),
            )
            .chain(
                self.optional_keywords
                    .iter_mut()
                    .map(|(_, parameter)| parameter),
            )
            .chain(self.rest_keywords.iter_mut())
        {
            parameter.parameter_type.map_names(resolve);
        }
        self.return_type.map_names(resolve);
    }
}

impl Block {
    pub fn substitute(&self, substitution: &Substitution) -> Block {
        Block {
            function: self.function.substitute(substitution),
            required: self.required,
            self_type: self
                .self_type
                .as_ref()
                .map(|self_type| self_type.substitute(substitution)),
        }
    }

    pub fn map_names(&mut self, resolve: &mut dyn FnMut(NameKind, &str) -> String) {
        self.function.map_names(resolve);
        if let Some(self_type) = &mut self.self_type {
            self_type.map_names(resolve);
        }
    }
}

impl MethodType {
    pub fn substitute(&self, substitution: &Substitution) -> MethodType {
        MethodType {
            type_parameters: self.type_parameters.clone(),
            function: self.function.substitute(substitution),
            block: self
                .block
                .as_ref()
                .map(|block| block.substitute(substitution)),
        }
    }

    pub fn map_names(&mut self, resolve: &mut dyn FnMut(NameKind, &str) -> String) {
        for type_parameter in &mut self.type_parameters {
            if let Some(upper_bound) = &mut type_parameter.upper_bound {
                upper_bound.map_names(resolve);
            }
            if let Some(default_type) = &mut type_parameter.default_type {
                default_type.map_names(resolve);
            }
        }
        self.function.map_names(resolve);
        if let Some(block) = &mut self.block {
            block.map_names(resolve);
        }
    }
}

impl fmt::Display for Type {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Type::Untyped => formatter.write_str("untyped"),
            Type::Top => formatter.write_str("top"),
            Type::Bottom => formatter.write_str("bot"),
            Type::Void => formatter.write_str("void"),
            Type::Nil => formatter.write_str("nil"),
            Type::Bool => formatter.write_str("bool"),
            Type::SelfType => formatter.write_str("self"),
            Type::Instance => formatter.write_str("instance"),
            Type::Class => formatter.write_str("class"),
            Type::ClassInstance { name, arguments }
            | Type::Interface { name, arguments }
            | Type::Alias { name, arguments } => {
                formatter.write_str(name)?;
                write_arguments(formatter, arguments)
            }
            Type::ClassSingleton { name } => write!(formatter, "singleton({name})"),
            Type::Union(types) => write_joined(formatter, types, " | ", true),
            Type::Intersection(types) => write_joined(formatter, types, " & ", true),
            Type::Optional(inner) => {
                if matches!(
                    **inner,
                    Type::Union(_) | Type::Intersection(_) | Type::Proc(_)
                ) {
                    write!(formatter, "({inner})?")
                } else {
                    write!(formatter, "{inner}?")
                }
            }
            Type::Tuple(types) => {
                formatter.write_str("[")?;
                write_joined(formatter, types, ", ", false)?;
                formatter.write_str("]")
            }
            Type::Record(fields) => {
                formatter.write_str("{ ")?;
                for (index, field) in fields.iter().enumerate() {
                    if index > 0 {
                        formatter.write_str(", ")?;
                    }
                    let optional_marker = if field.required { "" } else { "?" };
                    write!(
                        formatter,
                        "{optional_marker}{}: {}",
                        field.name, field.field_type
                    )?;
                }
                formatter.write_str(" }")
            }
            Type::Proc(proc_type) => {
                formatter.write_str("^")?;
                write_function(formatter, &proc_type.function, proc_type.block.as_ref())
            }
            Type::Literal(literal) => match literal {
                Literal::Integer(value) => formatter.write_str(value),
                Literal::String(value) => write!(formatter, "{value:?}"),
                Literal::Symbol(value) => write!(formatter, ":{value}"),
                Literal::True => formatter.write_str("true"),
                Literal::False => formatter.write_str("false"),
            },
            Type::Variable(name) => formatter.write_str(name),
        }
    }
}

impl fmt::Display for MethodType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if !self.type_parameters.is_empty() {
            let names: Vec<&str> = self
                .type_parameters
                .iter()
                .map(|type_parameter| type_parameter.name.as_str())
                .collect();
            write!(formatter, "[{}] ", names.join(", "))?;
        }
        write_function(formatter, &self.function, self.block.as_ref())
    }
}

fn write_arguments(formatter: &mut fmt::Formatter<'_>, arguments: &[Type]) -> fmt::Result {
    if arguments.is_empty() {
        return Ok(());
    }
    formatter.write_str("[")?;
    write_joined(formatter, arguments, ", ", false)?;
    formatter.write_str("]")
}

fn write_joined(
    formatter: &mut fmt::Formatter<'_>,
    types: &[Type],
    separator: &str,
    parenthesize_nested: bool,
) -> fmt::Result {
    for (index, member) in types.iter().enumerate() {
        if index > 0 {
            formatter.write_str(separator)?;
        }
        if parenthesize_nested
            && matches!(
                member,
                Type::Union(_) | Type::Intersection(_) | Type::Proc(_)
            )
        {
            write!(formatter, "({member})")?;
        } else {
            write!(formatter, "{member}")?;
        }
    }
    Ok(())
}

fn write_parameters(formatter: &mut fmt::Formatter<'_>, function: &Function) -> fmt::Result {
    if function.untyped_parameters {
        return formatter.write_str("(?)");
    }
    let mut parts: Vec<String> = Vec::new();
    let describe = |parameter: &Parameter| match &parameter.name {
        Some(name) => format!("{} {name}", parameter.parameter_type),
        None => parameter.parameter_type.to_string(),
    };
    parts.extend(function.required_positionals.iter().map(describe));
    parts.extend(
        function
            .optional_positionals
            .iter()
            .map(|parameter| format!("?{}", describe(parameter))),
    );
    if let Some(parameter) = &function.rest_positionals {
        parts.push(format!("*{}", describe(parameter)));
    }
    parts.extend(function.trailing_positionals.iter().map(describe));
    parts.extend(
        function
            .required_keywords
            .iter()
            .map(|(name, parameter)| format!("{name}: {}", describe(parameter))),
    );
    parts.extend(
        function
            .optional_keywords
            .iter()
            .map(|(name, parameter)| format!("?{name}: {}", describe(parameter))),
    );
    if let Some(parameter) = &function.rest_keywords {
        parts.push(format!("**{}", describe(parameter)));
    }
    write!(formatter, "({})", parts.join(", "))
}

fn write_function(
    formatter: &mut fmt::Formatter<'_>,
    function: &Function,
    block: Option<&Block>,
) -> fmt::Result {
    write_parameters(formatter, function)?;
    if let Some(block) = block {
        let optional_marker = if block.required { "" } else { "?" };
        write!(formatter, " {optional_marker}{{ ")?;
        write_parameters(formatter, &block.function)?;
        write!(formatter, " -> {} }}", block.function.return_type)?;
    }
    write!(formatter, " -> {}", function.return_type)
}
