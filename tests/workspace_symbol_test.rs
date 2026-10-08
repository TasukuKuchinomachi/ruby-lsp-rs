mod support;

use serde_json::{Value, json};
use support::{TestOptions, TestServer, ruby_config, with_server};

const CLASS: u64 = 5;
const NAMESPACE: u64 = 3;
const METHOD: u64 = 6;
const PROPERTY: u64 = 7;
const CONSTRUCTOR: u64 = 9;
const CONSTANT: u64 = 14;

fn server() -> TestServer {
    with_server(None, support::DEFAULT_URI, TestOptions { stub_no_typechecker: true, ..TestOptions::default() })
}

fn workspace_symbol(server: &mut TestServer, query: &str) -> Vec<Value> {
    let result = server.result("workspace/symbol", json!({ "query": query }));
    result.as_array().cloned().unwrap_or_else(|| panic!("Expected an array, got {result}"))
}

fn assert_first(server: &mut TestServer, query: &str, name: &str, kind: u64) {
    let result = workspace_symbol(server, query);
    let first = result.first().unwrap_or_else(|| panic!("No result for {query}"));
    assert_eq!(first["name"], name);
    assert_eq!(first["kind"], kind);
}

#[test]
fn returns_index_entries_based_on_query() {
    let mut server = server();
    server.index_source("file:///fake.rb", "class Foo; end\nmodule Bar; end\n\nCONSTANT = 1\n");

    assert_first(&mut server, "Foo", "Foo", CLASS);
    assert_first(&mut server, "Bar", "Bar", NAMESPACE);
    assert_first(&mut server, "CONST", "CONSTANT", CONSTANT);
}

#[test]
fn fuzzy_matches_symbols() {
    let mut server = server();
    server.index_source("file:///fake.rb", "class Foo; end\nmodule Bar; end\n\nCONSTANT = 1\n");

    assert_first(&mut server, "Floo", "Foo", CLASS);
    assert_first(&mut server, "Bear", "Bar", NAMESPACE);
    assert_first(&mut server, "CONF", "CONSTANT", CONSTANT);
}

#[test]
fn symbols_include_container_name() {
    let mut server = server();
    server.index_source("file:///fake.rb", "module Foo\n  class Bar; end\nend\n");

    let result = workspace_symbol(&mut server, "Foo::Bar");
    assert_eq!(result[0]["name"], "Foo::Bar");
    assert_eq!(result[0]["kind"], CLASS);
    assert_eq!(result[0]["containerName"], "Foo");
}

#[test]
fn does_not_include_symbols_from_dependencies() {
    let Some(rubylibdir) = ruby_config("rubylibdir") else {
        eprintln!("Skipping: Ruby is not available to locate rubylibdir");
        return;
    };
    let mut server = with_server(
        None,
        support::DEFAULT_URI,
        TestOptions { stub_no_typechecker: true, rubylibdir: Some(rubylibdir.clone()), ..TestOptions::default() },
    );
    server.index_file(&std::path::Path::new(&rubylibdir).join("pathname.rb"), None);

    assert!(workspace_symbol(&mut server, "Pathname").is_empty());
}

#[test]
fn does_not_include_private_constants() {
    let mut server = server();
    server.index_source("file:///fake.rb", "class Foo\n  CONSTANT = 1\n  private_constant(:CONSTANT)\nend\n");

    let result = workspace_symbol(&mut server, "Foo::CONSTANT");
    assert_eq!(result.len(), 1);
    assert_eq!(result[0]["name"], "Foo");
}

#[test]
fn returns_method_symbols() {
    let mut server = server();
    server.index_source(
        "file:///fake.rb",
        "class Foo\n  attr_reader :baz\n\n  def initialize; end\n  def bar; end\nend\n",
    );

    assert_first(&mut server, "bar", "bar", METHOD);
    assert_first(&mut server, "initialize", "initialize", CONSTRUCTOR);
    assert_first(&mut server, "baz", "baz", PROPERTY);
}

#[test]
fn returns_symbols_from_unsaved_files() {
    let mut server = server();
    server.index_source("untitled:Untitled-1", "class Foo; end\n");

    assert_first(&mut server, "Foo", "Foo", CLASS);
}

#[test]
fn returns_location_of_entries() {
    let mut server = server();
    server.index_source("file:///fake.rb", "module Foo\n  class Bar; end\nend\n");

    let result = workspace_symbol(&mut server, "Foo::Bar");
    assert_eq!(
        result[0]["location"],
        json!({ "uri": "file:///fake.rb", "range": support::range_json(1, 2, 1, 16) })
    );
}
