use std::path::{Path, PathBuf};

use rbs_environment::{Environment, Loader, SignatureDiagnostic, signature_syntax_error};

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

class String
  def upcase: () -> String
end

class Integer
end

class Symbol
end

module Comparable
end

module Enumerable[unchecked out Elem]
end

class Array[unchecked out Elem]
  include Enumerable[Elem]
end

class Hash[unchecked out K, unchecked out V = untyped]
end

interface _Each[T]
  def each: () { (T) -> void } -> void
end

type json = String | Integer
"#;

const LIBRARY: &str = r#"
class Library
  def run: () -> void
end
"#;

const SIGNATURE_PATH: &str = "sig/app.rbs";

fn load(signature: &str) -> Environment {
    let mut loader = Loader::new();
    loader.add_source(Path::new("core.rbs"), MINIMAL_CORE);
    loader.add_source(Path::new("library.rbs"), LIBRARY);
    loader.add_signature_source(Path::new(SIGNATURE_PATH), signature);
    loader.finish()
}

/// `(code, 1-based line, 1-based column, message)`
fn summaries(environment: &Environment) -> Vec<(&'static str, u32, u32, String)> {
    environment
        .signature_diagnostics
        .iter()
        .map(|diagnostic| {
            (
                diagnostic.code,
                diagnostic.start.line + 1,
                diagnostic.start.utf16_column + 1,
                diagnostic.message.clone(),
            )
        })
        .collect()
}

#[test]
fn accepts_valid_signatures() {
    let environment = load(
        r#"
module App
  class User
    include Comparable

    attr_reader name: String
    @tags: Array[String]

    def initialize: (String name) -> void
    def each_tag: () { (String) -> void } -> void
    def options: () -> Hash[Symbol]
    def to_json: () -> json
    def each: [T] (_Each[T]) -> void
    def self.build: () -> instance
    def bounded: [A < Array[B], B] (A) -> B
    alias full_name name
  end

  class Admin < User
    def name: () -> String | ...
  end

  VERSION: String
end

class Library
  def stop: () -> void
end
"#,
    );
    assert_eq!(summaries(&environment), Vec::new());
}

#[test]
fn reports_unknown_type_names_at_each_reference() {
    let environment = load(
        r#"
class Searcher
  def initialize: (Missing::Relation relation) -> void
  def search: () -> (Missing::Relation | String)
  def each: () -> _Missing
  def alias_type: () -> missing_alias
  def singleton_type: () -> singleton(Missing)
  RELATIONS: Array[::Missing::Relation]
end
"#,
    );
    assert_eq!(
        summaries(&environment),
        vec![
            (
                "RBS::UnknownTypeName",
                3,
                20,
                "Cannot find type `Missing::Relation`".to_string()
            ),
            (
                "RBS::UnknownTypeName",
                4,
                22,
                "Cannot find type `Missing::Relation`".to_string()
            ),
            (
                "RBS::UnknownTypeName",
                5,
                19,
                "Cannot find type `_Missing`".to_string()
            ),
            (
                "RBS::UnknownTypeName",
                6,
                25,
                "Cannot find type `missing_alias`".to_string()
            ),
            (
                "RBS::UnknownTypeName",
                7,
                29,
                "Cannot find type `Missing`".to_string()
            ),
            (
                "RBS::UnknownTypeName",
                8,
                20,
                "Cannot find type `::Missing::Relation`".to_string()
            ),
        ]
    );
    let diagnostic = &environment.signature_diagnostics[0];
    assert_eq!(diagnostic.path, PathBuf::from(SIGNATURE_PATH));
    assert_eq!((diagnostic.end.line, diagnostic.end.utf16_column), (2, 36));
}

#[test]
fn resolves_names_through_nesting_use_directives_and_class_aliases() {
    let environment = load(
        r#"
use App::Models::*

module App
  module Models
    class User
    end
  end

  class Account = Models::User

  class Service
    def user: () -> Models::User
    def account: () -> Account
  end
end

class Controller
  def user: () -> User
end
"#,
    );
    assert_eq!(summaries(&environment), Vec::new());
}

