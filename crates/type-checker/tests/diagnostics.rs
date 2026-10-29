mod support;

use type_checker::{AnalysisOptions, Severity, SteepVersion};

#[test]
fn calling_a_missing_method_on_an_optional_reports_no_method() {
    let environment = environment_or_skip!();
    let checked = support::check(
        environment,
        r#"
name = Greeter.new("a").maybe_name
name.upcase
"#,
    );
    assert_eq!(
        checked.diagnostic_messages(),
        vec!["Ruby::NoMethod: Type `(::String | nil)` does not have method `upcase`"]
    );
    assert_eq!(checked.diagnostic_text("Ruby::NoMethod"), Some("upcase"));
}

#[test]
fn calling_an_undefined_method_reports_no_method() {
    let environment = environment_or_skip!();
    let checked = support::check(
        environment,
        r#"
"text".no_such_method
value = [1, "a"].first || 0
value.upcase

class Greeter
  def call_missing
    undefined_helper
  end
end
"#,
    );
    assert_eq!(
        checked.messages_with_code("Ruby::NoMethod"),
        vec![
            "Type `::String` does not have method `no_such_method`",
            "Type `(::Integer | ::String)` does not have method `upcase`",
            "Type `::Greeter` does not have method `undefined_helper`",
        ]
    );
    assert_eq!(
        checked.messages_with_code("Ruby::UndeclaredMethodDefinition"),
        vec!["Method `::Greeter#call_missing` is not declared in RBS"]
    );
}

#[test]
fn no_method_is_not_reported_for_untyped_receivers() {
    let environment = environment_or_skip!();
    let checked = support::check(
        environment,
        r#"
def anything(value)
  value.whatever
end
UnknownLibrary::Client.new.request
[].first.whatever
"#,
    );
    assert!(
        checked.messages_with_code("Ruby::NoMethod").is_empty(),
        "{:?}",
        checked.diagnostic_messages()
    );
}

#[test]
fn no_method_is_reported_like_steep_regardless_of_ruby_definitions() {
    let environment = environment_or_skip!();
    let checked = support::check(
        environment,
        r#"
Dynamic.new.anything_goes

module Shape
  def outline
    method_from_including_class
  end
end

class Greeter
  def defined_in_ruby_only
    helper_written_below
  end

  def helper_written_below
    1
  end

  define_method(:generated) { 1 }

  def use_generated
    generated
  end
end
"#,
    );
    assert_eq!(
        checked.messages_with_code("Ruby::NoMethod"),
        vec![
            "Type `::Dynamic` does not have method `anything_goes`",
            "Type `::Shape` does not have method `method_from_including_class`",
            "Type `::Greeter` does not have method `helper_written_below`",
            "Type `::Greeter` does not have method `generated`",
        ]
    );
}

#[test]
fn positional_argument_count_is_checked() {
    let environment = environment_or_skip!();
    let checked = support::check(
        environment,
        r#"
greeter = Greeter.new
Greeter.new("a").greet("b", "c")
"#,
    );
    assert_eq!(
        checked.diagnostic_codes(),
        vec![
            "Ruby::InsufficientPositionalArguments",
            "Ruby::UnexpectedPositionalArgument"
        ]
    );
    assert_eq!(
        checked.diagnostic_text("Ruby::UnexpectedPositionalArgument"),
        Some("\"c\"")
    );
}

#[test]
fn keyword_arguments_are_checked() {
    let environment = environment_or_skip!();
    let checked = support::check(
        environment,
        r#"
greeter = Greeter.new("a")
greeter.with_keywords(age: 1)
greeter.with_keywords(name: "a", color: "red")
greeter.with_keywords(name: "a", age: 2)
"#,
    );
    assert_eq!(
        checked.diagnostic_codes(),
        vec![
            "Ruby::InsufficientKeywordArguments",
            "Ruby::UnexpectedKeywordArgument"
        ]
    );
    assert_eq!(
        checked.diagnostic_text("Ruby::UnexpectedKeywordArgument"),
        Some("color")
    );
}

