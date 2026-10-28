mod support;

use std::sync::OnceLock;

use rbs_environment::Environment;
use type_checker::{AnalysisOptions, Severity, SteepVersion};

const SIGNATURES: &str = r#"
class Time
  def to_fs: (?Symbol format) -> String
  def to_fs: (?Symbol format) -> String
end

class Object
  def it: () -> untyped
end

module Worker
  module ClassMethods
    def worker_options: (**untyped) -> void
    def worker_options: (**untyped) -> void
  end
end

class Job
  extend Worker::ClassMethods
  def perform: () -> void
end

class Clock
  def now: () -> Time
  def maybe_now: () -> Time?
  def label: () -> String
  def build: () -> void
  @label: String
end

class Settings
  ENTRIES: Array[{ name: String }]
  LITERAL_ENTRIES: Array[{ name: String }]
end
"#;

const STEEP_COMPATIBLE: AnalysisOptions = AnalysisOptions {
    steep_compatible: true,
    steep_version: SteepVersion::Version2_0,
};

fn environment() -> Option<&'static Environment> {
    static ENVIRONMENT: OnceLock<Option<Environment>> = OnceLock::new();
    ENVIRONMENT
        .get_or_init(|| support::environment_with(SIGNATURES))
        .as_ref()
}

macro_rules! steep_compatible_environment_or_skip {
    () => {
        match environment() {
            Some(environment) => environment,
            None => {
                eprintln!("skipped: RBS core signatures are not found");
                return;
            }
        }
    };
}

/// `(code, covered source text)` of every diagnostic
fn diagnostics(checked: &support::Checked) -> Vec<(&'static str, String)> {
    checked
        .analysis
        .diagnostics
        .iter()
        .map(|diagnostic| {
            (
                diagnostic.code,
                checked.source[diagnostic.start..diagnostic.end].to_string(),
            )
        })
        .collect()
}

fn diagnostics_with_code(checked: &support::Checked, code: &str) -> Vec<String> {
    diagnostics(checked)
        .into_iter()
        .filter(|(diagnostic_code, _)| *diagnostic_code == code)
        .map(|(_, text)| text)
        .collect()
}

fn both_modes(environment: &Environment, source: &str) -> (support::Checked, support::Checked) {
    (
        support::check(environment, source),
        support::check_with_options(environment, source, STEEP_COMPATIBLE),
    )
}

#[test]
fn a_call_on_a_class_whose_definition_fails_to_build_is_an_unexpected_error_and_untyped() {
    let environment = steep_compatible_environment_or_skip!();
    let source = "Time.now.no_such_method\nClock.new.now.strftime(1)\n";
    let (default, compatible) = both_modes(environment, source);

    assert_eq!(
        diagnostics(&compatible),
        vec![
            ("Ruby::UnexpectedError", "Time.now".to_string()),
            (
                "Ruby::UnexpectedError",
                "Clock.new.now.strftime(1)".to_string()
            ),
        ]
    );
    let unexpected_error = &compatible.analysis.diagnostics[0];
    assert_eq!(unexpected_error.severity, Severity::Hint);
    assert!(unexpected_error.message.starts_with(
        "UnexpectedError: sig/library.rbs:3:2...3:39: ::Time#to_fs has duplicated definitions"
    ));

    assert_eq!(
        diagnostics_with_code(&default, "Ruby::UnexpectedError"),
        Vec::<String>::new()
    );
    assert_eq!(
        diagnostics_with_code(&default, "Ruby::NoMethod"),
        vec!["no_such_method"]
    );
    assert_eq!(
        diagnostics_with_code(&default, "Ruby::ArgumentTypeMismatch"),
        vec!["1"]
    );
}

