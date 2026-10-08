mod support;

use ruby_lsp_rs::indexer::rbs_indexer::RbsIndexer;
use serde_json::{Value, json};
use support::{DEFAULT_URI, TestOptions, TestServer, rbs_core_path, with_server};

fn no_typechecker() -> TestOptions {
    TestOptions { stub_no_typechecker: true, ..TestOptions::default() }
}

fn hover(server: &mut TestServer, line: u32, character: u32) -> Value {
    let uri = server.uri.clone();
    server.result(
        "textDocument/hover",
        json!({ "textDocument": { "uri": uri }, "position": { "line": line, "character": character } }),
    )
}

fn hover_contents(server: &mut TestServer, line: u32, character: u32) -> String {
    let result = hover(server, line, character);
    result["contents"]["value"].as_str().unwrap_or_else(|| panic!("Expected hover contents, got {result}")).to_string()
}

fn index_ruby_core(server: &TestServer) -> bool {
    let Some(rbs_core_path) = rbs_core_path() else { return false };
    RbsIndexer::new(&mut server.index()).index_core_directory(&rbs_core_path);
    true
}

fn assert_contains(contents: &str, expected: &str) {
    assert!(contents.contains(expected), "Expected {contents:?} to contain {expected:?}");
}

#[test]
fn hovering_over_private_constant_from_the_same_namespace() {
    let source = "class A\n  CONST = 123\n  private_constant(:CONST)\n\n  CONST\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, no_typechecker());
    assert_contains(&hover_contents(&mut server, 4, 2), "CONST");
}

#[test]
fn hovering_on_erb() {
    let mut server = with_server(Some("<% String %>\n"), "file:///fake.erb", no_typechecker());
    if !index_ruby_core(&server) {
        return;
    }
    let contents = hover_contents(&mut server, 0, 4);
    assert!(contents.contains("String\n") || contents.contains("String`"), "{contents}");
}

#[test]
fn hovering_for_global_variables() {
    let source = "# and write node\n$bar &&= 1\n# operator write node\n$baz += 1\n# or write node\n$qux ||= 1\n# target write node\n$quux, $corge = 1\n# write node\n$foo = 1\n# read node\n$DEBUG\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    let core_indexed = index_ruby_core(&server);

    let expectations = [
        (1, "and write node"),
        (3, "operator write node"),
        (5, "or write node"),
        (7, "target write node"),
        (9, "write node"),
        (11, "The debug flag"),
    ];
    for (line, documentation) in expectations {
        if line == 11 && !core_indexed {
            continue;
        }
        assert_contains(&hover_contents(&mut server, line, 1), documentation);
    }
}

#[test]
fn hover_apply_target_correction() {
    let source = "$bar &&= 1\n$baz += 1\n$qux ||= 1\n$foo = 1\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    for line in 0..4 {
        assert_eq!(hover(&mut server, line, 5), Value::Null);
    }
}

#[test]
fn hovering_precision() {
    let source = "module Foo\n  module Bar\n    class Baz\n    end\n  end\nend\n\nFoo::Bar::Baz\n";
    let mut server = with_server(Some(source), DEFAULT_URI, no_typechecker());
    assert_contains(&hover_contents(&mut server, 7, 0), "```ruby\nFoo\n```");
    assert_contains(&hover_contents(&mut server, 7, 5), "```ruby\nFoo::Bar\n```");
    assert_contains(&hover_contents(&mut server, 7, 10), "```ruby\nFoo::Bar::Baz\n```");
}

#[test]
fn hovering_methods_invoked_on_implicit_self() {
    let source = "# typed: false\n\nclass A\n  # Hello from `foo`\n  def foo; end\n\n  def bar\n    foo\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, no_typechecker());
    assert_contains(&hover_contents(&mut server, 7, 4), "Hello from `foo`");
}

#[test]
fn hovering_methods_with_two_definitions() {
    let source = "# typed: false\n\nclass A\n  # Hello from first `foo`\n  def foo; end\n\n  def bar\n    foo\n  end\nend\n\nclass A\n  # Hello from second `foo`\n  def foo; end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, no_typechecker());
    let contents = hover_contents(&mut server, 7, 4);
    assert_contains(&contents, "Hello from first `foo`");
    assert_contains(&contents, "Hello from second `foo`");
}

#[test]
fn hovering_methods_invoked_on_explicit_self() {
    let source = "# typed: false\n\nclass A\n  # Hello from `foo`\n  def foo; end\n\n  def bar\n    self.foo\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, no_typechecker());
    assert_contains(&hover_contents(&mut server, 7, 9), "Hello from `foo`");
}

#[test]
fn hovering_over_private_constant_from_different_namespace() {
    let source = "class A\n  CONST = 123\n  private_constant(:CONST)\nend\n\nA::CONST # invalid private reference\n";
    let mut server = with_server(Some(source), DEFAULT_URI, no_typechecker());
    assert_eq!(hover(&mut server, 5, 3), Value::Null);
}

