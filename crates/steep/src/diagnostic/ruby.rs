use std::fmt::Write;

use steep_interface::{
    method_type::{MethodType, TypeParam},
    subtyping::{
        relation::AnyRelation,
        result::{FailureError, SubtypingResult},
    },
    types::Type,
};

use crate::{
    ruby::node::{Node, SourceRange},
    ruby_exception::RubyException,
    signature::diagnostic as signature,
};
use steep_interface::steep_version::SteepVersion;

#[derive(Clone, Debug, PartialEq)]
pub struct Diagnostic {
    pub error_name: &'static str,
    pub location: Option<SourceRange>,
    pub header_line: String,
    pub detail_lines: Option<String>,
}

impl Diagnostic {
    pub fn new(
        error_name: &'static str,
        location: Option<SourceRange>,
        header_line: impl Into<String>,
    ) -> Diagnostic {
        Diagnostic {
            error_name,
            location,
            header_line: header_line.into(),
            detail_lines: None,
        }
    }

    pub fn with_result(mut self, result: &SubtypingResult) -> Diagnostic {
        self.detail_lines = result_detail_lines(result);
        self
    }

    pub fn with_detail_lines(mut self, detail_lines: Option<String>) -> Diagnostic {
        self.detail_lines = detail_lines;
        self
    }

    pub fn diagnostic_code(&self) -> String {
        format!("Ruby::{}", self.error_name)
    }

    pub fn full_message(&self) -> String {
        match &self.detail_lines {
            Some(detail) => format!("{}\n{detail}", self.header_line),
            None => self.header_line.clone(),
        }
    }

    pub fn is_arity_error(&self) -> bool {
        matches!(
            self.error_name,
            "UnexpectedBlockGiven"
                | "RequiredBlockMissing"
                | "UnexpectedPositionalArgument"
                | "InsufficientPositionalArguments"
                | "UnexpectedKeywordArgument"
                | "InsufficientKeywordArguments"
        )
    }
}

pub fn result_detail_lines(result: &SubtypingResult) -> Option<String> {
    let mut lines = String::new();
    for (index, message) in result
        .failure_path()
        .iter()
        .filter_map(|step| step.relation().diagnostic_message())
        .enumerate()
    {
        writeln!(lines, "{}{message}", "  ".repeat(index + 1)).unwrap();
    }
    let lines = lines.trim_end_matches('\n').to_string();
    (!lines.is_empty()).then_some(lines)
}

fn result_printer2_result_line(result: &SubtypingResult) -> Option<String> {
    match result {
        SubtypingResult::Failure { relation, error } => match error {
            FailureError::UnknownPair { .. } => None,
            FailureError::UnsatisfiedConstraints(_) => {
                Some(format!("Unsatisfied constraints: {relation}"))
            }
            FailureError::MethodMissing { name } => Some(format!("Method `{name}` is missing")),
            FailureError::BlockMismatch { .. } => Some(format!("Incomaptible block: {relation}")),
            FailureError::ParameterMismatch { .. } => match relation {
                AnyRelation::Params(params) => Some(format!(
                    "Incompatible arity: {} and {}",
                    params.super_type, params.sub_type
                )),
                _ => Some(format!("Incompatible arity: {relation}")),
            },
            FailureError::PolyMethodSubtyping { .. } => Some(format!(
                "Unsupported polymorphic method comparison: {relation}"
            )),
            FailureError::SelfBindingMismatch => {
                Some(format!("Incompatible block self type: {relation}"))
            }
            FailureError::LoopAbort => None,
        },
        _ => Some(result.relation().to_string()),
    }
}

pub fn result_printer2_detail_lines(result: &SubtypingResult) -> Option<String> {
    let mut lines = String::new();
    for (index, message) in result
        .failure_path()
        .iter()
        .filter_map(|step| result_printer2_result_line(step))
        .enumerate()
    {
        writeln!(lines, "{}{message}", "  ".repeat(index + 1)).unwrap();
    }
    let lines = lines.trim_end_matches('\n').to_string();
    (!lines.is_empty()).then_some(lines)
}

