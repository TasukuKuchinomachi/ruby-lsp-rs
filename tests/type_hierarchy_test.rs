mod support;

use serde_json::{Value, json};
use support::{DEFAULT_URI, TestOptions, TestServer, with_server};

fn prepare_type_hierarchy(source: &str, line: u32, character: u32) -> Value {
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    server.result(
        "textDocument/prepareTypeHierarchy",
        json!({ "textDocument": { "uri": DEFAULT_URI }, "position": { "line": line, "character": character } }),
    )
}

fn supertypes(server: &mut TestServer, name: &str) -> Value {
    server.result("typeHierarchy/supertypes", json!({ "textDocument": { "uri": DEFAULT_URI }, "item": { "name": name } }))
}

fn names(result: &Value) -> Vec<String> {
    result.as_array().unwrap().iter().map(|item| item["name"].as_str().unwrap().to_string()).collect()
}

#[test]
fn prepare_type_hierarchy_returns_nil_if_no_node_at_position() {
    assert_eq!(prepare_type_hierarchy("class Foo; end\n", 0, 1), Value::Null);
}

#[test]
fn prepare_type_hierarchy_returns_constant_path_name() {
    let result = prepare_type_hierarchy("class Foo::Bar; end\n", 0, 12);
    assert_eq!(result[0]["name"], "Foo::Bar");
}

#[test]
fn prepare_type_hierarchy_returns_nil_if_constant_not_indexed() {
    assert_eq!(prepare_type_hierarchy("puts Bar\n", 0, 6), Value::Null);
}

#[test]
fn prepare_type_hierarchy_returns_constant_name_if_indexed() {
    let result = prepare_type_hierarchy("class Bar; end\nputs Bar\n", 1, 6);
    assert_eq!(result[0]["name"], "Bar");
}

#[test]
fn prepare_type_hierarchy_only_returns_the_first_entry() {
    let result = prepare_type_hierarchy("class Bar; end\nclass Bar; end\nputs Bar\n", 2, 6);
    assert_eq!(names(&result), ["Bar"]);
}

#[test]
fn prepare_type_hierarchy_item_shape() {
    let result = prepare_type_hierarchy("class Bar; end\nputs Bar\n", 1, 6);
    assert_eq!(
        result,
        json!([{
            "name": "Bar",
            "kind": 5,
            "uri": DEFAULT_URI,
            "range": support::range_json(0, 0, 0, 14),
            "selectionRange": support::range_json(0, 0, 0, 14),
        }])
    );
}

#[test]
fn type_hierarchy_supertypes_returns_nil_if_item_name_not_indexed() {
    let mut server = with_server(Some("class Foo; end\n"), DEFAULT_URI, TestOptions::default());
    assert_eq!(supertypes(&mut server, "Bar"), Value::Null);
}

#[test]
fn type_hierarchy_supertypes_returns_empty_array_if_no_supertypes() {
    let mut server = with_server(Some("class Foo::Bar; end\n"), DEFAULT_URI, TestOptions::default());
    assert_eq!(supertypes(&mut server, "Foo::Bar"), json!([]));
}

#[test]
fn type_hierarchy_returns_supertypes() {
    let source = "module Foo\n  class Bar; end\n  class Baz < Bar; end\n  class Qux < Baz; end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());

    assert_eq!(names(&supertypes(&mut server, "Foo::Qux")), ["Foo::Baz"]);
    assert_eq!(names(&supertypes(&mut server, "Foo::Baz")), ["Foo::Bar"]);
    assert_eq!(supertypes(&mut server, "Foo::Bar"), json!([]));
}

#[test]
fn type_hierarchy_supertypes_include_mixins() {
    let source = "module Mixin; end\nclass Parent; end\nclass Child < Parent\n  include Mixin\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    let result = supertypes(&mut server, "Child");

    assert_eq!(names(&result), ["Parent", "Mixin"]);
    assert_eq!(result[0]["detail"], "fake.rb");
    assert_eq!(result[1]["kind"], 3);
}
