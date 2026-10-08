mod support;

use type_checker::TypeHintKind::{
    AttributeType, BlockParameterType, KeywordParameterType, MethodParameterType,
    MethodReturnType, VariableType,
};

#[test]
fn literals_are_inferred_as_their_classes() {
    let environment = environment_or_skip!();
    let checked = support::check(
        environment,
        r#"
integer‸ = 1
float‸ = 1.5
string‸ = "a#{integer}"
symbol‸ = :name
nothing‸ = nil
flag‸ = true
regexp‸ = /a+/
range‸ = 1..10
"#,
    );
    let expected = [
        "Integer",
        "Float",
        "String",
        "Symbol",
        "nil",
        "bool",
        "Regexp",
        "Range[Integer]",
    ];
    for (index, expected_type) in expected.iter().enumerate() {
        assert_eq!(
            checked.hint(index, VariableType).as_deref(),
            Some(*expected_type),
            "marker {index}"
        );
        assert!(
            checked.is_obvious_hint(index),
            "marker {index} should be obvious"
        );
    }
}

#[test]
fn collection_literals_join_their_element_types() {
    let environment = environment_or_skip!();
    let checked = support::check(
        environment,
        r#"
mixed‸ = [1, "a"]
empty‸ = []
hash‸ = { name: "a", age: 1 }
empty_hash‸ = {}
"#,
    );
    assert_eq!(
        checked.hint(0, VariableType).as_deref(),
        Some("Array[Integer | String]")
    );
    assert_eq!(
        checked.hint(1, VariableType).as_deref(),
        Some("Array[untyped]")
    );
    assert_eq!(
        checked.hint(2, VariableType).as_deref(),
        Some("Hash[Symbol, String | Integer]")
    );
    assert_eq!(
        checked.hint(3, VariableType).as_deref(),
        Some("Hash[untyped, untyped]")
    );
}

#[test]
fn method_calls_use_the_core_signatures() {
    let environment = environment_or_skip!();
    let checked = support::check(
        environment,
        r#"
upcased‸ = "abc".upcase
length‸ = [1, 2].length
first‸ = [1, 2].find { |number| number > 1 }
"#,
    );
    assert_eq!(checked.hint(0, VariableType).as_deref(), Some("String"));
    assert!(!checked.is_obvious_hint(0));
    assert_eq!(checked.hint(1, VariableType).as_deref(), Some("Integer"));
    assert_eq!(checked.hint(2, VariableType).as_deref(), Some("Integer?"));
}

#[test]
fn generic_method_binds_its_type_parameter_from_the_block_body() {
    let environment = environment_or_skip!();
    let checked = support::check(
        environment,
        r#"
strings‸ = [1, 2].map { |number‸| number.to_s }
lengths‸ = ["a"].map(&:length)
total‸ = ["a", "bb"].inject(0) { |sum‸, text‸| sum + text.size }
"#,
    );
    assert_eq!(
        checked.hint(0, VariableType).as_deref(),
        Some("Array[String]")
    );
    assert_eq!(
        checked.hint(1, BlockParameterType).as_deref(),
        Some("Integer")
    );
    assert_eq!(
        checked.hint(2, VariableType).as_deref(),
        Some("Array[Integer]")
    );
    assert_eq!(checked.hint(3, VariableType).as_deref(), Some("Integer"));
    assert_eq!(
        checked.hint(4, BlockParameterType).as_deref(),
        Some("Integer")
    );
    assert_eq!(
        checked.hint(5, BlockParameterType).as_deref(),
        Some("String")
    );
}

#[test]
fn block_parameters_destructure_a_yielded_pair() {
    let environment = environment_or_skip!();
    let checked = support::check(
        environment,
        r#"
{ name: 1 }.each { |key‸, value‸| key }
{ name: 1 }.each { |pair‸| pair }
[[1, "a"]].each { |(number‸, text‸)| number }
"#,
    );
    assert_eq!(
        checked.hint(0, BlockParameterType).as_deref(),
        Some("Symbol")
    );
    assert_eq!(
        checked.hint(1, BlockParameterType).as_deref(),
        Some("Integer")
    );
    assert_eq!(
        checked.hint(2, BlockParameterType).as_deref(),
        Some("[Symbol, Integer]")
    );
    assert_eq!(
        checked.hint(3, BlockParameterType).as_deref(),
        Some("Integer | String")
    );
}