fn expression(node: &Node) -> Option<SourceRange> {
    node.location.expression
}

fn selector_to_end(node: &Node) -> Option<SourceRange> {
    let send = match node.type_ {
        "send" | "csend" => Some(node),
        "block" | "numblock" | "itblock" => node.child_node(0).map(|node| &**node),
        _ => None,
    };
    match send {
        Some(send) => send
            .location
            .selector
            .map(|selector| selector.with_end_pos(send.expression().end_pos)),
        None => expression(node),
    }
}

pub fn incompatible_assignment(
    node: &Node,
    lhs_type: &Type,
    rhs_type: &Type,
    result: &SubtypingResult,
) -> Diagnostic {
    let element = match node.type_ {
        "ivasgn" | "lvasgn" | "gvasgn" | "cvasgn" => "a variable",
        "casgn" => "a constant",
        _ => "an expression",
    };
    Diagnostic::new(
        "IncompatibleAssignment",
        expression(node),
        format!("Cannot assign a value of type `{rhs_type}` to {element} of type `{lhs_type}`"),
    )
    .with_result(result)
}

pub fn unexpected_positional_argument(node: &Node) -> Diagnostic {
    Diagnostic::new(
        "UnexpectedPositionalArgument",
        expression(node),
        "Unexpected positional argument",
    )
}

pub fn insufficient_positional_arguments(node: &Node) -> Diagnostic {
    Diagnostic::new(
        "InsufficientPositionalArguments",
        selector_to_end(node),
        "More positional arguments are required",
    )
}

pub fn unexpected_keyword_argument(node: &Node) -> Diagnostic {
    let location = match node.type_ {
        "pair" => node.child_node(0).and_then(|key| expression(key)),
        _ => expression(node),
    };
    Diagnostic::new(
        "UnexpectedKeywordArgument",
        location,
        "Unexpected keyword argument",
    )
}

pub fn insufficient_keyword_arguments(node: &Node, missing_keywords: &[String]) -> Diagnostic {
    Diagnostic::new(
        "InsufficientKeywordArguments",
        selector_to_end(node),
        format!(
            "More keyword arguments are required: {}",
            missing_keywords.join(", ")
        ),
    )
}

pub fn unresolved_overloading(
    node: &Node,
    receiver_type: &Type,
    method_name: &str,
    method_types: &[MethodType],
) -> Diagnostic {
    let mut detail = String::from("Method types:\n");
    let definition = format!("  def {method_name}");
    for (index, method_type) in method_types.iter().enumerate() {
        if index == 0 {
            writeln!(detail, "{definition}: {method_type}").unwrap();
        } else {
            writeln!(detail, "{}| {method_type}", " ".repeat(definition.len())).unwrap();
        }
    }
    Diagnostic::new(
        "UnresolvedOverloading",
        expression(node),
        format!(
            "Cannot find compatible overloading of method `{method_name}` of type `{receiver_type}`"
        ),
    )
    .with_detail_lines(Some(detail.trim_end_matches('\n').to_string()))
}

pub fn argument_type_mismatch(
    node: &Node,
    expected: &Type,
    actual: &Type,
    result: &SubtypingResult,
) -> Diagnostic {
    Diagnostic::new(
        "ArgumentTypeMismatch",
        expression(node),
        format!("Cannot pass a value of type `{actual}` as an argument of type `{expected}`"),
    )
    .with_result(result)
}

pub fn no_method(
    node: &Node,
    type_: &Type,
    method: &str,
    steep_version: SteepVersion,
) -> Diagnostic {
    let location = match (node.type_, steep_version) {
        ("send", _) => node.location.operator.or(node.location.selector),
        ("block", _) | ("numblock" | "itblock", SteepVersion::Version2_1_0) => {
            node.child_node(0).and_then(|send| send.location.selector)
        }
        _ => None,
    };
    Diagnostic::new(
        "NoMethod",
        location.or(expression(node)),
        format!("Type `{type_}` does not have method `{method}`"),
    )
}