#[test]
fn a_failed_nil_check_makes_the_receiver_untyped() {
    let environment = steep_compatible_environment_or_skip!();
    let source = r#"
time = Clock.new.maybe_now
return if time.nil?
time.no_such_method
"#;
    let (default, compatible) = both_modes(environment, source);
    assert_eq!(
        diagnostics(&compatible),
        vec![("Ruby::UnexpectedError", "time.nil?".to_string())]
    );
    assert_eq!(
        diagnostics_with_code(&default, "Ruby::NoMethod"),
        vec!["no_such_method"]
    );
}

#[test]
fn a_class_body_that_needs_a_broken_definition_is_not_checked() {
    let environment = steep_compatible_environment_or_skip!();
    let source = r#"
class Job
  worker_options queue: 1

  def perform
    1.no_such_method
  end
end
"#;
    let (default, compatible) = both_modes(environment, source);
    assert_eq!(
        diagnostics(&compatible),
        vec![("Ruby::UnexpectedError", source.trim().to_string())]
    );
    assert_eq!(
        diagnostics_with_code(&default, "Ruby::NoMethod"),
        vec!["no_such_method"]
    );
}

#[test]
fn a_symbol_block_argument_on_a_broken_definition_is_an_unexpected_error() {
    let environment = steep_compatible_environment_or_skip!();
    let checked = support::check_with_options(
        environment,
        "[Clock.new.now].map(&:year)\n",
        STEEP_COMPATIBLE,
    );
    assert_eq!(
        diagnostics(&checked),
        vec![("Ruby::UnexpectedError", "&:year".to_string())]
    );
}

#[test]
fn default_values_of_a_singleton_method_are_reported_twice() {
    let environment = steep_compatible_environment_or_skip!();
    let source = r#"
class Clock
  def self.at(time = Time.now)
    time
  end

  def at(time = Time.now)
    time
  end
end
"#;
    let (default, compatible) = both_modes(environment, source);
    assert_eq!(
        diagnostics_with_code(&compatible, "Ruby::UnexpectedError"),
        vec!["Time.now", "Time.now", "Time.now"]
    );
    assert_eq!(
        diagnostics_with_code(&default, "Ruby::UnexpectedError"),
        Vec::<String>::new()
    );
}

#[test]
fn it_is_a_method_call() {
    let environment = steep_compatible_environment_or_skip!();
    let (default, compatible) = both_modes(environment, "[1].map { it.no_such_method }\n");
    assert_eq!(diagnostics(&compatible), Vec::<(&str, String)>::new());
    assert_eq!(
        diagnostics_with_code(&default, "Ruby::NoMethod"),
        vec!["no_such_method"]
    );
}

#[test]
fn an_instance_variable_without_a_signature_is_untyped() {
    let environment = steep_compatible_environment_or_skip!();
    let source = r#"
class Clock
  def label
    @count = 1
    @count.no_such_method
    @label
  end
end
"#;
    let (default, compatible) = both_modes(environment, source);
    assert_eq!(
        diagnostics_with_code(&compatible, "Ruby::NoMethod"),
        Vec::<String>::new()
    );
    assert_eq!(
        diagnostics_with_code(&compatible, "Ruby::UnknownInstanceVariable"),
        vec!["@count"]
    );
    assert_eq!(
        diagnostics_with_code(&default, "Ruby::NoMethod"),
        vec!["no_such_method"]
    );
}

#[test]
fn a_method_only_in_ruby_code_returns_untyped() {
    let environment = steep_compatible_environment_or_skip!();
    let source = r#"
class Clock
  def helper
    1
  end

  def label
    helper.no_such_method
    "label"
  end
end
"#;
    let (default, compatible) = both_modes(environment, source);
    assert_eq!(
        diagnostics_with_code(&compatible, "Ruby::NoMethod"),
        vec!["helper"]
    );
    assert_eq!(
        diagnostics_with_code(&default, "Ruby::NoMethod"),
        vec!["helper", "no_such_method"]
    );
}

