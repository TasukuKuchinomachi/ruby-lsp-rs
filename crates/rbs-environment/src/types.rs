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
    /// `T?` is a union with `nil`, which keeps the place of `nil` among the members as Steep does
    Union(Vec<Type>),
    Intersection(Vec<Type>),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Variance {
    #[default]
    Invariant,
    /// `out T`
    Covariant,
    /// `in T`
    Contravariant,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TypeParameter {
    pub name: String,
    pub upper_bound: Option<Type>,
    pub default_type: Option<Type>,
    pub variance: Variance,
    /// `unchecked out T`, whose variance RBS does not check
    pub unchecked: bool,
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

    /// Builds a union as Steep's `Union.build` does: nested unions are flattened, duplicates removed in order, `bot`
    /// dropped, and `untyped` or `top` swallows the rest
    pub fn union(types: Vec<Type>) -> Type {
        let mut members: Vec<Type> = Vec::new();
        let mut pending = types;
        pending.reverse();
        while let Some(member) = pending.pop() {
            match member {
                Type::Union(inner) => pending.extend(inner.into_iter().rev()),
                Type::Bottom => {}
                Type::Untyped => return Type::Untyped,
                Type::Top => return Type::Top,
                other => {
                    if !members.contains(&other) {
                        members.push(other);
                    }
                }
            }
        }
        match members.len() {
            0 => Type::Bottom,
            1 => members.pop().unwrap(),
            _ => Type::Union(members),
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
            Type::Union(types) if types.contains(&Type::Nil) => {
                let others: Vec<Type> = types.iter().filter(|member| **member != Type::Nil).cloned().collect();
                match others.as_slice() {
                    [single] if !matches!(single, Type::Intersection(_) | Type::Proc(_)) => write!(formatter, "{single}?"),
                    _ => {
                        formatter.write_str("(")?;
                        write_joined(formatter, &others, " | ", true)?;
                        formatter.write_str(")?")
                    }
                }
            }
            Type::Union(types) => write_joined(formatter, types, " | ", true),
            Type::Intersection(types) => write_joined(formatter, types, " & ", true),
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

/// Writes a type as Steep's `to_s` does in diagnostic messages: names are absolute (`::Integer`), unions and
/// intersections are always parenthesized, `T?` is `(T | nil)` and records use `:key => T`
pub struct SteepDisplay<'a>(pub &'a Type);

/// Writes a method type as Steep's `Interface::MethodType#to_s` does
pub struct SteepMethodTypeDisplay<'a>(pub &'a MethodType);

impl Type {
    pub fn steep_display(&self) -> SteepDisplay<'_> {
        SteepDisplay(self)
    }
}

impl MethodType {
    pub fn steep_display(&self) -> SteepMethodTypeDisplay<'_> {
        SteepMethodTypeDisplay(self)
    }
}

fn steep_joined(types: &[Type], separator: &str) -> String {
    types
        .iter()
        .map(|member| member.steep_display().to_string())
        .collect::<Vec<String>>()
        .join(separator)
}

impl fmt::Display for SteepDisplay<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
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
                write!(formatter, "::{name}")?;
                if !arguments.is_empty() {
                    write!(formatter, "[{}]", steep_joined(arguments, ", "))?;
                }
                Ok(())
            }
            Type::ClassSingleton { name } => write!(formatter, "singleton(::{name})"),
            Type::Union(types) => write!(formatter, "({})", steep_joined(types, " | ")),
            Type::Intersection(types) => write!(formatter, "({})", steep_joined(types, " & ")),
            Type::Tuple(types) => write!(formatter, "[{}]", steep_joined(types, ", ")),
            Type::Record(fields) => {
                let mut sorted: Vec<&RecordField> = fields.iter().collect();
                sorted.sort_by(|left, right| left.name.cmp(&right.name));
                let entries: Vec<String> = sorted
                    .iter()
                    .map(|field| {
                        let optional_marker = if field.required { "" } else { "?" };
                        format!(
                            "{optional_marker}{} => {}",
                            ruby_symbol_inspect(&field.name),
                            field.field_type.steep_display()
                        )
                    })
                    .collect();
                write!(formatter, "{{ {} }}", entries.join(", "))
            }
            Type::Proc(proc_type) => {
                let self_binding = proc_type
                    .self_type
                    .as_ref()
                    .map(|self_type| format!("[self: {}] ", self_type.steep_display()))
                    .unwrap_or_default();
                let parameters = steep_parameters(&proc_type.function).unwrap_or_default();
                match &proc_type.block {
                    Some(block) => write!(
                        formatter,
                        "^{parameters} {self_binding}{} -> {}",
                        SteepBlockDisplay(block),
                        proc_type.function.return_type.steep_display()
                    ),
                    None => write!(
                        formatter,
                        "^{parameters} {self_binding}-> {}",
                        proc_type.function.return_type.steep_display()
                    ),
                }
            }
            Type::Literal(literal) => match literal {
                Literal::Integer(value) => formatter.write_str(value),
                Literal::String(value) => formatter.write_str(&ruby_string_inspect(value)),
                Literal::Symbol(value) => formatter.write_str(&ruby_symbol_inspect(value)),
                Literal::True => formatter.write_str("true"),
                Literal::False => formatter.write_str("false"),
            },
            Type::Variable(name) => formatter.write_str(name),
        }
    }
}