fn rake_specification() -> Option<Value> {
    let output = std::process::Command::new("ruby")
        .args([
            "-rjson",
            "-e",
            "spec = Gem::Specification.find_by_name('rake'); print JSON.generate(name: spec.name, version: spec.version.to_s, homepage: spec.homepage)",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    serde_json::from_slice(&output.stdout).ok()
}

#[test]
fn hovering_over_gemfile_dependency_name() {
    let Some(specification) = rake_specification() else { return };
    let mut server = with_server(Some("gem 'rake'\n"), "file:///Gemfile", TestOptions::default());
    let contents = hover_contents(&mut server, 0, 5);
    for key in ["name", "version", "homepage"] {
        assert_contains(&contents, specification[key].as_str().unwrap());
    }
}

#[test]
fn hovering_over_gemfile_dependency_triggers_only_for_first_arg() {
    let mut server = with_server(Some("gem 'rake', '~> 1.0'\n"), "file:///Gemfile", TestOptions::default());
    assert_eq!(hover(&mut server, 0, 13), Value::Null);
}

#[test]
fn hovering_over_gemfile_dependency_with_missing_argument() {
    let mut server = with_server(Some("gem()\n"), "file:///Gemfile", TestOptions::default());
    assert_eq!(hover(&mut server, 0, 0), Value::Null);
}

#[test]
fn hovering_over_gemfile_dependency_with_non_gem_argument() {
    let mut server = with_server(Some("gem(method_call)\n"), "file:///Gemfile", TestOptions::default());
    assert_eq!(hover(&mut server, 0, 0), Value::Null);
}

#[test]
fn hover_precision_for_methods_with_block_arguments() {
    let source = "class Foo\n  # Hello\n  def foo(&block); end\n\n  def bar\n    foo(&:argument)\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    assert_eq!(hover(&mut server, 5, 12), Value::Null);
    assert_contains(&hover_contents(&mut server, 5, 4), "Hello");
}

#[test]
fn hover_instance_variables() {
    let source = "class Foo\n  def initialize\n    # Hello\n    @a = 1\n  end\n\n  def bar\n    @a\n  end\n\n  def baz\n    @a = 5\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    assert_contains(&hover_contents(&mut server, 7, 4), "Hello");
    assert_contains(&hover_contents(&mut server, 11, 4), "Hello");
}

#[test]
fn hovering_for_class_variables() {
    let source = "class Foo\n  def foo\n    # or write node\n    @@a ||= 1\n  end\n\n  def bar\n    # operator write node\n    @@a += 5\n  end\n\n  def baz\n    @@a\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    let contents = hover_contents(&mut server, 12, 6);
    assert_contains(&contents, "or write node");
    assert_contains(&contents, "operator write node");
}

#[test]
fn hovering_for_inherited_class_variables() {
    let source = "module Foo\n  def set_variable\n    # Foo\n    @@bar = 1\n  end\nend\n\nclass Parent\n  def set_variable\n    # Parent\n    @@bar = 5\n  end\nend\n\nclass Child < Parent\n  include Foo\n\n  def do_something\n    @@bar\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    let contents = hover_contents(&mut server, 18, 4);
    assert_contains(&contents, "Foo");
    assert_contains(&contents, "Parent");
}

#[test]
fn hovering_for_class_variables_in_different_context() {
    let source = "class Foo\n  # comment 1\n  @@a = 1\n\n  class << self\n    # comment 2\n    @@a = 2\n\n    def foo\n      # comment 3\n      @@a = 3\n    end\n  end\n\n  def bar\n    # comment 4\n    @@a = 4\n  end\n\n  def self.baz\n    # comment 5\n    @@a = 5\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    let contents = hover_contents(&mut server, 2, 4);
    for comment in ["comment 1", "comment 2", "comment 3", "comment 4", "comment 5"] {
        assert_contains(&contents, comment);
    }
}

#[test]
fn hovering_over_inherited_methods() {
    let source = "module Foo\n  module First\n    # Method 1\n    def method1; end\n  end\n\n  class Bar\n    # Method 2\n    def method2; end\n  end\n\n  class Baz < Bar\n    include First\n\n    def method3\n      method1\n      method2\n    end\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    assert_contains(&hover_contents(&mut server, 15, 6), "Method 1");
    assert_contains(&hover_contents(&mut server, 16, 6), "Method 2");
}

#[test]
fn hover_for_inherited_instance_variables() {
    let source = "module Foo\n  def set_ivar\n    # Foo\n    @a = 1\n  end\nend\n\nclass Parent\n  def initialize\n    # Parent\n    @a = 5\n  end\nend\n\nclass Child < Parent\n  include Foo\n\n  def do_something\n    @a\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    let contents = hover_contents(&mut server, 18, 4);
    assert_contains(&contents, "Foo");
    assert_contains(&contents, "Parent");
}

#[test]
fn hover_for_methods_shows_parameters() {
    let source = "class Foo\n  def bar(a, b = 1, *c, d:, e: 1, **f, &g)\n  end\n\n  def baz\n    bar\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    assert_contains(&hover_contents(&mut server, 5, 4), "bar(a, b = <default>, *c, d:, e: <default>, **f, &g)");
}

#[test]
fn hover_for_methods_shows_overload_count() {
    let mut server = with_server(Some("String.try_convert\n"), DEFAULT_URI, TestOptions::default());
    if !index_ruby_core(&server) {
        return;
    }
    let contents = hover_contents(&mut server, 0, 8);
    assert_contains(&contents, "try_convert(object)");
    assert_contains(&contents, "(+2 overloads)");
}

#[test]
fn hover_for_singleton_methods() {
    let source = "class Foo\n  # bar\n  def self.bar\n  end\n\n  class << self\n    # baz\n    def baz; end\n  end\nend\n\nFoo.bar\nFoo.baz\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    assert_contains(&hover_contents(&mut server, 11, 4), "bar");
    assert_contains(&hover_contents(&mut server, 12, 4), "baz");
}

#[test]
fn definition_for_class_instance_variables() {
    let source = "class Foo\n  # Hey!\n  @a = 123\n\n  def self.bar\n    @a\n  end\n\n  class << self\n    def baz\n      @a\n    end\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    assert_contains(&hover_contents(&mut server, 5, 4), "Hey!");
    assert_contains(&hover_contents(&mut server, 10, 6), "Hey!");
}

#[test]
fn hover_for_aliased_methods() {
    let source = "class Parent\n  # Original\n  def bar; end\nend\n\nclass Child < Parent\n  # Alias\n  alias baz bar\n\n  def do_something\n    baz\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    let contents = hover_contents(&mut server, 10, 4);
    assert_contains(&contents, "Alias");
    assert_contains(&contents, "Original");
}

#[test]
fn hover_for_super_calls() {
    let source = "class Parent\n  # Foo\n  def foo; end\n  # Bar\n  def bar; end\nend\n\nclass Child < Parent\n  def foo(a)\n    super()\n  end\n\n  def bar\n    super\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    assert_contains(&hover_contents(&mut server, 9, 4), "Foo");
    assert_contains(&hover_contents(&mut server, 13, 4), "Bar");
}

#[test]
fn hover_is_disabled_for_self_methods_on_typed_true() {
    let source = "# typed: true\nclass Child\n  def foo\n    bar\n  end\n\n  # Hey!\n  def bar\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    assert_eq!(hover(&mut server, 3, 4), Value::Null);
}

#[test]
fn hover_is_disabled_for_instance_variables_on_typed_strict() {
    let source = "# typed: strict\nclass Child\n  def initialize\n    # Hello\n    @something = 123 #: Integer\n  end\n\n  def bar\n    @something\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    assert_eq!(hover(&mut server, 8, 4), Value::Null);
}

#[test]
fn hover_is_disabled_on_super_for_typed_true() {
    let source = "# typed: true\nclass Parent\n  def foo; end\nend\nclass Child < Parent\n  def foo\n    super\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    assert_eq!(hover(&mut server, 6, 4), Value::Null);
}

#[test]
fn hover_for_guessed_receivers() {
    let source = "class User\n  def name; end\nend\n\nuser.name\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    let contents = hover_contents(&mut server, 4, 5);
    assert_contains(&contents, "Guessed receiver: User");
    assert_contains(&contents, "Learn more about guessed types");
}

#[test]
fn hover_for_keywords() {
    let cases = [("yield", "def foo\n  yield\nend\n"), ("break", "while true\n  break\nend\n")];
    for (keyword, source) in cases {
        let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
        let contents = hover_contents(&mut server, 1, 2);
        assert_contains(&contents, &format!("```ruby\n{keyword}\n```"));
        assert_contains(&contents, ruby_lsp_rs::requests::support::static_docs::keyword_documentation(keyword).unwrap());
        let expected_uri = ruby_lsp_rs::uri::from_path_buf(
            &ruby_lsp_rs::requests::support::static_docs::static_docs_path().join(format!("{keyword}.md")),
        );
        assert_contains(&contents, &format!("[Read more]({expected_uri})"));
        assert!(std::path::Path::new(&ruby_lsp_rs::uri::to_path(&expected_uri).unwrap()).is_file());
    }
}

#[test]
fn hover_call_node_precision() {
    let source = "class Foo\n  def message\n    \"hello!\"\n  end\nend\n\nclass Bar\n  def with_foo(foo)\n    @foo_message = foo.message\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    assert_eq!(hover(&mut server, 8, 19), Value::Null);
    assert_ne!(hover(&mut server, 8, 23), Value::Null);
}

#[test]
fn hovering_constants_shows_complete_name() {
    let source = "# typed: ignore\nmodule Foo\n  CONST = 123\n\n  module Bar\n    class Baz; end\n\n    Baz\n  end\nend\n\nQUX = 42\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    assert_contains(&hover_contents(&mut server, 7, 4), "```ruby\nFoo::Bar::Baz\n```");
    assert_contains(&hover_contents(&mut server, 2, 2), "```ruby\nFoo::CONST\n```");
    assert_contains(&hover_contents(&mut server, 11, 0), "```ruby\nQUX\n```");
}