#[test]
fn numbered_and_it_parameters_are_typed() {
    let environment = environment_or_skip!();
    let checked = support::check(
        environment,
        r#"
["a"].each { _1‸ }
["a"].each { it‸ }
"#,
    );
    assert_eq!(checked.hover_type(0).as_deref(), Some("String"));
    assert_eq!(checked.hover_type(1).as_deref(), Some("String"));
}

#[test]
fn new_returns_an_instance_and_checks_initialize_arguments() {
    let environment = environment_or_skip!();
    let checked = support::check(
        environment,
        r#"
greeter‸ = Greeter.new("Alice")
container‸ = Container.new(1)
value‸ = container.value
mapped‸ = container.map_value { |inner| inner.to_s }
"#,
    );
    assert_eq!(checked.hint(0, VariableType).as_deref(), Some("Greeter"));
    assert!(checked.is_obvious_hint(0));
    assert_eq!(
        checked.hint(1, VariableType).as_deref(),
        Some("Container[Integer]")
    );
    assert_eq!(checked.hint(2, VariableType).as_deref(), Some("Integer"));
    assert_eq!(
        checked.hint(3, VariableType).as_deref(),
        Some("Container[String]")
    );
    assert!(
        checked.diagnostic_codes().is_empty(),
        "{:?}",
        checked.diagnostic_messages()
    );
}

#[test]
fn singleton_methods_and_included_modules_are_found() {
    let environment = environment_or_skip!();
    let checked = support::check(
        environment,
        r#"
box‸ = Box.build(1)
area‸ = box.area
default‸ = Greeter.default
"#,
    );
    assert_eq!(checked.hint(0, VariableType).as_deref(), Some("Box"));
    assert_eq!(checked.hint(1, VariableType).as_deref(), Some("Float"));
    assert_eq!(checked.hint(2, VariableType).as_deref(), Some("Greeter"));
}

#[test]
fn union_receivers_call_every_member() {
    let environment = environment_or_skip!();
    let checked = support::check(
        environment,
        r#"
value = [1, "a"].first || 0
text‸ = value.to_s
"#,
    );
    assert_eq!(checked.hint(0, VariableType).as_deref(), Some("String"));
}

#[test]
fn safe_navigation_adds_nil_to_the_result() {
    let environment = environment_or_skip!();
    let checked = support::check(
        environment,
        r#"
name = Greeter.new("a").maybe_name
upcased‸ = name&.upcase
"#,
    );
    assert_eq!(checked.hint(0, VariableType).as_deref(), Some("String?"));
    assert!(
        checked.diagnostic_codes().is_empty(),
        "{:?}",
        checked.diagnostic_messages()
    );
}

#[test]
fn if_narrows_away_nil_and_merges_branches() {
    let environment = environment_or_skip!();
    let checked = support::check(
        environment,
        r#"
name = Greeter.new("a").maybe_name
if name
  name‸.upcase
else
  name‸
end
result‸ = name ? 1 : "none"
"#,
    );
    assert_eq!(checked.hover_type(0).as_deref(), Some("String"));
    assert_eq!(checked.hover_type(1).as_deref(), Some("nil"));
    assert_eq!(
        checked.hint(2, VariableType).as_deref(),
        Some("Integer | String")
    );
    assert!(
        checked.diagnostic_codes().is_empty(),
        "{:?}",
        checked.diagnostic_messages()
    );
}

#[test]
fn nil_check_and_type_check_narrow_locals() {
    let environment = environment_or_skip!();
    let checked = support::check(
        environment,
        r#"
name = Greeter.new("a").maybe_name
unless name.nil?
  name‸
end
value = [1, "a"].first
if value.is_a?(String)
  value‸
end
if value && !value.is_a?(Integer)
  value‸
end
"#,
    );
    assert_eq!(checked.hover_type(0).as_deref(), Some("String"));
    assert_eq!(checked.hover_type(1).as_deref(), Some("String"));
    assert_eq!(checked.hover_type(2).as_deref(), Some("String"));
}

