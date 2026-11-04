use std::collections::BTreeSet;

use crate::{diagnostic::configuration::Severity, ruby::node::SourceRange};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum StrictCheck {
    RubySyntaxError,
    SignatureLoadFailure,
    TypeCheckAborted,
    UnsupportedSyntax,
    SingletonClassReceiver,
    DefinedExpression,
    UnexpectedSplat,
    UnexpectedAssignmentTarget,
    MultipleAssignmentTarget,
    SkipAssignment,
    InvalidTypeAssertion,
    UnrecognizedAnnotation,
    UnconstrainedTypeVariable,
    UntypedMethodParameter,
    UntypedCallArguments,
    UntypedYieldArguments,
    UntypedReceiver,
    UntypedInUnion,
    DroppedUntypedBranch,
    DroppedUntypedHashElement,
    DroppedHashSplat,
    UntypedAssignedToDeclaredVariable,
    HiddenSteepDiagnostics,
    UntypedBlockParameters,
    IgnoredBlockParameter,
    BlockGivenToUntypedMethod,
    UntypedLambdaParameters,
    UntypedEmptyLambda,
    UncheckedJumpValue,
    BareRescueVariable,
    UntypedRescueVariable,
    RescueAssignmentTarget,
    UntypedForVariable,
    UntypedMultipleAssignment,
}

pub struct CheckGroup {
    pub title: &'static str,
    pub checks: &'static [StrictCheck],
}

pub const GROUPS: &[CheckGroup] = &[
    CheckGroup {
        title: "Files Steep does not type check while reporting nothing visible",
        checks: &[
            StrictCheck::RubySyntaxError,
            StrictCheck::SignatureLoadFailure,
            StrictCheck::TypeCheckAborted,
        ],
    },
    CheckGroup {
        title: "Code Steep types as untyped without checking it",
        checks: &[
            StrictCheck::UnsupportedSyntax,
            StrictCheck::SingletonClassReceiver,
            StrictCheck::DefinedExpression,
            StrictCheck::UnexpectedSplat,
            StrictCheck::UnexpectedAssignmentTarget,
            StrictCheck::MultipleAssignmentTarget,
            StrictCheck::SkipAssignment,
        ],
    },
    CheckGroup {
        title: "Comments Steep ignores",
        checks: &[
            StrictCheck::InvalidTypeAssertion,
            StrictCheck::UnrecognizedAnnotation,
        ],
    },
    CheckGroup {
        title: "Types Steep cannot determine from the signatures and makes untyped",
        checks: &[
            StrictCheck::UnconstrainedTypeVariable,
            StrictCheck::UntypedMethodParameter,
            StrictCheck::UntypedCallArguments,
            StrictCheck::UntypedYieldArguments,
        ],
    },
    CheckGroup {
        title: "Untyped values Steep passes on or drops without checking",
        checks: &[
            StrictCheck::UntypedReceiver,
            StrictCheck::UntypedInUnion,
            StrictCheck::DroppedUntypedBranch,
            StrictCheck::DroppedUntypedHashElement,
            StrictCheck::DroppedHashSplat,
            StrictCheck::UntypedAssignedToDeclaredVariable,
        ],
    },
    CheckGroup {
        title: "Steep diagnostics the Steepfile hides",
        checks: &[StrictCheck::HiddenSteepDiagnostics],
    },
    CheckGroup {
        title: "Blocks, lambdas, rescue, for and multiple assignments Steep types as untyped",
        checks: &[
            StrictCheck::UntypedBlockParameters,
            StrictCheck::IgnoredBlockParameter,
            StrictCheck::BlockGivenToUntypedMethod,
            StrictCheck::UntypedLambdaParameters,
            StrictCheck::UntypedEmptyLambda,
            StrictCheck::UncheckedJumpValue,
            StrictCheck::BareRescueVariable,
            StrictCheck::UntypedRescueVariable,
            StrictCheck::RescueAssignmentTarget,
            StrictCheck::UntypedForVariable,
            StrictCheck::UntypedMultipleAssignment,
        ],
    },
];

impl StrictCheck {
    pub fn all() -> impl Iterator<Item = StrictCheck> {
        GROUPS.iter().flat_map(|group| group.checks.iter().copied())
    }

