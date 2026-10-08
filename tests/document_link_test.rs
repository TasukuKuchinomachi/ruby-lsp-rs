mod document_link_support;
mod support;

use document_link_support::{document_links, with_placeholders};
use serde_json::{Value, json};
use support::{DEFAULT_URI, TestOptions, with_server};

fn document_link_result(source: &str) -> Value {
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    server.result("textDocument/documentLink", json!({ "textDocument": { "uri": DEFAULT_URI } }))
}

#[test]
fn magic_source_links_on_unsaved_files() {
    let source = "# source://erb/#1\ndef bar\nend\n";
    assert_eq!(document_link_result(source), json!([]));
    assert!(document_links(source, DEFAULT_URI).is_empty());
}

#[test]
fn magic_source_links_with_invalid_uris() {
    let source = "# source://some_file /#123\ndef bar\nend\n";
    assert_eq!(document_link_result(source), json!([]));
    assert!(document_links(source, DEFAULT_URI).is_empty());
}

#[test]
fn package_url_links() {
    let source = "# pkg:gem/erb#:99\ndef bar\nend\n";
    assert_eq!(document_link_result(source), json!([]));
    assert!(document_links(source, DEFAULT_URI).is_empty());
}

#[test]
fn source_link_with_single_line_sig_block() {
    let source = "# source://syntax_tree//lib/syntax_tree.rb#39\nsig { returns(String) }\ndef foo\nend\n";
    let links = document_links(source, DEFAULT_URI);
    assert!(!links.is_empty(), "Expected document link for source comment above single-line sig block");
}

#[test]
fn source_link_with_multi_line_sig_block() {
    let source = "# source://syntax_tree//lib/syntax_tree.rb#39\nsig do\n  params(\n    x: Integer,\n  ).returns(String)\nend\ndef foo(x)\nend\n";
    let links = document_links(source, DEFAULT_URI);
    assert!(!links.is_empty(), "Expected document link for source comment above multi-line sig block");
}

#[test]
fn package_url_links_with_invalid_uris() {
    let source = "# pkg:gem/rubocop$1.78.0#:99\ndef bar\nend\n";
    assert_eq!(document_link_result(source), json!([]));
    assert!(document_links(source, DEFAULT_URI).is_empty());
}

#[test]
fn source_link_resolves_the_version_from_the_rbi_file_name() {
    let source = "# source://syntax_tree//lib/syntax_tree.rb#7\ndef foo\nend\n";
    let links = document_links(source, "file:///sorbet/rbi/gems/syntax_tree%25409.8.7.rbi");
    assert_eq!(links.len(), 1);
    assert_eq!(with_placeholders(&links[0].target), "file://BUNDLER_PATH/gems/syntax_tree-SYNTAX_TREE_VERSION/lib/syntax_tree.rb#7");

    let links = document_links(source, "file:///sorbet/rbi/gems/syntax_tree%25401.0.0.rbi");
    assert!(links.is_empty());
}

#[test]
fn package_url_link_without_line_number_has_empty_fragment() {
    let source = "# pkg:gem/syntax_tree#lib/syntax_tree.rb\ndef foo\nend\n";
    let links = document_links(source, DEFAULT_URI);
    assert_eq!(links.len(), 1);
    assert!(links[0].target.ends_with("lib/syntax_tree.rb#"), "{}", links[0].target);
}

#[test]
fn links_are_not_returned_for_non_ruby_documents() {
    let mut server = with_server(None, DEFAULT_URI, TestOptions::default());
    let uri = "file:///fake.rbs";
    server.did_open(uri, "# source://syntax_tree//lib/syntax_tree.rb#39\nclass Foo\nend\n", "rbs");
    assert_eq!(server.result("textDocument/documentLink", json!({ "textDocument": { "uri": uri } })), Value::Null);
}