struct SteepBlockDisplay<'a>(&'a Block);

impl fmt::Display for SteepBlockDisplay<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let block = self.0;
        let optional_marker = if block.required { "" } else { "?" };
        let self_binding = block
            .self_type
            .as_ref()
            .map(|self_type| format!("[self: {}] ", self_type.steep_display()))
            .unwrap_or_default();
        write!(
            formatter,
            "{optional_marker}{{ {} {self_binding}-> {} }}",
            steep_parameters(&block.function).unwrap_or_default(),
            block.function.return_type.steep_display()
        )
    }
}

impl fmt::Display for SteepMethodTypeDisplay<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let method_type = self.0;
        if !method_type.type_parameters.is_empty() {
            let parameters: Vec<String> = method_type
                .type_parameters
                .iter()
                .map(|parameter| match &parameter.upper_bound {
                    Some(upper_bound) => format!("{} < {}", parameter.name, upper_bound.steep_display()),
                    None => parameter.name.clone(),
                })
                .collect();
            write!(formatter, "[{}] ", parameters.join(", "))?;
        }
        let parameters = steep_parameters(&method_type.function).unwrap_or_else(|| "(?)".to_string());
        formatter.write_str(&parameters)?;
        if let Some(block) = &method_type.block {
            write!(formatter, " {}", SteepBlockDisplay(block))?;
        }
        write!(formatter, " -> {}", method_type.function.return_type.steep_display())
    }
}

/// Steep's `Params#to_s`, which leaves out the required positionals after a rest. `None` for `(?)`
fn steep_parameters(function: &Function) -> Option<String> {
    if function.untyped_parameters {
        return None;
    }
    let mut parts: Vec<String> = Vec::new();
    parts.extend(
        function
            .required_positionals
            .iter()
            .map(|parameter| parameter.parameter_type.steep_display().to_string()),
    );
    parts.extend(
        function
            .optional_positionals
            .iter()
            .map(|parameter| format!("?{}", parameter.parameter_type.steep_display())),
    );
    if let Some(parameter) = &function.rest_positionals {
        parts.push(format!("*{}", parameter.parameter_type.steep_display()));
    }
    parts.extend(
        function
            .required_keywords
            .iter()
            .map(|(name, parameter)| format!("{name}: {}", parameter.parameter_type.steep_display())),
    );
    parts.extend(
        function
            .optional_keywords
            .iter()
            .map(|(name, parameter)| format!("?{name}: {}", parameter.parameter_type.steep_display())),
    );
    if let Some(parameter) = &function.rest_keywords {
        parts.push(format!("**{}", parameter.parameter_type.steep_display()));
    }
    Some(format!("({})", parts.join(", ")))
}

/// `Symbol#inspect`: `:foo`, or `:"foo bar"` when the name is not a plain identifier, operator or variable name
fn ruby_symbol_inspect(name: &str) -> String {
    let is_identifier = |text: &str| {
        let mut characters = text.chars();
        characters
            .next()
            .is_some_and(|first| first.is_alphabetic() || first == '_' || !first.is_ascii())
            && characters.all(|character| character.is_alphanumeric() || character == '_' || !character.is_ascii())
    };
    let plain = is_identifier(name)
        || name.strip_suffix(['?', '!', '=']).is_some_and(is_identifier)
        || name
            .strip_prefix("@@")
            .or_else(|| name.strip_prefix('@'))
            .or_else(|| name.strip_prefix('$'))
            .is_some_and(is_identifier)
        || matches!(
            name,
            "+" | "-"
                | "*"
                | "/"
                | "%"
                | "**"
                | "=="
                | "==="
                | "!="
                | "<"
                | "<="
                | ">"
                | ">="
                | "<=>"
                | "<<"
                | ">>"
                | "!"
                | "=~"
                | "!~"
                | "&"
                | "|"
                | "^"
                | "~"
                | "+@"
                | "-@"
                | "[]"
                | "[]="
                | "`"
        );
    if plain {
        format!(":{name}")
    } else {
        format!(":{}", ruby_string_inspect(name))
    }
}

/// `String#inspect` for the characters a literal type can hold
fn ruby_string_inspect(value: &str) -> String {
    let mut inspected = String::from("\"");
    let mut characters = value.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '#' if characters.peek().is_some_and(|next| matches!(next, '{' | '$' | '@')) => inspected.push_str("\\#"),
            '"' => inspected.push_str("\\\""),
            '\\' => inspected.push_str("\\\\"),
            '\n' => inspected.push_str("\\n"),
            '\t' => inspected.push_str("\\t"),
            '\r' => inspected.push_str("\\r"),
            '\u{1b}' => inspected.push_str("\\e"),
            character if character.is_control() => inspected.push_str(&format!("\\x{:02X}", character as u32)),
            character => inspected.push(character),
        }
    }
    inspected.push('"');
    inspected
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
