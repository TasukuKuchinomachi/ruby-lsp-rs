use std::{fmt, sync::Arc};

use crate::location::{AstLocation, Location, TypeLocation};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Namespace {
    pub absolute: bool,
    pub path: Vec<String>,
}

impl Namespace {
    pub fn root() -> Namespace {
        Namespace {
            absolute: true,
            path: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.path.is_empty()
    }

    pub fn append(&self, name: &str) -> Namespace {
        let mut path = self.path.clone();
        path.push(name.to_string());
        Namespace {
            absolute: self.absolute,
            path,
        }
    }

    pub fn join(&self, other: &Namespace) -> Namespace {
        if other.absolute {
            return other.clone();
        }
        let mut path = self.path.clone();
        path.extend(other.path.iter().cloned());
        Namespace {
            absolute: self.absolute,
            path,
        }
    }

    pub fn to_type_name(&self) -> TypeName {
        let mut path = self.path.clone();
        let name = path.pop().expect("an empty namespace has no type name");
        TypeName {
            namespace: Namespace {
                absolute: self.absolute,
                path,
            },
            name,
        }
    }
}

impl fmt::Display for Namespace {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.absolute {
            formatter.write_str("::")?;
        }
        for name in &self.path {
            write!(formatter, "{name}::")?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TypeName {
    pub namespace: Namespace,
    pub name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TypeNameKind {
    Class,
    Interface,
    Alias,
}

impl TypeName {
    pub fn new(namespace: Namespace, name: &str) -> TypeName {
        TypeName {
            namespace,
            name: name.to_string(),
        }
    }

    pub fn parse(string: &str) -> TypeName {
        let absolute = string.starts_with("::");
        let mut path: Vec<String> = string
            .trim_start_matches("::")
            .split("::")
            .map(str::to_string)
            .collect();
        let name = path.pop().unwrap_or_default();
        TypeName {
            namespace: Namespace { absolute, path },
            name,
        }
    }

    pub fn kind(&self) -> TypeNameKind {
        if self.name.starts_with('_') {
            TypeNameKind::Interface
        } else if self
            .name
            .starts_with(|character: char| character.is_uppercase())
        {
            TypeNameKind::Class
        } else {
            TypeNameKind::Alias
        }
    }

    pub fn is_class(&self) -> bool {
        self.kind() == TypeNameKind::Class
    }

    pub fn is_absolute(&self) -> bool {
        self.namespace.absolute
    }

    pub fn to_namespace(&self) -> Namespace {
        self.namespace.append(&self.name)
    }

    pub fn with_prefix(&self, prefix: &Namespace) -> TypeName {
        TypeName {
            namespace: prefix.join(&self.namespace),
            name: self.name.clone(),
        }
    }

    pub fn absolute(&self) -> TypeName {
        TypeName {
            namespace: Namespace {
                absolute: true,
                path: self.namespace.path.clone(),
            },
            name: self.name.clone(),
        }
    }

    pub fn join(&self, other: &TypeName) -> TypeName {
        other.with_prefix(&self.to_namespace())
    }
}

impl fmt::Display for TypeName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}{}", self.namespace, self.name)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Literal {
    Integer(String),
    String(String),
    Symbol(String),
    Bool(bool),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RecordField {
    pub key: Literal,
    pub type_: Type,
    pub required: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Type {
    Bool,
    Void,
    Any,
    Nil,
    Top,
    Bottom,
    SelfType,
    Instance,
    Class,
    Variable(String),
    ClassInstance {
        name: TypeName,
        args: Vec<Type>,
        location: TypeLocation,
    },
    ClassSingleton {
        name: TypeName,
        args: Vec<Type>,
        location: TypeLocation,
    },
    Interface {
        name: TypeName,
        args: Vec<Type>,
        location: TypeLocation,
    },
    Alias {
        name: TypeName,
        args: Vec<Type>,
        location: TypeLocation,
    },
    Tuple(Vec<Type>),
    Record(Vec<RecordField>),
    Optional(Box<Type>),
    Union(Vec<Type>),
    Intersection(Vec<Type>),
    Proc {
        function: Box<Callable>,
        block: Option<Box<Block>>,
        self_type: Option<Box<Type>>,
    },
    Literal(Literal),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Parameter {
    pub type_: Type,
    pub name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Function {
    pub required_positionals: Vec<Parameter>,
    pub optional_positionals: Vec<Parameter>,
    pub rest_positionals: Option<Parameter>,
    pub trailing_positionals: Vec<Parameter>,
    pub required_keywords: Vec<(String, Parameter)>,
    pub optional_keywords: Vec<(String, Parameter)>,
    pub rest_keywords: Option<Parameter>,
    pub return_type: Type,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Callable {
    Function(Box<Function>),
    Untyped { return_type: Type },
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Block {
    pub type_: Callable,
    pub required: bool,
    pub self_type: Option<Type>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Variance {
    Invariant,
    Covariant,
    Contravariant,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TypeParameter {
    pub name: String,
    pub variance: Variance,
    pub unchecked: bool,
    pub upper_bound: Option<Type>,
    pub lower_bound: Option<Type>,
    pub default_type: Option<Type>,
    pub location: AstLocation,
    pub default_type_location: AstLocation,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MethodType {
    pub type_parameters: Vec<TypeParameter>,
    pub type_: Callable,
    pub block: Option<Block>,
    pub location: AstLocation,
}

#[derive(Clone, Debug)]
pub struct Overload {
    pub method_type: Arc<MethodType>,
    pub annotations: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MethodKind {
    Instance,
    Singleton,
    SingletonInstance,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Visibility {
    Public,
    Private,
}

#[derive(Clone, Debug)]
pub struct MethodDefinition {
    pub name: String,
    pub kind: MethodKind,
    pub overloads: Vec<Overload>,
    pub overloading: bool,
    pub visibility: Option<Visibility>,
    pub annotations: Vec<String>,
    pub location: Option<Location>,
    pub declared_in_ruby: bool,
    pub unannotated_ruby_definition: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttributeKind {
    Reader,
    Writer,
    Accessor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemberKind {
    Instance,
    Singleton,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AttributeInstanceVariable {
    Unspecified,
    Empty,
    Name(String),
}

#[derive(Clone, Debug)]
pub struct Attribute {
    pub attribute_kind: AttributeKind,
    pub name: String,
    pub type_: Type,
    pub kind: MemberKind,
    pub instance_variable: AttributeInstanceVariable,
    pub visibility: Option<Visibility>,
    pub annotations: Vec<String>,
    pub location: Option<Location>,
    pub type_location: Option<Location>,
    pub declared_in_ruby: bool,
}

#[derive(Clone, Debug)]
pub struct Alias {
    pub new_name: String,
    pub old_name: String,
    pub kind: MemberKind,
    pub annotations: Vec<String>,
    pub location: Option<Location>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MixinKind {
    Include,
    Extend,
    Prepend,
}

#[derive(Clone, Debug)]
pub struct Mixin {
    pub mixin_kind: MixinKind,
    pub name: TypeName,
    pub args: Vec<Type>,
    pub location: Option<Location>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VariableKind {
    InstanceVariable,
    ClassInstanceVariable,
    ClassVariable,
}

#[derive(Clone, Debug)]
pub struct Variable {
    pub variable_kind: VariableKind,
    pub name: String,
    pub type_: Type,
    pub location: Option<Location>,
    pub name_location: Option<Location>,
    pub type_location: Option<Location>,
}

#[derive(Clone, Debug)]
pub enum Member {
    MethodDefinition(MethodDefinition),
    Attribute(Attribute),
    Alias(Alias),
    Mixin(Mixin),
    Variable(Variable),
    Visibility(Visibility),
    Declaration(Declaration),
}

#[derive(Clone, Debug)]
pub struct Super {
    pub name: TypeName,
    pub args: Vec<Type>,
    pub location: Option<Location>,
}

#[derive(Clone, Debug)]
pub struct ClassDeclaration {
    pub name: TypeName,
    pub type_parameters: Vec<TypeParameter>,
    pub super_class: Option<Super>,
    pub members: Vec<Member>,
    pub annotations: Vec<String>,
    pub location: Option<Location>,
    pub name_location: Option<Location>,
    pub declared_in_ruby: bool,
}

#[derive(Clone, Debug)]
pub struct ModuleSelf {
    pub name: TypeName,
    pub args: Vec<Type>,
    pub location: Option<Location>,
}

#[derive(Clone, Debug)]
pub struct ModuleDeclaration {
    pub name: TypeName,
    pub type_parameters: Vec<TypeParameter>,
    pub self_types: Vec<ModuleSelf>,
    pub members: Vec<Member>,
    pub annotations: Vec<String>,
    pub location: Option<Location>,
    pub name_location: Option<Location>,
    pub declared_in_ruby: bool,
}

#[derive(Clone, Debug)]
pub struct InterfaceDeclaration {
    pub name: TypeName,
    pub type_parameters: Vec<TypeParameter>,
    pub members: Vec<Member>,
    pub annotations: Vec<String>,
    pub location: Option<Location>,
    pub name_location: Option<Location>,
}

#[derive(Clone, Debug)]
pub struct TypeAliasDeclaration {
    pub name: TypeName,
    pub type_parameters: Vec<TypeParameter>,
    pub type_: Type,
    pub annotations: Vec<String>,
    pub location: Option<Location>,
    pub name_location: Option<Location>,
    pub type_parameters_location: Option<Location>,
    pub type_location: Option<Location>,
}

#[derive(Clone, Debug)]
pub struct ConstantDeclaration {
    pub name: TypeName,
    pub type_: Type,
    pub annotations: Vec<String>,
    pub location: Option<Location>,
}

#[derive(Clone, Debug)]
pub struct GlobalDeclaration {
    pub name: String,
    pub type_: Type,
    pub annotations: Vec<String>,
    pub location: Option<Location>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClassOrModule {
    Class,
    Module,
}

#[derive(Clone, Debug)]
pub struct AliasDeclaration {
    pub kind: ClassOrModule,
    pub new_name: TypeName,
    pub old_name: TypeName,
    pub annotations: Vec<String>,
    pub location: Option<Location>,
    pub new_name_location: Option<Location>,
    pub old_name_location: Option<Location>,
}

#[derive(Clone, Debug)]
pub enum Declaration {
    Class(Arc<ClassDeclaration>),
    Module(Arc<ModuleDeclaration>),
    Interface(Arc<InterfaceDeclaration>),
    TypeAlias(Arc<TypeAliasDeclaration>),
    Constant(Arc<ConstantDeclaration>),
    Global(Arc<GlobalDeclaration>),
    ClassAlias(Arc<AliasDeclaration>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UseClause {
    Single {
        type_name: TypeName,
        new_name: Option<String>,
    },
    Wildcard {
        namespace: Namespace,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Directive {
    Use(Vec<UseClause>),
}

impl Literal {
    pub fn inspect(&self) -> String {
        match self {
            Literal::Integer(string) => inspect_integer(string),
            Literal::String(string) => inspect_string(string),
            Literal::Symbol(symbol) => inspect_symbol(symbol),
            Literal::Bool(value) => value.to_string(),
        }
    }
}

fn inspect_integer(string: &str) -> String {
    let digits = string.replace('_', "");
    let (negative, digits) = match digits.strip_prefix('-') {
        Some(digits) => (true, digits.to_string()),
        None => (false, digits.trim_start_matches('+').to_string()),
    };
    let lowercase = digits.to_ascii_lowercase();
    let (radix, body) = if let Some(body) = lowercase.strip_prefix("0x") {
        (16, body)
    } else if let Some(body) = lowercase.strip_prefix("0b") {
        (2, body)
    } else if let Some(body) = lowercase.strip_prefix("0o") {
        (8, body)
    } else if let Some(body) = lowercase.strip_prefix("0d") {
        (10, body)
    } else if lowercase.len() > 1 && lowercase.starts_with('0') {
        (8, &lowercase[1..])
    } else {
        (10, lowercase.as_str())
    };
    match i128::from_str_radix(body, radix) {
        Ok(value) if negative => (-value).to_string(),
        Ok(value) => value.to_string(),
        Err(_) => string.to_string(),
    }
}

fn inspect_string(string: &str) -> String {
    let mut result = String::from("\"");
    let mut characters = string.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '"' => result.push_str("\\\""),
            '\\' => result.push_str("\\\\"),
            '\n' => result.push_str("\\n"),
            '\t' => result.push_str("\\t"),
            '\r' => result.push_str("\\r"),
            '\u{1b}' => result.push_str("\\e"),
            '#' if matches!(characters.peek(), Some('{' | '$' | '@')) => result.push_str("\\#"),
            character if character.is_control() => {
                result.push_str(&format!("\\x{:02X}", character as u32))
            }
            character => result.push(character),
        }
    }
    result.push('"');
    result
}

fn is_identifier_character(character: char) -> bool {
    character.is_alphanumeric() || character == '_' || !character.is_ascii()
}

fn is_simple_symbol(symbol: &str) -> bool {
    const OPERATORS: &[&str] = &[
        "+", "-", "*", "/", "%", "**", "==", "===", "!=", "=~", "!~", "<=>", "<", "<=", ">", ">=",
        "<<", ">>", "&", "|", "^", "~", "!", "+@", "-@", "[]", "[]=", "`",
    ];
    if OPERATORS.contains(&symbol) {
        return true;
    }
    let body = symbol
        .strip_prefix("@@")
        .or_else(|| symbol.strip_prefix('@'))
        .or_else(|| symbol.strip_prefix('$'))
        .unwrap_or(symbol);
    let is_variable = body.len() != symbol.len();
    let body = if is_variable {
        body
    } else {
        body.strip_suffix(['?', '!', '=']).unwrap_or(body)
    };
    let mut characters = body.chars();
    match characters.next() {
        Some(first) if !first.is_ascii_digit() && is_identifier_character(first) => {
            characters.all(is_identifier_character)
        }
        _ => false,
    }
}

fn inspect_symbol(symbol: &str) -> String {
    if is_simple_symbol(symbol) {
        format!(":{symbol}")
    } else {
        format!(":{}", inspect_string(symbol))
    }
}

impl Type {
    pub fn to_string_with_level(&self, level: u8) -> String {
        match self {
            Type::Bool => "bool".to_string(),
            Type::Void => "void".to_string(),
            Type::Any => "untyped".to_string(),
            Type::Nil => "nil".to_string(),
            Type::Top => "top".to_string(),
            Type::Bottom => "bot".to_string(),
            Type::SelfType => "self".to_string(),
            Type::Instance => "instance".to_string(),
            Type::Class => "class".to_string(),
            Type::Variable(name) => name.clone(),
            Type::ClassInstance { name, args, .. }
            | Type::Interface { name, args, .. }
            | Type::Alias { name, args, .. } => application_to_string(&name.to_string(), args),
            Type::ClassSingleton { name, args, .. } => {
                application_to_string(&format!("singleton({name})"), args)
            }
            Type::Tuple(types) => {
                if types.is_empty() {
                    "[ ]".to_string()
                } else {
                    format!("[ {} ]", join_types(types))
                }
            }
            Type::Record(fields) => {
                if fields.is_empty() {
                    return "{ }".to_string();
                }
                let fields: Vec<String> = fields
                    .iter()
                    .map(|field| {
                        let string = match &field.key {
                            Literal::Symbol(key) if is_plain_record_key(key) => {
                                format!("{key}: {}", field.type_)
                            }
                            key => format!("{} => {}", key.inspect(), field.type_),
                        };
                        if field.required {
                            string
                        } else {
                            format!("?{string}")
                        }
                    })
                    .collect();
                format!("{{ {} }}", fields.join(", "))
            }
            Type::Optional(type_) => match type_.as_ref() {
                Type::Literal(Literal::Symbol(_)) => format!("{} ?", type_.to_string_with_level(1)),
                Type::Proc { .. } => format!("({})?", type_.to_string_with_level(1)),
                _ => format!("{}?", type_.to_string_with_level(1)),
            },
            Type::Union(types) => {
                let strings: Vec<String> = types
                    .iter()
                    .map(|type_| match type_ {
                        Type::Intersection(_) => type_.to_string_with_level(level.max(1)),
                        _ => type_.to_string(),
                    })
                    .collect();
                if level > 0 {
                    format!("({})", strings.join(" | "))
                } else {
                    strings.join(" | ")
                }
            }
            Type::Intersection(types) => {
                let strings: Vec<String> = types
                    .iter()
                    .map(|type_| type_.to_string_with_level(2))
                    .collect();
                if level > 0 {
                    format!("({})", strings.join(" & "))
                } else {
                    strings.join(" & ")
                }
            }
            Type::Proc {
                function,
                block,
                self_type,
            } => {
                let self_binding = self_type_binding(self_type.as_deref());
                match block {
                    Some(block) => format!(
                        "^({}) {self_binding}{}{{ ({}) {}-> {} }} -> {}",
                        function.parameters_to_string(),
                        if block.required { "" } else { "?" },
                        block.type_.parameters_to_string(),
                        self_type_binding(block.self_type.as_ref()),
                        block.type_.return_to_string(),
                        function.return_to_string(),
                    ),
                    None => format!(
                        "^({}) {self_binding}-> {}",
                        function.parameters_to_string(),
                        function.return_to_string()
                    ),
                }
            }
            Type::Literal(literal) => literal.inspect(),
        }
    }
}

impl Type {
    pub fn each_type(&self) -> Vec<&Type> {
        match self {
            Type::ClassInstance { args, .. }
            | Type::ClassSingleton { args, .. }
            | Type::Interface { args, .. }
            | Type::Alias { args, .. }
            | Type::Tuple(args)
            | Type::Union(args)
            | Type::Intersection(args) => args.iter().collect(),
            Type::Record(fields) => fields
                .iter()
                .filter(|field| field.required)
                .chain(fields.iter().filter(|field| !field.required))
                .map(|field| &field.type_)
                .collect(),
            Type::Optional(inner) => vec![inner],
            Type::Proc {
                function,
                block,
                self_type,
            } => {
                let mut types = function.each_type();
                if let Some(self_type) = self_type {
                    types.push(self_type);
                }
                if let Some(block) = block {
                    types.extend(block.type_.each_type());
                    types.extend(block.self_type.iter());
                }
                types
            }
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
            | Type::Literal(_) => Vec::new(),
        }
    }

    pub fn map_type(&self, map: &mut dyn FnMut(&Type) -> Type) -> Type {
        let mut all = |types: &[Type]| -> Vec<Type> { types.iter().map(&mut *map).collect() };
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
            Type::Union(types) => Type::Union(all(types)),
            Type::Intersection(types) => Type::Intersection(all(types)),
            Type::Record(fields) => Type::Record(
                fields
                    .iter()
                    .map(|field| RecordField {
                        type_: map(&field.type_),
                        ..field.clone()
                    })
                    .collect(),
            ),
            Type::Optional(inner) => Type::Optional(Box::new(map(inner))),
            Type::Proc {
                function,
                block,
                self_type,
            } => Type::Proc {
                function: Box::new(function.map_type(map)),
                block: block.as_ref().map(|block| {
                    Box::new(Block {
                        type_: block.type_.map_type(map),
                        required: block.required,
                        self_type: block.self_type.as_ref().map(&mut *map),
                    })
                }),
                self_type: self_type.as_ref().map(|type_| Box::new(map(type_))),
            },
            _ => self.clone(),
        }
    }

    pub fn location(&self) -> Option<&Location> {
        match self {
            Type::ClassInstance { location, .. }
            | Type::ClassSingleton { location, .. }
            | Type::Interface { location, .. }
            | Type::Alias { location, .. } => location.location(),
            _ => None,
        }
    }

    pub fn free_variables(&self) -> Vec<String> {
        let mut variables = Vec::new();
        self.collect_free_variables(&mut variables);
        variables
    }

    fn collect_free_variables(&self, variables: &mut Vec<String>) {
        if let Type::Variable(name) = self {
            if !variables.contains(name) {
                variables.push(name.clone());
            }
            return;
        }
        for type_ in self.each_type() {
            type_.collect_free_variables(variables);
        }
    }
}

impl Callable {
    pub fn map_type(&self, map: &mut dyn FnMut(&Type) -> Type) -> Callable {
        match self {
            Callable::Function(function) => {
                let mut parameter = |parameter: &Parameter| Parameter {
                    type_: map(&parameter.type_),
                    name: parameter.name.clone(),
                };
                let required_positionals = function
                    .required_positionals
                    .iter()
                    .map(&mut parameter)
                    .collect();
                let optional_positionals = function
                    .optional_positionals
                    .iter()
                    .map(&mut parameter)
                    .collect();
                let rest_positionals = function.rest_positionals.as_ref().map(&mut parameter);
                let trailing_positionals = function
                    .trailing_positionals
                    .iter()
                    .map(&mut parameter)
                    .collect();
                let required_keywords = function
                    .required_keywords
                    .iter()
                    .map(|(name, value)| (name.clone(), parameter(value)))
                    .collect();
                let optional_keywords = function
                    .optional_keywords
                    .iter()
                    .map(|(name, value)| (name.clone(), parameter(value)))
                    .collect();
                let rest_keywords = function.rest_keywords.as_ref().map(&mut parameter);
                Callable::Function(Box::new(Function {
                    required_positionals,
                    optional_positionals,
                    rest_positionals,
                    trailing_positionals,
                    required_keywords,
                    optional_keywords,
                    rest_keywords,
                    return_type: map(&function.return_type),
                }))
            }
            Callable::Untyped { return_type } => Callable::Untyped {
                return_type: map(return_type),
            },
        }
    }

    pub fn each_parameter_type(&self) -> Vec<&Type> {
        let Callable::Function(function) = self else {
            return Vec::new();
        };
        function
            .required_positionals
            .iter()
            .chain(&function.optional_positionals)
            .chain(&function.rest_positionals)
            .chain(&function.trailing_positionals)
            .chain(
                function
                    .required_keywords
                    .iter()
                    .map(|(_, parameter)| parameter),
            )
            .chain(
                function
                    .optional_keywords
                    .iter()
                    .map(|(_, parameter)| parameter),
            )
            .chain(&function.rest_keywords)
            .map(|parameter| &parameter.type_)
            .collect()
    }

    pub fn each_type(&self) -> Vec<&Type> {
        let mut types = self.each_parameter_type();
        types.push(self.return_type());
        types
    }
}

impl MethodType {
    pub fn each_type(&self) -> Vec<&Type> {
        let mut types = self.type_.each_type();
        if let Some(block) = &self.block {
            types.extend(block.type_.each_type());
            types.extend(block.self_type.iter());
        }
        types
    }
}

fn is_plain_record_key(key: &str) -> bool {
    let mut characters = key.chars();
    matches!(characters.next(), Some(first) if first.is_ascii_alphabetic() || first == '_')
        && characters.all(|character| character.is_ascii_alphanumeric() || character == '_')
}

fn self_type_binding(self_type: Option<&Type>) -> String {
    match self_type {
        Some(type_) => format!("[self: {type_}] "),
        None => String::new(),
    }
}

fn join_types(types: &[Type]) -> String {
    types
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

fn application_to_string(name: &str, args: &[Type]) -> String {
    if args.is_empty() {
        name.to_string()
    } else {
        format!("{name}[{}]", join_types(args))
    }
}

impl fmt::Display for Type {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.to_string_with_level(0))
    }
}

impl fmt::Display for Parameter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.name {
            Some(name)
                if name
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || character == '_') =>
            {
                write!(formatter, "{} {name}", self.type_)
            }
            Some(name) => write!(formatter, "{} `{name}`", self.type_),
            None => write!(formatter, "{}", self.type_),
        }
    }
}

impl Callable {
    pub fn return_type(&self) -> &Type {
        match self {
            Callable::Function(function) => &function.return_type,
            Callable::Untyped { return_type } => return_type,
        }
    }

    pub fn parameters_to_string(&self) -> String {
        let function = match self {
            Callable::Function(function) => function,
            Callable::Untyped { .. } => return "?".to_string(),
        };
        let mut parameters: Vec<String> = Vec::new();
        parameters.extend(
            function
                .required_positionals
                .iter()
                .map(ToString::to_string),
        );
        parameters.extend(
            function
                .optional_positionals
                .iter()
                .map(|parameter| format!("?{parameter}")),
        );
        if let Some(parameter) = &function.rest_positionals {
            parameters.push(format!("*{parameter}"));
        }
        parameters.extend(
            function
                .trailing_positionals
                .iter()
                .map(ToString::to_string),
        );
        parameters.extend(
            function
                .required_keywords
                .iter()
                .map(|(name, parameter)| format!("{name}: {parameter}")),
        );
        parameters.extend(
            function
                .optional_keywords
                .iter()
                .map(|(name, parameter)| format!("?{name}: {parameter}")),
        );
        if let Some(parameter) = &function.rest_keywords {
            parameters.push(format!("**{parameter}"));
        }
        parameters.join(", ")
    }

    pub fn return_to_string(&self) -> String {
        self.return_type().to_string_with_level(1)
    }
}

impl fmt::Display for TypeParameter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.unchecked {
            formatter.write_str("unchecked ")?;
        }
        match self.variance {
            Variance::Invariant => {}
            Variance::Covariant => formatter.write_str("out ")?,
            Variance::Contravariant => formatter.write_str("in ")?,
        }
        formatter.write_str(&self.name)?;
        if let Some(type_) = &self.upper_bound {
            write!(formatter, " < {type_}")?;
        }
        if let Some(type_) = &self.lower_bound {
            write!(formatter, " > {type_}")?;
        }
        if let Some(type_) = &self.default_type {
            write!(formatter, " = {type_}")?;
        }
        Ok(())
    }
}

impl fmt::Display for MethodType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if !self.type_parameters.is_empty() {
            let parameters: Vec<String> = self
                .type_parameters
                .iter()
                .map(ToString::to_string)
                .collect();
            write!(formatter, "[{}] ", parameters.join(", "))?;
        }
        match &self.block {
            Some(block) => write!(
                formatter,
                "({}) {}{{ ({}) {}-> {} }} -> {}",
                self.type_.parameters_to_string(),
                if block.required { "" } else { "?" },
                block.type_.parameters_to_string(),
                self_type_binding(block.self_type.as_ref()),
                block.type_.return_to_string(),
                self.type_.return_to_string(),
            ),
            None => write!(
                formatter,
                "({}) -> {}",
                self.type_.parameters_to_string(),
                self.type_.return_to_string()
            ),
        }
    }
}
