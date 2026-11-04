use rbs_definition::{
    ancestor_builder::Ancestor,
    ast::{ClassOrModule, TypeName},
    errors::{RbsError, mixin_name, variance_name},
    location::Location,
};
use steep_interface::{
    factory::Factory,
    subtyping::{
        relation::AnyRelation,
        result::{FailureError, SubtypingResult},
    },
};

use crate::expectations::{self, Position, Range};

#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub error_name: &'static str,
    pub location: Option<Location>,
    pub header_line: String,
    pub detail_lines: Option<String>,
}

impl Diagnostic {
    fn new(
        error_name: &'static str,
        location: Option<Location>,
        header_line: String,
    ) -> Diagnostic {
        Diagnostic {
            error_name,
            location,
            header_line,
            detail_lines: None,
        }
    }

    fn with_result(mut self, result: &SubtypingResult) -> Diagnostic {
        let lines: Vec<String> = result
            .failure_path()
            .into_iter()
            .filter_map(result_line)
            .enumerate()
            .map(|(index, line)| format!("{}{line}", "  ".repeat(index + 1)))
            .collect();
        self.detail_lines = (!lines.is_empty()).then(|| lines.join("\n"));
        self
    }

    pub fn diagnostic_code(&self) -> String {
        format!("RBS::{}", self.error_name)
    }

    pub fn full_message(&self) -> String {
        match &self.detail_lines {
            Some(detail) => format!("{}\n{detail}", self.header_line),
            None => self.header_line.clone(),
        }
    }

    pub fn path(&self) -> Option<&str> {
        self.location.as_ref().map(Location::name)
    }

    pub fn is_syntax_error(&self) -> bool {
        matches!(self.error_name, "SyntaxError" | "UnexpectedError")
    }

    pub fn severity(&self) -> &'static str {
        if self.error_name == "DeprecatedTypeName" {
            "WARNING"
        } else {
            "ERROR"
        }
    }

    pub fn to_expectation(&self) -> Option<expectations::Diagnostic> {
        let location = self.location.as_ref()?;
        Some(expectations::Diagnostic {
            range: Range {
                start: Position {
                    line: location.start_line() as u32,
                    character: location.start_column() as u32,
                },
                end: Position {
                    line: location.end_line() as u32,
                    character: location.end_column() as u32,
                },
            },
            severity: self.severity().to_string(),
            message: self.full_message(),
            code: self.diagnostic_code(),
        })
    }

    pub fn same_as(&self, other: &Diagnostic) -> bool {
        self.error_name == other.error_name
            && self.header_line == other.header_line
            && match (&self.location, &other.location) {
                (Some(left), Some(right)) => left.same_range(right),
                (None, None) => true,
                _ => false,
            }
    }
}

fn result_line(result: &SubtypingResult) -> Option<String> {
    let SubtypingResult::Failure { relation, error } = result else {
        return Some(result.relation().to_string());
    };
    match error {
        FailureError::UnknownPair { .. } | FailureError::LoopAbort => None,
        FailureError::UnsatisfiedConstraints(_) => {
            Some(format!("Unsatisfied constraints: {relation}"))
        }
        FailureError::MethodMissing { name } => Some(format!("Method `{name}` is missing")),
        FailureError::BlockMismatch { .. } => Some(format!("Incomaptible block: {relation}")),
        FailureError::ParameterMismatch { .. } => Some(match relation {
            AnyRelation::Params(params) => format!(
                "Incompatible arity: {} and {}",
                params.super_type, params.sub_type
            ),
            relation => format!("Incompatible arity: {relation}"),
        }),
        FailureError::PolyMethodSubtyping { .. } => Some(format!(
            "Unsupported polymorphic method comparison: {relation}"
        )),
        FailureError::SelfBindingMismatch => {
            Some(format!("Incompatible block self type: {relation}"))
        }
    }
}

