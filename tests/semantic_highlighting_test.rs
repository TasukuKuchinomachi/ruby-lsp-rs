mod support;

use ruby_lsp_rs::requests::semantic_highlighting::compute_delta;
use ruby_lsp_rs::requests::support::semantic_highlighting_response_builder::{
    SemanticHighlightingResponseBuilder, TOKEN_MODIFIERS, TOKEN_TYPES, TokenModifier, TokenType, encode,
};
use ruby_lsp_rs::position::Encoding;
use serde_json::{Value, json};
use support::{DEFAULT_URI, TestOptions, uninitialized_server, with_server};

fn assert_expected_token_result(current_tokens: &[u32], previous_tokens: &[u32]) {
    let delta = compute_delta(current_tokens, previous_tokens, "1");
    let edit = delta.edits.first().expect("an edit is computed");
    let mut previous_tokens = previous_tokens.to_vec();
    previous_tokens.splice(edit.start..edit.start + edit.delete_count, edit.data.iter().copied());
    assert_eq!(current_tokens, previous_tokens.as_slice());
}

#[test]
fn inserting_something_at_the_end() {
    assert_expected_token_result(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 100, 200, 300, 400, 500], &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
}

#[test]
fn inserting_something_in_the_beginning() {
    assert_expected_token_result(
        &[10, 100, 200, 300, 400, 500, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10],
        &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10],
    );
}

#[test]
fn inserting_something_in_the_middle() {
    assert_expected_token_result(
        &[1, 2, 3, 4, 5, 10, 100, 200, 300, 400, 500, 6, 7, 8, 9, 10],
        &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10],
    );
}

#[test]
fn deleting_at_the_end() {
    assert_expected_token_result(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10], &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 100, 200, 300, 400, 500]);
}

#[test]
fn deleting_at_the_beginning() {
    assert_expected_token_result(
        &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10],
        &[10, 100, 200, 300, 400, 500, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10],
    );
}

#[test]
fn deleting_in_the_middle() {
    assert_expected_token_result(
        &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10],
        &[1, 2, 3, 4, 5, 10, 100, 200, 300, 400, 500, 6, 7, 8, 9, 10],
    );
}

#[test]
fn computing_deltas_1() {
    assert_expected_token_result(
        &[3, 4, 3, 13, 1, 1, 2, 1, 8, 0, 1, 0, 1, 13, 0, 1, 2, 9, 13, 0, 1, 2, 1, 8, 0],
        &[3, 4, 3, 13, 1, 1, 2, 1, 8, 0, 2, 2, 9, 13, 0, 1, 2, 1, 8, 0],
    );
}

#[test]
fn computing_deltas_2() {
    assert_expected_token_result(
        &[
            3, 4, 21, 13, 1, 1, 2, 12, 13, 0, 0, 73, 5, 13, 0, 2, 2, 5, 8, 0, 0, 8, 7, 0, 0, 0, 9, 8, 0, 0, 0, 10, 20, 0, 0, 0,
            21, 13, 13, 0, 4, 4, 5, 13, 0, 2, 0, 1, 8, 0, 1, 0, 1, 8, 0, 2, 2, 12, 13, 0, 0, 73, 5, 8, 0,
        ],
        &[
            3, 4, 21, 13, 1, 1, 2, 12, 13, 0, 0, 73, 5, 13, 0, 2, 2, 5, 8, 0, 0, 8, 7, 0, 0, 0, 9, 8, 0, 0, 0, 10, 20, 0, 0, 0,
            21, 13, 13, 0, 4, 4, 5, 13, 0, 2, 0, 1, 8, 0, 3, 2, 12, 13, 0, 0, 73, 5, 8, 0,
        ],
    );
}

#[test]
fn computing_deltas_3() {
    assert_expected_token_result(
        &[3, 6, 3, 2, 1, 0, 0, 3, 0, 0, 1, 6, 4, 13, 1, 1, 4, 1, 8, 0],
        &[3, 6, 3, 2, 1, 0, 0, 3, 0, 0, 1, 6, 4, 13, 1],
    );
}

#[test]
fn computing_delta_4() {
    assert_expected_token_result(
        &[3, 6, 3, 2, 1, 0, 0, 3, 0, 0, 1, 6, 4, 13, 1, 1, 4, 1, 8, 0, 1, 4, 1, 8, 0],
        &[3, 6, 3, 2, 1, 0, 0, 3, 0, 0, 1, 6, 4, 13, 1, 1, 4, 1, 8, 0],
    );
}