#[test]
fn argument_type_mismatch_follows_steep_subtyping() {
    let environment = environment_or_skip!();
    let checked = support::check(
        environment,
        r#"
greeter = Greeter.new("a")
greeter.greet(1)
greeter.greet(greeter.maybe_name)
greeter.greet([].first)
def forward(value)
  Greeter.new("a").greet(value)
end
"#,
    );
    assert_eq!(
        checked.messages_with_code("Ruby::ArgumentTypeMismatch"),
        vec![
            "Cannot pass a value of type `::Integer` as an argument of type `::String`\n  ::Integer <: ::String\n    ::Numeric <: ::String\n      ::Object <: ::String\n        ::BasicObject <: ::String",
            "Cannot pass a value of type `(::String | nil)` as an argument of type `::String`\n  (::String | nil) <: ::String\n    nil <: ::String",
        ]
    );
    assert_eq!(
        checked.diagnostic_text("Ruby::ArgumentTypeMismatch"),
        Some("1")
    );
}

#[test]
fn method_body_and_return_mismatches_are_reported() {
    let environment = environment_or_skip!();
    let checked = support::check(
        environment,
        r#"
class Greeter
  def wrong_body
    "not an integer"
  end

  def count_for(value)
    return "too many" if value > 10
    value
  end

  def loud
    "void accepts anything"
  end

  def tags
    []
  end
end
"#,
    );
    let messages: Vec<String> = checked
        .diagnostic_messages()
        .into_iter()
        .filter(|message| !message.starts_with("Ruby::MethodDefinitionMissing"))
        .collect();
    assert_eq!(
        messages,
        vec![
            "Ruby::MethodBodyTypeMismatch: Cannot allow method body have type `::String` because declared as type `::Integer`\n  ::String <: ::Integer\n    ::Object <: ::Integer\n      ::BasicObject <: ::Integer",
            "Ruby::ReturnTypeMismatch: The method cannot return a value of type `::String` because declared as type `::Integer`\n  ::String <: ::Integer\n    ::Object <: ::Integer\n      ::BasicObject <: ::Integer",
        ]
    );
    assert_eq!(
        checked.diagnostic_text("Ruby::MethodBodyTypeMismatch"),
        Some("wrong_body")
    );
}

#[test]
fn unknown_constants_are_warnings_even_when_defined_only_in_ruby() {
    let environment = environment_or_skip!();
    let checked = support::check(
        environment,
        r#"
MissingThing.call
String::Missing
LOCAL_VALUE = 1
LOCAL_VALUE
class Defined; end
Defined
"#,
    );
    assert_eq!(
        checked.messages_with_code("Ruby::UnknownConstant"),
        vec![
            "Cannot find the declaration of constant: `MissingThing`",
            "Cannot find the declaration of constant: `String::Missing`",
            "Cannot find the declaration of constant: `LOCAL_VALUE`",
            "Cannot find the declaration of constant: `LOCAL_VALUE`",
            "Cannot find the declaration of class: `Defined`",
            "Cannot find the declaration of constant: `Defined`",
        ]
    );
    assert!(
        checked
            .analysis
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.code == "Ruby::UnknownConstant")
            .all(|diagnostic| diagnostic.severity == Severity::Warning)
    );
}

#[test]
fn files_with_syntax_errors_do_not_panic() {
    let environment = environment_or_skip!();
    for source in [
        "def foo(\n  x = \n",
        "class < end",
        "[1, 2].map { |x| x. }",
        "if\nend end end",
        "foo&.&.bar",
        "a, = *",
        "def x = ",
        "case\nin [\n",
        "->(a, *b, **c, &d) { a.b.c }.()",
        "@x ||= begin; rescue => @y; ensure; end",
    ] {
        let _ = type_checker::analyze(environment, source.as_bytes());
    }
}

