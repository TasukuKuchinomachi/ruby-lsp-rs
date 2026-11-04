use std::fmt;

use crate::{
    ancestor_builder::Ancestor,
    ast::{ClassOrModule, MixinKind, Type, TypeName, TypeParameter, Variance},
    location::{Location, location_to_string},
    parse::ParsingError,
};

#[derive(Clone, Debug)]
pub enum RbsError {
    Parsing(Box<ParsingError>),
    DuplicatedDeclaration {
        name: String,
        location: Option<Location>,
    },
    GenericParameterMismatch {
        name: TypeName,
        location: Option<Location>,
    },
    InvalidTypeApplication {
        type_name: TypeName,
        args: Box<[Type]>,
        params: Box<[TypeParameter]>,
        location: Option<Location>,
    },
    RecursiveAncestor {
        ancestors: Vec<Ancestor>,
        location: Option<Location>,
    },
    NoTypeFound {
        type_name: TypeName,
        location: Option<Location>,
    },
    NoSuperclassFound {
        type_name: TypeName,
        location: Option<Location>,
    },
    InheritModule {
        super_name: TypeName,
        location: Option<Location>,
    },
    NoSelfTypeFound {
        type_name: TypeName,
        location: Option<Location>,
    },
    NoMixinFound {
        type_name: TypeName,
        location: Option<Location>,
    },
    DuplicatedMethodDefinition {
        type_name: TypeName,
        singleton: bool,
        method_name: String,
        locations: Vec<Option<Location>>,
    },
    DuplicatedInterfaceMethodDefinition {
        type_name: TypeName,
        singleton: bool,
        method_name: String,
        location: Option<Location>,
    },
    InstanceVariableDuplication {
        type_name: TypeName,
        variable_name: String,
        location: Option<Location>,
    },
    ClassInstanceVariableDuplication {
        type_name: TypeName,
        variable_name: String,
        location: Option<Location>,
    },
    UnknownMethodAlias {
        type_name: TypeName,
        original_name: Box<str>,
        aliased_name: Box<str>,
        location: Option<Location>,
    },
    SuperclassMismatch {
        name: TypeName,
        location: Option<Location>,
    },
    InvalidOverloadMethod {
        type_name: TypeName,
        method_name: String,
        singleton: bool,
        location: Option<Location>,
    },
    InvalidVarianceAnnotation {
        type_name: TypeName,
        param: Box<TypeParameter>,
        location: Option<Location>,
    },
    RecursiveAliasDefinition {
        type_name: TypeName,
        names: Vec<String>,
        location: Option<Location>,
    },
    MixinClass {
        type_name: TypeName,
        member_name: Box<TypeName>,
        mixin_kind: MixinKind,
        location: Option<Location>,
    },
    RecursiveTypeAlias {
        alias_names: Vec<TypeName>,
        location: Option<Location>,
    },
    NonregularTypeAlias {
        type_name: TypeName,
        nonregular_type: Box<Type>,
        location: Option<Location>,
    },
    CyclicTypeParameterBound {
        type_name: TypeName,
        method_name: Option<Box<str>>,
        params: Vec<TypeParameter>,
        location: Option<Location>,
    },
    InconsistentClassModuleAlias {
        kind: ClassOrModule,
        new_name: TypeName,
        old_name: Box<TypeName>,
        location: Option<Location>,
    },
    CyclicClassAliasDefinition {
        new_name: TypeName,
        location: Option<Location>,
    },
    TypeParamDefaultReference {
        type_param: Box<TypeParameter>,
        location: Option<Location>,
    },
    RuntimeError(String),
}

pub fn variance_name(variance: Variance) -> &'static str {
    match variance {
        Variance::Invariant => "invariant",
        Variance::Covariant => "covariant",
        Variance::Contravariant => "contravariant",
    }
}

pub fn mixin_name(mixin_kind: MixinKind) -> &'static str {
    match mixin_kind {
        MixinKind::Include => "include",
        MixinKind::Extend => "extend",
        MixinKind::Prepend => "prepend",
    }
}

impl RbsError {
    pub fn location(&self) -> Option<&Location> {
        match self {
            RbsError::Parsing(error) => Some(&error.location),
            RbsError::DuplicatedMethodDefinition { locations, .. } => {
                locations.first().and_then(Option::as_ref)
            }
            RbsError::DuplicatedDeclaration { location, .. }
            | RbsError::GenericParameterMismatch { location, .. }
            | RbsError::InvalidTypeApplication { location, .. }
            | RbsError::RecursiveAncestor { location, .. }
            | RbsError::NoTypeFound { location, .. }
            | RbsError::NoSuperclassFound { location, .. }
            | RbsError::InheritModule { location, .. }
            | RbsError::NoSelfTypeFound { location, .. }
            | RbsError::NoMixinFound { location, .. }
            | RbsError::DuplicatedInterfaceMethodDefinition { location, .. }
            | RbsError::InstanceVariableDuplication { location, .. }
            | RbsError::ClassInstanceVariableDuplication { location, .. }
            | RbsError::UnknownMethodAlias { location, .. }
            | RbsError::SuperclassMismatch { location, .. }
            | RbsError::InvalidOverloadMethod { location, .. }
            | RbsError::InvalidVarianceAnnotation { location, .. }
            | RbsError::RecursiveAliasDefinition { location, .. }
            | RbsError::MixinClass { location, .. }
            | RbsError::RecursiveTypeAlias { location, .. }
            | RbsError::NonregularTypeAlias { location, .. }
            | RbsError::CyclicTypeParameterBound { location, .. }
            | RbsError::InconsistentClassModuleAlias { location, .. }
            | RbsError::CyclicClassAliasDefinition { location, .. }
            | RbsError::TypeParamDefaultReference { location, .. } => location.as_ref(),
            RbsError::RuntimeError(_) => None,
        }
    }

