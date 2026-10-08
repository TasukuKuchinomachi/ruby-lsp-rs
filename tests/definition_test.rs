mod support;

use std::path::Path;

use serde_json::{Value, json};
use support::{DEFAULT_URI, TestOptions, range_json, rbs_core_path, ruby_config, with_server};

fn no_typechecker() -> TestOptions {
    TestOptions { stub_no_typechecker: true, ..TestOptions::default() }
}

fn target_start_line(location: &Value) -> u64 {
    location["targetRange"]["start"]["line"].as_u64().unwrap()
}

fn range_start_line(location: &Value) -> u64 {
    location["range"]["start"]["line"].as_u64().unwrap()
}

#[test]
fn jumping_to_default_gems() {
    let Some(rubylibdir) = ruby_config("rubylibdir") else { return };
    let mut server = with_server(Some("Pathname"), DEFAULT_URI, TestOptions { rubylibdir: Some(rubylibdir.clone()), ..TestOptions::default() });
    server.index_file(&Path::new(&rubylibdir).join("pathname.rb"), None);
    assert!(!server.definition(0, 0).is_empty());
}

#[test]
fn constant_precision() {
    let source = "module Foo\n  module Bar\n    class Baz\n    end\n  end\nend\n\nFoo::Bar::Baz\n";
    let mut server = with_server(Some(source), DEFAULT_URI, no_typechecker());
    assert_eq!(server.definition(7, 0)[0]["targetRange"], range_json(0, 0, 5, 3));
    assert_eq!(server.definition(7, 5)[0]["targetRange"], range_json(1, 2, 4, 5));
    assert_eq!(server.definition(7, 10)[0]["targetRange"], range_json(2, 4, 3, 7));
}

#[test]
fn multibyte_character_precision() {
    let source = "module Fほげ\n  module Bar\n    class Baz\n    end\n  end\nend\n\nFほげ::Bar::Baz\n";
    let mut server = with_server(Some(source), DEFAULT_URI, no_typechecker());
    assert_eq!(server.definition(7, 0)[0]["targetRange"], range_json(0, 0, 5, 3));
}

#[test]
fn jumping_to_default_require_of_a_gem() {
    let Some(rubylibdir) = ruby_config("rubylibdir") else { return };
    let mut server = with_server(Some("require \"bundler\""), DEFAULT_URI, TestOptions::default());
    let bundler_path = Path::new(&rubylibdir).join("bundler.rb");
    server.index_file(&bundler_path, Some("bundler"));
    for entry in std::fs::read_dir(Path::new(&rubylibdir).join("bundler")).unwrap().filter_map(Result::ok) {
        let path = entry.path();
        if path.extension().is_some_and(|extension| extension == "rb") {
            let require_path = ruby_lsp_rs::uri::require_path(&path.to_string_lossy(), &rubylibdir);
            server.index_file(&path, Some(&require_path));
        }
    }
    let response = server.definition(0, 10);
    assert_eq!(response[0]["uri"], json!(ruby_lsp_rs::uri::from_path_buf(&bundler_path)));
}

#[test]
fn jumping_to_private_constant_inside_the_same_namespace() {
    let source = "class A\n  CONST = 123\n  private_constant(:CONST)\n\n  CONST\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, no_typechecker());
    assert_eq!(server.definition(4, 2)[0]["targetUri"], json!(DEFAULT_URI));
}

#[test]
fn jumping_to_private_constant_from_different_namespace() {
    let source = "class A\n  CONST = 123\n  private_constant(:CONST)\nend\n\nA::CONST # invalid private reference\n";
    let mut server = with_server(Some(source), DEFAULT_URI, no_typechecker());
    assert!(server.definition(5, 3).is_empty());
}

#[test]
fn jumping_to_method_definitions_when_declaration_exists() {
    let source = "# typed: false\n\nclass A\n  def bar\n    foo\n  end\n\n  def foo; end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    assert_eq!(server.definition(4, 4)[0]["targetUri"], json!(DEFAULT_URI));
}

