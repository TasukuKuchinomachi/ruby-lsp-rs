use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Error = 1,
    Warning = 2,
    Information = 3,
    Hint = 4,
}

impl Severity {
    pub fn parse(name: &str) -> Option<Severity> {
        match name {
            "error" => Some(Severity::Error),
            "warning" => Some(Severity::Warning),
            "information" => Some(Severity::Information),
            "hint" => Some(Severity::Hint),
            _ => None,
        }
    }

    pub fn expectation_name(self) -> &'static str {
        match self {
            Severity::Error => "ERROR",
            Severity::Warning => "WARNING",
            Severity::Information => "INFORMATION",
            Severity::Hint => "HINT",
        }
    }
}

pub const ALL: &[&str] = &[
    "AnnotationSyntaxError",
    "ArgumentTypeMismatch",
    "BlockBodyTypeMismatch",
    "BlockTypeMismatch",
    "BreakTypeMismatch",
    "ClassModuleMismatch",
    "DeprecatedReference",
    "DifferentMethodParameterKind",
    "ElseOnExhaustiveCase",
    "FallbackAny",
    "FalseAssertion",
    "ImplicitBreakValueMismatch",
    "IncompatibleAnnotation",
    "IncompatibleArgumentForwarding",
    "IncompatibleAssignment",
    "InsufficientKeywordArguments",
    "InsufficientPositionalArguments",
    "InsufficientTypeArgument",
    "InvalidIgnoreComment",
    "LibraryRBSError",
    "MethodArityMismatch",
    "MethodBodyTypeMismatch",
    "MethodDefinitionInUndeclaredModule",
    "MethodDefinitionMissing",
    "MethodParameterMismatch",
    "MethodReturnTypeAnnotationMismatch",
    "MultipleAssignmentConversionError",
    "NoMethod",
    "ProcHintIgnored",
    "ProcTypeExpected",
    "RBSError",
    "RedundantIgnoreComment",
    "RequiredBlockMissing",
    "ReturnTypeMismatch",
    "SetterBodyTypeMismatch",
    "SetterReturnTypeMismatch",
    "SyntaxError",
    "TypeArgumentMismatchError",
    "UnannotatedEmptyCollection",
    "UndeclaredMethodDefinition",
    "UnexpectedBlockGiven",
    "UnexpectedDynamicMethod",
    "UnexpectedError",
    "UnexpectedJump",
    "UnexpectedJumpValue",
    "UnexpectedKeywordArgument",
    "UnexpectedPositionalArgument",
    "UnexpectedSuper",
    "UnexpectedTypeArgument",
    "UnexpectedYield",
    "UnknownConstant",
    "UnknownConstantAssigned",
    "UnknownGlobalVariable",
    "UnknownInstanceVariable",
    "UnknownRecordKey",
    "UnreachableBranch",
    "UnreachableValueBranch",
    "UnresolvedOverloading",
    "UnsatisfiableConstraint",
    "UnsupportedSyntax",
];

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Configuration {
    pub severities: BTreeMap<String, Option<Severity>>,
}

use Severity::{Error, Hint, Information, Warning};

const DEFAULT: &[(&str, Option<Severity>)] = &[
    ("AnnotationSyntaxError", Some(Error)),
    ("ArgumentTypeMismatch", Some(Error)),
    ("BlockBodyTypeMismatch", Some(Warning)),
    ("BlockTypeMismatch", Some(Warning)),
    ("BreakTypeMismatch", Some(Hint)),
    ("DeprecatedReference", Some(Warning)),
    ("DifferentMethodParameterKind", Some(Hint)),
    ("FallbackAny", Some(Hint)),
    ("FalseAssertion", Some(Hint)),
    ("ImplicitBreakValueMismatch", Some(Hint)),
    ("IncompatibleAnnotation", Some(Hint)),
    ("IncompatibleArgumentForwarding", Some(Warning)),
    ("IncompatibleAssignment", Some(Hint)),
    ("InsufficientKeywordArguments", Some(Error)),
    ("InsufficientPositionalArguments", Some(Error)),
    ("InsufficientTypeArgument", Some(Hint)),
    ("InvalidIgnoreComment", Some(Warning)),
    ("RedundantIgnoreComment", Some(Warning)),
    ("MethodArityMismatch", Some(Error)),
    ("MethodBodyTypeMismatch", Some(Error)),
    ("MethodDefinitionInUndeclaredModule", Some(Information)),
    ("MethodDefinitionMissing", None),
    ("MethodParameterMismatch", Some(Error)),
    ("MethodReturnTypeAnnotationMismatch", Some(Hint)),
    ("MultipleAssignmentConversionError", Some(Hint)),
    ("LibraryRBSError", Some(Error)),
    ("NoMethod", Some(Error)),
    ("ProcHintIgnored", Some(Hint)),
    ("ProcTypeExpected", Some(Hint)),
    ("RBSError", Some(Information)),
    ("RequiredBlockMissing", Some(Error)),
    ("ReturnTypeMismatch", Some(Error)),
    ("SetterBodyTypeMismatch", Some(Information)),
    ("SetterReturnTypeMismatch", Some(Information)),
    ("ClassModuleMismatch", Some(Error)),
    ("SyntaxError", Some(Information)),
    ("TypeArgumentMismatchError", Some(Hint)),
    ("UnannotatedEmptyCollection", Some(Warning)),
    ("UndeclaredMethodDefinition", Some(Warning)),
    ("UnexpectedBlockGiven", Some(Warning)),
    ("UnexpectedDynamicMethod", Some(Hint)),
    ("UnexpectedError", Some(Hint)),
    ("UnexpectedJump", Some(Hint)),
    ("UnexpectedJumpValue", Some(Hint)),
    ("UnexpectedKeywordArgument", Some(Error)),
    ("UnexpectedPositionalArgument", Some(Error)),
    ("UnexpectedSuper", Some(Information)),
    ("UnexpectedTypeArgument", Some(Hint)),
    ("UnexpectedYield", Some(Warning)),
    ("UnknownConstant", Some(Warning)),
    ("UnknownGlobalVariable", Some(Warning)),
    ("UnknownRecordKey", Some(Information)),
    ("UnknownInstanceVariable", Some(Information)),
    ("UnreachableBranch", Some(Hint)),
    ("UnreachableValueBranch", Some(Hint)),
    ("UnresolvedOverloading", Some(Error)),
    ("UnsatisfiableConstraint", Some(Hint)),
    ("UnsupportedSyntax", Some(Hint)),
];