pub fn return_type_mismatch(
    node: &Node,
    expected: &Type,
    actual: &Type,
    result: &SubtypingResult,
) -> Diagnostic {
    Diagnostic::new(
        "ReturnTypeMismatch",
        expression(node),
        format!("The method cannot return a value of type `{actual}` because declared as type `{expected}`"),
    )
    .with_result(result)
}

pub fn setter_return_type_mismatch(
    node: &Node,
    method_name: &str,
    expected: &Type,
    actual: &Type,
    result: &SubtypingResult,
) -> Diagnostic {
    Diagnostic::new(
        "SetterReturnTypeMismatch",
        expression(node),
        format!("The setter method `{method_name}` cannot return a value of type `{actual}` because declared as type `{expected}`"),
    )
    .with_result(result)
}

fn begin_to_end(node: &Node) -> Option<SourceRange> {
    match (node.location.begin, node.location.end) {
        (Some(begin), Some(end)) => Some(begin.join(end)),
        _ => expression(node),
    }
}

pub fn unexpected_block_given(node: &Node) -> Diagnostic {
    Diagnostic::new(
        "UnexpectedBlockGiven",
        begin_to_end(node),
        "The method cannot be called with a block",
    )
}

pub fn required_block_missing(node: &Node) -> Diagnostic {
    let location = if node.type_ == "super" || node.type_ == "zsuper" {
        node.location.keyword
    } else {
        node.location.selector
    };
    Diagnostic::new(
        "RequiredBlockMissing",
        location,
        "The method cannot be called without a block",
    )
}

pub fn block_type_mismatch(
    node: &Node,
    expected: &Type,
    actual: &Type,
    result: &SubtypingResult,
) -> Diagnostic {
    Diagnostic::new(
        "BlockTypeMismatch",
        expression(node),
        format!(
            "Cannot pass a value of type `{actual}` as a block-pass-argument of type `{expected}`"
        ),
    )
    .with_result(result)
}

pub fn block_body_type_mismatch(
    node: &Node,
    expected: &Type,
    actual: &Type,
    result: &SubtypingResult,
) -> Diagnostic {
    Diagnostic::new(
        "BlockBodyTypeMismatch",
        begin_to_end(node),
        format!(
            "Cannot allow block body have type `{actual}` because declared as type `{expected}`"
        ),
    )
    .with_result(result)
}

pub fn break_type_mismatch(
    node: &Node,
    expected: &Type,
    actual: &Type,
    result: &SubtypingResult,
) -> Diagnostic {
    Diagnostic::new(
        "BreakTypeMismatch",
        expression(node),
        format!(
            "Cannot break with a value of type `{actual}` because type `{expected}` is assumed"
        ),
    )
    .with_result(result)
}

pub fn implicit_break_value_mismatch(
    node: &Node,
    jump_type: &Type,
    result: &SubtypingResult,
) -> Diagnostic {
    Diagnostic::new(
        "ImplicitBreakValueMismatch",
        expression(node),
        format!("Breaking without a value may result an error because a value of type `{jump_type}` is expected"),
    )
    .with_result(result)
}

pub fn unexpected_jump(node: &Node) -> Diagnostic {
    Diagnostic::new("UnexpectedJump", expression(node), "Cannot jump from here")
}

pub fn unexpected_jump_value(node: &Node) -> Diagnostic {
    Diagnostic::new(
        "UnexpectedJumpValue",
        expression(node),
        format!("The value given to {} will be ignored", node.type_),
    )
}

pub fn class_module_mismatch(node: &Node, name: &str) -> Diagnostic {
    let header = if node.type_ == "module" {
        format!("{name} is declared as a class in RBS")
    } else {
        format!("{name} is declared as a module in RBS")
    };
    Diagnostic::new("ClassModuleMismatch", node.location.name, header)
}