#[test]
fn can_jump_to_method_with_two_definitions() {
    let first_source = "# typed: false\n\nclass A\n  def bar\n    foo\n  end\n\n  def foo; end\nend\n";
    let mut server = with_server(Some(first_source), DEFAULT_URI, TestOptions::default());
    let second_uri = "file:///folder/fake2.rb";
    let second_source = "# typed: false\n\nclass A\n  def foo; end\nend\n";
    server.did_open(second_uri, second_source, "ruby");
    server.index_source(second_uri, second_source);

    let response = server.definition(4, 4);
    assert_eq!(response[0]["targetUri"], json!(DEFAULT_URI));
    assert_eq!(response[1]["targetUri"], json!(second_uri));
}

#[test]
fn jumping_to_method_method_calls_on_explicit_self() {
    let source = "# typed: false\n\nclass A\n  def bar\n    self.foo\n  end\n\n  def foo; end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    assert_eq!(server.definition(4, 9)[0]["targetUri"], json!(DEFAULT_URI));
}

#[test]
fn does_nothing_when_declaration_does_not_exist() {
    let source = "# typed: false\n\nclass A\n  def bar\n    foo\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    assert!(server.definition(4, 4).is_empty());
}

#[test]
fn jumping_to_autoload_definition_when_declaration_exists() {
    let source = "# typed: ignore\n\nclass Foo\n  autoload :Bar, \"bar\"\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    server.index_source("file:///fake/path/bar.rb", "class Foo::Bar; end\n");
    server.index_source("file:///fake/path/baz.rb", "class Foo::Bar; end\n");
    let response = server.definition(3, 12);
    assert_eq!(response.len(), 2);
    assert_eq!(response[0]["targetUri"], json!("file:///fake/path/bar.rb"));
    assert_eq!(response[1]["targetUri"], json!("file:///fake/path/baz.rb"));
}

#[test]
fn go_to_definition_for_send_symbol() {
    let source = "class Foo\n  def foo; end\nend\n\nobj = Foo.new\nobj.send(:foo)\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    let response = server.definition(5, 11);
    assert_eq!(response[0]["targetRange"]["start"]["line"], json!(1));
    assert_eq!(response[0]["targetRange"]["end"]["line"], json!(1));
}

#[test]
fn go_to_definition_for_send_symbol_multiple_objects() {
    let source = "class Foo\n  def foo; end\nend\n\nclass Bar\n  def foo; end\nend\n\nf = Foo.new\nb = Bar.new\n\nf.send(:foo)\nb.send(:foo)\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    let response = server.definition(11, 9);
    assert_eq!(response.len(), 2);
    assert_eq!(target_start_line(&response[0]), 1);
    assert_eq!(target_start_line(&response[1]), 5);
}

#[test]
fn go_to_definition_ignores_non_method_send_symbol() {
    let source = "class Foo\n  def foo; end\nend\n\nobj = Foo.new\nobj.send(:foo, :foo)\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    assert!(server.definition(5, 17).is_empty());
}

#[test]
fn go_to_definition_for_send_string() {
    let source = "class Foo\n  def foo; end\nend\n\nobj = Foo.new\nobj.send(\"foo\")\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    let response = server.definition(5, 11);
    assert_eq!(target_start_line(&response[0]), 1);
    assert_eq!(response[0]["targetRange"]["end"]["line"], json!(1));
}

#[test]
fn go_to_definition_for_send_string_multiple_objects() {
    let source = "class Foo\n  def foo; end\nend\n\nclass Bar\n  def foo; end\nend\n\nf = Foo.new\nb = Bar.new\n\nf.send(\"foo\")\nb.send(\"foo\")\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    let response = server.definition(11, 9);
    assert_eq!(response.len(), 2);
    assert_eq!(target_start_line(&response[0]), 1);
    assert_eq!(target_start_line(&response[1]), 5);
}

#[test]
fn go_to_definition_ignores_non_method_string_symbol() {
    let source = "class Foo\n  def foo; end\nend\n\nobj = Foo.new\nobj.send(\"foo\", \"foo\")\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    assert!(server.definition(5, 17).is_empty());
}

#[test]
fn does_nothing_when_autoload_declaration_does_not_exist() {
    let source = "# typed: ignore\n\nclass Foo\n  autoload :Bar, \"bar\"\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    assert!(server.definition(3, 3).is_empty());
}

