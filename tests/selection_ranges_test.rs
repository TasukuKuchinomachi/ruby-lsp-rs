mod support;

use serde_json::{Value, json};
use support::{DEFAULT_URI, TestOptions, range_json, with_server};

#[test]
fn answers_one_element_per_position() {
    let mut server = with_server(Some("def foo\n  bar\nend\n"), DEFAULT_URI, TestOptions::default());
    let result = server.result(
        "textDocument/selectionRange",
        json!({
            "textDocument": { "uri": DEFAULT_URI },
            "positions": [{ "line": 1, "character": 3 }, { "line": 10, "character": 0 }],
        }),
    );

    let ranges = result.as_array().unwrap();
    assert_eq!(ranges.len(), 2);
    assert_eq!(ranges[0]["range"], range_json(1, 2, 1, 5));
    assert_eq!(ranges[0]["parent"]["range"], range_json(1, 2, 1, 5));
    assert_eq!(ranges[0]["parent"]["parent"]["range"], range_json(0, 0, 2, 3));
    assert_eq!(ranges[1], Value::Null);
}

#[test]
fn answers_null_for_every_position_of_non_ruby_documents() {
    let mut server = with_server(None, DEFAULT_URI, TestOptions::default());
    let uri = "file:///fake.rbs";
    server.did_open(uri, "class Foo\nend\n", "rbs");
    let result = server.result(
        "textDocument/selectionRange",
        json!({ "textDocument": { "uri": uri }, "positions": [{ "line": 0, "character": 0 }] }),
    );
    assert_eq!(result, json!([null]));
}
