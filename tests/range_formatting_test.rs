mod formatter_support;
mod support;

use formatter_support::workspace;
use ruby_lsp_rs::document::{ContentChange, Document, LanguageId};
use ruby_lsp_rs::position::Encoding;
use serde_json::{Value, json};
use support::{TestOptions, with_server};

const SOURCE: &str = "class Foo\n\n  def foo\n    [\n    1,\n    2,\n    3,\n    4,\n    ]\n  end\nend\n";

fn range_formatting(formatter: &str, source: &str, range: Value) -> Option<(String, Value)> {
    let workspace = workspace("range_formatting", &[])?;
    let uri = workspace.uri_for("test/requests/range_formatting_test.rb");
    let initialization_options = workspace.server_options(json!({ "formatter": formatter }));
    let mut server =
        with_server(Some(source), &uri, TestOptions { initialization_options: Some(initialization_options), ..TestOptions::default() });
    let result = server.result(
        "textDocument/rangeFormatting",
        json!({ "textDocument": { "uri": uri }, "range": range, "options": { "tabSize": 2, "insertSpaces": true } }),
    );
    Some((uri, result))
}

#[test]
fn syntax_tree_supports_range_formatting() {
    let range = json!({ "start": { "line": 3, "character": 2 }, "end": { "line": 8, "character": 5 } });
    let Some((uri, edits)) = range_formatting("syntax_tree", SOURCE, range) else { return };

    let edits: Vec<ContentChange> = edits
        .as_array()
        .unwrap()
        .iter()
        .map(|edit| serde_json::from_value(json!({ "range": edit["range"], "text": edit["newText"] })).unwrap())
        .collect();
    let mut document = Document::new(&uri, SOURCE.to_string(), 1, LanguageId::Ruby, Encoding::Utf16);
    document.push_edits(&edits, 2).unwrap();

    assert_eq!(document.source(), "class Foo\n\n  def foo\n    [1, 2, 3, 4]\n  end\nend\n");
}

#[test]
fn rubocop_does_not_support_range_formatting() {
    let range = json!({ "start": { "line": 3, "character": 2 }, "end": { "line": 8, "character": 5 } });
    let Some((_, edits)) = range_formatting("rubocop_internal", SOURCE, range) else { return };
    assert_eq!(edits, Value::Null);
}

#[test]
fn returns_nil_for_syntax_invalid_documents() {
    let range = json!({ "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 7 } });
    let Some((_, edits)) = range_formatting("syntax_tree", "def foo", range) else { return };
    assert_eq!(edits, Value::Null);
}

#[test]
fn returns_nil_when_no_node_is_within_the_range() {
    let range = json!({ "start": { "line": 1, "character": 0 }, "end": { "line": 1, "character": 0 } });
    let Some((_, edits)) = range_formatting("syntax_tree", SOURCE, range) else { return };
    assert_eq!(edits, Value::Null);
}

#[test]
fn returns_nil_when_formatter_is_none() {
    let range = json!({ "start": { "line": 3, "character": 2 }, "end": { "line": 8, "character": 5 } });
    let Some((_, edits)) = range_formatting("none", SOURCE, range) else { return };
    assert_eq!(edits, Value::Null);
}