#[test]
fn reports_unknown_superclasses_mixins_and_namespaces() {
    let environment = load(
        r#"
class Child < MissingParent
  include MissingModule
  extend _MissingInterface
end

class Missing::Nested
end

module Concern : MissingSelf
end

class Alias = MissingTarget
"#,
    );
    assert_eq!(
        summaries(&environment),
        vec![
            (
                "RBS::UnknownTypeName",
                2,
                1,
                "Cannot find type `MissingParent`".to_string()
            ),
            (
                "RBS::UnknownTypeName",
                3,
                3,
                "Cannot find type `MissingModule`".to_string()
            ),
            (
                "RBS::UnknownTypeName",
                4,
                3,
                "Cannot find type `_MissingInterface`".to_string()
            ),
            (
                "RBS::UnknownTypeName",
                7,
                1,
                "Cannot find type `::Missing`".to_string()
            ),
            (
                "RBS::UnknownTypeName",
                10,
                18,
                "Cannot find type `MissingSelf`".to_string()
            ),
            (
                "RBS::UnknownTypeName",
                13,
                15,
                "Cannot find type `MissingTarget`".to_string()
            ),
        ]
    );
}

#[test]
fn reports_invalid_type_applications() {
    let environment = load(
        r#"
class Box
  def items: () -> Array
  def pair: () -> Array[String, Integer]
  def name: () -> String[Integer]
  def table: () -> Hash[Symbol, String, Integer]
  def each: () -> _Each
end

class Strings < Array
end
"#,
    );
    assert_eq!(
        summaries(&environment),
        vec![
            (
                "RBS::InvalidTypeApplication",
                3,
                20,
                "Type `::Array` is generic but used as a non generic type".to_string()
            ),
            (
                "RBS::InvalidTypeApplication",
                4,
                19,
                "Type `::Array` expects 1 arguments, but 2 arguments are given".to_string()
            ),
            (
                "RBS::InvalidTypeApplication",
                5,
                19,
                "Type `::String` is not generic but used as a generic type with 1 arguments"
                    .to_string()
            ),
            (
                "RBS::InvalidTypeApplication",
                6,
                20,
                "Type `::Hash` expects 2 arguments, but 3 arguments are given".to_string()
            ),
            (
                "RBS::InvalidTypeApplication",
                7,
                19,
                "Type `::_Each` is generic but used as a non generic type".to_string()
            ),
            (
                "RBS::InvalidTypeApplication",
                10,
                17,
                "Type `::Array` is generic but used as a non generic type".to_string()
            ),
        ]
    );
}

#[test]
fn reports_duplicated_methods_in_signature_files_only() {
    let environment = load(
        r#"
class Library
  def run: () -> Integer
end

class Report
  def title: () -> String
  attr_reader title: String
  def self.build: () -> Report
  def self.build: () -> Report
  alias title2 title
  def title2: () -> String
end

interface _Named
  def name: () -> String
  def name: () -> String
end
"#,
    );
    let message = |method_name: &str, owner: &str| {
        format!(
            "Non-overloading method definition of `{method_name}` in `{owner}` cannot be duplicated"
        )
    };
    assert_eq!(
        summaries(&environment),
        vec![
            (
                "RBS::DuplicatedMethodDefinition",
                3,
                3,
                message("run", "::Library")
            ),
            (
                "RBS::DuplicatedMethodDefinition",
                7,
                3,
                message("title", "::Report")
            ),
            (
                "RBS::DuplicatedMethodDefinition",
                8,
                3,
                message("title", "::Report")
            ),
            (
                "RBS::DuplicatedMethodDefinition",
                9,
                3,
                message("build", "::Report")
            ),
            (
                "RBS::DuplicatedMethodDefinition",
                10,
                3,
                message("build", "::Report")
            ),
            (
                "RBS::DuplicatedMethodDefinition",
                11,
                3,
                message("title2", "::Report")
            ),
            (
                "RBS::DuplicatedMethodDefinition",
                12,
                3,
                message("title2", "::Report")
            ),
            (
                "RBS::DuplicatedMethodDefinition",
                16,
                3,
                message("name", "::_Named")
            ),
            (
                "RBS::DuplicatedMethodDefinition",
                17,
                3,
                message("name", "::_Named")
            ),
        ]
    );
    let library_run = environment
        .find_instance_method("Library", "run")
        .expect("Library#run is not found");
    assert_eq!(library_run.overloads[0].to_string(), "() -> Integer");
}

