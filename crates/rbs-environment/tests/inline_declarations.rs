use std::path::Path;

use rbs_environment::{Environment, Loader, Type};

const MINIMAL_CORE: &str = r#"
class BasicObject
  def initialize: () -> void
end

module Kernel : BasicObject
end

class Object < BasicObject
  include Kernel
end

class Module
end

class Class < Module
end

module Comparable
end

class String
end

class Integer
end

class Float
end

class Symbol
end

class Array[unchecked out Elem]
end
"#;

fn load_inline(source: &str) -> Environment {
    let mut loader = Loader::new();
    loader.add_source(Path::new("core.rbs"), MINIMAL_CORE);
    loader.add_inline_source(Path::new("lib/app.rb"), source);
    let environment = loader.finish();
    assert_eq!(environment.errors, Vec::new());
    environment
}

fn overload_strings(environment: &Environment, class_name: &str, method_name: &str) -> Vec<String> {
    let method = environment
        .find_instance_method(class_name, method_name)
        .unwrap_or_else(|| panic!("{class_name}#{method_name} is not found"));
    method.overloads.iter().map(ToString::to_string).collect()
}

fn singleton_overload_strings(
    environment: &Environment,
    class_name: &str,
    method_name: &str,
) -> Vec<String> {
    let method = environment
        .find_singleton_method(class_name, method_name)
        .unwrap_or_else(|| panic!("{class_name}.{method_name} is not found"));
    method.overloads.iter().map(ToString::to_string).collect()
}

/// `(line, column, code, message)` of every signature diagnostic, 1-based like an editor shows them
fn diagnostics(environment: &Environment) -> Vec<(u32, u32, &str, &str)> {
    environment
        .signature_diagnostics
        .iter()
        .map(|diagnostic| {
            (
                diagnostic.start.line + 1,
                diagnostic.start.byte_column + 1,
                diagnostic.code,
                diagnostic.message.as_str(),
            )
        })
        .collect()
}

#[test]
fn declares_methods_from_each_annotation_style() {
    let environment = load_inline(
        r#"
class Calculator
  # @rbs (Integer, Integer) -> Integer
  def add(x, y) = x + y

  #: (Integer) -> Integer
  #: (Float) -> Float
  def negate(x) = -x

  # @rbs x: Integer -- the operand
  # @rbs return: String
  def describe(x, y = 1, *rest, label:, verbose: false, **options, &block) = ""

  def multiply(x, y) #: Integer
    x * y
  end

  def unannotated(a) = a

  # @rbs () -> Calculator
  def self.build = new

  # @rbs &block: ? (Integer) -> void
  def each(&block) = nil
end
"#,
    );
    assert_eq!(environment.signature_diagnostics, Vec::new());
    assert_eq!(
        overload_strings(&environment, "Calculator", "add"),
        vec!["(Integer, Integer) -> Integer"]
    );
    assert_eq!(
        overload_strings(&environment, "Calculator", "negate"),
        vec!["(Integer) -> Integer", "(Float) -> Float"]
    );
    assert_eq!(
        overload_strings(&environment, "Calculator", "describe"),
        vec![
            "(Integer x, ?untyped y, *untyped rest, label: untyped, ?verbose: untyped, **untyped options) ?{ (?) -> untyped } -> String"
        ]
    );
    assert_eq!(
        overload_strings(&environment, "Calculator", "multiply"),
        vec!["(untyped x, untyped y) -> Integer"]
    );
    assert_eq!(
        overload_strings(&environment, "Calculator", "unannotated"),
        vec!["(?) -> untyped"]
    );
    assert_eq!(
        singleton_overload_strings(&environment, "Calculator", "build"),
        vec!["() -> Calculator"]
    );
    assert_eq!(
        overload_strings(&environment, "Calculator", "each"),
        vec!["() ?{ (Integer) -> void } -> untyped"]
    );
}

#[test]
fn declares_attributes_instance_variables_constants_and_aliases() {
    let environment = load_inline(
        r#"
module Shop
  VERSION = "1.0"
  LIMIT = 10
  ITEMS = [1, "a"] #: [Integer, String]
  OTHER = compute

  class Item
    # @rbs @price: Integer

    attr_reader :name #: String
    attr_accessor :count, :total #: Integer
    attr_writer :label
  end

  Product = Item #: class-alias
  Mixin = Comparable #: module-alias
end
"#,
    );
    assert_eq!(environment.signature_diagnostics, Vec::new());
    let constant_type = |name: &str| environment.constants[name].constant_type.to_string();
    assert_eq!(constant_type("Shop::VERSION"), "String");
    assert_eq!(constant_type("Shop::LIMIT"), "Integer");
    assert_eq!(constant_type("Shop::ITEMS"), "[Integer, String]");
    assert_eq!(constant_type("Shop::OTHER"), "untyped");
    let item = &environment.classes["Shop::Item"];
    assert_eq!(item.instance_variables["@price"].to_string(), "Integer");
    assert_eq!(item.instance_variables["@name"].to_string(), "String");
    assert_eq!(
        overload_strings(&environment, "Shop::Item", "name"),
        vec!["() -> String"]
    );
    assert_eq!(
        overload_strings(&environment, "Shop::Item", "total="),
        vec!["(Integer total) -> Integer"]
    );
    assert_eq!(
        overload_strings(&environment, "Shop::Item", "label="),
        vec!["(untyped label) -> untyped"]
    );
    assert_eq!(environment.class_aliases["Shop::Product"], "Shop::Item");
    assert_eq!(environment.class_aliases["Shop::Mixin"], "Comparable");
}