pub fn method_arity_mismatch(node: &Node, method_type: &MethodType) -> Diagnostic {
    let arguments = match node.type_ {
        "def" => node.child_node(1),
        "defs" => node.child_node(2),
        _ => None,
    };
    Diagnostic::new(
        "MethodArityMismatch",
        arguments
            .and_then(|arguments| arguments.location.expression)
            .or(node.location.name),
        format!("Method parameters are incompatible with declaration `{method_type}`"),
    )
}

pub fn method_parameter_mismatch(node: &Node, method_type: &MethodType) -> Diagnostic {
    Diagnostic::new(
        "MethodParameterMismatch",
        expression(node),
        format!("The method parameter is incompatible with the declaration `{method_type}`"),
    )
}

pub fn different_method_parameter_kind(node: &Node, method_type: &MethodType) -> Diagnostic {
    Diagnostic::new(
        "DifferentMethodParameterKind",
        expression(node),
        format!("The method parameter has different kind from the declaration `{method_type}`"),
    )
}

pub fn method_return_type_annotation_mismatch(
    node: &Node,
    method_type: &Type,
    annotation_type: &Type,
    result: &SubtypingResult,
) -> Diagnostic {
    Diagnostic::new(
        "MethodReturnTypeAnnotationMismatch",
        expression(node),
        format!("Annotation `@type return` specifies type `{annotation_type}` where declared as type `{method_type}`"),
    )
    .with_result(result)
}

pub fn method_body_type_mismatch(
    node: &Node,
    expected: &Type,
    actual: &Type,
    result: &SubtypingResult,
) -> Diagnostic {
    Diagnostic::new(
        "MethodBodyTypeMismatch",
        node.location.name,
        format!(
            "Cannot allow method body have type `{actual}` because declared as type `{expected}`"
        ),
    )
    .with_result(result)
}

pub fn setter_body_type_mismatch(
    node: &Node,
    expected: &Type,
    actual: &Type,
    result: &SubtypingResult,
    method_name: &str,
) -> Diagnostic {
    Diagnostic::new(
        "SetterBodyTypeMismatch",
        node.location.name,
        format!("Setter method `{method_name}` cannot have type `{actual}` because declared as type `{expected}`"),
    )
    .with_result(result)
}

pub fn unexpected_yield(node: &Node) -> Diagnostic {
    Diagnostic::new(
        "UnexpectedYield",
        expression(node),
        "No block given for `yield`",
    )
}

pub fn unexpected_super(node: &Node, method: Option<&str>) -> Diagnostic {
    let header = match method {
        Some(method) => format!("No superclass method `{method}` defined"),
        None => "`super` is not allowed from outside of method".to_string(),
    };
    Diagnostic::new("UnexpectedSuper", expression(node), header)
}

pub fn method_definition_missing(
    node: &Node,
    module_name: &str,
    module_kind: bool,
    missing_method: &str,
) -> Diagnostic {
    let method_name = if module_kind {
        format!(".{missing_method}")
    } else {
        format!("#{missing_method}")
    };
    Diagnostic::new(
        "MethodDefinitionMissing",
        node.child_node(0).and_then(|name| expression(name)),
        format!("Cannot find implementation of method `{module_name}{method_name}`"),
    )
}

pub fn unexpected_dynamic_method(node: &Node, method_name: &str) -> Diagnostic {
    Diagnostic::new(
        "UnexpectedDynamicMethod",
        node.child_node(0).and_then(|name| expression(name)),
        format!("@dynamic annotation contains unknown method name `{method_name}`"),
    )
}

pub fn unknown_constant(node: &Node, name: &str, kind: &str) -> Diagnostic {
    Diagnostic::new(
        "UnknownConstant",
        node.location.name,
        format!("Cannot find the declaration of {kind}: `{name}`"),
    )
}