#[test]
fn case_when_narrows_the_subject() {
    let environment = environment_or_skip!();
    let checked = support::check(
        environment,
        r#"
value = [1, "a", :b].first
case value
when String
  value‸
when Integer, Symbol
  value‸
end
"#,
    );
    assert_eq!(checked.hover_type(0).as_deref(), Some("String"));
    assert_eq!(checked.hover_type(1).as_deref(), Some("Integer | Symbol"));
}

#[test]
fn guard_clauses_narrow_the_following_statements() {
    let environment = environment_or_skip!();
    let checked = support::check(
        environment,
        r#"
def shout(greeter)
  name = Greeter.new("a").maybe_name
  return unless name
  name‸.upcase
end

def whisper
  name = Greeter.new("a").maybe_name
  raise ArgumentError, "missing" if name.nil?
  name‸.downcase
end
"#,
    );
    assert_eq!(checked.hover_type(0).as_deref(), Some("String"));
    assert_eq!(checked.hover_type(1).as_deref(), Some("String"));
    assert!(
        checked.messages_with_code("Ruby::NoMethod").is_empty(),
        "{:?}",
        checked.diagnostic_messages()
    );
}

#[test]
fn instance_variables_are_typed_from_assignments_anywhere_in_the_class() {
    let environment = environment_or_skip!();
    let checked = support::check(
        environment,
        r#"
class Counter
  def increment‸
    @count‸ + 1
  end

  def initialize
    @count‸ = 0
  end
end
"#,
    );
    assert_eq!(
        checked.hint(0, MethodReturnType).as_deref(),
        Some("Integer")
    );
    assert_eq!(checked.hover_type(1).as_deref(), Some("Integer"));
    assert_eq!(checked.hint(2, VariableType).as_deref(), Some("Integer"));
}

#[test]
fn instance_variables_declared_in_rbs_use_the_declaration() {
    let environment = environment_or_skip!();
    let checked = support::check(
        environment,
        r#"
class Greeter
  def lookup
    @cache‸
  end
end
"#,
    );
    assert_eq!(
        checked.hover_type(0).as_deref(),
        Some("Hash[Symbol, Integer]")
    );
}

#[test]
fn attribute_hints_come_from_rbs_or_instance_variables() {
    let environment = environment_or_skip!();
    let checked = support::check(
        environment,
        r#"
class Greeter
  attr_reader ‸:name
end

class Counter
  attr_accessor ‸:count, ‸:label

  def initialize
    @count = 0
    @label = "counter"
  end
end
"#,
    );
    assert_eq!(checked.hint(0, AttributeType).as_deref(), Some("String"));
    assert_eq!(checked.hint(1, AttributeType).as_deref(), Some("Integer"));
    assert_eq!(checked.hint(2, AttributeType).as_deref(), Some("String"));
}

#[test]
fn method_return_hints_use_the_declaration_or_the_body() {
    let environment = environment_or_skip!();
    let checked = support::check(
        environment,
        r#"
class Greeter
  def greet(target)‸
    "Hello, #{target}"
  end

  def maybe_name‸
    nil
  end
end

class Helper
  def label‸ = "label"

  def pick(flag)‸
    return 1 if flag
    "none"
  end
end
"#,
    );
    assert_eq!(checked.hint(0, MethodReturnType).as_deref(), Some("String"));
    assert_eq!(
        checked.hint(1, MethodReturnType).as_deref(),
        Some("String?")
    );
    assert_eq!(checked.hint(2, MethodReturnType).as_deref(), Some("String"));
    assert_eq!(
        checked.hint(3, MethodReturnType).as_deref(),
        Some("Integer | String")
    );
}

#[test]
fn rbs_parameters_type_the_method_body() {
    let environment = environment_or_skip!();
    let checked = support::check(
        environment,
        r#"
class Greeter
  def greet(target)
    target‸
  end

  def each_tag
    yield "tag"
  end
end
"#,
    );
    assert_eq!(checked.hover_type(0).as_deref(), Some("String"));
}

#[test]
fn methods_without_signatures_are_inferred_from_their_body() {
    let environment = environment_or_skip!();
    let checked = support::check(
        environment,
        r#"
class Report
  def build
    total‸ = sum
    title‸ = Report.new.title
  end

  def sum
    1 + 2
  end

  def title
    "report"
  end
end

def recursive
  recursive
end
looped‸ = recursive
"#,
    );
    assert_eq!(checked.hint(0, VariableType).as_deref(), Some("Integer"));
    assert_eq!(checked.hint(1, VariableType).as_deref(), Some("String"));
    assert_eq!(checked.hint(2, VariableType).as_deref(), Some("untyped"));
}