#[test]
fn computing_delta_5() {
    assert_expected_token_result(
        &[3, 6, 3, 2, 1, 0, 0, 3, 0, 0, 1, 6, 4, 13, 1, 1, 4, 1, 8, 0, 1, 4, 1, 8, 0, 0, 2, 4, 13, 0],
        &[3, 6, 3, 2, 1, 0, 0, 3, 0, 0, 1, 6, 4, 13, 1, 1, 4, 1, 8, 0, 1, 4, 1, 8, 0, 0, 2, 4, 13, 0, 1, 4, 1, 8, 0],
    );
}

#[test]
fn computing_delta_6() {
    assert_expected_token_result(
        &[3, 6, 3, 2, 1, 0, 0, 3, 0, 0, 1, 6, 4, 13, 1, 1, 4, 1, 8, 0, 1, 4, 1, 8, 0, 0, 2, 4, 13, 0],
        &[3, 6, 3, 2, 1, 0, 0, 3, 0, 0, 1, 6, 4, 13, 1, 1, 4, 1, 8, 0, 1, 4, 1, 8, 0, 1, 4, 1, 8, 0, 0, 2, 4, 13, 0],
    );
}

#[test]
fn computing_delta_7() {
    assert_expected_token_result(
        &[
            3, 6, 3, 2, 1, 0, 0, 3, 0, 0, 1, 6, 4, 13, 1, 1, 4, 1, 8, 0, 1, 4, 1, 8, 0, 0, 2, 2, 13, 0, 1, 4, 1, 8, 0, 0, 2, 4,
            13, 0,
        ],
        &[
            3, 6, 3, 2, 1, 0, 0, 3, 0, 0, 1, 6, 4, 13, 1, 1, 4, 1, 8, 0, 1, 4, 1, 8, 0, 0, 2, 1, 13, 0, 1, 4, 1, 8, 0, 0, 2, 4,
            13, 0,
        ],
    );
}

#[test]
fn identical_tokens_produce_no_edits() {
    let delta = compute_delta(&[1, 2, 3, 4, 5], &[1, 2, 3, 4, 5], "7");
    assert_eq!(serde_json::to_value(delta).unwrap(), json!({ "resultId": "7", "edits": [] }));
}

fn decode_tokens(data: &Value) -> Vec<Value> {
    let data: Vec<u64> = data.as_array().unwrap().iter().map(|value| value.as_u64().unwrap()).collect();
    data.chunks(5)
        .map(|token| {
            json!({
                "delta_line": token[0],
                "delta_start_char": token[1],
                "length": token[2],
                "token_type": token[3],
                "token_modifiers": token[4],
            })
        })
        .collect()
}

#[test]
fn semantic_highlighting_full_request() {
    let source = "class Post\n  custom_method :foo\n  before_create :set_defaults\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    let result = server.result("textDocument/semanticTokens/full", json!({ "textDocument": { "uri": DEFAULT_URI } }));

    let decoded_response = decode_tokens(&result["data"]);
    assert_eq!(
        decoded_response[0],
        json!({ "delta_line": 0, "delta_start_char": 6, "length": 4, "token_type": 2, "token_modifiers": 1 })
    );
    assert_eq!(
        decoded_response[1],
        json!({ "delta_line": 1, "delta_start_char": 2, "length": 13, "token_type": 13, "token_modifiers": 0 })
    );
    assert!(result["resultId"].is_string());
}

#[test]
fn response_builder_lets_listeners_replace_the_last_token() {
    let source = "class Post\n  before_create :set_defaults\nend\n";
    let mut response_builder = SemanticHighlightingResponseBuilder::new(source.as_bytes(), Encoding::Utf16);
    response_builder.add_token(6, 10, TokenType::Class, &[TokenModifier::Declaration]);
    response_builder.add_token(13, 26, TokenType::Method, &[]);

    assert!(response_builder.last_token_matches(13));
    assert!(!response_builder.last_token_matches(6));
    let token = response_builder.last().unwrap();
    token.replace_type(TokenType::Keyword);
    token.replace_modifier(&[TokenModifier::Declaration]);

    let data = encode(&response_builder.into_response());
    assert_eq!(data, vec![0, 6, 4, 2, 1, 1, 2, 13, 15, 1]);
}