    pub fn name(self) -> &'static str {
        match self {
            StrictCheck::RubySyntaxError => "ruby-syntax-error",
            StrictCheck::SignatureLoadFailure => "signature-load-failure",
            StrictCheck::TypeCheckAborted => "type-check-aborted",
            StrictCheck::UnsupportedSyntax => "unsupported-syntax",
            StrictCheck::SingletonClassReceiver => "singleton-class-receiver",
            StrictCheck::DefinedExpression => "defined-expression",
            StrictCheck::UnexpectedSplat => "unexpected-splat",
            StrictCheck::UnexpectedAssignmentTarget => "unexpected-assignment-target",
            StrictCheck::MultipleAssignmentTarget => "multiple-assignment-target",
            StrictCheck::SkipAssignment => "skip-assignment",
            StrictCheck::InvalidTypeAssertion => "invalid-type-assertion",
            StrictCheck::UnrecognizedAnnotation => "unrecognized-annotation",
            StrictCheck::UnconstrainedTypeVariable => "unconstrained-type-variable",
            StrictCheck::UntypedMethodParameter => "untyped-method-parameter",
            StrictCheck::UntypedCallArguments => "untyped-call-arguments",
            StrictCheck::UntypedYieldArguments => "untyped-yield-arguments",
            StrictCheck::UntypedReceiver => "untyped-receiver",
            StrictCheck::UntypedInUnion => "untyped-in-union",
            StrictCheck::DroppedUntypedBranch => "dropped-untyped-branch",
            StrictCheck::DroppedUntypedHashElement => "dropped-untyped-hash-element",
            StrictCheck::DroppedHashSplat => "dropped-hash-splat",
            StrictCheck::UntypedAssignedToDeclaredVariable => {
                "untyped-assigned-to-declared-variable"
            }
            StrictCheck::HiddenSteepDiagnostics => "hidden-steep-diagnostics",
            StrictCheck::UntypedBlockParameters => "untyped-block-parameters",
            StrictCheck::IgnoredBlockParameter => "ignored-block-parameter",
            StrictCheck::BlockGivenToUntypedMethod => "block-given-to-untyped-method",
            StrictCheck::UntypedLambdaParameters => "untyped-lambda-parameters",
            StrictCheck::UntypedEmptyLambda => "untyped-empty-lambda",
            StrictCheck::UncheckedJumpValue => "unchecked-jump-value",
            StrictCheck::BareRescueVariable => "bare-rescue-variable",
            StrictCheck::UntypedRescueVariable => "untyped-rescue-variable",
            StrictCheck::RescueAssignmentTarget => "rescue-assignment-target",
            StrictCheck::UntypedForVariable => "untyped-for-variable",
            StrictCheck::UntypedMultipleAssignment => "untyped-multiple-assignment",
        }
    }

    pub fn code(self) -> String {
        let pascal_case: String = self
            .name()
            .split('-')
            .map(|word| {
                let mut characters = word.chars();
                characters
                    .next()
                    .map(|first| first.to_ascii_uppercase().to_string() + characters.as_str())
                    .unwrap_or_default()
            })
            .collect();
        format!("Strict::{pascal_case}")
    }

    pub fn description(self) -> &'static str {
        match self {
            StrictCheck::RubySyntaxError => {
                "A file with a Ruby syntax error, whose Ruby::SyntaxError the Steepfile does not show"
            }
            StrictCheck::SignatureLoadFailure => {
                "A file skipped because the signatures of its target failed to load, without Ruby::LibraryRBSError"
            }
            StrictCheck::TypeCheckAborted => {
                "A file whose type checking a Ruby exception aborted outside TypeConstruction#synthesize"
            }
            StrictCheck::UnsupportedSyntax => {
                "Syntax Steep does not support, typed as untyped with its children unchecked"
            }
            StrictCheck::SingletonClassReceiver => {
                "`class << expression` whose receiver type Steep cannot open, with its body unchecked"
            }
            StrictCheck::DefinedExpression => {
                "The expression inside `defined?(...)`, never type checked"
            }
            StrictCheck::UnexpectedSplat => {
                "A splat where Steep does not expect one, typed as untyped"
            }
            StrictCheck::UnexpectedAssignmentTarget => {
                "An operator assignment (`+=`, `||=`, `&&=`) to a target Steep does not handle"
            }
            StrictCheck::MultipleAssignmentTarget => {
                "A target of a multiple assignment other than a local, instance or global variable"
            }
            StrictCheck::SkipAssignment => {
                "`__skip__ = expression`, whose right hand side is not type checked"
            }
            StrictCheck::InvalidTypeAssertion => "A `#: type` comment whose type does not parse",
            StrictCheck::UnrecognizedAnnotation => {
                "A `# @type`, `# @implements` or `# @dynamic` comment that matches no annotation syntax"
            }
            StrictCheck::UnconstrainedTypeVariable => {
                "A type variable in the return type of a method call that nothing constrains, typed as untyped"
            }
            StrictCheck::UntypedMethodParameter => {
                "A parameter of a method definition typed as untyped because the method type takes `(?)`"
            }
            StrictCheck::UntypedCallArguments => {
                "Arguments of a method call not checked against parameters because the method type takes `(?)`"
            }
            StrictCheck::UntypedYieldArguments => {
                "`yield` whose arguments are not checked because the method or block type takes `(?)`"
            }
            StrictCheck::UntypedReceiver => {
                "A method call on an untyped receiver, whose arguments and block are not checked against any method type"
            }
            StrictCheck::UntypedInUnion => {
                "An untyped operand of `&&`, `||`, `&.`, `rescue` or a range that makes the whole union untyped"
            }
            StrictCheck::DroppedUntypedBranch => {
                "An untyped branch of `if` or `case` whose type Steep drops from the type of the value"
            }
            StrictCheck::DroppedUntypedHashElement => {
                "An untyped key or value of a hash literal whose type Steep drops from the type of the hash"
            }
            StrictCheck::DroppedHashSplat => {
                "A `**` in a hash literal whose value is not a Hash, with its keys and values dropped from the type of the hash"
            }
            StrictCheck::UntypedAssignedToDeclaredVariable => {
                "An untyped value assigned to a local variable with a declared type, which Steep assumes to have that type"
            }
            StrictCheck::HiddenSteepDiagnostics => {
                "Steep's own diagnostics the Steepfile hides, at the severity of the strict preset (error if it hides them too)"
            }
            StrictCheck::UntypedBlockParameters => {
                "Block parameters typed as untyped because the block type has `(?)` parameters or one untyped parameter to expand"
            }
            StrictCheck::IgnoredBlockParameter => {
                "A keyword or trailing block parameter Steep gives no type, leaving the variable unknown"
            }
            StrictCheck::BlockGivenToUntypedMethod => {
                "A block given to a method of type `(?) -> T` without a block type, typed without any hint"
            }
            StrictCheck::UntypedLambdaParameters => {
                "A lambda parameter typed as untyped because the lambda has no proc type hint"
            }
            StrictCheck::UntypedEmptyLambda => {
                "A lambda without a body, whose return type Steep makes untyped instead of nil"
            }
            StrictCheck::UncheckedJumpValue => {
                "The value of `break` or `next` in a block or lambda Steep checks against nothing"
            }
            StrictCheck::BareRescueVariable => {
                "`rescue => variable` without exception classes, typing the variable as untyped instead of StandardError"
            }
            StrictCheck::UntypedRescueVariable => {
                "A rescue variable typed as untyped because of an exception class expression or its type parameters"
            }
            StrictCheck::RescueAssignmentTarget => {
                "`rescue => target` to something other than a local variable, which Steep never assigns"
            }
            StrictCheck::UntypedForVariable => {
                "The variable of a `for` loop typed as untyped from the collection or its `each`"
            }
            StrictCheck::UntypedMultipleAssignment => {
                "A multiple assignment whose untyped right hand side makes every target untyped"
            }
        }
    }

    pub fn parse(name: &str) -> Option<StrictCheck> {
        StrictCheck::all().find(|check| check.name() == name)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EnabledChecks {
    checks: BTreeSet<StrictCheck>,
}

impl EnabledChecks {
    pub fn strict() -> EnabledChecks {
        EnabledChecks {
            checks: StrictCheck::all()
                .filter(|check| *check != StrictCheck::HiddenSteepDiagnostics)
                .collect(),
        }
    }

    pub fn enable(&mut self, check: StrictCheck) {
        self.checks.insert(check);
    }

    pub fn disable(&mut self, check: StrictCheck) {
        self.checks.remove(&check);
    }

    pub fn contains(&self, check: StrictCheck) -> bool {
        self.checks.contains(&check)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Observation {
    pub check: StrictCheck,
    pub location: SourceRange,
    pub message: String,
    pub covered_by: Option<StrictCheck>,
}

impl Observation {
    pub fn new(
        check: StrictCheck,
        location: SourceRange,
        message: impl Into<String>,
    ) -> Observation {
        Observation {
            check,
            location,
            message: message.into(),
            covered_by: None,
        }
    }

    pub fn covered_by(self, check: Option<StrictCheck>) -> Observation {
        Observation {
            covered_by: check,
            ..self
        }
    }

    pub fn is_shown(&self, enabled_checks: &EnabledChecks) -> bool {
        enabled_checks.contains(self.check)
            && !self
                .covered_by
                .is_some_and(|check| enabled_checks.contains(check))
    }

    pub fn severity(&self) -> Severity {
        Severity::Error
    }
}