#[test]
fn constants_resolve_to_classes_constants_and_file_values() {
    let environment = environment_or_skip!();
    let checked = support::check(
        environment,
        r#"
class Settings
  DEFAULT = "default"

  def value
    DEFAULT‸
  end
end
klass‸ = String
limit‸ = LIMIT
"#,
    );
    assert_eq!(checked.hover_type(0).as_deref(), Some("String"));
    assert_eq!(
        checked.hint(1, VariableType).as_deref(),
        Some("singleton(String)")
    );
    assert_eq!(checked.hint(2, VariableType).as_deref(), Some("Integer"));
}

#[test]
fn compound_assignments_keep_the_variable_type() {
    let environment = environment_or_skip!();
    let checked = support::check(
        environment,
        r#"
name = Greeter.new("a").maybe_name
name‸ ||= "default"
count = 1
count += 1
count‸
first‸, second‸ = 1, "two"
head‸, *tail‸ = [1, 2, 3]
"#,
    );
    assert_eq!(checked.hint(0, VariableType).as_deref(), Some("String"));
    assert_eq!(checked.hover_type(1).as_deref(), Some("Integer"));
    assert_eq!(checked.hint(2, VariableType).as_deref(), Some("Integer"));
    assert_eq!(checked.hint(3, VariableType).as_deref(), Some("String"));
    assert_eq!(checked.hint(4, VariableType).as_deref(), Some("Integer"));
    assert_eq!(
        checked.hint(5, VariableType).as_deref(),
        Some("Array[Integer]")
    );
}

#[test]
fn rescue_binds_the_exception_class() {
    let environment = environment_or_skip!();
    let checked = support::check(
        environment,
        r#"
begin
  Integer("1")
rescue ArgumentError, TypeError => error
  error‸
rescue => other
  other‸
end
"#,
    );
    assert_eq!(
        checked.hover_type(0).as_deref(),
        Some("ArgumentError | TypeError")
    );
    assert_eq!(checked.hover_type(1).as_deref(), Some("StandardError"));
}

#[test]
fn steep_type_annotations_override_inference() {
    let environment = environment_or_skip!();
    let checked = support::check(
        environment,
        r#"
# @type var items: Array[String]
items‸ = []

class Holder
  # @type ivar @value: Integer

  def read
    @value‸
  end
end

Greeter.new("a").instance_eval do
  # @type self: Greeter
  self‸.greet("b")
end

numbers‸ = [] #: Array[Integer]
"#,
    );
    assert_eq!(
        checked.hint(0, VariableType).as_deref(),
        Some("Array[String]")
    );
    assert_eq!(checked.hover_type(1).as_deref(), Some("Integer"));
    assert_eq!(checked.hover_type(2).as_deref(), Some("Greeter"));
    assert_eq!(
        checked.hint(3, VariableType).as_deref(),
        Some("Array[Integer]")
    );
}

#[test]
fn hover_on_method_call_describes_the_method() {
    let environment = environment_or_skip!();
    let checked = support::check(
        environment,
        r#"
Greeter.new("a").gr‸eet("b")
"#,
    );
    let entry = checked
        .analysis
        .hover_at(checked.marker(0))
        .expect("hover on greet");
    match &entry.kind {
        type_checker::HoverKind::Method {
            owner,
            method_name,
            overloads,
            return_type,
            comment,
            singleton,
            ..
        } => {
            assert_eq!(owner, "Greeter");
            assert_eq!(method_name, "greet");
            assert!(!singleton);
            assert_eq!(
                overloads
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>(),
                vec!["(String target) -> String"]
            );
            assert_eq!(return_type.to_string(), "String");
            assert_eq!(comment.as_deref().map(str::trim), Some("Greets somebody"));
        }
        other => panic!("unexpected hover {other:?}"),
    }
    let definition = checked
        .analysis
        .definition_at(checked.marker(0))
        .expect("definition of greet");
    assert!(
        definition
            .targets
            .iter()
            .any(|location| location.path.ends_with("sig/fixture.rbs"))
    );
}