const STRICT: &[(&str, Option<Severity>)] = &[
    ("AnnotationSyntaxError", Some(Error)),
    ("ArgumentTypeMismatch", Some(Error)),
    ("BlockBodyTypeMismatch", Some(Error)),
    ("BlockTypeMismatch", Some(Error)),
    ("BreakTypeMismatch", Some(Error)),
    ("DeprecatedReference", Some(Warning)),
    ("DifferentMethodParameterKind", Some(Error)),
    ("FallbackAny", Some(Warning)),
    ("FalseAssertion", Some(Error)),
    ("ImplicitBreakValueMismatch", Some(Information)),
    ("IncompatibleAnnotation", Some(Error)),
    ("IncompatibleArgumentForwarding", Some(Error)),
    ("IncompatibleAssignment", Some(Error)),
    ("InsufficientKeywordArguments", Some(Error)),
    ("InsufficientPositionalArguments", Some(Error)),
    ("InsufficientTypeArgument", Some(Error)),
    ("InvalidIgnoreComment", Some(Warning)),
    ("RedundantIgnoreComment", Some(Warning)),
    ("MethodArityMismatch", Some(Error)),
    ("MethodBodyTypeMismatch", Some(Error)),
    ("MethodDefinitionInUndeclaredModule", Some(Warning)),
    ("MethodDefinitionMissing", Some(Hint)),
    ("MethodParameterMismatch", Some(Error)),
    ("MethodReturnTypeAnnotationMismatch", Some(Error)),
    ("MultipleAssignmentConversionError", Some(Error)),
    ("NoMethod", Some(Error)),
    ("ProcHintIgnored", Some(Information)),
    ("ProcTypeExpected", Some(Error)),
    ("RBSError", Some(Error)),
    ("RequiredBlockMissing", Some(Error)),
    ("ReturnTypeMismatch", Some(Error)),
    ("SetterBodyTypeMismatch", Some(Error)),
    ("SetterReturnTypeMismatch", Some(Error)),
    ("ClassModuleMismatch", Some(Error)),
    ("SyntaxError", Some(Information)),
    ("TypeArgumentMismatchError", Some(Error)),
    ("UnannotatedEmptyCollection", Some(Error)),
    ("UndeclaredMethodDefinition", Some(Warning)),
    ("UnexpectedBlockGiven", Some(Error)),
    ("UnexpectedDynamicMethod", Some(Information)),
    ("UnexpectedError", Some(Information)),
    ("UnexpectedJump", Some(Error)),
    ("UnexpectedJumpValue", Some(Error)),
    ("UnexpectedKeywordArgument", Some(Error)),
    ("UnexpectedPositionalArgument", Some(Error)),
    ("UnexpectedSuper", Some(Error)),
    ("UnexpectedTypeArgument", Some(Error)),
    ("UnexpectedYield", Some(Error)),
    ("UnknownConstant", Some(Error)),
    ("UnknownGlobalVariable", Some(Error)),
    ("UnknownRecordKey", Some(Warning)),
    ("UnknownInstanceVariable", Some(Error)),
    ("UnreachableBranch", Some(Information)),
    ("UnreachableValueBranch", Some(Warning)),
    ("UnresolvedOverloading", Some(Error)),
    ("UnsatisfiableConstraint", Some(Error)),
    ("UnsupportedSyntax", Some(Information)),
];

