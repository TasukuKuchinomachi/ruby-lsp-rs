use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DiagnosticSeverity {
    Error,
    Warning,
    Information,
    Hint,
}

impl DiagnosticSeverity {
    pub fn from_name(name: &str) -> Option<DiagnosticSeverity> {
        match name {
            "error" => Some(DiagnosticSeverity::Error),
            "warning" => Some(DiagnosticSeverity::Warning),
            "information" => Some(DiagnosticSeverity::Information),
            "hint" => Some(DiagnosticSeverity::Hint),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            DiagnosticSeverity::Error => "error",
            DiagnosticSeverity::Warning => "warning",
            DiagnosticSeverity::Information => "information",
            DiagnosticSeverity::Hint => "hint",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticTemplate {
    Default,
    Strict,
    Lenient,
    Silent,
    AllError,
}

impl DiagnosticTemplate {
    pub fn from_method_name(name: &str) -> Option<DiagnosticTemplate> {
        match name {
            "default" => Some(DiagnosticTemplate::Default),
            "strict" => Some(DiagnosticTemplate::Strict),
            "lenient" => Some(DiagnosticTemplate::Lenient),
            "silent" => Some(DiagnosticTemplate::Silent),
            "all_error" => Some(DiagnosticTemplate::AllError),
            _ => None,
        }
    }

    pub fn severity(self, code: &str) -> Option<Option<DiagnosticSeverity>> {
        let (_, default, strict, lenient) = RUBY_DIAGNOSTIC_TEMPLATES
            .iter()
            .find(|(template_code, ..)| *template_code == code)?;
        Some(match self {
            DiagnosticTemplate::Default => *default,
            DiagnosticTemplate::Strict => *strict,
            DiagnosticTemplate::Lenient => *lenient,
            DiagnosticTemplate::Silent => None,
            DiagnosticTemplate::AllError => Some(DiagnosticSeverity::Error),
        })
    }
}

pub fn ruby_diagnostic_codes() -> impl Iterator<Item = &'static str> {
    RUBY_DIAGNOSTIC_TEMPLATES.iter().map(|(code, ..)| *code)
}

use DiagnosticSeverity::{Error, Hint, Information, Warning};

/// Transcribed from steep 2.0.0 `lib/steep/diagnostic/ruby.rb` (`Ruby.default`, `Ruby.strict`, `Ruby.lenient`). Each
/// template there is `all_error.merge(...)`, so a code it leaves out is `:error`: that is why `Ruby::LibraryRBSError`,
/// which `strict` does not list, is `Error` in the strict column. `silent` and `all_error` need no column
const RUBY_DIAGNOSTIC_TEMPLATES: &[(
    &str,
    Option<DiagnosticSeverity>,
    Option<DiagnosticSeverity>,
    Option<DiagnosticSeverity>,
)] = &[
    (
        "Ruby::AnnotationSyntaxError",
        Some(Error),
        Some(Error),
        Some(Error),
    ),
    (
        "Ruby::ArgumentTypeMismatch",
        Some(Error),
        Some(Error),
        Some(Information),
    ),
    (
        "Ruby::BlockBodyTypeMismatch",
        Some(Warning),
        Some(Error),
        Some(Information),
    ),
    (
        "Ruby::BlockTypeMismatch",
        Some(Warning),
        Some(Error),
        Some(Information),
    ),
    (
        "Ruby::BreakTypeMismatch",
        Some(Hint),
        Some(Error),
        Some(Hint),
    ),
    ("Ruby::ClassModuleMismatch", Some(Error), Some(Error), None),
    (
        "Ruby::DeprecatedReference",
        Some(Warning),
        Some(Warning),
        Some(Warning),
    ),
    (
        "Ruby::DifferentMethodParameterKind",
        Some(Hint),
        Some(Error),
        None,
    ),
    ("Ruby::FallbackAny", Some(Hint), Some(Warning), None),
    ("Ruby::FalseAssertion", Some(Hint), Some(Error), None),
    (
        "Ruby::ImplicitBreakValueMismatch",
        Some(Hint),
        Some(Information),
        None,
    ),
    (
        "Ruby::IncompatibleAnnotation",
        Some(Hint),
        Some(Error),
        None,
    ),
    (
        "Ruby::IncompatibleArgumentForwarding",
        Some(Warning),
        Some(Error),
        Some(Information),
    ),
    (
        "Ruby::IncompatibleAssignment",
        Some(Hint),
        Some(Error),
        Some(Hint),
    ),
    (
        "Ruby::InsufficientKeywordArguments",
        Some(Error),
        Some(Error),
        Some(Information),
    ),
    (
        "Ruby::InsufficientPositionalArguments",
        Some(Error),
        Some(Error),
        Some(Information),
    ),
    (
        "Ruby::InsufficientTypeArgument",
        Some(Hint),
        Some(Error),
        None,
    ),
    (
        "Ruby::InvalidIgnoreComment",
        Some(Warning),
        Some(Warning),
        Some(Warning),
    ),
    (
        "Ruby::LibraryRBSError",
        Some(Error),
        Some(Error),
        Some(Error),
    ),
    (
        "Ruby::MethodArityMismatch",
        Some(Error),
        Some(Error),
        Some(Information),
    ),
    (
        "Ruby::MethodBodyTypeMismatch",
        Some(Error),
        Some(Error),
        Some(Warning),
    ),
    (
        "Ruby::MethodDefinitionInUndeclaredModule",
        Some(Information),
        Some(Warning),
        Some(Hint),
    ),
    ("Ruby::MethodDefinitionMissing", None, Some(Hint), None),
    (
        "Ruby::MethodParameterMismatch",
        Some(Error),
        Some(Error),
        Some(Warning),
    ),
    (
        "Ruby::MethodReturnTypeAnnotationMismatch",
        Some(Hint),
        Some(Error),
        None,
    ),
    (
        "Ruby::MultipleAssignmentConversionError",
        Some(Hint),
        Some(Error),
        None,
    ),
    (
        "Ruby::NoMethod",
        Some(Error),
        Some(Error),
        Some(Information),
    ),
    ("Ruby::ProcHintIgnored", Some(Hint), Some(Information), None),
    ("Ruby::ProcTypeExpected", Some(Hint), Some(Error), None),
    (
        "Ruby::RBSError",
        Some(Information),
        Some(Error),
        Some(Information),
    ),
    (
        "Ruby::RedundantIgnoreComment",
        Some(Warning),
        Some(Warning),
        Some(Hint),
    ),
    (
        "Ruby::RequiredBlockMissing",
        Some(Error),
        Some(Error),
        Some(Hint),
    ),
    (
        "Ruby::ReturnTypeMismatch",
        Some(Error),
        Some(Error),
        Some(Warning),
    ),
    (
        "Ruby::SetterBodyTypeMismatch",
        Some(Information),
        Some(Error),
        None,
    ),
    (
        "Ruby::SetterReturnTypeMismatch",
        Some(Information),
        Some(Error),
        None,
    ),
    (
        "Ruby::SyntaxError",
        Some(Information),
        Some(Information),
        Some(Information),
    ),
    (
        "Ruby::TypeArgumentMismatchError",
        Some(Hint),
        Some(Error),
        None,
    ),
    (
        "Ruby::UnannotatedEmptyCollection",
        Some(Warning),
        Some(Error),
        Some(Hint),
    ),
    (
        "Ruby::UndeclaredMethodDefinition",
        Some(Warning),
        Some(Warning),
        Some(Information),
    ),
    (
        "Ruby::UnexpectedBlockGiven",
        Some(Warning),
        Some(Error),
        Some(Hint),
    ),
    (
        "Ruby::UnexpectedDynamicMethod",
        Some(Hint),
        Some(Information),
        None,
    ),
    (
        "Ruby::UnexpectedError",
        Some(Hint),
        Some(Information),
        Some(Hint),
    ),
    ("Ruby::UnexpectedJump", Some(Hint), Some(Error), None),
    ("Ruby::UnexpectedJumpValue", Some(Hint), Some(Error), None),
    (
        "Ruby::UnexpectedKeywordArgument",
        Some(Error),
        Some(Error),
        Some(Information),
    ),
    (
        "Ruby::UnexpectedPositionalArgument",
        Some(Error),
        Some(Error),
        Some(Information),
    ),
    (
        "Ruby::UnexpectedSuper",
        Some(Information),
        Some(Error),
        None,
    ),
    (
        "Ruby::UnexpectedTypeArgument",
        Some(Hint),
        Some(Error),
        None,
    ),
    (
        "Ruby::UnexpectedYield",
        Some(Warning),
        Some(Error),
        Some(Information),
    ),
    (
        "Ruby::UnknownConstant",
        Some(Warning),
        Some(Error),
        Some(Hint),
    ),
    (
        "Ruby::UnknownGlobalVariable",
        Some(Warning),
        Some(Error),
        Some(Hint),
    ),
    (
        "Ruby::UnknownInstanceVariable",
        Some(Information),
        Some(Error),
        Some(Hint),
    ),
    (
        "Ruby::UnknownRecordKey",
        Some(Information),
        Some(Warning),
        Some(Hint),
    ),
    (
        "Ruby::UnreachableBranch",
        Some(Hint),
        Some(Information),
        Some(Hint),
    ),
    (
        "Ruby::UnreachableValueBranch",
        Some(Hint),
        Some(Warning),
        Some(Hint),
    ),
    (
        "Ruby::UnresolvedOverloading",
        Some(Error),
        Some(Error),
        Some(Information),
    ),
    (
        "Ruby::UnsatisfiableConstraint",
        Some(Hint),
        Some(Error),
        Some(Hint),
    ),
    (
        "Ruby::UnsupportedSyntax",
        Some(Hint),
        Some(Information),
        Some(Hint),
    ),
];

/// A `configure_code_diagnostics` result: Steep diagnostic code (`Ruby::NoMethod`) to severity, where `None` disables the
/// diagnostic
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticConfiguration {
    severities: HashMap<String, Option<DiagnosticSeverity>>,
}

impl Default for DiagnosticConfiguration {
    fn default() -> Self {
        DiagnosticConfiguration::from_template(DiagnosticTemplate::Default)
    }
}

impl DiagnosticConfiguration {
    pub fn from_template(template: DiagnosticTemplate) -> DiagnosticConfiguration {
        let severities = ruby_diagnostic_codes()
            .map(|code| (code.to_string(), template.severity(code).flatten()))
            .collect();
        DiagnosticConfiguration { severities }
    }

    pub fn set(&mut self, code: &str, severity: Option<DiagnosticSeverity>) {
        self.severities.insert(code.to_string(), severity);
    }

    pub fn merge(&mut self, other: &DiagnosticConfiguration) {
        for (code, severity) in &other.severities {
            self.set(code, *severity);
        }
    }

    /// `None` for a code Steep does not know, `Some(None)` for a disabled one. A known code missing from a partial hash
    /// (a group given `{ D::Ruby::NoMethod => :hint }` without a block) is `Error`, because steep 2.0.0
    /// `lib/steep/diagnostic/lsp_formatter.rb` looks severities up with `config.fetch(klass, ERROR)`
    pub fn severity(&self, code: &str) -> Option<Option<DiagnosticSeverity>> {
        match self.severities.get(code) {
            Some(severity) => Some(*severity),
            None => ruby_diagnostic_codes()
                .any(|known| known == code)
                .then_some(Some(DiagnosticSeverity::Error)),
        }
    }

    pub fn severities(&self) -> &HashMap<String, Option<DiagnosticSeverity>> {
        &self.severities
    }
}

impl FromIterator<(String, Option<DiagnosticSeverity>)> for DiagnosticConfiguration {
    fn from_iter<I: IntoIterator<Item = (String, Option<DiagnosticSeverity>)>>(
        iterator: I,
    ) -> Self {
        DiagnosticConfiguration {
            severities: iterator.into_iter().collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn covers_every_steep_ruby_diagnostic() {
        assert_eq!(ruby_diagnostic_codes().count(), 58);
    }

    #[test]
    fn reads_each_template() {
        assert_eq!(
            DiagnosticTemplate::Default.severity("Ruby::IncompatibleAssignment"),
            Some(Some(Hint))
        );
        assert_eq!(
            DiagnosticTemplate::Default.severity("Ruby::MethodDefinitionMissing"),
            Some(None)
        );
        assert_eq!(
            DiagnosticTemplate::Strict.severity("Ruby::LibraryRBSError"),
            Some(Some(Error))
        );
        assert_eq!(
            DiagnosticTemplate::Lenient.severity("Ruby::NoMethod"),
            Some(Some(Information))
        );
        assert_eq!(
            DiagnosticTemplate::Silent.severity("Ruby::NoMethod"),
            Some(None)
        );
        assert_eq!(
            DiagnosticTemplate::AllError.severity("Ruby::UnreachableBranch"),
            Some(Some(Error))
        );
        assert_eq!(
            DiagnosticTemplate::Default.severity("Ruby::NotASteepDiagnostic"),
            None
        );
    }

    #[test]
    fn treats_known_codes_missing_from_a_partial_hash_as_errors() {
        let configuration: DiagnosticConfiguration = [("Ruby::NoMethod".to_string(), Some(Hint))]
            .into_iter()
            .collect();
        assert_eq!(configuration.severity("Ruby::NoMethod"), Some(Some(Hint)));
        assert_eq!(
            configuration.severity("Ruby::UnreachableBranch"),
            Some(Some(Error))
        );
        assert_eq!(configuration.severity("Ruby::NotASteepDiagnostic"), None);
    }

    #[test]
    fn orders_severities_from_the_most_severe() {
        assert!(Error < Warning && Warning < Information && Information < Hint);
    }
}
