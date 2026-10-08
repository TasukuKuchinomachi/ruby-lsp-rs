mod support;

use serde_json::{Value, json};
use support::{DEFAULT_URI, TestOptions, with_server};

fn document_symbols(source: &str) -> Vec<Value> {
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions { stub_no_typechecker: true, ..TestOptions::default() });
    let result = server.result("textDocument/documentSymbol", json!({ "textDocument": { "uri": DEFAULT_URI } }));
    result.as_array().cloned().unwrap_or_else(|| panic!("Expected an array, got {result}"))
}

fn names(symbols: &[Value]) -> Vec<&str> {
    symbols.iter().map(|symbol| symbol["name"].as_str().unwrap()).collect()
}

#[test]
fn instance_variable_with_shorthand_assignment() {
    let response = document_symbols("@foo = 1\n@bar += 2\n@baz -= 3\n@qux ||= 4\n@quux &&= 5\n");
    assert_eq!(names(&response), ["@foo", "@bar", "@baz", "@qux", "@quux"]);
}

#[test]
fn instance_variable_with_destructuring_assignment() {
    let response = document_symbols("@a, @b = [1, 2]\n@c, @d, @e = [3, 4, 5]\n");
    assert_eq!(names(&response), ["@a", "@b", "@c", "@d", "@e"]);
}

#[test]
fn labels_blank_names() {
    let response = document_symbols("def\n");
    assert_eq!(names(&response), ["<blank>"]);
}

#[test]
fn nests_symbols_inside_namespaces() {
    let response = document_symbols("class Foo\n  def bar; end\n\n  class << self\n    def baz; end\n  end\nend\n");
    assert_eq!(names(&response), ["Foo"]);
    let children = response[0]["children"].as_array().unwrap();
    assert_eq!(names(children), ["bar", "<< self"]);
    let singleton_children = children[1]["children"].as_array().unwrap();
    assert_eq!(names(singleton_children), ["baz"]);
    assert_eq!(singleton_children[0]["kind"], 12);
}

#[test]
fn returns_null_for_non_ruby_documents() {
    let uri = "file:///fake.rbs";
    let mut server = with_server(None, uri, TestOptions { stub_no_typechecker: true, ..TestOptions::default() });
    server.did_open(uri, "class Foo\nend\n", "rbs");
    let result = server.result("textDocument/documentSymbol", json!({ "textDocument": { "uri": uri } }));
    assert_eq!(result, Value::Null);
}