    pub fn class_name(&self) -> &'static str {
        match self {
            RbsError::Parsing(_) => "RBS::ParsingError",
            RbsError::DuplicatedDeclaration { .. } => "RBS::DuplicatedDeclarationError",
            RbsError::GenericParameterMismatch { .. } => "RBS::GenericParameterMismatchError",
            RbsError::InvalidTypeApplication { .. } => "RBS::InvalidTypeApplicationError",
            RbsError::RecursiveAncestor { .. } => "RBS::RecursiveAncestorError",
            RbsError::NoTypeFound { .. } => "RBS::NoTypeFoundError",
            RbsError::NoSuperclassFound { .. } => "RBS::NoSuperclassFoundError",
            RbsError::InheritModule { .. } => "RBS::InheritModuleError",
            RbsError::NoSelfTypeFound { .. } => "RBS::NoSelfTypeFoundError",
            RbsError::NoMixinFound { .. } => "RBS::NoMixinFoundError",
            RbsError::DuplicatedMethodDefinition { .. } => "RBS::DuplicatedMethodDefinitionError",
            RbsError::DuplicatedInterfaceMethodDefinition { .. } => {
                "RBS::DuplicatedInterfaceMethodDefinitionError"
            }
            RbsError::InstanceVariableDuplication { .. } => "RBS::InstanceVariableDuplicationError",
            RbsError::ClassInstanceVariableDuplication { .. } => {
                "RBS::ClassInstanceVariableDuplicationError"
            }
            RbsError::UnknownMethodAlias { .. } => "RBS::UnknownMethodAliasError",
            RbsError::SuperclassMismatch { .. } => "RBS::SuperclassMismatchError",
            RbsError::InvalidOverloadMethod { .. } => "RBS::InvalidOverloadMethodError",
            RbsError::InvalidVarianceAnnotation { .. } => "RBS::InvalidVarianceAnnotationError",
            RbsError::RecursiveAliasDefinition { .. } => "RBS::RecursiveAliasDefinitionError",
            RbsError::MixinClass { .. } => "RBS::MixinClassError",
            RbsError::RecursiveTypeAlias { .. } => "RBS::RecursiveTypeAliasError",
            RbsError::NonregularTypeAlias { .. } => "RBS::NonregularTypeAliasError",
            RbsError::CyclicTypeParameterBound { .. } => "RBS::CyclicTypeParameterBound",
            RbsError::InconsistentClassModuleAlias { .. } => {
                "RBS::InconsistentClassModuleAliasError"
            }
            RbsError::CyclicClassAliasDefinition { .. } => "RBS::CyclicClassAliasDefinitionError",
            RbsError::TypeParamDefaultReference { .. } => "RBS::TypeParamDefaultReferenceError",
            RbsError::RuntimeError(_) => "RuntimeError",
        }
    }

    pub fn is_loading_error(&self) -> bool {
        matches!(
            self,
            RbsError::DuplicatedDeclaration { .. } | RbsError::GenericParameterMismatch { .. }
        )
    }
}

fn qualified_method_name(type_name: &TypeName, singleton: bool, method_name: &str) -> String {
    let separator = if singleton { "." } else { "#" };
    format!("{type_name}{separator}{method_name}")
}

fn join<T: fmt::Display>(items: &[T], separator: &str) -> String {
    items
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(separator)
}

