mod support;

use serde_json::{Value, json};
use support::{TestOptions, with_server};

fn inlay_hints(source: &str, inlay_hint_configuration: Value) -> Value {
    let uri = "file:///foo.rb";
    let initialization_options = json!({
        "initializationOptions": { "featuresConfiguration": { "inlayHint": inlay_hint_configuration } },
    });
    let mut server =
        with_server(Some(source), uri, TestOptions { initialization_options: Some(initialization_options), ..TestOptions::default() });
    server.result(
        "textDocument/inlayHint",
        json!({
            "textDocument": { "uri": uri },
            "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 20, "character": 20 } },
        }),
    )
}

#[test]
fn skip_implicit_hash_value() {
    let result = inlay_hints("{bar:, baz:}\n", json!({ "implicitRescue": true, "implicitHashValue": false }));
    assert_eq!(result, json!([]));
}

#[test]
fn skip_implicit_rescue() {
    let result = inlay_hints("begin\nrescue\nend\n", json!({ "implicitRescue": false, "implicitHashValue": true }));
    assert_eq!(result, json!([]));
}

#[test]
fn filters_hints_outside_of_the_requested_range() {
    let uri = "file:///foo.rb";
    let initialization_options = json!({
        "initializationOptions": { "featuresConfiguration": { "inlayHint": { "enableAll": true } } },
    });
    let mut server = with_server(
        Some("begin\nrescue\nend\n\nbegin\nrescue\nend\n"),
        uri,
        TestOptions { initialization_options: Some(initialization_options), ..TestOptions::default() },
    );
    let request = |server: &mut support::TestServer, start_line: u32, end_line: u32| {
        server.result(
            "textDocument/inlayHint",
            json!({
                "textDocument": { "uri": uri },
                "range": { "start": { "line": start_line, "character": 0 }, "end": { "line": end_line, "character": 0 } },
            }),
        )
    };

    assert_eq!(request(&mut server, 0, 2).as_array().unwrap().len(), 1);
    let cached = request(&mut server, 4, 6);
    assert_eq!(cached.as_array().unwrap().len(), 1);
    assert_eq!(cached[0]["position"], json!({ "line": 5, "character": 6 }));
}