#[test]
fn deeply_nested_code_does_not_overflow_the_stack() {
    let environment = environment_or_skip!();
    let nested_arrays = format!("x = {}1{}", "[".repeat(5000), "]".repeat(5000));
    let _ = type_checker::analyze(environment, nested_arrays.as_bytes());
    let chained_calls = format!("x = 1{}", ".to_s.size".repeat(3000));
    let analysis = type_checker::analyze(environment, chained_calls.as_bytes());
    assert!(!analysis.type_hints.is_empty());
    let nested_ifs = format!("{}1{}", "if a\n".repeat(3000), "\nend".repeat(3000));
    let _ = type_checker::analyze(environment, nested_ifs.as_bytes());
}

#[test]
fn private_methods_called_with_an_explicit_receiver_are_not_resolved() {
    let environment = environment_or_skip!();
    let checked = support::check(
        environment,
        r#"
"text".puts("a")
puts("a")
self.puts("a")
"#,
    );
    assert_eq!(
        checked.diagnostic_messages(),
        vec!["Ruby::NoMethod: Type `::String` does not have method `puts`"]
    );
}

#[test]
fn class_reopened_with_singleton_class_syntax_defines_singleton_methods() {
    let environment = environment_or_skip!();
    let checked = support::check(
        environment,
        r#"
class Greeter
  class << Greeter
    def build_twice
      default
    end
  end
end
"#,
    );
    assert!(
        checked.messages_with_code("Ruby::NoMethod").is_empty(),
        "{:?}",
        checked.diagnostic_messages()
    );
    assert_eq!(
        checked.messages_with_code("Ruby::UndeclaredMethodDefinition"),
        vec!["Method `::Greeter.build_twice` is not declared in RBS"]
    );
}

#[test]
fn unresolved_overloading_is_reported_when_every_overload_certainly_rejects_the_arguments() {
    let environment = environment_or_skip!();
    let checked = support::check(
        environment,
        r#"
1 + "string"
1 + 2.5
1 + [].first
"#,
    );
    assert_eq!(
        checked.diagnostic_codes(),
        vec!["Ruby::UnresolvedOverloading"]
    );
    let message = &checked.analysis.diagnostics[0].message;
    assert!(
        message.starts_with("Cannot find compatible overloading of method `+` of type `::Integer`\nMethod types:\n  def +: (::Integer) -> ::Integer\n       | (::Float) -> ::Float"),
        "{message}"
    );
    assert_eq!(
        checked.diagnostic_text("Ruby::UnresolvedOverloading"),
        Some("1 + \"string\"")
    );
}

#[test]
fn unresolved_overloading_is_not_reported_for_untyped_arguments() {
    let environment = environment_or_skip!();
    let checked = support::check(environment, "def add(value)\n  1 + value\nend\n");
    assert!(
        checked
            .messages_with_code("Ruby::UnresolvedOverloading")
            .is_empty(),
        "{:?}",
        checked.diagnostic_messages()
    );
}

#[test]
fn unions_are_checked_member_by_member_like_steep() {
    let environment = environment_or_skip!();
    let checked = support::check(
        environment,
        r#"
value = [1, "a"].first
1 + value
Greeter.new("a").greet(value)
"#,
    );
    assert_eq!(
        checked.diagnostic_codes(),
        vec!["Ruby::UnresolvedOverloading", "Ruby::ArgumentTypeMismatch"]
    );
}

#[test]
fn literal_arguments_take_the_literal_types_a_parameter_expects() {
    let environment = environment_or_skip!();
    let checked = support::check(
        environment,
        "[3, 1].sort_by { |number| number }.first(2)\n\"text\".encode(\"UTF-8\", invalid: :replace)\n",
    );
    assert!(
        checked.diagnostic_codes().is_empty(),
        "{:?}",
        checked.diagnostic_messages()
    );
}