#[test]
fn declares_names_rbs_reads_only_in_backquotes() {
    let environment = load_inline(
        r#"
class Item
  attr_reader :"two words" #: String
  attr_reader :type #: Integer
  attr_reader :ready? #: Integer

  #: (Integer) -> Integer
  def +(other) = other

  #: () -> Integer
  def count = 1
end
"#,
    );
    assert_eq!(environment.signature_diagnostics, Vec::new());
    assert_eq!(overload_strings(&environment, "Item", "two words"), vec!["() -> String"]);
    assert_eq!(overload_strings(&environment, "Item", "type"), vec!["() -> Integer"]);
    assert_eq!(overload_strings(&environment, "Item", "ready?"), vec!["() -> Integer"]);
    assert_eq!(overload_strings(&environment, "Item", "+"), vec!["(Integer) -> Integer"]);
    assert_eq!(overload_strings(&environment, "Item", "count"), vec!["() -> Integer"]);
}

#[test]
fn an_unannotated_method_takes_the_type_of_the_method_it_overrides() {
    let environment = load_inline(
        r#"
class Base
  # @rbs (Integer) -> Integer
  def compute(x) = x

  # @rbs () -> String
  def self.label = "base"
end

class Child < Base
  def compute(x) = x

  # @rbs ...
  def self.label = "child"
end

class GrandChild < Child
  def compute(x) = x
end

class Overloading < Base
  # @rbs (String) -> String | ...
  def compute(x) = x
end
"#,
    );
    assert_eq!(environment.signature_diagnostics, Vec::new());
    assert_eq!(
        overload_strings(&environment, "Child", "compute"),
        vec!["(Integer) -> Integer"]
    );
    assert_eq!(
        overload_strings(&environment, "GrandChild", "compute"),
        vec!["(Integer) -> Integer"]
    );
    assert_eq!(
        singleton_overload_strings(&environment, "Child", "label"),
        vec!["() -> String"]
    );
    assert_eq!(
        overload_strings(&environment, "Overloading", "compute"),
        vec!["(String) -> String", "(Integer) -> Integer"]
    );
}

#[test]
fn module_self_annotations_are_the_self_types_of_the_module() {
    let environment = load_inline(
        r#"
# @rbs module-self: Comparable -- needs comparison
# @rbs module-self: Array[String]
module Sortable
  include Comparable
end
"#,
    );
    assert_eq!(environment.signature_diagnostics, Vec::new());
    let sortable = &environment.classes["Sortable"];
    let self_types: Vec<String> = sortable
        .module_self_types
        .iter()
        .map(|ancestor| Type::generic(&ancestor.name, ancestor.arguments.clone()).to_string())
        .collect();
    assert_eq!(self_types, vec!["Comparable", "Array[String]"]);
    assert_eq!(sortable.includes[0].name, "Comparable");
}

#[test]
fn reports_what_the_inline_parser_reports() {
    let environment = load_inline(
        r#"# @rbs (Integer) -> String
def top_level(x) = x

attr_reader :top_level_attribute

object = Object
class object::Dynamic
end

class Errors
  include Comparable, Kernel

  # @rbs (Integer) -> Integer -> String
  def syntax_error(x) = x

  # @rbs @inside: Integer
  def unused_instance_variable = 1

  # @rbs unknown: Integer
  def unused_parameter(x) = x

  attr_reader "string_name"

  Missing = something #: class-alias
end
"#,
    );
    assert_eq!(
        diagnostics(&environment),
        vec![
            (
                1,
                3,
                "RBS::InlineDiagnostic",
                "Unused inline rbs annotation"
            ),
            (
                2,
                5,
                "RBS::InlineDiagnostic",
                "Top-level method definition is not supported"
            ),
            (
                4,
                1,
                "RBS::InlineDiagnostic",
                "Top-level attribute definition is not supported"
            ),
            (
                7,
                7,
                "RBS::InlineDiagnostic",
                "Class name must be a constant"
            ),
            (
                11,
                3,
                "RBS::InlineDiagnostic",
                "Mixing multiple modules with one call is not supported"
            ),
            (
                13,
                5,
                "RBS::InlineDiagnostic",
                "Syntax error: expected a token `pEOF`"
            ),
            (
                16,
                5,
                "RBS::InlineDiagnostic",
                "Unused inline rbs annotation"
            ),
            (
                19,
                5,
                "RBS::InlineDiagnostic",
                "Unused inline rbs annotation"
            ),
            (
                22,
                15,
                "RBS::InlineDiagnostic",
                "Attribute name must be a symbol"
            ),
            (
                24,
                24,
                "RBS::InlineDiagnostic",
                "Class name is missing in class alias declaration"
            ),
        ]
    );
}

#[test]
fn locations_point_into_the_ruby_file() {
    let environment = load_inline(
        "class Greeter\n  # @rbs (String) -> Unknown\n  def greet(name)\n    name\n  end\nend\n",
    );
    assert_eq!(
        diagnostics(&environment),
        vec![(
            2,
            22,
            "RBS::UnknownTypeName",
            "Cannot find type `::Unknown`"
        )]
    );
    let greet = &environment.classes["Greeter"].instance_methods["greet"];
    let location = greet.location.as_ref().unwrap();
    assert_eq!(location.path.as_path(), Path::new("lib/app.rb"));
    assert_eq!(
        (
            location.start.line,
            location.start.byte_column,
            location.end.line,
            location.end.byte_column
        ),
        (2, 2, 4, 5)
    );
}