pub fn unknown_instance_variable(node: &Node, name: &str) -> Diagnostic {
    Diagnostic::new(
        "UnknownInstanceVariable",
        node.location.name,
        format!("Cannot find the declaration of instance variable: `{name}`"),
    )
}

pub fn unknown_global_variable(node: &Node, name: &str) -> Diagnostic {
    Diagnostic::new(
        "UnknownGlobalVariable",
        node.location.name,
        format!("Cannot find the declaration of global variable: `{name}`"),
    )
}

pub fn fallback_any(node: &Node) -> Diagnostic {
    Diagnostic::new(
        "FallbackAny",
        expression(node),
        "Cannot detect the type of the expression",
    )
}

pub fn unsatisfiable_constraint(
    node: &Node,
    method_type: &MethodType,
    variable: &str,
    sub_type: &Type,
    super_type: &Type,
    result: &SubtypingResult,
) -> Diagnostic {
    Diagnostic::new(
        "UnsatisfiableConstraint",
        expression(node),
        format!("Unsatisfiable constraint `{sub_type} <: {variable} <: {super_type}` is generated through {method_type}"),
    )
    .with_result(result)
}

pub fn incompatible_annotation(
    node: &Node,
    variable_name: &str,
    relation: &str,
    result: &SubtypingResult,
) -> Diagnostic {
    Diagnostic::new(
        "IncompatibleAnnotation",
        expression(node),
        format!(
            "Type annotation about `{variable_name}` is incompatible since {relation} doesn't hold"
        ),
    )
    .with_result(result)
}

pub fn unreachable_branch(node: &Node) -> Diagnostic {
    Diagnostic::new(
        "UnreachableBranch",
        expression(node),
        "The branch is unreachable",
    )
}

pub fn unreachable_value_branch(
    node: &Node,
    type_: &Type,
    location: Option<SourceRange>,
) -> Diagnostic {
    Diagnostic::new(
        "UnreachableValueBranch",
        location.or(expression(node)),
        format!("The branch may evaluate to a value of `{type_}` but unreachable"),
    )
}

pub fn proc_type_expected(node: &Node, type_: &Type) -> Diagnostic {
    Diagnostic::new(
        "ProcTypeExpected",
        expression(node),
        format!("Proc type is expected but `{type_}` is specified"),
    )
}

pub fn multiple_assignment_conversion_error(
    node: &Node,
    original_type: &Type,
    returned_type: &Type,
) -> Diagnostic {
    Diagnostic::new(
        "MultipleAssignmentConversionError",
        expression(node),
        format!(
            "Cannot convert `{original_type}` to Array or tuple (`#to_ary` returns `{returned_type}`)"
        ),
    )
}

pub fn unexpected_error(node: &Node, exception: &RubyException) -> Diagnostic {
    Diagnostic::new(
        "UnexpectedError",
        expression(node),
        format!(
            "UnexpectedError: {}({})",
            exception.message, exception.class_name
        ),
    )
}

pub fn unsupported_syntax(node: &Node, message: Option<&str>) -> Diagnostic {
    let header = match message {
        Some(message) => message.to_string(),
        None => format!("Syntax `{}` is not supported in Steep", node.type_),
    };
    Diagnostic::new("UnsupportedSyntax", expression(node), header)
}

pub fn syntax_error(message: &str, location: SourceRange) -> Diagnostic {
    Diagnostic::new(
        "SyntaxError",
        Some(location),
        format!("SyntaxError: {message}"),
    )
}

pub fn annotation_syntax_error(message: &str, location: SourceRange) -> Diagnostic {
    Diagnostic::new(
        "AnnotationSyntaxError",
        Some(location),
        format!("Type annotation has a syntax error: {message}"),
    )
}