#[test]
fn definitions_are_checked_against_their_declarations_like_steep() {
    let environment = environment_or_skip!();
    let checked = support::check(
        environment,
        r#"
class Greeter
  def count_for(value = 1)
    value
  end

  def greet(*targets)
    super
  end

  def wrong_body(extra)
    1
  end
end
"#,
    );
    let diagnostics: Vec<&type_checker::TypeDiagnostic> = checked
        .analysis
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code != "Ruby::MethodDefinitionMissing")
        .collect();
    assert_eq!(
        diagnostics.iter().map(|diagnostic| diagnostic.code).collect::<Vec<_>>(),
        vec![
            "Ruby::DifferentMethodParameterKind",
            "Ruby::DifferentMethodParameterKind",
            "Ruby::UnexpectedSuper",
            "Ruby::MethodParameterMismatch",
        ],
        "{:?}",
        checked.diagnostic_messages()
    );
    let severities: Vec<Severity> = diagnostics.iter().map(|diagnostic| diagnostic.severity).collect();
    assert_eq!(
        severities,
        vec![
            Severity::Hint,
            Severity::Hint,
            Severity::Information,
            Severity::Error
        ]
    );
}

#[test]
fn assignments_to_declared_variables_and_constants_are_checked() {
    let environment = environment_or_skip!();
    let checked = support::check(
        environment,
        r#"
# @type var count: Integer
count = "one"
LIMIT = "many"
class Greeter
  def initialize(name)
    @cache = []
  end
end
"#,
    );
    assert_eq!(
        checked.messages_with_code("Ruby::IncompatibleAssignment"),
        vec![
            "Cannot assign a value of type `::String` to a variable of type `::Integer`\n  ::String <: ::Integer\n    ::Object <: ::Integer\n      ::BasicObject <: ::Integer",
            "Cannot assign a value of type `::String` to a constant of type `::Integer`\n  ::String <: ::Integer\n    ::Object <: ::Integer\n      ::BasicObject <: ::Integer",
            "Cannot assign a value of type `::Array[untyped]` to a variable of type `::Hash[::Symbol, ::Integer]`\n  ::Array[untyped] <: ::Hash[::Symbol, ::Integer]\n    ::Object <: ::Hash[::Symbol, ::Integer]\n      ::BasicObject <: ::Hash[::Symbol, ::Integer]",
        ]
    );
    assert_eq!(
        checked.diagnostic_text("Ruby::IncompatibleAssignment"),
        Some("count = \"one\"")
    );
}

#[test]
fn blocks_are_checked_against_the_method_type() {
    let environment = environment_or_skip!();
    let checked = support::check(
        environment,
        r#"
greeter = Greeter.new("a")
greeter.each_tag
greeter.greet("b") { 1 }
[greeter].map(&:no_such_method)
"#,
    );
    assert_eq!(
        checked.diagnostic_codes(),
        vec![
            "Ruby::RequiredBlockMissing",
            "Ruby::UnexpectedBlockGiven",
            "Ruby::BlockTypeMismatch"
        ],
        "{:?}",
        checked.diagnostic_messages()
    );
}

#[test]
fn unreachable_branches_and_unsupported_syntax_are_hints() {
    let environment = environment_or_skip!();
    let checked = support::check(
        environment,
        r#"
count = 1
if count.nil?
  count
end
case count
in Integer
  count
end
"#,
    );
    assert_eq!(
        checked.diagnostic_codes(),
        vec!["Ruby::UnreachableBranch", "Ruby::UnsupportedSyntax"]
    );
    assert_eq!(
        checked.diagnostic_text("Ruby::UnreachableBranch"),
        Some("if")
    );
}

#[test]
fn a_symbol_block_argument_is_checked_against_the_block_return_type() {
    let environment = environment_or_skip!();
    let source = r#"
counter = TagCounter.new
counter.optional_count(&:size)
counter.required_count(&:size)
counter.optional_count(&:upcase)
counter.required_count(&:upcase)
"#;
    let mismatch = "Cannot pass a value of type `^(::String) -> ::String` as a block-pass-argument of type `^(::String) -> ::Integer`\n  ^(::String) -> ::String <: ^(::String) -> ::Integer\n    ::String <: ::Integer\n      ::Object <: ::Integer\n        ::BasicObject <: ::Integer";
    let checked = support::check(environment, source);
    assert_eq!(
        checked.messages_with_code("Ruby::BlockTypeMismatch"),
        vec![mismatch, mismatch]
    );
    assert_eq!(checked.diagnostic_text("Ruby::BlockTypeMismatch"), Some("&:upcase"));

    let options = AnalysisOptions {
        steep_version: SteepVersion::Version2_0,
        ..AnalysisOptions::default()
    };
    let checked = support::check_with_options(environment, source, options);
    assert_eq!(checked.messages_with_code("Ruby::BlockTypeMismatch"), vec![mismatch]);
}