#[test]
fn reports_duplicated_declarations() {
    let environment = load(
        r#"
module Library
end

interface _Each[T]
end

type json = String

MAXIMUM: Integer
MAXIMUM: Integer

$stdout_override: String
$stdout_override: String

class Integer = String
"#,
    );
    assert_eq!(
        summaries(&environment),
        vec![
            (
                "RBS::DuplicatedDeclaration",
                2,
                1,
                "Declaration of `::Library` is duplicated".to_string()
            ),
            (
                "RBS::DuplicatedDeclaration",
                5,
                1,
                "Declaration of `::_Each` is duplicated".to_string()
            ),
            (
                "RBS::DuplicatedDeclaration",
                8,
                1,
                "Declaration of `::json` is duplicated".to_string()
            ),
            (
                "RBS::DuplicatedDeclaration",
                11,
                1,
                "Declaration of `::MAXIMUM` is duplicated".to_string()
            ),
            (
                "RBS::DuplicatedDeclaration",
                14,
                1,
                "Declaration of `$stdout_override` is duplicated".to_string()
            ),
            (
                "RBS::DuplicatedDeclaration",
                16,
                1,
                "Declaration of `::Integer` is duplicated".to_string()
            ),
        ]
    );
}

#[test]
fn reports_mismatched_reopenings_and_misused_ancestors() {
    let environment = load(
        r#"
class Array
end

class Parent
end

class Child < Parent
end

class Child < String
end

class FromModule < Comparable
end

class Mixing
  include String
end
"#,
    );
    assert_eq!(
        summaries(&environment),
        vec![
            (
                "RBS::GenericParameterMismatch",
                2,
                1,
                "Different generic parameters are specified across definitions of `::Array`"
                    .to_string()
            ),
            (
                "RBS::SuperclassMismatch",
                8,
                1,
                "Different superclasses are specified for `::Child`".to_string()
            ),
            (
                "RBS::SuperclassMismatch",
                11,
                1,
                "Different superclasses are specified for `::Child`".to_string()
            ),
            (
                "RBS::InheritModuleError",
                14,
                20,
                "Cannot inherit from a module `::Comparable`".to_string()
            ),
            (
                "RBS::MixinClassError",
                18,
                3,
                "Cannot include a class `::String` in the definition of `::Mixing`".to_string()
            ),
        ]
    );
}

#[test]
fn reports_invalid_overloads_and_unknown_method_aliases() {
    let environment = load(
        r#"
class Library
  def run: (Integer) -> void | ...
end

class Report
  def title: () -> String | ...
  alias heading missing_title
  alias self.create self.missing_build
end

interface _Titled
  alias heading title
end
"#,
    );
    assert_eq!(
        summaries(&environment),
        vec![
            (
                "RBS::InvalidMethodOverload",
                7,
                3,
                "Cannot find a non-overloading definition of `title` in `::Report`".to_string()
            ),
            (
                "RBS::UnknownMethodAlias",
                8,
                3,
                "Cannot find the original method `missing_title` in `::Report`".to_string()
            ),
            (
                "RBS::UnknownMethodAlias",
                9,
                3,
                "Cannot find the original method `missing_build` in `::Report`".to_string()
            ),
            (
                "RBS::UnknownMethodAlias",
                13,
                3,
                "Cannot find the original method `title` in `::_Titled`".to_string()
            ),
        ]
    );
}

#[test]
fn reports_syntax_errors_at_the_token() {
    let environment = load("class Broken\n  def run: () -> \nend\n");
    let [diagnostic] = environment.signature_diagnostics.as_slice() else {
        panic!("{:?}", environment.signature_diagnostics);
    };
    assert_eq!(diagnostic.code, "RBS::SyntaxError");
    assert_eq!(
        (diagnostic.start.line, diagnostic.start.utf16_column),
        (2, 0)
    );
    assert_eq!((diagnostic.end.line, diagnostic.end.utf16_column), (2, 3));
    assert!(
        diagnostic.message.starts_with("Syntax error: "),
        "{}",
        diagnostic.message
    );
    assert!(
        diagnostic.message.ends_with("token=`end`"),
        "{}",
        diagnostic.message
    );
    assert!(environment.classes.contains_key("Object"));
}

#[test]
fn finds_syntax_errors_in_unsaved_sources() {
    let path = Path::new(SIGNATURE_PATH);
    assert_eq!(signature_syntax_error(path, "class Valid\nend\n"), None);
    let Some(SignatureDiagnostic { code, start, .. }) =
        signature_syntax_error(path, "class 1\nend\n")
    else {
        panic!("No syntax error");
    };
    assert_eq!(code, "RBS::SyntaxError");
    assert_eq!((start.line, start.utf16_column), (0, 6));
}

#[test]
fn leaves_library_files_unvalidated() {
    let mut loader = Loader::new();
    loader.add_source(Path::new("core.rbs"), MINIMAL_CORE);
    loader.add_source(
        Path::new("broken_library.rbs"),
        "class Broken < Missing\n  def run: () -> Unknown\nend\n",
    );
    let environment = loader.finish();
    assert_eq!(environment.signature_diagnostics, Vec::new());
}
