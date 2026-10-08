use std::path::Path;

use rbs_environment::{
    Ancestor, ClassKind, Environment, Loader, MethodKind, SingletonAncestor, Type, Visibility,
    parse_type,
};

const MINIMAL_CORE: &str = r#"
class BasicObject
  def initialize: () -> void
  def !: () -> bool
end

module Kernel : BasicObject
  def puts: (*untyped objects) -> nil
end

class Object < BasicObject
  include Kernel
end

class Module
  def name: () -> String?
end

class Class < Module
  def allocate: () -> untyped
end

class String
  def initialize: (?String source) -> void
end

class Integer
  def +: (Integer other) -> Integer
end

class Symbol
end

module Enumerable[unchecked out Elem]
  def map: [U] () { (Elem item) -> U } -> Array[U]
  def first: () -> Elem?
end

class Array[unchecked out Elem]
  include Enumerable[Elem]

  def initialize: (?Integer size, ?Elem default) -> void
  def each: () { (Elem item) -> void } -> self
end

class Hash[unchecked out K, unchecked out V]
  include Enumerable[[K, V]]
end
"#;

fn load(sources: &[(&str, &str)]) -> Environment {
    let mut loader = Loader::new();
    loader.add_source(Path::new("core.rbs"), MINIMAL_CORE);
    for (path, source) in sources {
        loader.add_source(Path::new(path), source);
    }
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

fn ancestor_strings(ancestors: &[Ancestor]) -> Vec<String> {
    ancestors
        .iter()
        .map(|ancestor| Type::generic(&ancestor.name, ancestor.arguments.clone()).to_string())
        .collect()
}

#[test]
fn resolves_relative_names_from_innermost_nesting_outwards() {
    let environment = load(&[(
        "app.rbs",
        r#"
module App
  class Error
  end

  module Models
    class Error
    end

    class User
      def last_error: () -> Error
      def app_error: () -> App::Error
      def top_level_string: () -> ::String
      def string: () -> String
    end
  end
end
"#,
    )]);
    assert_eq!(
        overload_strings(&environment, "App::Models::User", "last_error"),
        vec!["() -> App::Models::Error"]
    );
    assert_eq!(
        overload_strings(&environment, "App::Models::User", "app_error"),
        vec!["() -> App::Error"]
    );
    assert_eq!(
        overload_strings(&environment, "App::Models::User", "top_level_string"),
        vec!["() -> String"]
    );
    assert_eq!(
        overload_strings(&environment, "App::Models::User", "string"),
        vec!["() -> String"]
    );
}

#[test]
fn resolves_interfaces_and_type_aliases_in_nesting() {
    let environment = load(&[(
        "app.rbs",
        r#"
module App
  type id = Integer | String

  interface _Named
    def name: () -> String
  end

  class User
    def id: () -> id
    def named: () -> _Named
  end
end
"#,
    )]);
    assert_eq!(
        overload_strings(&environment, "App::User", "id"),
        vec!["() -> App::id"]
    );
    assert_eq!(
        overload_strings(&environment, "App::User", "named"),
        vec!["() -> App::_Named"]
    );
    assert_eq!(
        environment.expand_alias("App::id", &[]),
        Some(parse_type("Integer | String").unwrap())
    );
}

#[test]
fn resolves_names_through_use_directives() {
    let environment = load(&[
        (
            "library.rbs",
            "module Library\n  class Client\n  end\n  class Server\n  end\nend\n",
        ),
        (
            "app.rbs",
            "use Library::Client, Library::Server as Backend\n\nclass App\n  def client: () -> Client\n  def backend: () -> Backend\nend\n",
        ),
    ]);
    assert_eq!(
        overload_strings(&environment, "App", "client"),
        vec!["() -> Library::Client"]
    );
    assert_eq!(
        overload_strings(&environment, "App", "backend"),
        vec!["() -> Library::Server"]
    );
}

#[test]
fn merges_reopened_classes_across_files() {
    let environment = load(&[
        (
            "first.rbs",
            "class Greeter\n  def hello: () -> String\nend\n",
        ),
        (
            "second.rbs",
            "class Greeter\n  def goodbye: () -> String\n  def hello: () -> Symbol\nend\n",
        ),
    ]);
    let greeter = &environment.classes["Greeter"];
    assert_eq!(greeter.locations.len(), 2);
    assert_eq!(
        greeter.super_class,
        Some(Ancestor {
            name: "Object".to_string(),
            arguments: Vec::new()
        })
    );
    assert_eq!(
        overload_strings(&environment, "Greeter", "goodbye"),
        vec!["() -> String"]
    );
    assert_eq!(
        overload_strings(&environment, "Greeter", "hello"),
        vec!["() -> Symbol"]
    );
}

#[test]
fn overloading_definition_prepends_to_existing_overloads() {
    let environment = load(&[
        (
            "first.rbs",
            "class Greeter\n  def hello: () -> String\nend\n",
        ),
        (
            "second.rbs",
            "class Greeter\n  def hello: (Integer) -> Symbol | ...\nend\n",
        ),
    ]);
    assert_eq!(
        overload_strings(&environment, "Greeter", "hello"),
        vec!["(Integer) -> Symbol", "() -> String"]
    );
}

#[test]
fn overloading_definition_loaded_before_the_original_still_comes_first() {
    let environment = load(&[
        (
            "app/series.rbs",
            "class Base\n  def create: () -> nil\nend\n\nclass Series < Base\n  def create: (*untyped) -> Series | ...\nend\n",
        ),
        (
            "rbs_rails/series.rbs",
            "class Series\n  def create: () -> Series | (Integer) -> Series\nend\n",
        ),
    ]);
    assert_eq!(
        overload_strings(&environment, "Series", "create"),
        vec![
            "(*untyped) -> Series",
            "() -> Series",
            "(Integer) -> Series"
        ]
    );
}

#[test]
fn overloading_definition_without_own_method_extends_inherited_method() {
    let environment = load(&[(
        "app.rbs",
        "class Base\n  def call: () -> String\nend\n\nclass Derived < Base\n  def call: (Integer) -> Integer | ...\nend\n",
    )]);
    assert_eq!(
        overload_strings(&environment, "Derived", "call"),
        vec!["(Integer) -> Integer", "() -> String"]
    );
}

#[test]
fn registers_attribute_methods_and_instance_variables() {
    let environment = load(&[(
        "app.rbs",
        r#"
class Person
  attr_reader name: String
  attr_writer age(@years): Integer
  attr_accessor nickname(): String?
  attr_reader self.count: Integer
end
"#,
    )]);
    assert_eq!(
        overload_strings(&environment, "Person", "name"),
        vec!["() -> String"]
    );
    assert_eq!(
        overload_strings(&environment, "Person", "age="),
        vec!["(Integer age) -> Integer"]
    );
    assert!(environment.find_instance_method("Person", "age").is_none());
    assert_eq!(
        overload_strings(&environment, "Person", "nickname"),
        vec!["() -> String?"]
    );
    assert_eq!(
        overload_strings(&environment, "Person", "nickname="),
        vec!["(String? nickname) -> String?"]
    );

    let person = &environment.classes["Person"];
    assert_eq!(
        person.instance_variables.get("@name"),
        Some(&Type::instance("String"))
    );
    assert_eq!(
        person.instance_variables.get("@years"),
        Some(&Type::instance("Integer"))
    );
    assert!(!person.instance_variables.contains_key("@nickname"));
    assert_eq!(
        person.class_instance_variables.get("@count"),
        Some(&Type::instance("Integer"))
    );
    assert_eq!(
        environment.instance_variable_type("Person", "@name"),
        Some(Type::instance("String"))
    );
}

#[test]
fn resolves_aliases_to_the_aliased_overloads() {
    let environment = load(&[(
        "app.rbs",
        "class Greeter\n  def hello: (String name) -> String\n  alias greet hello\n  alias self.build self.new\nend\n",
    )]);
    let greet = environment
        .find_instance_method("Greeter", "greet")
        .unwrap();
    assert_eq!(greet.definition.alias_of.as_deref(), Some("hello"));
    assert_eq!(
        greet
            .overloads
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        vec!["(String name) -> String"]
    );

    let build = environment
        .find_singleton_method("Greeter", "build")
        .unwrap();
    assert_eq!(
        build
            .overloads
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        vec!["() -> instance"]
    );
}

#[test]
fn applies_visibility_members_and_module_functions() {
    let environment = load(&[(
        "app.rbs",
        r#"
class Service
  def run: () -> void
  private
  def helper: () -> void
  public def exposed: () -> void
  public
  def other: () -> void
end

module Utility
  def self?.format: () -> String
end
"#,
    )]);
    let service = &environment.classes["Service"];
    assert_eq!(
        service.instance_methods["run"].visibility,
        Visibility::Public
    );
    assert_eq!(
        service.instance_methods["helper"].visibility,
        Visibility::Private
    );
    assert_eq!(
        service.instance_methods["exposed"].visibility,
        Visibility::Public
    );
    assert_eq!(
        service.instance_methods["other"].visibility,
        Visibility::Public
    );

    let utility = &environment.classes["Utility"];
    assert_eq!(utility.kind, ClassKind::Module);
    assert_eq!(utility.super_class, None);
    assert_eq!(
        utility.instance_methods["format"].visibility,
        Visibility::Private
    );
    assert_eq!(
        utility.singleton_methods["format"].visibility,
        Visibility::Public
    );
    assert_eq!(
        utility.singleton_methods["format"].kind,
        MethodKind::Singleton
    );
}

#[test]
fn ancestors_substitute_type_arguments_along_the_chain() {
    let environment = load(&[("app.rbs", "class StringList < Array[String]\nend\n")]);
    assert_eq!(
        ancestor_strings(&environment.ancestors("Array")),
        vec![
            "Array[Elem]",
            "Enumerable[Elem]",
            "Object",
            "Kernel",
            "BasicObject"
        ]
    );
    assert_eq!(
        ancestor_strings(&environment.ancestors("Hash")),
        vec![
            "Hash[K, V]",
            "Enumerable[[K, V]]",
            "Object",
            "Kernel",
            "BasicObject"
        ]
    );
    assert_eq!(
        ancestor_strings(&environment.ancestors("StringList")),
        vec![
            "StringList",
            "Array[String]",
            "Enumerable[String]",
            "Object",
            "Kernel",
            "BasicObject"
        ]
    );
}

#[test]
fn ancestors_put_prepends_first_and_later_includes_before_earlier_ones() {
    let environment = load(&[(
        "app.rbs",
        "module First\nend\nmodule Second\nend\nmodule Wrapper\nend\nclass Widget\n  include First\n  include Second\n  prepend Wrapper\nend\n",
    )]);
    assert_eq!(
        ancestor_strings(&environment.ancestors("Widget")),
        vec![
            "Wrapper",
            "Widget",
            "Second",
            "First",
            "Object",
            "Kernel",
            "BasicObject"
        ]
    );
}

#[test]
fn finds_methods_from_included_modules_with_substituted_type_arguments() {
    let environment = load(&[("app.rbs", "class StringList < Array[String]\nend\n")]);
    assert_eq!(
        overload_strings(&environment, "Array", "map"),
        vec!["[U] () { (Elem item) -> U } -> Array[U]"]
    );
    assert_eq!(
        overload_strings(&environment, "Hash", "first"),
        vec!["() -> [K, V]?"]
    );
    assert_eq!(
        overload_strings(&environment, "StringList", "first"),
        vec!["() -> String?"]
    );
    assert_eq!(
        overload_strings(&environment, "StringList", "puts"),
        vec!["(*untyped objects) -> nil"]
    );
}

#[test]
fn module_lookup_falls_back_to_self_types_and_object() {
    let environment = load(&[(
        "app.rbs",
        "class Base\n  def base_method: () -> Integer\nend\n\nmodule Concern : Base\nend\n",
    )]);
    assert_eq!(
        overload_strings(&environment, "Concern", "base_method"),
        vec!["() -> Integer"]
    );
    assert_eq!(
        overload_strings(&environment, "Concern", "puts"),
        vec!["(*untyped objects) -> nil"]
    );
}

#[test]
fn singleton_ancestors_follow_extends_superclasses_and_class() {
    let environment = load(&[(
        "app.rbs",
        "module Helpers\n  def helper: () -> Integer\nend\n\nclass Base\n  extend Helpers\n  def self.create: () -> instance\nend\n\nclass Derived < Base\nend\n",
    )]);
    let ancestors = environment.singleton_ancestors("Derived");
    assert_eq!(
        ancestors[..4],
        [
            SingletonAncestor::Singleton("Derived".to_string()),
            SingletonAncestor::Singleton("Base".to_string()),
            SingletonAncestor::Instance(Ancestor {
                name: "Helpers".to_string(),
                arguments: Vec::new()
            }),
            SingletonAncestor::Singleton("Object".to_string()),
        ]
    );
    assert_eq!(
        ancestors.last(),
        Some(&SingletonAncestor::Instance(Ancestor {
            name: "BasicObject".to_string(),
            arguments: Vec::new()
        }))
    );

    let create = environment
        .find_singleton_method("Derived", "create")
        .unwrap();
    assert_eq!(create.definition.owner, "Base");
    assert!(
        environment
            .find_singleton_method("Derived", "helper")
            .is_some()
    );
    assert!(
        environment
            .find_singleton_method("Derived", "allocate")
            .is_some()
    );
    assert!(
        environment
            .find_singleton_method("Derived", "name")
            .is_some()
    );
}

#[test]
fn derives_new_from_initialize() {
    let environment = load(&[]);
    let string_new = environment.find_singleton_method("String", "new").unwrap();
    assert_eq!(
        string_new
            .overloads
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        vec!["(?String source) -> instance"]
    );
    let array_new = environment.find_singleton_method("Array", "new").unwrap();
    assert_eq!(
        array_new
            .overloads
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        vec!["[Elem] (?Integer size, ?Elem default) -> Array[Elem]"]
    );
}

#[test]
fn interface_lookup_follows_includes() {
    let environment = load(&[(
        "app.rbs",
        "interface _Each[T]\n  def each: () { (T) -> void } -> void\nend\n\ninterface _Collection[T]\n  include _Each[T]\n  def size: () -> Integer\nend\n",
    )]);
    let each = environment
        .find_interface_method("_Collection", "each")
        .unwrap();
    assert_eq!(
        each.overloads
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        vec!["() { (T) -> void } -> void"]
    );
    assert!(
        environment
            .find_interface_method("_Collection", "size")
            .is_some()
    );
}

#[test]
fn resolves_constant_names_through_nesting_ancestors_and_class_aliases() {
    let environment = load(&[(
        "app.rbs",
        r#"
module App
  VERSION: String

  class Base
    LIMIT: Integer
  end

  class Child < Base
  end

  class Alias = Base
end

class Shortcut = App::Child
"#,
    )]);
    let nesting = vec!["App".to_string(), "App::Child".to_string()];
    assert_eq!(
        environment.resolve_constant_name(&nesting, "VERSION"),
        Some("App::VERSION".to_string())
    );
    assert_eq!(
        environment.resolve_constant_name(&nesting, "LIMIT"),
        Some("App::Base::LIMIT".to_string())
    );
    assert_eq!(
        environment.resolve_constant_name(&nesting, "Base"),
        Some("App::Base".to_string())
    );
    assert_eq!(
        environment.resolve_constant_name(&nesting, "String"),
        Some("String".to_string())
    );
    assert_eq!(environment.resolve_constant_name(&nesting, "Missing"), None);
    assert_eq!(
        environment.resolve_constant_name(&[], "Shortcut"),
        Some("Shortcut".to_string())
    );
    assert_eq!(
        environment.resolve_class_name("Shortcut"),
        Some("App::Child")
    );
    assert_eq!(
        environment
            .class_aliases
            .get("App::Alias")
            .map(String::as_str),
        Some("App::Base")
    );
    assert_eq!(
        environment.resolve_constant_name(&[], "App::Alias::LIMIT"),
        Some("App::Base::LIMIT".to_string())
    );
}

#[test]
fn resolve_type_names_uses_the_given_nesting() {
    let environment = load(&[("app.rbs", "module App\n  class User\n  end\nend\n")]);
    let mut parsed = parse_type("Array[User]?").unwrap();
    environment.resolve_type_names(&mut parsed, &["App".to_string()]);
    assert_eq!(parsed.to_string(), "Array[App::User]?");
}

#[test]
fn parse_type_accepts_types_only_valid_in_return_position() {
    assert_eq!(parse_type("void"), Ok(Type::Void));
    assert_eq!(parse_type("self?").unwrap().to_string(), "self?");
    assert_eq!(
        parse_type("^(Integer) -> String").unwrap().to_string(),
        "^(Integer) -> String"
    );
    assert_eq!(
        parse_type("{ name: String, ?age: Integer }")
            .unwrap()
            .to_string(),
        "{ name: String, ?age: Integer }"
    );
    assert!(parse_type("Array[").is_err());
}

#[test]
fn records_locations_and_comments() {
    let environment = load(&[(
        "app.rbs",
        "# A greeter\nclass Greeter\n  # Says hello\n  def hello: () -> \"héllo\"\nend\n",
    )]);
    let greeter = &environment.classes["Greeter"];
    assert_eq!(greeter.comment.as_deref(), Some("A greeter"));
    assert_eq!(greeter.locations[0].start.line, 1);
    let hello = &greeter.instance_methods["hello"];
    assert_eq!(hello.comment.as_deref(), Some("Says hello"));
    let location = hello.location.as_ref().unwrap();
    assert_eq!(location.path.as_path(), Path::new("app.rbs"));
    assert_eq!(
        (
            location.start.line,
            location.start.byte_column,
            location.start.utf16_column
        ),
        (3, 2, 2)
    );
    assert_eq!(
        (
            location.end.line,
            location.end.byte_column,
            location.end.utf16_column
        ),
        (3, 27, 26)
    );
}

#[test]
fn parse_errors_are_reported_and_the_file_is_skipped() {
    let mut loader = Loader::new();
    loader.add_source(Path::new("core.rbs"), MINIMAL_CORE);
    loader.add_source(
        Path::new("broken.rbs"),
        "class Broken\n  def x: () ->\nend\n",
    );
    let environment = loader.finish();
    assert_eq!(environment.errors.len(), 1);
    assert_eq!(environment.errors[0].path, Path::new("broken.rbs"));
    assert!(!environment.classes.contains_key("Broken"));
    assert!(environment.classes.contains_key("String"));
}

#[test]
fn records_method_annotations_and_attribute_readers() {
    let environment = load(&[(
        "uploader.rbs",
        r#"
class Uploader
  %a{pure} def small_image_file: () -> String?
  def thumbnail: %a{pure} () -> String? | %a{deprecated} (Integer size) -> String?
  def plain: () -> String
  attr_reader name: String
  attr_accessor title: String?
  attr_writer secret: String
end
"#,
    )]);
    let declaration = &environment.classes["Uploader"];
    let method = |name: &str| &declaration.instance_methods[name];

    assert_eq!(
        method("small_image_file").annotations,
        vec!["pure".to_string()]
    );
    assert_eq!(
        method("thumbnail").annotations,
        vec!["pure".to_string(), "deprecated".to_string()]
    );
    assert!(method("plain").annotations.is_empty());

    assert!(method("name").is_attribute_reader);
    assert!(method("title").is_attribute_reader);
    assert!(!method("title=").is_attribute_reader);
    assert!(!method("secret=").is_attribute_reader);
    assert!(!method("small_image_file").is_attribute_reader);
}

#[test]
fn subclass_include_of_a_module_already_in_the_superclass_takes_precedence() {
    let environment = load(&[(
        "relation.rbs",
        "class Base\n  include Enumerable[untyped]\nend\n\nclass Derived < Base\n  include Enumerable[String]\nend\n",
    )]);
    assert_eq!(
        ancestor_strings(&environment.ancestors("Derived")),
        vec![
            "Derived",
            "Enumerable[String]",
            "Base",
            "Object",
            "Kernel",
            "BasicObject"
        ]
    );
    assert_eq!(
        overload_strings(&environment, "Derived", "map"),
        vec!["[U] () { (String item) -> U } -> Array[U]"]
    );
}