#[test]
fn comparing_with_a_literal_narrows_to_the_equal_literal_types_in_steep_2_0() {
    let environment = environment_or_skip!();
    let checked = support::check_with_options(
        environment,
        r#"
# @type var result: Greeter | :missing
result = :missing
if result == :missing
  result.no_such_method
else
  result.no_such_method
end
name = Greeter.new("a").name
if name == "Alice"
  name
end
"#,
        AnalysisOptions { steep_version: SteepVersion::Version2_0, ..AnalysisOptions::default() },
    );
    assert_eq!(
        checked.messages_with_code("Ruby::NoMethod"),
        vec![
            "Type `:missing` does not have method `no_such_method`",
            "Type `::Greeter` does not have method `no_such_method`",
        ]
    );
    assert_eq!(checked.diagnostic_text("Ruby::UnreachableBranch"), Some("if"));
}

#[test]
fn comparing_with_a_literal_narrows_to_the_literal_when_it_can_be_a_value_of_the_type_in_steep_2_1() {
    let environment = environment_or_skip!();
    let checked = support::check(
        environment,
        r#"
name = Greeter.new("a").name
if name == "Alice"
  name.no_such_method
else
  name.no_such_method
end
greeter = Greeter.new("a")
if greeter == :fatal
  greeter
end
"#,
    );
    assert_eq!(
        checked.messages_with_code("Ruby::NoMethod"),
        vec![
            "Type `\"Alice\"` does not have method `no_such_method`",
            "Type `::String` does not have method `no_such_method`",
        ]
    );
    assert_eq!(checked.diagnostic_text("Ruby::UnreachableBranch"), Some("if"));
}

#[test]
fn lambda_arguments_take_self_from_the_proc_type_of_the_parameter() {
    let environment = environment_or_skip!();
    let checked = support::check(
        environment,
        r#"
Greeter.with_scope(-> { greet("b") })
Greeter.check_if(:name, presence: true, if: -> { greet("b") })
"#,
    );
    assert!(
        checked.diagnostic_codes().is_empty(),
        "{:?}",
        checked.diagnostic_messages()
    );
}

#[test]
fn super_calls_a_generic_ancestor_with_the_type_arguments_of_the_class() {
    let Some(environment) = support::environment_with(
        r#"
class Parent[T]
  def value: () -> T
  def put: (T) -> void
end

class Child < Parent[String]
  def value: () -> String
  def put: (String) -> void
end

module Holder[U]
  def held: () -> U
end

class Holding
  include Holder[Integer]
  def held: () -> Integer
end
"#,
    ) else {
        eprintln!("skipped: RBS core signatures are not found");
        return;
    };
    let source = r#"
class Child < Parent
  def value
    super.no_such_method
    ""
  end

  def put(item)
    super(1)
  end
end

class Holding
  def held
    super.no_such_method
    1
  end
end
"#;
    let options = AnalysisOptions {
        steep_compatible: true,
        ..AnalysisOptions::default()
    };
    let checked = support::check_with_options(&environment, source, options);
    assert_eq!(
        checked.messages_with_code("Ruby::NoMethod"),
        vec![
            "Type `::String` does not have method `no_such_method`",
            "Type `::Integer` does not have method `no_such_method`",
        ]
    );
    assert_eq!(
        checked
            .messages_with_code("Ruby::ArgumentTypeMismatch")
            .iter()
            .map(|message| message.lines().next().unwrap_or_default())
            .collect::<Vec<_>>(),
        vec!["Cannot pass a value of type `::Integer` as an argument of type `::String`"]
    );
}
