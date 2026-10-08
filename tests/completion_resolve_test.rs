mod support;

use ruby_lsp_rs::indexer::rbs_indexer::RbsIndexer;
use ruby_lsp_rs::requests::support::common::markdown_from_index_entries;
use ruby_lsp_rs::requests::support::static_docs::{keyword_documentation, static_docs_path};
use serde_json::{Value, json};
use support::{DEFAULT_URI, TestOptions, TestServer, rbs_core_path, with_server};

fn no_typechecker() -> TestOptions {
    TestOptions { stub_no_typechecker: true, ..TestOptions::default() }
}

fn resolve(server: &mut TestServer, item: Value) -> Value {
    server.result("completionItem/resolve", item)
}

fn documentation(result: &Value) -> String {
    result["documentation"]["value"].as_str().unwrap_or_else(|| panic!("Expected documentation, got {result}")).to_string()
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
fn completion_resolve_for_constant() {
    let source = "module Foo\n  # This is a class that does things\n  class Bar\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, no_typechecker());
    let existing_item = json!({ "label": "Foo::Bar", "insertText": "Bar" });
    let result = resolve(&mut server, existing_item.clone());

    let expected_markdown = {
        let mut index = server.index();
        let entries = index.get_owned("Foo::Bar").unwrap();
        markdown_from_index_entries(&mut index, "Foo::Bar", &entries, None, None)
    };
    let mut expected = existing_item;
    expected["documentation"] = json!({ "kind": "markdown", "value": expected_markdown });

    assert_contains(&documentation(&result), "This is a class that does things");
    assert_eq!(result, expected);
    assert!(result.get("labelDetails").is_none());
}

#[test]
fn completion_resolve_with_owner_present() {
    let source = "class Bar\n  def initialize\n    # Bar!\n    @a = 1\n  end\nend\n\nclass Foo\n  def initialize\n    # Foo!\n    @a = 1\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, no_typechecker());

    let result = resolve(&mut server, json!({ "label": "@a", "kind": 5, "data": { "owner_name": "Foo" } }));
    assert_contains(&documentation(&result), "Foo");

    let result = resolve(&mut server, json!({ "label": "@a", "kind": 5, "data": { "owner_name": "Bar" } }));
    assert_contains(&documentation(&result), "Bar");
}

#[test]
fn inserts_method_parameters_in_label_details() {
    let source = "class Bar\n  def foo(a, b, c)\n  end\n\n  def bar\n    f\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, no_typechecker());
    let result = resolve(&mut server, json!({ "label": "foo", "kind": 2, "data": { "owner_name": "Bar" } }));
    assert_contains(&documentation(&result), "(a, b, c)");
}

#[test]
fn indicates_signature_count_in_label_details() {
    let mut server = with_server(Some("String.try_convert\n"), DEFAULT_URI, no_typechecker());
    if !index_ruby_core(&server) {
        return;
    }
    let result = resolve(
        &mut server,
        json!({ "label": "try_convert", "kind": 2, "data": { "owner_name": "String::<Class:String>" } }),
    );
    let contents = documentation(&result);
    assert_contains(&contents, "try_convert(object)");
    assert_contains(&contents, "(+2 overloads)");
}

#[test]
fn resolve_handles_method_aliases() {
    let mut server = with_server(Some(""), DEFAULT_URI, no_typechecker());
    if !index_ruby_core(&server) {
        return;
    }
    server.index().resolve_method("kind_of?", "Kernel", false);

    let result = resolve(&mut server, json!({ "label": "kind_of?", "kind": 2, "data": { "owner_name": "Kernel" } }));
    assert_contains(&documentation(&result), "**Definitions**: [kernel.rbs]");
}

#[test]
fn completion_documentation_for_guessed_types() {
    let source = "class User\n  def foo(a, b, c)\n  end\nend\n\nuser.f\n";
    let mut server = with_server(Some(source), DEFAULT_URI, no_typechecker());
    let result = resolve(
        &mut server,
        json!({ "label": "foo", "kind": 2, "data": { "owner_name": "User", "guessed_type": "User" } }),
    );
    let contents = documentation(&result);
    assert_contains(&contents, "Guessed receiver: User");
    assert_contains(&contents, "Learn more about guessed types");
}

#[test]
fn resolve_for_keywords() {
    let mut server = with_server(Some("def foo\n  yield\nend\n"), DEFAULT_URI, no_typechecker());
    let result = resolve(&mut server, json!({ "label": "yield", "kind": 14, "data": { "keyword": true } }));
    let contents = documentation(&result);
    assert_contains(&contents, "```ruby\nyield\n```");
    assert_contains(&contents, keyword_documentation("yield").unwrap());
    let expected_uri = ruby_lsp_rs::uri::from_path_buf(&static_docs_path().join("yield.md"));
    assert_contains(&contents, &format!("[Read more]({expected_uri})"));
}

#[test]
fn resolve_for_require_completion() {
    let mut server = with_server(Some("require \"\"\n"), DEFAULT_URI, no_typechecker());
    let result = resolve(&mut server, json!({ "label": "foo", "kind": 17, "data": {} }));
    assert!(result.get("documentation").is_none());
}

#[test]
fn resolve_returns_delegated_completion_items_unchanged() {
    let mut server = with_server(Some(""), DEFAULT_URI, no_typechecker());
    let item = json!({ "label": "div", "kind": 10, "data": { "delegateCompletion": true } });
    assert_eq!(resolve(&mut server, item.clone()), item);
}