#[test]
fn methods_with_dynamic_namespace_is_also_suggested() {
    let source = "# typed: false\n\nclass self::A\n  def foo; end\n\n  def bar\n    foo\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    let response = server.definition(6, 4);
    assert_eq!(response.len(), 1);
    assert_eq!(response[0]["targetRange"], range_json(3, 2, 3, 14));
}

#[test]
fn definitions_are_listed_for_method_with_unknown_receiver() {
    let source = "# typed: false\n\nclass A\n  def foo; end\nend\n\nclass B\n  def foo; end\nend\n\nobj.foo\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    let response = server.definition(10, 4);
    assert_eq!(response.len(), 2);
    assert_eq!(response[0]["targetRange"], range_json(3, 2, 3, 14));
    assert_eq!(response[1]["targetRange"], range_json(7, 2, 7, 14));
}

#[test]
fn definition_for_guessed_receiver_is_listed() {
    let source = "# typed: false\n\nclass Cheetah\n  def meow; end\nend\n\nclass Cat\n  def meow; end\nend\n\ncat = Cat.new\ncat.meow\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    let response = server.definition(11, 4);
    assert_eq!(response.len(), 1);
    assert_eq!(response[0]["targetRange"], range_json(7, 2, 7, 15));
}

#[test]
fn guessed_receiver_is_treated_as_unknown_when_no_declaration_exists() {
    let source = "# typed: false\n\nclass Animal\n  def eat; end\nend\n\nclass Cheetah\n  def meow; end\nend\n\nclass Cat\n  def meow; end\nend\n\nanimal = Cat.new\nanimal.meow\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    assert_eq!(server.definition(15, 7).len(), 2);
}

#[test]
fn definitions_for_unknown_receiver_is_capped() {
    let mut source = "# typed: false\n".to_string();
    for number in 1..=13 {
        source.push_str(&format!("class Class{number}\n  def foo; end\nend\n"));
    }
    source.push_str("\nobj.foo");
    let mut server = with_server(Some(&source), DEFAULT_URI, TestOptions::default());
    assert_eq!(server.definition(41, 4).len(), 10);
}

#[test]
fn definitions_are_listed_in_erb_files_as_unknown_receiver() {
    let mut server = with_server(Some("<%= foo %>\n"), "file:///fake.erb", TestOptions::default());
    server.index_source("file:///fake/path/foo.rb", "class Bar\n  def foo; end\n\n  def bar; end\nend\n");
    let response = server.definition(0, 4);
    assert_eq!(response.len(), 1);
    assert_eq!(response[0]["targetRange"], range_json(1, 2, 1, 14));
}

#[test]
fn definition_precision_for_methods_with_block_arguments() {
    let source = "class Foo\n  def foo(&block); end\n\n  def argument; end\nend\n\nbar.foo(&:argument)\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    assert_eq!(target_start_line(&server.definition(6, 12)[0]), 3);
    assert_eq!(target_start_line(&server.definition(6, 4)[0]), 1);
}

#[test]
fn definition_for_method_call_inside_arguments() {
    let source = "class Foo\n  def foo; end\n\n  def bar(a:, b:); end\n\n  def baz\n    bar(a: foo, b: 42)\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    let response = server.definition(6, 11);
    assert_eq!(target_start_line(&response[0]), 1);
}

#[test]
fn definition_for_global_variables() {
    let Some(rbs_core_path) = rbs_core_path() else { return };
    let source = "$bar &&= 1\n$bar += 1\n$foo ||= 1\n$bar, $foo = 1\n$foo = 1\n$DEBUG\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    ruby_lsp_rs::indexer::rbs_indexer::RbsIndexer::new(&mut server.index()).index_core_directory(&rbs_core_path);

    let response = server.definition(0, 1);
    assert_eq!(response.len(), 3);
    assert_eq!(range_start_line(&response[0]), 0);
    assert_eq!(range_start_line(&response[1]), 1);
    assert_eq!(range_start_line(&response[2]), 3);

    let response = server.definition(2, 1);
    assert_eq!(response.len(), 3);
    assert_eq!(range_start_line(&response[0]), 2);
    assert_eq!(range_start_line(&response[1]), 3);
    assert_eq!(range_start_line(&response[2]), 4);

    let response = server.definition(5, 1);
    let uri = response[0]["uri"].as_str().unwrap();
    assert!(uri.contains("/core/global_variables.rbs"), "unexpected uri {uri}");
    let range = &response[0]["range"];
    assert_eq!(range["start"]["line"], range["end"]["line"]);
    assert!(range["start"]["character"].as_u64() < range["end"]["character"].as_u64());
}

