mod support;

use ruby_lsp_rs::indexer::rbs_indexer::RbsIndexer;
use serde_json::{Value, json};
use support::{DEFAULT_URI, TestOptions, TestServer, rbs_core_path, with_server};

fn signature_help(server: &mut TestServer, line: u32, character: u32, context: Option<Value>) -> Value {
    let mut params = json!({ "textDocument": { "uri": DEFAULT_URI }, "position": { "line": line, "character": character } });
    if let Some(context) = context {
        params["context"] = context;
    }
    server.result("textDocument/signatureHelp", params)
}

fn first_signature(result: &Value) -> &Value {
    &result["signatures"][0]
}

fn index_ruby_core(server: &TestServer) -> bool {
    let Some(rbs_core_path) = rbs_core_path() else { return false };
    RbsIndexer::new(&mut server.index()).index_core_directory(&rbs_core_path);
    true
}

fn assert_contains(contents: &Value, expected: &str) {
    let contents = contents.as_str().unwrap();
    assert!(contents.contains(expected), "Expected {contents:?} to contain {expected:?}");
}

#[test]
fn initial_request() {
    let source = "class Foo\n  def bar(a, b)\n  end\n\n  def baz\n    bar()\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    let result =
        signature_help(&mut server, 5, 7, Some(json!({ "triggerCharacter": "(", "activeSignatureHelp": null })));
    assert_eq!(first_signature(&result)["label"], "bar(a, b)");
    assert_eq!(result["activeParameter"], 0);
}

#[test]
fn concats_documentations_from_both_definitions() {
    let source = "class Foo\n  # first definition\n  def bar(a, b)\n  end\n\n  def baz\n    bar()\n  end\nend\n\nclass Foo\n  # second definition\n  def bar(c, d)\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    let result =
        signature_help(&mut server, 6, 7, Some(json!({ "triggerCharacter": "(", "activeSignatureHelp": null })));
    let signature = first_signature(&result);
    assert_eq!(signature["label"], "bar(a, b)");
    assert_eq!(result["activeParameter"], 0);
    assert_contains(&signature["documentation"]["value"], "first definition");
    assert_contains(&signature["documentation"]["value"], "second definition");
}

#[test]
fn help_after_comma() {
    let source = "class Foo\n  def bar(a, b)\n  end\n\n  def baz\n    bar(a,)\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    let result = signature_help(&mut server, 5, 9, Some(json!({ "triggerCharacter": "," })));
    assert_eq!(first_signature(&result)["label"], "bar(a, b)");
    assert_eq!(result["activeParameter"], 1);
}

#[test]
fn keyword_arguments() {
    let source = "class Foo\n  def bar(a:, b:)\n  end\n\n  def baz\n    bar(b: 1,)\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    let result =
        signature_help(&mut server, 5, 12, Some(json!({ "triggerCharacter": ",", "activeSignatureHelp": null })));
    assert_eq!(first_signature(&result)["label"], "bar(a:, b:)");
    assert_eq!(result["activeParameter"], 1);
}

#[test]
fn skipped_arguments() {
    let source = "class Foo\n  def bar(a, b = 123, c:, d:)\n  end\n\n  def baz\n    bar(a, c: 1,)\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    let result =
        signature_help(&mut server, 5, 15, Some(json!({ "triggerCharacter": ",", "activeSignatureHelp": null })));
    assert_eq!(first_signature(&result)["label"], "bar(a, b = <default>, c:, d:)");
    assert_eq!(result["activeParameter"], 2);
}

#[test]
fn help_for_splats() {
    let source = "class Foo\n  def bar(*a)\n  end\n\n  def baz\n    bar(a, b, c, d, e)\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    let result = signature_help(&mut server, 5, 20, Some(json!({})));
    assert_eq!(first_signature(&result)["label"], "bar(*a)");
    assert_eq!(result["activeParameter"], 0);
}