#[test]
fn provider_uses_lsp_default_library_modifier_casing() {
    let mut server = uninitialized_server(Vec::new());
    let result = server.result("initialize", json!({ "initializationOptions": {} }));
    let legend = &result["capabilities"]["semanticTokensProvider"]["legend"];
    let token_modifiers: Vec<&str> = legend["tokenModifiers"].as_array().unwrap().iter().map(|value| value.as_str().unwrap()).collect();
    assert!(token_modifiers.contains(&"defaultLibrary"));
    assert!(!token_modifiers.contains(&"default_library"));
    assert_eq!(token_modifiers, TOKEN_MODIFIERS);
    let token_types: Vec<&str> = legend["tokenTypes"].as_array().unwrap().iter().map(|value| value.as_str().unwrap()).collect();
    assert_eq!(token_types, TOKEN_TYPES);
    assert_eq!(result["capabilities"]["semanticTokensProvider"]["full"], json!({ "delta": true }));
    assert_eq!(result["capabilities"]["semanticTokensProvider"]["range"], json!(true));
}

#[test]
fn delta_request_returns_edits_against_the_previous_result() {
    let source = "def foo(a)\n  a\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    let full = server.result("textDocument/semanticTokens/full", json!({ "textDocument": { "uri": DEFAULT_URI } }));
    let previous_result_id = full["resultId"].as_str().unwrap().to_string();
    let previous_data: Vec<u32> = serde_json::from_value(full["data"].clone()).unwrap();

    server.notify(
        "textDocument/didChange",
        json!({
            "textDocument": { "uri": DEFAULT_URI, "version": 2 },
            "contentChanges": [{ "range": support::range_json(1, 3, 1, 3), "text": "\n  a" }],
        }),
    );
    let delta = server.result(
        "textDocument/semanticTokens/full/delta",
        json!({ "textDocument": { "uri": DEFAULT_URI }, "previousResultId": previous_result_id }),
    );
    assert_ne!(delta["resultId"], full["resultId"]);
    let edits = delta["edits"].as_array().unwrap();
    assert_eq!(edits.len(), 1);

    let mut data = previous_data;
    let start = edits[0]["start"].as_u64().unwrap() as usize;
    let delete_count = edits[0]["deleteCount"].as_u64().unwrap() as usize;
    let inserted: Vec<u32> = serde_json::from_value(edits[0]["data"].clone()).unwrap();
    data.splice(start..start + delete_count, inserted);
    assert_eq!(data, vec![1, 2, 1, 7, 0, 1, 2, 1, 7, 0]);
}

#[test]
fn delta_request_with_unknown_previous_result_returns_full_tokens() {
    let mut server = with_server(Some("foo\n"), DEFAULT_URI, TestOptions::default());
    let result = server.result(
        "textDocument/semanticTokens/full/delta",
        json!({ "textDocument": { "uri": DEFAULT_URI }, "previousResultId": "unknown" }),
    );
    assert_eq!(result["data"], json!([0, 0, 3, 13, 0]));
    assert!(result.get("edits").is_none());
}

#[test]
fn range_request_only_returns_tokens_within_the_lines() {
    let source = "foo\nbar\nbaz\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    let result = server.result(
        "textDocument/semanticTokens/range",
        json!({ "textDocument": { "uri": DEFAULT_URI }, "range": support::range_json(1, 0, 1, 0) }),
    );
    assert_eq!(result["data"], json!([1, 0, 3, 13, 0]));
}

#[test]
fn documents_past_the_expensive_limit_return_null() {
    let source = "a = 1\n".repeat(20_000);
    let mut server = with_server(Some(&source), DEFAULT_URI, TestOptions::default());
    for (method, params) in [
        ("textDocument/semanticTokens/full", json!({ "textDocument": { "uri": DEFAULT_URI } })),
        ("textDocument/semanticTokens/full/delta", json!({ "textDocument": { "uri": DEFAULT_URI }, "previousResultId": "1" })),
        ("textDocument/semanticTokens/range", json!({ "textDocument": { "uri": DEFAULT_URI }, "range": support::range_json(0, 0, 1, 0) })),
    ] {
        assert_eq!(server.result(method, params), Value::Null);
    }
}

#[test]
fn non_ruby_documents_return_null() {
    let uri = "file:///fake.rbs";
    let mut server = with_server(None, uri, TestOptions::default());
    server.did_open(uri, "class Foo\nend\n", "rbs");
    let result = server.result("textDocument/semanticTokens/full", json!({ "textDocument": { "uri": uri } }));
    assert_eq!(result, Value::Null);
}