impl fmt::Display for RbsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let location = location_to_string(self.location());
        match self {
            RbsError::Parsing(error) => formatter.write_str(&error.message()),
            RbsError::DuplicatedDeclaration { name, .. } => {
                write!(formatter, "{location}: Duplicated declaration: {name}")
            }
            RbsError::GenericParameterMismatch { name, .. } => {
                write!(formatter, "{location}: Generic parameters mismatch: {name}")
            }
            RbsError::InvalidTypeApplication {
                type_name,
                args,
                params,
                ..
            } => write!(
                formatter,
                "{location}: {type_name} expects parameters [{}], but given args [{}]",
                join(params, ", "),
                join(args, ", ")
            ),
            RbsError::RecursiveAncestor { ancestors, .. } => write!(
                formatter,
                "{location}: Detected recursive ancestors: {}",
                join(ancestors, " < ")
            ),
            RbsError::NoTypeFound { type_name, .. } => {
                write!(formatter, "{location}: Could not find {type_name}")
            }
            RbsError::NoSuperclassFound { type_name, .. } => {
                write!(
                    formatter,
                    "{location}: Could not find super class: {type_name}"
                )
            }
            RbsError::InheritModule { super_name, .. } => {
                write!(
                    formatter,
                    "{location}: Cannot inherit a module: {super_name}"
                )
            }
            RbsError::NoSelfTypeFound { type_name, .. } => {
                write!(
                    formatter,
                    "{location}: Could not find self type: {type_name}"
                )
            }
            RbsError::NoMixinFound { type_name, .. } => {
                write!(formatter, "{location}: Could not find mixin: {type_name}")
            }
            RbsError::DuplicatedMethodDefinition {
                type_name,
                singleton,
                method_name,
                locations,
                ..
            } => {
                write!(
                    formatter,
                    "{location}: {} has duplicated definitions",
                    qualified_method_name(type_name, *singleton, method_name)
                )?;
                if locations.len() > 1 {
                    let others: Vec<String> = locations[1..]
                        .iter()
                        .map(|location| location_to_string(location.as_ref()))
                        .collect();
                    write!(formatter, " in {}", others.join(", "))?;
                }
                Ok(())
            }
            RbsError::DuplicatedInterfaceMethodDefinition {
                type_name,
                singleton,
                method_name,
                ..
            } => write!(
                formatter,
                "{location}: Duplicated method definition: {}",
                qualified_method_name(type_name, *singleton, method_name)
            ),
            RbsError::InstanceVariableDuplication {
                type_name,
                variable_name,
                ..
            } => write!(
                formatter,
                "{location}: Duplicated instance variable name `{variable_name}` in `{type_name}`"
            ),
            RbsError::ClassInstanceVariableDuplication {
                type_name,
                variable_name,
                ..
            } => write!(
                formatter,
                "{location}: Duplicated class instance variable name `{variable_name}` in `{type_name}`"
            ),
            RbsError::UnknownMethodAlias {
                type_name,
                original_name,
                aliased_name,
                ..
            } => write!(
                formatter,
                "{location}: Unknown method alias name: {original_name} => {aliased_name} ({type_name})"
            ),
            RbsError::SuperclassMismatch { name, .. } => {
                write!(formatter, "{location}: Superclass mismatch: {name}")
            }
            RbsError::InvalidOverloadMethod {
                type_name,
                method_name,
                singleton,
                ..
            } => write!(
                formatter,
                "{location}: Invalid method overloading: {}",
                qualified_method_name(type_name, *singleton, method_name)
            ),
            RbsError::InvalidVarianceAnnotation { param, .. } => write!(
                formatter,
                "{location}: Type parameter variance error: {} is {} but used as incompatible variance",
                param.name,
                variance_name(param.variance)
            ),
            RbsError::RecursiveAliasDefinition {
                type_name, names, ..
            } => write!(
                formatter,
                "{location}: Recursive aliases in {type_name}: {}",
                names.join(", ")
            ),
            RbsError::MixinClass {
                type_name,
                member_name,
                mixin_kind,
                ..
            } => write!(
                formatter,
                "{location}: Cannot {} a class `{member_name}` in the definition of `{type_name}`",
                mixin_name(*mixin_kind)
            ),
            RbsError::RecursiveTypeAlias { alias_names, .. } => write!(
                formatter,
                "{location}: Recursive type alias definition found for: {}",
                alias_names
                    .iter()
                    .map(|name| name.name.clone())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            RbsError::NonregularTypeAlias {
                type_name,
                nonregular_type,
                ..
            } => write!(
                formatter,
                "{location}: Nonregular generic type alias is prohibited: {type_name}, {nonregular_type}"
            ),
            RbsError::CyclicTypeParameterBound { .. } => {
                write!(
                    formatter,
                    "{location}: Cyclic type parameter bound is prohibited"
                )
            }
            RbsError::InconsistentClassModuleAlias {
                kind,
                new_name,
                old_name,
                ..
            } => {
                let (expected, actual) = match kind {
                    ClassOrModule::Class => ("class", "module"),
                    ClassOrModule::Module => ("module", "class"),
                };
                write!(
                    formatter,
                    "{location}: A {expected} `{new_name}` cannot be an alias of a {actual} `{old_name}`"
                )
            }
            RbsError::CyclicClassAliasDefinition { new_name, .. } => {
                write!(formatter, "{location}: A {new_name} is a cyclic definition")
            }
            RbsError::TypeParamDefaultReference { type_param, .. } => write!(
                formatter,
                "{location}: the default of {} cannot include optional type parameter",
                type_param.name
            ),
            RbsError::RuntimeError(message) => formatter.write_str(message),
        }
    }
}

impl From<RbsError> for String {
    fn from(error: RbsError) -> String {
        error.to_string()
    }
}

impl From<String> for RbsError {
    fn from(message: String) -> RbsError {
        RbsError::RuntimeError(message)
    }
}

impl From<&str> for RbsError {
    fn from(message: &str) -> RbsError {
        RbsError::RuntimeError(message.to_string())
    }
}