#[test]
fn proc_is_a_plain_method_call() {
    let environment = steep_compatible_environment_or_skip!();
    let source = "proc { 1 }.call.no_such_method\nlambda { 1 }.call.no_such_method\n";
    let (default, compatible) = both_modes(environment, source);
    assert_eq!(
        diagnostics_with_code(&compatible, "Ruby::NoMethod"),
        vec!["no_such_method"]
    );
    assert_eq!(
        diagnostics_with_code(&default, "Ruby::NoMethod"),
        vec!["no_such_method", "no_such_method"]
    );
}

#[test]
fn a_numbered_reference_is_optional() {
    let environment = steep_compatible_environment_or_skip!();
    let source = "\"a\" =~ /(a)/\n$1.upcase\n";
    let (default, compatible) = both_modes(environment, source);
    assert_eq!(
        compatible.messages_with_code("Ruby::NoMethod"),
        vec!["Type `(::String | nil)` does not have method `upcase`"]
    );
    assert_eq!(
        diagnostics_with_code(&default, "Ruby::NoMethod"),
        Vec::<String>::new()
    );
}

#[test]
fn a_frozen_collection_literal_is_not_typed_with_the_declared_type() {
    let environment = steep_compatible_environment_or_skip!();
    let source = r#"
class Settings
  ENTRIES = [{ name: "a" }].freeze
  LITERAL_ENTRIES = [{ name: "a" }]
end
"#;
    let (default, compatible) = both_modes(environment, source);
    assert_eq!(
        diagnostics_with_code(&compatible, "Ruby::IncompatibleAssignment"),
        vec!["ENTRIES = [{ name: \"a\" }].freeze"]
    );
    assert_eq!(
        diagnostics_with_code(&default, "Ruby::IncompatibleAssignment"),
        Vec::<String>::new()
    );
}

#[test]
fn a_void_value_cannot_be_assigned_to_a_declared_variable() {
    let environment = steep_compatible_environment_or_skip!();
    let source = r#"
class Clock
  def label
    @label = build
  end
end
"#;
    let (default, compatible) = both_modes(environment, source);
    assert_eq!(
        compatible.messages_with_code("Ruby::IncompatibleAssignment"),
        vec!["Cannot assign a value of type `void` to a variable of type `::String`\n  void <: ::String"]
    );
    assert_eq!(
        diagnostics_with_code(&default, "Ruby::IncompatibleAssignment"),
        Vec::<String>::new()
    );
}

#[test]
fn an_unreachable_branch_of_a_ternary_is_reported_at_the_branch() {
    let environment = steep_compatible_environment_or_skip!();
    let source = "value = 1\nlabel = value.nil? ? \"nil\" : \"value\"\n";
    let (default, compatible) = both_modes(environment, source);
    assert_eq!(
        diagnostics_with_code(&compatible, "Ruby::UnreachableBranch"),
        vec!["\"nil\""]
    );
    assert_eq!(
        diagnostics_with_code(&default, "Ruby::UnreachableBranch"),
        vec!["?"]
    );
}

#[test]
fn a_splat_in_rescue_is_unsupported() {
    let environment = steep_compatible_environment_or_skip!();
    let source = r#"
ERRORS = [StandardError]
begin
  1
rescue *ERRORS => error
  error
end
"#;
    let (default, compatible) = both_modes(environment, source);
    assert_eq!(
        diagnostics_with_code(&compatible, "Ruby::UnsupportedSyntax"),
        vec!["*ERRORS"]
    );
    assert_eq!(
        diagnostics_with_code(&default, "Ruby::UnsupportedSyntax"),
        Vec::<String>::new()
    );
}

#[test]
fn a_local_variable_that_is_nil_keeps_its_type_in_a_block() {
    let environment = steep_compatible_environment_or_skip!();
    let source = "value = nil\n[1].each { value.no_such_method }\n";
    let (default, compatible) = both_modes(environment, source);
    assert_eq!(
        diagnostics_with_code(&compatible, "Ruby::NoMethod"),
        vec!["no_such_method"]
    );
    assert_eq!(
        diagnostics_with_code(&default, "Ruby::NoMethod"),
        Vec::<String>::new()
    );
}
