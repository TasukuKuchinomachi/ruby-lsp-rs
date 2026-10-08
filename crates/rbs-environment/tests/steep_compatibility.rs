use std::path::Path;

use rbs_environment::{DefinitionError, Environment, Loader, MethodKind};

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
end

class Integer
end

class Array[unchecked out Elem]
end

class Time
  def to_s: () -> String
  def year: () -> Integer
end

class Date
  def year: () -> Integer
end
"#;

const LIBRARY: &str = r#"
class Time
  def to_s: () -> String
end

module Worker
  module ClassMethods
    def options: () -> void
    def options: () -> void
  end
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

/// `(code, 1-based line, 1-based column)`
fn steep_compatible_summaries(environment: &Environment) -> Vec<(&'static str, u32, u32)> {
    environment
        .steep_compatible_signature_diagnostics()
        .into_iter()
        .map(|diagnostic| {
            (
                diagnostic.code,
                diagnostic.start.line + 1,
                diagnostic.start.utf16_column + 1,
            )
        })
        .collect()
}

fn duplicated_method_name(error: Option<&DefinitionError>) -> Option<String> {
    match error? {
        DefinitionError::DuplicatedMethodDefinition(duplicated) => Some(format!(
            "{}#{}",
            duplicated.type_name, duplicated.method_name
        )),
        DefinitionError::NoTypeFound { .. } => None,
    }
}

#[test]
fn records_duplicated_methods_of_libraries_in_load_order() {
    let environment = load("");
    let duplicated = &environment.duplicated_method_definitions;
    assert_eq!(duplicated.len(), 2);
    let time = duplicated
        .iter()
        .find(|duplicated| duplicated.type_name == "Time")
        .unwrap();
    assert_eq!(time.kind, MethodKind::Instance);
    assert_eq!(time.method_name, "to_s");
    let paths: Vec<&Path> = time
        .locations
        .iter()
        .map(|location| location.path.as_path())
        .collect();
    assert_eq!(paths, vec![Path::new("core.rbs"), Path::new("library.rbs")]);
}

#[test]
fn a_duplicated_instance_method_breaks_both_definitions_of_the_class() {
    let environment = load("");
    assert_eq!(
        duplicated_method_name(environment.instance_definition_error("Time")),
        Some("Time#to_s".to_string())
    );
    assert_eq!(
        duplicated_method_name(environment.singleton_definition_error("Time")),
        Some("Time#to_s".to_string())
    );
    assert_eq!(environment.instance_definition_error("Date"), None);
    assert_eq!(environment.singleton_definition_error("Date"), None);
    assert_eq!(environment.instance_definition_error("Missing"), None);
}

#[test]
fn a_broken_definition_spreads_to_subclasses_and_through_mixins() {
    let environment = load(
        r#"
class LocalTime < Time
end

class Job
  extend Worker::ClassMethods
end

class Mixed
  include Worker::ClassMethods
end
"#,
    );
    assert_eq!(
        duplicated_method_name(environment.instance_definition_error("LocalTime")),
        Some("Time#to_s".to_string())
    );
    assert_eq!(environment.instance_definition_error("Job"), None);
    assert_eq!(
        duplicated_method_name(environment.singleton_definition_error("Job")),
        Some("Worker::ClassMethods#options".to_string())
    );
    assert_eq!(
        duplicated_method_name(environment.instance_definition_error("Mixed")),
        Some("Worker::ClassMethods#options".to_string())
    );
}

#[test]
fn an_unknown_type_in_an_instance_method_other_than_initialize_breaks_the_definition() {
    let environment = load(
        r#"
class Searcher
  def initialize: (Missing records) -> void
  def self.parse: (Missing value) -> void
end

class Finder
  def initialize: (Missing records) -> void
  def search: () -> Missing::Relation
end
"#,
    );
    assert_eq!(environment.instance_definition_error("Searcher"), None);
    assert_eq!(environment.singleton_definition_error("Searcher"), None);
    let error = environment.instance_definition_error("Finder").unwrap();
    let DefinitionError::NoTypeFound {
        type_name,
        location,
    } = error
    else {
        panic!("expected NoTypeFound: {error:?}");
    };
    assert_eq!(type_name, "Missing::Relation");
    assert_eq!(location.as_ref().unwrap().start.line + 1, 9);
    assert!(
        error
            .message()
            .ends_with(": Could not find Missing::Relation(RBS::NoTypeFoundError)")
    );
}

#[test]
fn the_message_follows_rbs() {
    let environment = load("");
    let message = environment
        .instance_definition_error("Time")
        .unwrap()
        .message();
    assert_eq!(
        message,
        "core.rbs:29:2...29:24: ::Time#to_s has duplicated definitions in library.rbs:3:2...3:24(RBS::DuplicatedMethodDefinitionError)"
    );
}

#[test]
fn steep_compatible_signature_diagnostics_keep_the_first_error_of_each_declaration() {
    let environment = load(
        r#"
class Searcher
  @records: Missing

  def initialize: (Missing records) -> void

  RECORDS: Array[Missing | Missing]

  def search: () -> (Missing | Missing)

  attr_reader records: Missing
end

class Other
  def self.build: () -> Missing
  def find: () -> Missing
end
"#,
    );
    assert_eq!(environment.signature_diagnostics.len(), 9);
    assert_eq!(
        steep_compatible_summaries(&environment),
        vec![
            ("RBS::UnknownTypeName", 7, 18),
            ("RBS::UnknownTypeName", 9, 22),
            ("RBS::UnknownTypeName", 16, 19),
        ]
    );
}

#[test]
fn steep_compatible_signature_diagnostics_report_a_duplicated_method_at_its_first_definition() {
    let environment = load(
        r#"
class Time
  def to_s: () -> String
end

class Report
  def title: () -> String
  def title: () -> String
end
"#,
    );
    let duplicated: Vec<(&str, u32, u32)> = environment
        .signature_diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code == "RBS::DuplicatedMethodDefinition")
        .map(|diagnostic| {
            (
                diagnostic.code,
                diagnostic.start.line + 1,
                diagnostic.start.utf16_column + 1,
            )
        })
        .collect();
    assert_eq!(
        duplicated,
        vec![
            ("RBS::DuplicatedMethodDefinition", 3, 3),
            ("RBS::DuplicatedMethodDefinition", 7, 3),
            ("RBS::DuplicatedMethodDefinition", 8, 3),
        ]
    );
    assert_eq!(
        steep_compatible_summaries(&environment),
        vec![("RBS::DuplicatedMethodDefinition", 7, 3)]
    );
}