#[test]
fn definition_apply_target_correction() {
    let source = "$foo &&= 1\n$foo += 1\n$foo ||= 1\n$foo = 1\nclass Foo\n  @foo &&= 1\n  @foo += 1\n  @foo ||= 1\n  @foo = 1\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    for line in [0, 1, 2, 3, 5, 6, 7, 8] {
        assert!(server.definition(line, 7).is_empty(), "line {line} should not have definitions");
    }
}

#[test]
fn definition_for_class_variables() {
    let source = "class Foo\n  def foo\n    @@a ||= 1\n  end\n\n  def bar\n    @@a += 5\n  end\n\n  def baz\n    @@a\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    let response = server.definition(10, 5);
    assert_eq!(response.len(), 2);
    assert_eq!(range_start_line(&response[0]), 2);
    assert_eq!(range_start_line(&response[1]), 6);
}

#[test]
fn definition_for_inherited_class_variables() {
    let source = "module Foo\n  def set_variable\n    @@bar = 1\n  end\nend\n\nclass Parent\n  def set_variable\n    @@bar = 5\n  end\nend\n\nclass Child < Parent\n  include Foo\n\n  def do_something\n    @@bar\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    let response = server.definition(16, 4);
    assert_eq!(range_start_line(&response[0]), 2);
    assert_eq!(range_start_line(&response[1]), 8);
}

#[test]
fn definition_for_class_variables_in_different_context() {
    let source = "class Foo\n  @@a = 1\n\n  class << self\n    @@a = 2\n\n    def foo\n      @@a = 3\n    end\n  end\n\n  def bar\n    @@a = 4\n  end\n\n  def self.baz\n    @@a = 5\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    let response = server.definition(1, 4);
    let lines: Vec<u64> = response.iter().map(range_start_line).collect();
    assert_eq!(lines, vec![1, 4, 7, 12, 16]);
}

#[test]
fn definition_for_instance_variables() {
    let source = "class Foo\n  def initialize\n    @a = 1\n  end\n\n  def bar\n    @a\n  end\n\n  def baz\n    @a = 5\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    assert_eq!(range_start_line(&server.definition(6, 4)[0]), 2);
    assert_eq!(range_start_line(&server.definition(10, 4)[0]), 2);
}

#[test]
fn definition_for_inherited_methods() {
    let source = "module Foo\n  module First\n    def method1; end\n  end\n\n  class Bar\n    def method2; end\n  end\n\n  class Baz < Bar\n    include First\n\n    def method3\n      method1\n      method2\n    end\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    assert_eq!(target_start_line(&server.definition(13, 6)[0]), 2);
    assert_eq!(target_start_line(&server.definition(14, 6)[0]), 6);
}

#[test]
fn definition_for_inherited_instance_variables() {
    let source = "module Foo\n  def set_ivar\n    @a = 1\n  end\nend\n\nclass Parent\n  def initialize\n    @a = 5\n  end\nend\n\nclass Child < Parent\n  include Foo\n\n  def do_something\n    @a\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    let response = server.definition(16, 4);
    assert_eq!(range_start_line(&response[0]), 2);
    assert_eq!(range_start_line(&response[1]), 8);
}

#[test]
fn definition_for_singleton_methods() {
    let source = "class Foo\n  def self.bar\n  end\n\n  class << self\n    def baz; end\n  end\nend\n\nFoo.bar\nFoo.baz\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    assert_eq!(target_start_line(&server.definition(9, 4)[0]), 1);
    assert_eq!(target_start_line(&server.definition(10, 4)[0]), 5);
}

