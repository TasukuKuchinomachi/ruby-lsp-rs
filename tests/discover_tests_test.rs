mod support;

use serde_json::{Value, json};
use support::{DEFAULT_URI, TestOptions, TestServer, fixture_path, with_server};

const MINITEST_STUB: &str = "module Minitest\n  class Test; end\nend\n";
const TEST_UNIT_STUB: &str = "module Test\n  module Unit\n    class TestCase; end\n  end\nend\n";
const MINITEST_SPEC_STUB: &str = "module Minitest\n  class Test; end\n  class Spec < Test; end\nend\n";
const ACTIVE_SUPPORT_DECLARATIVE_STUB: &str = "module Minitest
  class Test; end
end

module ActiveSupport
  module Testing
    module Declarative
    end
  end

  class TestCase < Minitest::Test
    extend Testing::Declarative
  end
end
";

fn fixture(name: &str) -> String {
    std::fs::read_to_string(fixture_path(name)).unwrap()
}

fn discover_tests(server: &mut TestServer, uri: &str) -> Vec<Value> {
    let result = server.result("rubyLsp/discoverTests", json!({ "textDocument": { "uri": uri } }));
    result.as_array().cloned().unwrap_or_else(|| panic!("Expected an array, got {result}"))
}

fn discover_with_stub(source: &str, stub: &str) -> Vec<Value> {
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    server.index_source(DEFAULT_URI, stub);
    discover_tests(&mut server, DEFAULT_URI)
}

fn with_minitest_test(source: &str) -> Vec<Value> {
    discover_with_stub(source, MINITEST_STUB)
}

fn with_test_unit(source: &str) -> Vec<Value> {
    discover_with_stub(source, TEST_UNIT_STUB)
}

fn with_minitest_spec_configured(source: &str) -> Vec<Value> {
    discover_with_stub(source, MINITEST_SPEC_STUB)
}

fn values(items: &[Value], key: &str) -> Vec<String> {
    items.iter().map(|item| item[key].as_str().unwrap().to_string()).collect()
}

fn labels(items: &[Value]) -> Vec<String> {
    values(items, "label")
}

fn ids(items: &[Value]) -> Vec<String> {
    values(items, "id")
}

fn children(item: &Value) -> Vec<Value> {
    item["children"].as_array().cloned().unwrap()
}