#[test]
fn help_for_blocks() {
    let source = "class Foo\n  def bar(a, &block)\n  end\n\n  def baz\n    bar(a,)\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    let result = signature_help(&mut server, 5, 9, Some(json!({})));
    assert_eq!(first_signature(&result)["label"], "bar(a, &block)");
    assert_eq!(result["activeParameter"], 1);
}

#[test]
fn requests_missing_context() {
    let source = "class Foo\n  def bar(a, b)\n  end\n\n  def baz\n    bar()\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    let result = signature_help(&mut server, 5, 7, None);
    assert_eq!(first_signature(&result)["label"], "bar(a, b)");
    assert_eq!(result["activeParameter"], 0);
}

#[test]
fn help_in_nested_method_calls_no_arguments() {
    let source = "class Foo\n  def bar(a)\n  end\n\n  def baz(b)\n  end\n\n  def qux\n    bar(baz())\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    let result = signature_help(&mut server, 8, 11, Some(json!({})));
    assert_eq!(first_signature(&result)["label"], "bar(a)");
    assert_eq!(result["activeParameter"], 0);
}

#[test]
fn help_in_nested_method_calls_with_arguments() {
    let source = "class Foo\n  def bar(a)\n  end\n\n  def baz(b)\n  end\n\n  def qux\n    bar(baz(123))\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    let result = signature_help(&mut server, 8, 11, Some(json!({})));
    assert_eq!(first_signature(&result)["label"], "bar(a)");
    assert_eq!(result["activeParameter"], 0);
}

#[test]
fn singleton_methods() {
    let source = "class Foo\n  def self.bar(a, b)\n  end\nend\n\nFoo.bar()\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    let result =
        signature_help(&mut server, 5, 7, Some(json!({ "triggerCharacter": "(", "activeSignatureHelp": null })));
    assert_eq!(first_signature(&result)["label"], "bar(a, b)");
    assert_eq!(result["activeParameter"], 0);
}

#[test]
fn aliased_methods() {
    let source = "class Parent\n  def bar(a); end\nend\n\nclass Child < Parent\n  alias baz bar\n\n  def do_something\n    baz()\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    let result = signature_help(&mut server, 8, 8, Some(json!({})));
    assert_eq!(first_signature(&result)["label"], "baz(a)");
    assert_eq!(result["activeParameter"], 0);
}

#[test]
fn help_is_disabled_on_typed_true() {
    let source = "# typed: true\nclass Foo\n  def bar(a, b)\n  end\n\n  def baz\n    bar()\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    let result =
        signature_help(&mut server, 6, 7, Some(json!({ "triggerCharacter": "(", "activeSignatureHelp": null })));
    assert_eq!(result, Value::Null);
}

#[test]
fn guessed_types() {
    let source = "class User\n  def subscribe!(news_letter)\n  end\nend\n\nuser.subscribe!()\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    let result = signature_help(&mut server, 5, 15, Some(json!({})));
    let signature = first_signature(&result);
    assert_eq!(signature["label"], "subscribe!(news_letter)");
    assert_contains(&signature["documentation"]["value"], "Guessed receiver: User");
}

#[test]
fn automatically_detects_active_overload() {
    let cases = [
        ("5.step()\n", 7, "step(limit = <default>, step = <default>, &<anonymous block>)"),
        ("5.step(1)\n", 8, "step(limit = <default>, step = <default>, &<anonymous block>)"),
        ("5.step(to: 5)\n", 8, "step(by: <default>, to: <default>, &<anonymous block>)"),
    ];
    for (source, character, expected_label) in cases {
        let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
        if !index_ruby_core(&server) {
            return;
        }
        let result = signature_help(&mut server, 0, character, Some(json!({})));
        let active_signature = result["activeSignature"].as_u64().unwrap() as usize;
        assert_eq!(result["signatures"][active_signature]["label"], expected_label, "{source}");
    }
}