#[test]
fn definition_for_class_instance_variables() {
    let source = "class Foo\n  @a = 123\n\n  def self.bar\n    @a\n  end\n\n  class << self\n    def baz\n      @a\n    end\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    assert_eq!(range_start_line(&server.definition(4, 4)[0]), 1);
    assert_eq!(range_start_line(&server.definition(9, 6)[0]), 1);
}

#[test]
fn definition_for_aliased_methods() {
    let source = "class Parent\n  def bar; end\nend\n\nclass Child < Parent\n  alias baz bar\n\n  def do_something\n    baz\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    assert_eq!(target_start_line(&server.definition(8, 4)[0]), 5);
}

#[test]
fn definition_for_super_calls() {
    let source = "class Parent\n  def foo; end\n  def bar; end\nend\n\nclass Child < Parent\n  def foo(a)\n    super()\n  end\n\n  def bar\n    super\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    assert_eq!(target_start_line(&server.definition(7, 4)[0]), 1);
    assert_eq!(target_start_line(&server.definition(11, 4)[0]), 2);
}

#[test]
fn definition_for_super_calls_is_disabled_on_typed_true() {
    let source = "# typed: true\nclass Parent\n  def foo; end\n  def bar; end\nend\n\nclass Child < Parent\n  def foo(a)\n    super()\n  end\n\n  def bar\n    super\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    assert!(server.definition(8, 4).is_empty());
    assert!(server.definition(12, 4).is_empty());
}

#[test]
fn definition_on_self_is_disabled_for_typed_true() {
    let source = "# typed: true\nclass Foo\n  def bar\n    baz\n  end\n\n  def baz\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    assert!(server.definition(3, 4).is_empty());
}

#[test]
fn definition_for_instance_variables_is_disabled_on_typed_strict() {
    let source = "# typed: strict\nclass Foo\n  def initialize\n    @something = 123 #: Integer\n  end\n\n  def baz\n    @something\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    assert!(server.definition(7, 4).is_empty());
}

#[test]
fn definition_call_node_precision() {
    let source = "class Foo\n  def message\n    \"hello!\"\n  end\nend\n\nclass Bar\n  def with_foo(foo)\n    @foo_message = foo.message\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    assert_eq!(
        server.definition(8, 19),
        vec![json!({ "targetUri": DEFAULT_URI, "targetRange": range_json(7, 15, 7, 18), "targetSelectionRange": range_json(7, 15, 7, 18) })]
    );
    let message_definitions = server.definition(8, 23);
    assert!(!message_definitions.is_empty());
    assert_ne!(message_definitions[0]["targetRange"]["start"]["line"], json!(7));
}

#[test]
fn definition_does_proper_dependency_checking_for_unsaved_files_for_methods() {
    let source = "# typed: true\nclass Foo\n  def bar\n  end\nend\n\nFoo.new.bar\n";
    let mut server = with_server(Some(source), "untitled:Untitled-1", TestOptions::default());
    assert!(server.definition(6, 8).is_empty());
}

#[test]
fn definition_does_proper_dependency_checking_for_unsaved_files_for_constants() {
    let source = "class Foo\n  def bar\n  end\nend\n\nFoo\n";
    let mut server = with_server(Some(source), "untitled:Untitled-1", TestOptions::default());
    assert!(server.definition(0, 8).is_empty());
}

#[test]
fn require_relative_resolves_against_the_document_directory() {
    let mut server = with_server(Some("require_relative \"../lib/foo\"\n"), "file:///project/test/foo_test.rb", no_typechecker());
    let response = server.definition(0, 20);
    assert_eq!(response, vec![json!({ "uri": "file:///project/lib/foo.rb", "range": range_json(0, 0, 0, 0) })]);
}

#[test]
fn definition_for_non_existing_document_is_an_invalid_params_error() {
    let mut server = with_server(None, DEFAULT_URI, no_typechecker());
    let response = server.request(
        "textDocument/definition",
        json!({ "textDocument": { "uri": "file:///does/not/exist.rb" }, "position": { "line": 0, "character": 0 } }),
    );
    assert_eq!(response.response_result.unwrap_err().code, lsp_server::ErrorCode::InvalidParams as i32);
}
