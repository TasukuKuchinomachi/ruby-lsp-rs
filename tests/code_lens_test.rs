mod support;
mod test_discovery_support;

use serde_json::{Value, json};
use test_discovery_support::bundled_server;

fn code_lens(source: &str, uri: &str, test_library: &str) -> Vec<Value> {
    let mut server = bundled_server(Some(source), uri, test_library, json!({ "experimentalFeaturesEnabled": true }));
    let result = server.result("textDocument/codeLens", json!({ "textDocument": { "uri": uri } }));
    result.as_array().cloned().unwrap_or_else(|| panic!("Expected an array, got {result}"))
}

fn title(code_lens: &Value) -> &str {
    code_lens["command"]["title"].as_str().unwrap()
}

fn command(code_lens: &Value) -> &str {
    code_lens["command"]["arguments"][2].as_str().unwrap()
}

#[test]
fn command_generation_for_minitest() {
    let source = "class FooTest < MiniTest::Test\n  def test_bar; end\nend\n";
    let response = code_lens(source, "file:///test/fake.rb", "minitest");

    assert_eq!(response.len(), 6);
    assert_eq!(title(&response[1]), "▶ Run In Terminal");
    assert_eq!(command(&response[1]), "bundle exec ruby -Itest /test/fake.rb --name \"/^FooTest(#|::)/\"");
    assert_eq!(title(&response[4]), "▶ Run In Terminal");
    assert_eq!(command(&response[4]), "bundle exec ruby -Itest /test/fake.rb --name FooTest#test_bar");
}

#[test]
fn command_generation_for_minitest_spec() {
    let source = "class FooTest < MiniTest::Spec\n  describe \"a\" do\n    it \"b\"\n  end\nend\n";
    let response = code_lens(source, "file:///spec/fake.rb", "minitest");

    assert_eq!(response.len(), 9);
    assert_eq!(title(&response[1]), "▶ Run In Terminal");
    assert_eq!(command(&response[1]), "bundle exec ruby -Ispec /spec/fake.rb --name \"/^FooTest(#|::)/\"");
    assert_eq!(title(&response[4]), "▶ Run In Terminal");
    assert_eq!(command(&response[4]), "bundle exec ruby -Ispec /spec/fake.rb --name \"/^FooTest::a(#|::)/\"");
    assert_eq!(title(&response[7]), "▶ Run In Terminal");
    assert_eq!(command(&response[7]), "bundle exec ruby -Ispec /spec/fake.rb --name \"/^FooTest::a#test_0001_b$/\"");
}

#[test]
fn command_generation_for_minitest_spec_handles_specify_alias_for_it() {
    let source = "describe \"a\" do\n  specify \"b\"\nend\n";
    let response = code_lens(source, "file:///spec/fake.rb", "minitest");

    assert_eq!(response.len(), 6);
}

#[test]
fn command_generation_for_test_unit() {
    let source = "class FooTest < Test::Unit::TestCase\n  def test_bar; end\nend\n";
    let response = code_lens(source, "file:///test/fake.rb", "test-unit");

    assert_eq!(response.len(), 6);
    assert_eq!(title(&response[1]), "▶ Run In Terminal");
    assert_eq!(command(&response[1]), "bundle exec ruby -Itest /test/fake.rb --testcase /FooTest/");
    assert_eq!(title(&response[4]), "▶ Run In Terminal");
    assert_eq!(command(&response[4]), "bundle exec ruby -Itest /test/fake.rb --testcase /FooTest/ --name test_bar");
}

#[test]
fn no_code_lens_for_unknown_test_framework() {
    let source = "class FooTest < Test::Unit::TestCase\n  def test_bar; end\nend\n";
    assert!(code_lens(source, "file:///fake.rb", "unknown").is_empty());
}

#[test]
fn no_code_lens_for_rspec() {
    let source = "class FooTest < Test::Unit::TestCase\n  def test_bar; end\nend\n";
    assert!(code_lens(source, "file:///fake.rb", "rspec").is_empty());
}

#[test]
fn no_code_lens_for_unsaved_files() {
    let source = "class FooTest < Test::Unit::TestCase\n  def test_bar; end\nend\n";
    assert!(code_lens(source, "untitled:Untitled-1", "minitest").is_empty());
}

#[test]
fn no_code_lens_for_unsaved_specs() {
    let source = "describe FooTest < Minitest::Spec\n  it \"does something\"; end\nend\n";
    assert!(code_lens(source, "untitled:Untitled-1", "minitest").is_empty());
}

#[test]
fn no_code_lens_for_nested_defs() {
    let source = "class FooTest < Test::Unit::TestCase\n  def test_bar\n    def test_baz; end\n  end\nend\n";
    assert_eq!(code_lens(source, "file:///fake.rb", "test-unit").len(), 6);
}