fn assert_all_items_tagged_with(items: &[Value], tag: &str) {
    for item in items {
        let tags: Vec<&str> = item["tags"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
        assert!(tags.contains(&format!("framework:{tag}").as_str()), "{item} is not tagged with {tag}");
        assert_all_items_tagged_with(&children(item), tag);
    }
}

#[test]
fn minitest() {
    let items = with_minitest_test(&fixture("minitest_tests.rb"));
    assert_eq!(labels(&items), ["Test", "AnotherTest"]);
    assert_eq!(
        labels(&children(&items[0])),
        ["test_public", "test_public_command", "test_another_public", "test_public_vcall", "test_with_q?"]
    );
    assert_eq!(labels(&children(&items[1])), ["test_public", "test_public_2"]);
    assert_all_items_tagged_with(&items, "minitest");
}

#[test]
fn minitest_with_nested_classes_and_modules() {
    let items = with_minitest_test(&fixture("minitest_nested_classes_and_modules.rb"));
    assert_eq!(
        labels(&items),
        ["Foo::FooTest", "Foo::Bar::BarTest", "Foo::Baz::BazTest", "Foo::Bar::FooBarTest", "Foo::Bar::FooBar::Test"]
    );
    assert_eq!(labels(&children(&items[0])), ["test_foo", "test_foo_2"]);
    assert_eq!(labels(&children(&items[1])), ["test_bar", "Foo::Bar::BarTest::Baz::BazTest"]);
    assert_eq!(labels(&children(&children(&items[1])[1])), ["test_baz", "test_baz_2"]);
    assert_eq!(labels(&children(&items[2])), ["test_baz"]);
    assert_eq!(labels(&children(&items[3])), ["test_foo_bar", "test_foo_bar_2"]);
    assert_eq!(labels(&children(&items[4])), ["test_foo_bar_baz"]);
    assert_all_items_tagged_with(&items, "minitest");
}

#[test]
fn complex_nested_example() {
    let source = "module First
  class MyTest < Minitest::Test
    def test_something; end

    class Random
      class NestedTest < Minitest::Test
        def test_something_else; end
      end
    end
  end
end
";
    let items = with_minitest_test(source);
    assert_eq!(ids(&items), ["First::MyTest"]);
    assert_eq!(ids(&children(&items[0])), ["First::MyTest#test_something", "First::MyTest::Random::NestedTest"]);
    assert_eq!(
        ids(&children(&children(&items[0])[1])),
        ["First::MyTest::Random::NestedTest#test_something_else"]
    );
    assert_all_items_tagged_with(&items, "minitest");
}

#[test]
fn minitest_with_dynamic_constant_path() {
    let items = with_minitest_test(&fixture("minitest_with_dynamic_constant_path.rb"));
    assert_eq!(labels(&items), ["<dynamic_reference>::Baz::Test", "<dynamic_reference>::Baz::SomeOtherTest"]);
    assert_eq!(
        labels(&children(&items[0])),
        ["test_something", "test_something_else", "<dynamic_reference>::Baz::Test::NestedTest"]
    );
    assert_eq!(labels(&children(&children(&items[0])[2])), ["test_nested"]);
    assert_eq!(labels(&children(&items[1])), ["test_stuff", "test_other_stuff"]);
    assert_all_items_tagged_with(&items, "minitest");
}

#[test]
fn test_unit_cases() {
    let source = "module Foo
  class MyTest < Test::Unit::TestCase
    def test_something; end

    private

    def test_should_not_be_included; end

    class NestedTest < Test::Unit::TestCase
      def test_hello; end
    end
  end
end
";
    let items = with_test_unit(source);
    assert_eq!(labels(&items), ["Foo::MyTest"]);
    assert_eq!(labels(&children(&items[0])), ["test_something", "Foo::MyTest::NestedTest"]);
    assert_eq!(labels(&children(&children(&items[0])[1])), ["test_hello"]);
    assert_all_items_tagged_with(&items, "test_unit");
}

const CODE_LENS_SOURCE: &str = "module Foo
  class MyTest < Test::Unit::TestCase
    def test_something; end

    def test_something_else; end
  end
end
";

fn code_lens_with_full_test_discovery(path: &str, initialization_options: Value) -> Vec<Value> {
    let uri = ruby_lsp_rs::uri::from_path(path);
    let mut server = with_server(
        Some(CODE_LENS_SOURCE),
        &uri,
        TestOptions { initialization_options: Some(initialization_options), ..TestOptions::default() },
    );
    server.index_source("file:///other_file.rb", TEST_UNIT_STUB);
    let result = server.result("textDocument/codeLens", json!({ "textDocument": { "uri": uri } }));
    result.as_array().cloned().unwrap()
}

#[test]
fn collects_code_lenses() {
    let items = code_lens_with_full_test_discovery(
        "/test/foo_test.rb",
        json!({ "initializationOptions": { "enabledFeatureFlags": { "all": true } } }),
    );
    assert_eq!(items.len(), 9);

    let expected = [
        ("run_test", 1, 2),
        ("run_test_in_terminal", 1, 2),
        ("debug_test", 1, 2),
        ("run_test", 2, 4),
        ("run_test_in_terminal", 2, 4),
        ("debug_test", 2, 4),
        ("run_test", 4, 4),
        ("run_test_in_terminal", 4, 4),
        ("debug_test", 4, 4),
    ];
    for (item, (kind, line, character)) in items.iter().zip(expected) {
        assert_eq!(item["data"]["kind"], kind);
        assert_eq!(item["range"], support::range_json(line, character, line, character + 1));
    }
}

#[test]
fn does_not_collect_code_lenses_when_disabled() {
    let items = code_lens_with_full_test_discovery(
        "/test/foo_test.rb",
        json!({
            "initializationOptions": {
                "enabledFeatureFlags": { "all": true },
                "featuresConfiguration": { "codeLens": { "enableTestCodeLens": false } },
            },
        }),
    );
    assert!(items.is_empty());
}

#[test]
fn does_not_collect_code_lenses_for_files_not_matching_path_convention() {
    let items = code_lens_with_full_test_discovery(
        "/tests/something.rb",
        json!({ "initializationOptions": { "enabledFeatureFlags": { "all": true } } }),
    );
    assert!(items.is_empty());
}

#[test]
fn ignores_minitest_tests_that_extend_active_support_declarative() {
    let source = "class MyTest < ActiveSupport::TestCase\n  def test_something; end\nend\n";
    let items = discover_with_stub(source, ACTIVE_SUPPORT_DECLARATIVE_STUB);
    assert!(items.is_empty());
}

#[test]
fn dynamic_constant_in_minitest_tests() {
    let source = "module var::Namespace
  class MyTest < Minitest::Test
    def test_something; end
    def do_something; end
  end
end

module Foo
  class var::OtherTest < Minitest::Test
    def test_something; end
  end
end
";
    let items = with_minitest_test(source);
    assert_eq!(labels(&items), ["<dynamic_reference>::Namespace::MyTest", "Foo::<dynamic_reference>::OtherTest"]);
    assert_eq!(labels(&children(&items[0])), ["test_something"]);
    assert_eq!(labels(&children(&items[1])), ["test_something"]);
    assert_all_items_tagged_with(&items, "minitest");
}

#[test]
fn dynamic_constant_in_test_unit_tests() {
    let source = "module var::Namespace
  class MyTest < Test::Unit::TestCase
    def test_something; end

    def do_something; end
  end
end

module Foo
  class var::OtherTest < Test::Unit::TestCase
    def test_something; end
  end
end
";
    let items = with_minitest_test(source);
    assert_eq!(labels(&items), ["<dynamic_reference>::Namespace::MyTest", "Foo::<dynamic_reference>::OtherTest"]);
    assert_eq!(labels(&children(&items[0])), ["test_something"]);
    assert_eq!(labels(&children(&items[1])), ["test_something"]);
    assert_all_items_tagged_with(&items, "test_unit");
}

#[test]
fn files_are_indexed_lazily_if_needed() {
    let directory = std::env::temp_dir().join(format!("ruby-lsp-rs-discover-tests-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("foo.rb");
    std::fs::write(&path, "class FooTest < Test::Unit::TestCase\n  def test_something; end\nend\n").unwrap();

    let mut server = with_server(None, DEFAULT_URI, TestOptions::default());
    server.index_source(DEFAULT_URI, TEST_UNIT_STUB);
    let items = discover_tests(&mut server, &ruby_lsp_rs::uri::from_path_buf(&path));
    std::fs::remove_dir_all(&directory).unwrap();

    assert_eq!(labels(&items), ["FooTest"]);
    assert_eq!(labels(&children(&items[0])), ["test_something"]);
    assert_all_items_tagged_with(&items, "test_unit");
}

#[test]
fn does_not_raise_on_duplicate_test_ids() {
    let source = "module Foo
  class MyTest < Test::Unit::TestCase
    def test_something; end

    # This one gets picked
    def test_something; end
  end
end
";
    let items = with_test_unit(source);
    assert_eq!(labels(&items), ["Foo::MyTest"]);
    let children = children(&items[0]);
    assert_eq!(children.len(), 1);
    assert_eq!(children[0]["range"]["start"]["line"], 5);
    assert_all_items_tagged_with(&items, "test_unit");
}

#[test]
fn discovers_top_level_specs() {
    let items = with_minitest_spec_configured(&fixture("minitest_spec_simple.rb"));
    assert_eq!(labels(&items), ["BogusSpec"]);
    assert_all_items_tagged_with(&items, "minitest");
}

#[test]
fn anonymous_examples() {
    let source = "class BogusSpec < Minitest::Spec\n  it do\n    assert true\n  end\nend\n";
    let items = with_minitest_spec_configured(source);
    assert_eq!(ids(&children(&items[0])), ["BogusSpec#test_0001_anonymous"]);
}

#[test]
fn discovers_nested_specs() {
    let items = with_minitest_spec_configured(&fixture("minitest_spec_nested.rb"));
    let top_level_specs = children(&items[0]);
    assert_eq!(ids(&top_level_specs), ["BogusSpec::First Spec"]);
    assert_eq!(
        ids(&children(&top_level_specs[0])),
        [
            "BogusSpec::First Spec#test_0004_test one",
            "BogusSpec::First Spec#test_0008_test two",
            "BogusSpec::First Spec#test_0012_test three",
        ]
    );
    assert_all_items_tagged_with(&items, "minitest");
}

#[test]
fn discovers_specs_without_class() {
    let items = with_minitest_spec_configured(&fixture("minitest_spec_tests.rb"));
    assert_eq!(ids(&items), ["Foo", "Foo::Bar", "Baz"]);
    assert_eq!(
        ids(&children(&items[0])),
        ["Foo#test_0001_it_level_one", "Foo::nested", "Foo#test_0013_it_level_one_again"]
    );
    assert_all_items_tagged_with(&items, "minitest");
}

#[test]
fn discovers_dynamic_spec_names() {
    let items = with_minitest_spec_configured(&fixture("minitest_spec_dynamic_name.rb"));
    assert_eq!(ids(&children(&children(&items[0])[0])), ["BogusSpec::First Spec#test_0004_dynamic_name"]);
    assert_all_items_tagged_with(&items, "minitest");
}

#[test]
fn handles_empty_specs() {
    let items = with_minitest_spec_configured(&fixture("minitest_spec_simple.rb"));
    assert!(children(&children(&items[0])[0]).is_empty());
    assert_all_items_tagged_with(&items, "minitest");
}

#[test]
fn handles_mixed_testing_styles_in_single_file() {
    let source = "class FooSpec < Minitest::Spec
  it \"does something\" do
  end

  def test_also_valid; end
end
";
    let items = with_minitest_spec_configured(source);
    assert_eq!(ids(&items), ["FooSpec"]);
    assert_eq!(ids(&children(&items[0])), ["FooSpec#test_0001_does something", "FooSpec#test_also_valid"]);
    assert_all_items_tagged_with(&items, "minitest");
}

#[test]
fn handles_specs_with_classes_inside() {
    let source = "class FooSpec < Minitest::Spec
  class Bar; end

  it \"does something\" do
  end
end

describe \"BazSpec\" do
  it \"does something\" do
  end
end
";
    let items = with_minitest_spec_configured(source);
    assert_eq!(ids(&items), ["FooSpec", "BazSpec"]);
    assert_eq!(ids(&children(&items[0])), ["FooSpec#test_0003_does something"]);
    assert_eq!(ids(&children(&items[1])), ["BazSpec#test_0008_does something"]);
    assert_all_items_tagged_with(&items, "minitest");
}

#[test]
fn complex_spec_case() {
    let source = "module First
  module Second
    module Third
      class MySpec < Minitest::Spec
        describe \"when something is true\" do
          describe \"and other thing is false\" do
            it \"does what's expected\" do
              assert(true)
            end
          end

          class NestedSpec < Minitest::Spec
            it \"does something else\" do
              assert(true)
            end
          end
        end
      end
    end
  end
end
";
    let items = with_minitest_spec_configured(source);
    let top_class = "First::Second::Third::MySpec";
    assert_eq!(ids(&items), [top_class]);
    assert_eq!(
        ids(&children(&items[0])),
        [format!("{top_class}::when something is true"), format!("{top_class}::NestedSpec")]
    );
    assert_eq!(
        ids(&children(&children(&items[0])[0])),
        [format!("{top_class}::when something is true::and other thing is false")]
    );
    assert_eq!(
        ids(&children(&children(&items[0])[1])),
        [format!("{top_class}::NestedSpec#test_0012_does something else")]
    );
    assert_eq!(
        ids(&children(&children(&children(&items[0])[0])[0])),
        [format!("{top_class}::when something is true::and other thing is false#test_0006_does what's expected")]
    );
    assert_all_items_tagged_with(&items, "minitest");
}

#[test]
fn ignores_methods_that_look_like_tests_in_other_namespaces() {
    let source = "class MyTest < Minitest::Test
  def test_something; end

  module Foo
    def test_something_else; end
  end

  class Bar
    def test_other_thing; end
  end
end
";
    let items = with_minitest_test(source);
    assert_eq!(ids(&items), ["MyTest"]);
    assert_eq!(ids(&children(&items[0])), ["MyTest#test_something"]);
}

#[test]
fn spec_ignores_methods_that_look_like_tests_in_other_namespaces() {
    let source = "class MySpec < Minitest::Spec
  it \"should do something\" do
  end

  module Foo
    def test_something_else; end

    it \"should do something else\" do
    end
  end

  class Bar
    def test_other_thing; end

    it \"another ignored call\" do
    end
  end
end
";
    let items = with_minitest_spec_configured(source);
    assert_eq!(ids(&items), ["MySpec"]);
    assert_eq!(ids(&children(&items[0])), ["MySpec#test_0001_should do something"]);
}

#[test]
fn spec_using_describe_ignores_methods_that_look_like_tests_in_other_namespaces() {
    let source = "describe \"MySpec\" do
  it \"should do something\" do
  end

  module Foo
    def test_something_else; end

    it \"should do something else\" do
    end
  end

  class Bar
    def test_other_thing; end

    it \"another ignored call\" do
    end
  end
end
";
    let items = with_minitest_spec_configured(source);
    assert_eq!(ids(&items), ["MySpec"]);
    assert_eq!(ids(&children(&items[0])), ["MySpec#test_0001_should do something"]);
}

#[test]
fn spec_using_describe_with_additional_descriptions() {
    let source = "describe(Foo, :extra_1, :extra_2) do
  describe(\"#some_method\") do
    it(\"does something\") do
      assert(true)
    end
  end
end
";
    let items = with_minitest_spec_configured(source);
    assert_eq!(ids(&items), ["Foo::extra_1::extra_2"]);
    assert_eq!(ids(&children(&items[0])), ["Foo::extra_1::extra_2::#some_method"]);
    assert_eq!(
        ids(&children(&children(&items[0])[0])),
        ["Foo::extra_1::extra_2::#some_method#test_0002_does something"]
    );
}

#[test]
fn can_discover_tests_even_if_parent_class_was_not_indexed() {
    let source = "class MyTest < Minitest::Test\n  def test_something; end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    let items = discover_tests(&mut server, DEFAULT_URI);
    assert_eq!(ids(&items), ["MyTest"]);
    assert_eq!(ids(&children(&items[0])), ["MyTest#test_something"]);
    assert_all_items_tagged_with(&items, "minitest");
}

#[test]
fn specs_defined_inside_modules() {
    let source = "module MyNamespace
  describe \"this thing\" do
    it \"does something\" do
    end
  end

  module OtherNamespace
    describe \"other test\" do
      it \"does something else\" do
      end
    end
  end
end
";
    let items = with_minitest_spec_configured(source);
    assert_eq!(ids(&items), ["this thing", "other test"]);
    assert_eq!(ids(&children(&items[0])), ["this thing#test_0002_does something"]);
    assert_eq!(ids(&children(&items[1])), ["other test#test_0008_does something else"]);
}