fn recursive_ancestor_name(ancestor: &Ancestor) -> String {
    match ancestor {
        Ancestor::Singleton { name } => format!("singleton({name})"),
        Ancestor::Instance { name, args, .. } if args.is_empty() => name.to_string(),
        Ancestor::Instance { name, args, .. } => format!(
            "{name}[{}]",
            args.iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

pub fn from_rbs_error(error: &RbsError, factory: Option<&Factory>) -> Result<Diagnostic, String> {
    let location = error.location().cloned();
    let diagnostic = match error {
        RbsError::Parsing(parsing) => Diagnostic::new("SyntaxError", location, parsing.message()),
        RbsError::DuplicatedDeclaration { name, .. } => Diagnostic::new(
            "DuplicatedDeclaration",
            location,
            format!("Declaration of `{name}` is duplicated"),
        ),
        RbsError::GenericParameterMismatch { name, .. } => Diagnostic::new(
            "GenericParameterMismatch",
            location,
            format!("Different generic parameters are specified across definitions of `{name}`"),
        ),
        RbsError::InvalidTypeApplication {
            type_name,
            args,
            params,
            ..
        } => {
            let header = if params.is_empty() {
                format!(
                    "Type `{type_name}` is not generic but used as a generic type with {} arguments",
                    args.len()
                )
            } else if args.is_empty() {
                format!("Type `{type_name}` is generic but used as a non generic type")
            } else {
                format!(
                    "Type `{type_name}` expects {} arguments, but {} arguments are given",
                    params.len(),
                    args.len()
                )
            };
            Diagnostic::new("InvalidTypeApplication", location, header)
        }
        RbsError::NoTypeFound { type_name, .. }
        | RbsError::NoSuperclassFound { type_name, .. }
        | RbsError::NoMixinFound { type_name, .. }
        | RbsError::NoSelfTypeFound { type_name, .. } => Diagnostic::new(
            "UnknownTypeName",
            location,
            format!("Cannot find type `{type_name}`"),
        ),
        RbsError::InvalidOverloadMethod {
            type_name,
            method_name,
            ..
        } => Diagnostic::new(
            "InvalidMethodOverload",
            location,
            format!("Cannot find a non-overloading definition of `{method_name}` in `{type_name}`"),
        ),
        RbsError::DuplicatedMethodDefinition {
            type_name,
            method_name,
            ..
        }
        | RbsError::DuplicatedInterfaceMethodDefinition {
            type_name,
            method_name,
            ..
        } => Diagnostic::new(
            "DuplicatedMethodDefinition",
            location,
            format!(
                "Non-overloading method definition of `{method_name}` in `{type_name}` cannot be duplicated"
            ),
        ),
        RbsError::UnknownMethodAlias {
            type_name,
            original_name,
            ..
        } => Diagnostic::new(
            "UnknownMethodAlias",
            location,
            format!("Cannot find the original method `{original_name}` in `{type_name}`"),
        ),
        RbsError::RecursiveAliasDefinition {
            type_name, names, ..
        } => Diagnostic::new(
            "RecursiveAlias",
            location,
            format!(
                "Circular method alias is detected in `{type_name}`: {}",
                names.join(" -> ")
            ),
        ),
        RbsError::RecursiveAncestor { ancestors, .. } => Diagnostic::new(
            "RecursiveAncestor",
            location,
            format!(
                "Circular inheritance/mix-in is detected: {}",
                ancestors
                    .iter()
                    .map(recursive_ancestor_name)
                    .collect::<Vec<_>>()
                    .join(" <: ")
            ),
        ),
        RbsError::SuperclassMismatch { name, .. } => Diagnostic::new(
            "SuperclassMismatch",
            location,
            format!("Different superclasses are specified for `{name}`"),
        ),
        RbsError::InvalidVarianceAnnotation { param, .. } => Diagnostic::new(
            "InvalidVarianceAnnotation",
            location,
            format!(
                "The variance of type parameter `{}` is {}, but used in incompatible position here",
                param.name,
                variance_name(param.variance)
            ),
        ),
        RbsError::MixinClass {
            type_name,
            member_name,
            mixin_kind,
            ..
        } => Diagnostic::new(
            "MixinClassError",
            location,
            format!(
                "Cannot {} a class `{member_name}` in the definition of `{type_name}`",
                mixin_name(*mixin_kind)
            ),
        ),
        RbsError::RecursiveTypeAlias { alias_names, .. } => Diagnostic::new(
            "RecursiveTypeAlias",
            location,
            format!(
                "Type aliases cannot be *directly recursive*: {}",
                alias_names
                    .iter()
                    .map(TypeName::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ),
        RbsError::NonregularTypeAlias {
            type_name,
            nonregular_type,
            ..
        } => {
            let factory = factory.ok_or("NonregularTypeAlias needs a factory")?;
            Diagnostic::new(
                "NonregularTypeAlias",
                location,
                format!(
                    "Type alias {type_name} is defined *non-regular*: {}",
                    factory.type_(nonregular_type)
                ),
            )
        }
        RbsError::InheritModule { super_name, .. } => Diagnostic::new(
            "InheritModuleError",
            location,
            format!("Cannot inherit from a module `{super_name}`"),
        ),
        RbsError::InconsistentClassModuleAlias {
            kind,
            new_name,
            old_name,
            ..
        } => Diagnostic::new(
            "InconsistentClassModuleAliasError",
            location,
            format!(
                "A {} `{new_name}` cannot be an alias of `{old_name}`",
                match kind {
                    ClassOrModule::Class => "class",
                    ClassOrModule::Module => "module",
                }
            ),
        ),
        RbsError::CyclicClassAliasDefinition { new_name, .. } => Diagnostic::new(
            "CyclicClassAliasDefinitionError",
            location,
            format!("{new_name} is a cyclic definition"),
        ),
        RbsError::TypeParamDefaultReference { type_param, .. } => {
            type_param_default_reference(&type_param.name, location)
        }
        RbsError::InstanceVariableDuplication {
            type_name,
            variable_name,
            ..
        } => Diagnostic::new(
            "InstanceVariableDuplicationError",
            location,
            format!("Duplicated instance variable name `{variable_name}` in `{type_name}`"),
        ),
        RbsError::ClassInstanceVariableDuplication {
            type_name,
            variable_name,
            ..
        } => Diagnostic::new(
            "ClassInstanceVariableDuplicationError",
            location,
            format!("Duplicated class instance variable name `{variable_name}` in `{type_name}`"),
        ),
        RbsError::CyclicTypeParameterBound { .. } | RbsError::RuntimeError(_) => {
            return Err(error.to_string());
        }
    };
    Ok(diagnostic)
}

pub fn type_param_default_reference(name: &str, location: Option<Location>) -> Diagnostic {
    Diagnostic::new(
        "TypeParamDefaultReferenceError",
        location,
        format!("The default type of `{name}` cannot depend on optional type parameters"),
    )
}

pub fn unsatisfiable_type_application(
    type_name: &TypeName,
    type_arg: &steep_interface::types::Type,
    upper_bound: &steep_interface::types::Type,
    result: &SubtypingResult,
    location: Option<Location>,
) -> Diagnostic {
    Diagnostic::new(
        "UnsatisfiableTypeApplication",
        location,
        format!(
            "Type application of `{type_name}` doesn't satisfy the constraints: {type_arg} <: {upper_bound}"
        ),
    )
    .with_result(result)
}

pub fn unsatisfiable_generics_default_type(
    name: &str,
    result: &SubtypingResult,
    location: Option<Location>,
) -> Diagnostic {
    Diagnostic::new(
        "UnsatisfiableGenericsDefaultType",
        location,
        format!(
            "The default type of `{name}` doesn't satisfy upper bound constraint: {}",
            result.relation()
        ),
    )
    .with_result(result)
}

pub fn module_self_type_error(
    name: &TypeName,
    result: &SubtypingResult,
    location: Option<Location>,
) -> Diagnostic {
    Diagnostic::new(
        "ModuleSelfTypeError",
        location,
        format!(
            "Module self type constraint in type `{name}` doesn't satisfy: `{}`",
            result.relation()
        ),
    )
    .with_result(result)
}

pub fn instance_variable_type_error(
    variable_type: &steep_interface::types::Type,
    parent_type: &steep_interface::types::Type,
    location: Option<Location>,
) -> Diagnostic {
    Diagnostic::new(
        "InstanceVariableTypeError",
        location,
        format!(
            "Instance variable cannot have different type with parents: {variable_type} <=> {parent_type}"
        ),
    )
}

pub fn class_variable_duplication_error(
    class_name: &TypeName,
    other_class_name: &TypeName,
    variable_name: &str,
    location: Option<Location>,
) -> Diagnostic {
    Diagnostic::new(
        "ClassVariableDuplicationError",
        location,
        format!(
            "Class variable definition `{variable_name}` in `{class_name}` may be overtaken by `{other_class_name}`"
        ),
    )
}

pub fn deprecated_type_name(
    type_name: &TypeName,
    message: Option<&str>,
    location: Option<Location>,
) -> Diagnostic {
    let mut header = format!("Type `{type_name}` is deprecated");
    if let Some(message) = message {
        header.push_str(": ");
        header.push_str(message);
    }
    Diagnostic::new("DeprecatedTypeName", location, header)
}