#[test]
fn definition_of_a_constant_points_to_its_declaration() {
    let environment = environment_or_skip!();
    let checked = support::check(environment, "Gree‸ter.new('a')\n");
    let definition = checked
        .analysis
        .definition_at(checked.marker(0))
        .expect("definition of Greeter");
    assert!(
        definition
            .targets
            .iter()
            .any(|location| location.path.ends_with("sig/fixture.rbs"))
    );
    let entry = checked
        .analysis
        .hover_at(checked.marker(0))
        .expect("hover on Greeter");
    assert!(
        matches!(&entry.kind, type_checker::HoverKind::Constant { name, .. } if name == "Greeter")
    );
}

#[test]
fn every_method_name_in_a_chain_has_a_hover() {
    let environment = environment_or_skip!();
    let checked = support::check(
        environment,
        r#"
def chain(unknown)
  "abc".upcase‸.chars‸.first‸
  unknown.hoge‸.fuu‸.aaa‸
  Greeter.new("a").maybe_name&.downcase‸&.size‸
  "abc".center‸(10).ljust‸(3, "x")
  [1, 2].map‸ { |number| number.to_s }.join‸(",")
end
"#,
    );
    let expected = [
        "String",
        "Array[String]",
        "String",
        "untyped",
        "untyped",
        "untyped",
        "String",
        "Integer",
        "String",
        "String",
        "Array[String]",
        "String",
    ];
    for (index, expected_type) in expected.iter().enumerate() {
        assert_eq!(
            checked.hover_type(index).as_deref(),
            Some(*expected_type),
            "marker {index}"
        );
    }
}

#[test]
fn variables_reassigned_in_loops_and_blocks_are_widened_on_every_iteration() {
    let environment = environment_or_skip!();
    let checked = support::check(
        environment,
        r#"
last = nil
["a", "b"].each do |item|
  last‸
  last = item
end
count = 0
while count < 10
  count‸
  count = count + 0.5
end
"#,
    );
    assert_eq!(checked.hover_type(0).as_deref(), Some("String?"));
    assert_eq!(checked.hover_type(1).as_deref(), Some("Integer | Float"));
}

#[test]
fn tuple_elements_and_self_class_are_precise() {
    let environment = environment_or_skip!();
    let checked = support::check(
        environment,
        r#"
location = binding.source_location
path‸ = location[0]
line‸ = location.last
klass‸ = "text".class
"#,
    );
    assert_eq!(checked.hint(0, VariableType).as_deref(), Some("String"));
    assert_eq!(checked.hint(1, VariableType).as_deref(), Some("Integer"));
    assert_eq!(
        checked.hint(2, VariableType).as_deref(),
        Some("singleton(String)")
    );
}

#[test]
fn return_types_of_each_union_member_narrow_the_receiver_like_steep() {
    let environment = environment_or_skip!();
    let checked = support::check(
        environment,
        r#"
name = Greeter.new("a").maybe_name
if name.present?
  name‸
else
  name‸
end
if name.blank?
  name‸
else
  name‸
end
unless name.present?
  name‸
end
"#,
    );
    assert_eq!(checked.hover_type(0).as_deref(), Some("String"));
    assert_eq!(checked.hover_type(1).as_deref(), Some("String?"));
    assert_eq!(checked.hover_type(2).as_deref(), Some("String?"));
    assert_eq!(checked.hover_type(3).as_deref(), Some("String"));
    assert_eq!(checked.hover_type(4).as_deref(), Some("String?"));
    assert!(
        checked.diagnostic_codes().is_empty(),
        "{:?}",
        checked.diagnostic_messages()
    );
}

#[test]
fn pure_calls_are_refined_until_their_receiver_is_reassigned() {
    let environment = environment_or_skip!();
    let checked = support::check(
        environment,
        r#"
class Uploader
  def pick
    if small_file.present?
      small_file.file
    end
    if parent
      parent.file
    end
    if unstable_file
      unstable_file.file
    end
  end
end

uploader = Uploader.new
if uploader.small_file
  uploader.small_file.file
  uploader = Uploader.new
  uploader.small_file.file
end
"#,
    );
    let lines: Vec<usize> = checked
        .analysis
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code == "Ruby::NoMethod")
        .map(|diagnostic| checked.source[..diagnostic.start].matches('\n').count() + 1)
        .collect();
    assert_eq!(lines, vec![11, 20], "{:?}", checked.diagnostic_messages());
}