pub fn false_assertion(node: &Node, assertion_type: &Type, node_type: &Type) -> Diagnostic {
    Diagnostic::new(
        "FalseAssertion",
        expression(node),
        format!(
            "Assertion cannot hold: no relationship between inferred type (`{node_type}`) and asserted type (`{assertion_type}`)"
        ),
    )
}

pub fn rbs_error(error: &signature::Diagnostic) -> Diagnostic {
    let location = error
        .location
        .as_ref()
        .map(|location| SourceRange::new(location.start(), location.end()));
    Diagnostic::new("RBSError", location, error.header_line.clone())
}

pub fn unexpected_type_argument(location: SourceRange, method_type: &MethodType) -> Diagnostic {
    Diagnostic::new(
        "UnexpectedTypeArgument",
        Some(location),
        format!("Unexpected type arg is given to method type `{method_type}`"),
    )
}

pub fn insufficient_type_argument(
    node: &Node,
    type_arguments: usize,
    method_type: &MethodType,
) -> Diagnostic {
    Diagnostic::new(
        "InsufficientTypeArgument",
        expression(node),
        format!(
            "Requires {} types, but {type_arguments} given: `{method_type}`",
            method_type.type_params.len()
        ),
    )
}

pub fn type_argument_mismatch_error(
    location: SourceRange,
    type_argument: &Type,
    type_parameter: &TypeParam,
    result: &SubtypingResult,
) -> Diagnostic {
    Diagnostic::new(
        "TypeArgumentMismatchError",
        Some(location),
        format!("Cannot pass a type `{type_argument}` as a type parameter `{type_parameter}`"),
    )
    .with_result(result)
}

pub fn proc_hint_ignored(node: &Node, hint_type: &Type) -> Diagnostic {
    Diagnostic::new(
        "ProcHintIgnored",
        expression(node),
        format!("The type hint given to the block is ignored: `{hint_type}`"),
    )
}

pub fn invalid_ignore_comment(location: SourceRange) -> Diagnostic {
    Diagnostic::new(
        "InvalidIgnoreComment",
        Some(location),
        "Invalid ignore comment",
    )
}

pub fn redundant_ignore_comment(location: SourceRange) -> Diagnostic {
    Diagnostic::new(
        "RedundantIgnoreComment",
        Some(location),
        "Redundant ignore comment",
    )
}

pub fn unannotated_empty_collection(node: &Node) -> Diagnostic {
    Diagnostic::new(
        "UnannotatedEmptyCollection",
        expression(node),
        format!("Empty {} doesn't have type annotation", node.type_),
    )
}

pub fn unknown_record_key(node: &Node, key_inspect: &str) -> Diagnostic {
    Diagnostic::new(
        "UnknownRecordKey",
        expression(node),
        format!("Unknown key `{key_inspect}` is given to a record type"),
    )
}

pub fn undeclared_method_definition(node: &Node, method_name: &str, type_name: &str) -> Diagnostic {
    let name = if node.type_ == "defs" {
        format!("{type_name}.{method_name}")
    } else {
        format!("{type_name}#{method_name}")
    };
    Diagnostic::new(
        "UndeclaredMethodDefinition",
        node.location.name,
        format!("Method `{name}` is not declared in RBS"),
    )
}

pub fn method_definition_in_undeclared_module(node: &Node, method_name: &str) -> Diagnostic {
    Diagnostic::new(
        "MethodDefinitionInUndeclaredModule",
        node.location.name,
        format!("Method `{method_name}` is defined in undeclared module"),
    )
}

pub fn deprecated_reference(
    node: &Node,
    location: Option<SourceRange>,
    message: Option<&str>,
) -> Diagnostic {
    let header = match node.type_ {
        "send" | "csend" | "block" | "numblock" | "itblock" => "The method is deprecated",
        "const" | "casgn" => "The constant is deprecated",
        _ => "The global variable is deprecated",
    };
    let header = match message {
        Some(message) => format!("{header}: {message}"),
        None => header.to_string(),
    };
    Diagnostic::new("DeprecatedReference", location, header)
}
