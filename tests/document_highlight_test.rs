mod support;

use serde_json::{Value, json};
use support::{DEFAULT_URI, TestOptions, range_json, with_server};

fn document_highlights(source: &str, line: u32, character: u32) -> Value {
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions { stub_no_typechecker: true, ..TestOptions::default() });
    server.result(
        "textDocument/documentHighlight",
        json!({ "textDocument": { "uri": DEFAULT_URI }, "position": { "line": line, "character": character } }),
    )
}

#[test]
fn highlights_local_variable_reads_and_writes() {
    let result = document_highlights("foo = 1\nfoo += 2\nputs foo\n", 2, 5);
    assert_eq!(
        result,
        json!([
            { "range": range_json(0, 0, 0, 3), "kind": 3 },
            { "range": range_json(1, 0, 1, 3), "kind": 3 },
            { "range": range_json(2, 5, 2, 8), "kind": 2 },
        ])
    );
}

#[test]
fn highlights_matching_keywords_for_multibyte_lines() {
    let result = document_highlights("if true\n  \"あいう\"\nend\n", 2, 1);
    assert_eq!(
        result,
        json!([
            { "range": range_json(0, 0, 0, 2), "kind": 1 },
            { "range": range_json(2, 0, 2, 3), "kind": 1 },
        ])
    );
}

#[test]
fn returns_empty_result_without_target() {
    assert_eq!(document_highlights("", 0, 0), json!([]));
}