const LENIENT: &[(&str, Option<Severity>)] = &[
    ("AnnotationSyntaxError", Some(Error)),
    ("ArgumentTypeMismatch", Some(Information)),
    ("BlockBodyTypeMismatch", Some(Information)),
    ("BlockTypeMismatch", Some(Information)),
    ("BreakTypeMismatch", Some(Hint)),
    ("DeprecatedReference", Some(Warning)),
    ("DifferentMethodParameterKind", None),
    ("FallbackAny", None),
    ("FalseAssertion", None),
    ("ImplicitBreakValueMismatch", None),
    ("IncompatibleAnnotation", None),
    ("IncompatibleArgumentForwarding", Some(Information)),
    ("IncompatibleAssignment", Some(Hint)),
    ("InsufficientKeywordArguments", Some(Information)),
    ("InsufficientPositionalArguments", Some(Information)),
    ("InsufficientTypeArgument", None),
    ("InvalidIgnoreComment", Some(Warning)),
    ("RedundantIgnoreComment", Some(Hint)),
    ("MethodArityMismatch", Some(Information)),
    ("MethodBodyTypeMismatch", Some(Warning)),
    ("MethodDefinitionInUndeclaredModule", Some(Hint)),
    ("MethodDefinitionMissing", None),
    ("MethodParameterMismatch", Some(Warning)),
    ("MethodReturnTypeAnnotationMismatch", None),
    ("MultipleAssignmentConversionError", None),
    ("LibraryRBSError", Some(Error)),
    ("NoMethod", Some(Information)),
    ("ProcHintIgnored", None),
    ("ProcTypeExpected", None),
    ("RBSError", Some(Information)),
    ("RequiredBlockMissing", Some(Hint)),
    ("ReturnTypeMismatch", Some(Warning)),
    ("SetterBodyTypeMismatch", None),
    ("SetterReturnTypeMismatch", None),
    ("ClassModuleMismatch", None),
    ("SyntaxError", Some(Information)),
    ("TypeArgumentMismatchError", None),
    ("UnannotatedEmptyCollection", Some(Hint)),
    ("UndeclaredMethodDefinition", Some(Information)),
    ("UnexpectedBlockGiven", Some(Hint)),
    ("UnexpectedDynamicMethod", None),
    ("UnexpectedError", Some(Hint)),
    ("UnexpectedJump", None),
    ("UnexpectedJumpValue", None),
    ("UnexpectedKeywordArgument", Some(Information)),
    ("UnexpectedPositionalArgument", Some(Information)),
    ("UnexpectedSuper", None),
    ("UnexpectedTypeArgument", None),
    ("UnexpectedYield", Some(Information)),
    ("UnknownConstant", Some(Hint)),
    ("UnknownGlobalVariable", Some(Hint)),
    ("UnknownRecordKey", Some(Hint)),
    ("UnknownInstanceVariable", Some(Hint)),
    ("UnreachableBranch", Some(Hint)),
    ("UnreachableValueBranch", Some(Hint)),
    ("UnresolvedOverloading", Some(Information)),
    ("UnsatisfiableConstraint", Some(Hint)),
    ("UnsupportedSyntax", Some(Hint)),
];

impl Configuration {
    pub fn all_error() -> Configuration {
        Configuration {
            severities: ALL
                .iter()
                .map(|name| (name.to_string(), Some(Error)))
                .collect(),
        }
    }

    fn all_error_merged(overrides: &[(&str, Option<Severity>)]) -> Configuration {
        let mut configuration = Configuration::all_error();
        for (name, severity) in overrides {
            configuration.severities.insert(name.to_string(), *severity);
        }
        configuration
    }

    pub fn default_configuration() -> Configuration {
        Configuration::all_error_merged(DEFAULT)
    }

    pub fn strict() -> Configuration {
        Configuration::all_error_merged(STRICT)
    }

    pub fn lenient() -> Configuration {
        Configuration::all_error_merged(LENIENT)
    }

    pub fn silent() -> Configuration {
        Configuration {
            severities: ALL.iter().map(|name| (name.to_string(), None)).collect(),
        }
    }

    pub fn preset(name: &str) -> Option<Configuration> {
        match name {
            "all_error" => Some(Configuration::all_error()),
            "default" => Some(Configuration::default_configuration()),
            "strict" => Some(Configuration::strict()),
            "lenient" => Some(Configuration::lenient()),
            "silent" => Some(Configuration::silent()),
            _ => None,
        }
    }

    pub fn merge(&mut self, other: &Configuration) {
        for (name, severity) in &other.severities {
            self.severities.insert(name.clone(), *severity);
        }
    }

    pub fn severity_for(&self, error_name: &str) -> Option<Severity> {
        match self.severities.get(error_name) {
            Some(severity) => *severity,
            None => Some(Error),
        }
    }
}