#[test]
fn block_parameters_follow_steep_when_the_block_type_has_fewer_parameters() {
    let environment = environment_or_skip!();
    let checked = support::check(
        environment,
        r#"
loop { |value‸| break }
[[1, "a"]].each { |number‸, text‸| number }
[1].each_with_index { |item‸, index‸, extra‸| item }
"#,
    );
    assert_eq!(checked.hint(0, BlockParameterType).as_deref(), Some("nil"));
    assert_eq!(
        checked.hint(1, BlockParameterType).as_deref(),
        Some("(Integer | String)?")
    );
    assert_eq!(
        checked.hint(2, BlockParameterType).as_deref(),
        Some("(Integer | String)?")
    );
    assert_eq!(
        checked.hint(3, BlockParameterType).as_deref(),
        Some("Integer")
    );
    assert_eq!(
        checked.hint(4, BlockParameterType).as_deref(),
        Some("Integer")
    );
    assert_eq!(checked.hint(5, BlockParameterType).as_deref(), Some("nil"));
}

#[test]
fn records_tuples_compact_and_inherited_constants_follow_steep() {
    let environment = environment_or_skip!();
    let checked = support::check(
        environment,
        r#"
record = Greeter.new("a").maybe_name ? { name: "a", count: 1 } : { name: "b", count: 2 }
ids‸ = [Greeter.new("a").maybe_name].compact
names‸ = { a: Greeter.new("a").maybe_name }.compact
"#,
    );
    assert_eq!(
        checked.hint(0, VariableType).as_deref(),
        Some("Array[String]")
    );
    assert_eq!(
        checked.hint(1, VariableType).as_deref(),
        Some("Hash[Symbol, String]")
    );
}

#[test]
fn undeclared_instance_variables_keep_their_inferred_type_and_are_reported_like_steep() {
    let environment = environment_or_skip!();
    let checked = support::check(
        environment,
        r#"
class Greeter
  def initialize(name)
    @undeclared = Greeter.new("a").maybe_name
  end

  def use_it
    @undeclared‸
  end
end
"#,
    );
    assert_eq!(checked.hover_type(0).as_deref(), Some("String?"));
    assert_eq!(
        checked.messages_with_code("Ruby::UnknownInstanceVariable"),
        vec!["Cannot find the declaration of instance variable: `@undeclared`"]
    );
    assert_eq!(
        checked.messages_with_code("Ruby::FallbackAny"),
        vec!["Cannot detect the type of the expression"]
    );
}

#[test]
fn method_parameters_show_their_declared_types() {
    let Some(environment) = support::environment_with(
        r#"
class Mailer
  def deliver: (String address, ?Integer retries, *Symbol tags, subject: String, ?priority: Integer, **bool flags) { (String) -> void } -> void
  def schedule: (?Integer? delay, ?queue: String?) -> void
end
"#,
    ) else {
        eprintln!("skipped: RBS core signatures are not found");
        return;
    };
    let checked = support::check(
        &environment,
        r#"
class Mailer
  def deliver(address‸, retries‸ = 3, *tags‸, subject:‸, priority:‸ 1, **flags‸, &block‸)
  end

  def schedule(delay‸ = nil, queue:‸ "mail")
  end

  def undeclared(value‸)
  end
end
"#,
    );
    let expected = [
        (0, MethodParameterType, Some("String")),
        (1, MethodParameterType, Some("Integer")),
        (2, MethodParameterType, Some("Array[Symbol]")),
        (3, KeywordParameterType, Some("String")),
        (4, KeywordParameterType, Some("Integer")),
        (5, MethodParameterType, Some("Hash[Symbol, bool]")),
        (6, MethodParameterType, Some("^(String) -> void")),
        (7, MethodParameterType, Some("Integer?")),
        (8, KeywordParameterType, Some("String?")),
        (9, MethodParameterType, Some("untyped")),
    ];
    for (marker_index, kind, expected_type) in expected {
        assert_eq!(
            checked.hint(marker_index, kind).as_deref(),
            expected_type,
            "marker {marker_index}"
        );
    }
    let obvious: Vec<usize> = (0..10)
        .filter(|marker_index| checked.is_obvious_hint(*marker_index))
        .collect();
    assert_eq!(obvious, vec![1, 4]);
}
