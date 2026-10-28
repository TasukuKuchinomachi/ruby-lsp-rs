mod support;

use serde_json::{Value, json};
use support::{TestOptions, fixture_path, with_server};

fn find_references(fixture_name: &str, line: u32, character: u32) -> Vec<Value> {
    let path = fixture_path(fixture_name);
    let source = std::fs::read_to_string(&path).unwrap();
    let uri = ruby_lsp_rs::uri::from_path_buf(&path);
    let mut server = with_server(None, &uri, TestOptions::default());
    server.index_file(&path, None);
    server.did_open(&uri, &source, "ruby");

    let result = server.result(
        "textDocument/references",
        json!({ "textDocument": { "uri": uri }, "position": { "line": line, "character": character } }),
    );
    result.as_array().cloned().unwrap_or_else(|| panic!("Expected an array, got {result}"))
}

#[test]
fn finds_constant_references() {
    let lines: Vec<u64> = find_references("rename_me.rb", 0, 6)
        .iter()
        .map(|location| location["range"]["start"]["line"].as_u64().unwrap())
        .collect();

    assert_eq!(vec![0, 3], lines);
}

#[test]
fn finds_method_references_across_documents() {
    let mut server = with_server(Some("def foo; end\nfoo\n"), "file:///first.rb", TestOptions::default());
    server.did_open("untitled:Untitled-1", "self.foo\n", "ruby");

    let result = server.result(
        "textDocument/references",
        json!({ "textDocument": { "uri": "file:///first.rb" }, "position": { "line": 1, "character": 0 } }),
    );

    let locations: Vec<(String, u64)> = result
        .as_array()
        .unwrap()
        .iter()
        .filter(|location| {
            let uri = location["uri"].as_str().unwrap();
            !uri.contains("/tests/") && !uri.contains("/target/")
        })
        .map(|location| (location["uri"].as_str().unwrap().to_string(), location["range"]["start"]["line"].as_u64().unwrap()))
        .collect();
    assert_eq!(
        vec![("file:///first.rb".to_string(), 0), ("file:///first.rb".to_string(), 1), ("untitled:Untitled-1".to_string(), 0)],
        locations
    );
}

#[test]
fn returns_null_for_non_ruby_documents() {
    let mut server = with_server(None, "file:///fake.rb", TestOptions::default());
    server.did_open("file:///fake.erb", "<%= foo %>", "erb");

    let result = server.result(
        "textDocument/references",
        json!({ "textDocument": { "uri": "file:///fake.erb" }, "position": { "line": 0, "character": 4 } }),
    );
    assert_eq!(Value::Null, result);
}
