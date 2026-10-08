use ruby_lsp_rs::document::{ContentChange, Document, LanguageId};
use ruby_lsp_rs::position::{Encoding, Position, Range};
use ruby_lsp_rs::requests::on_type_formatting::OnTypeFormatting;
use serde_json::{Value, json};

fn document(source: &str) -> Document {
    Document::new(
        "file:///fake.rb",
        source.to_string(),
        1,
        LanguageId::Ruby,
        Encoding::Utf16,
    )
}

fn push_edit(document: &mut Document, start: (u32, u32), end: (u32, u32), text: &str) {
    let range = Range::new(Position::new(start.0, start.1), Position::new(end.0, end.1));
    let version = document.version() + 1;
    document
        .push_edits(
            &[ContentChange {
                range: Some(range),
                text: text.to_string(),
            }],
            version,
        )
        .unwrap();
}

fn edits_for(
    document: &Document,
    line: u32,
    character: u32,
    trigger_character: &str,
    client_name: &str,
) -> Value {
    let edits = OnTypeFormatting::new(
        document,
        Position::new(line, character),
        trigger_character,
        client_name,
    )
    .perform();
    serde_json::to_value(edits).unwrap()
}

#[test]
fn adding_missing_ends() {
    let mut document = document("");
    push_edit(&mut document, (0, 0), (0, 0), "class Foo");
    let edits = edits_for(&document, 1, 2, "\n", "Visual Studio Code");
    let expected_edits = json!([
      {
        "range": { "start": { "line": 1, "character": 2 }, "end": { "line": 1, "character": 2 } },
        "newText": "\n",
      },
      {
        "range": { "start": { "line": 1, "character": 2 }, "end": { "line": 1, "character": 2 } },
        "newText": "end",
      },
      {
        "range": { "start": { "line": 1, "character": 2 }, "end": { "line": 1, "character": 2 } },
        "newText": "$0",
      },
    ]);
    assert_eq!(expected_edits, edits);
}

#[test]
fn adding_missing_curly_brace_in_string_interpolation() {
    let mut document = document("");
    push_edit(&mut document, (0, 0), (0, 0), "\"something#{\"");
    let edits = edits_for(&document, 0, 11, "{", "Visual Studio Code");
    let expected_edits = json!([
      {
        "range": { "start": { "line": 0, "character": 11 }, "end": { "line": 0, "character": 11 } },
        "newText": "}",
      },
      {
        "range": { "start": { "line": 0, "character": 11 }, "end": { "line": 0, "character": 11 } },
        "newText": "$0",
      },
    ]);
    assert_eq!(expected_edits, edits);
}

#[test]
fn adding_missing_pipe() {
    let mut document = document("");
    push_edit(&mut document, (0, 0), (0, 0), "[].each do |");
    let edits = edits_for(&document, 0, 12, "|", "Visual Studio Code");
    let expected_edits = json!([
      {
        "range": { "start": { "line": 0, "character": 12 }, "end": { "line": 0, "character": 12 } },
        "newText": "|",
      },
      {
        "range": { "start": { "line": 0, "character": 12 }, "end": { "line": 0, "character": 12 } },
        "newText": "$0",
      },
    ]);
    assert_eq!(expected_edits, edits);
}

#[test]
fn pipe_is_not_added_in_regular_or_pipe() {
    let mut document = document("");
    push_edit(&mut document, (0, 0), (0, 0), "|");
    let edits = edits_for(&document, 0, 2, "|", "Visual Studio Code");
    assert_eq!(json!([]), edits);
}

#[test]
fn pipe_is_removed_if_user_adds_manually_after_completion() {
    let mut document = document("");
    push_edit(&mut document, (0, 0), (0, 0), "[].each do |");
    push_edit(&mut document, (0, 12), (0, 12), "|");
    let edits = edits_for(&document, 0, 12, "|", "Visual Studio Code");
    let expected_edits = json!([
      {
        "range": { "start": { "line": 0, "character": 12 }, "end": { "line": 0, "character": 12 } },
        "newText": "|",
      },
      {
        "range": { "start": { "line": 0, "character": 12 }, "end": { "line": 0, "character": 12 } },
        "newText": "$0",
      },
    ]);
    assert_eq!(expected_edits, edits);
    assert_eq!("[].each do ||", document.source());
    push_edit(&mut document, (0, 13), (0, 13), "|");
    let edits = edits_for(&document, 0, 13, "|", "Visual Studio Code");
    let expected_edits = json!([
      {
        "range": { "start": { "line": 0, "character": 13 }, "end": { "line": 0, "character": 14 } },
        "newText": "",
      },
      {
        "range": { "start": { "line": 0, "character": 13 }, "end": { "line": 0, "character": 13 } },
        "newText": "$0",
      },
    ]);
    assert_eq!(expected_edits, edits);
}

#[test]
fn pipe_is_removed_if_user_adds_manually_after_block_argument() {
    let mut document = document("");
    push_edit(&mut document, (0, 0), (0, 0), "[].each do |elem|");
    push_edit(&mut document, (0, 17), (0, 17), "|");
    let edits = edits_for(&document, 0, 17, "|", "Visual Studio Code");
    let expected_edits = json!([
      {
        "range": { "start": { "line": 0, "character": 17 }, "end": { "line": 0, "character": 18 } },
        "newText": "",
      },
      {
        "range": { "start": { "line": 0, "character": 17 }, "end": { "line": 0, "character": 17 } },
        "newText": "$0",
      },
    ]);
    assert_eq!(expected_edits, edits);
}

#[test]
fn comment_continuation() {
    let mut document = document("");
    push_edit(&mut document, (0, 0), (0, 0), "    #    something");
    let edits = edits_for(&document, 0, 14, "\n", "Visual Studio Code");
    let expected_edits = json!([
      {
        "range": { "start": { "line": 0, "character": 14 }, "end": { "line": 0, "character": 14 } },
        "newText": "#    ",
      },
    ]);
    assert_eq!(expected_edits, edits);
}

#[test]
fn comment_continuation_does_not_apply_to_rbs_signatures() {
    let mut document = document("");
    push_edit(&mut document, (0, 0), (0, 0), "    #: (String) -> String");
    let edits = edits_for(&document, 0, 14, "\n", "Visual Studio Code");
    assert_eq!(json!([]), edits);
}

#[test]
fn comment_continuation_does_not_apply_to_trailing_rbs_signature() {
    let mut document = document("");
    push_edit(&mut document, (0, 0), (0, 0), "attr_reader :name #: String");
    let edits = edits_for(&document, 0, 14, "\n", "Visual Studio Code");
    assert_eq!(json!([]), edits);
}

#[test]
fn keyword_handling() {
    let mut document = document("");
    push_edit(&mut document, (0, 0), (0, 0), "\"def\"g\"");
    let edits = edits_for(&document, 0, 5, "\n", "Visual Studio Code");
    assert_eq!(json!([]), edits);
}

#[test]
fn comment_continuation_with_other_line_break_matches() {
    let mut document = document("");
    push_edit(&mut document, (0, 0), (0, 0), "    #    while");
    let edits = edits_for(&document, 0, 14, "\n", "Visual Studio Code");
    let expected_edits = json!([
      {
        "range": { "start": { "line": 0, "character": 14 }, "end": { "line": 0, "character": 14 } },
        "newText": "#    ",
      },
    ]);
    assert_eq!(expected_edits, edits);
}

#[test]
fn comment_continuation_when_inserting_new_line_in_the_middle() {
    let mut document = document("");
    push_edit(&mut document, (0, 0), (0, 0), "# while blah blah");
    let edits = edits_for(&document, 0, 7, "\n", "Visual Studio Code");
    let expected_edits = json!([
      {
        "range": { "start": { "line": 0, "character": 7 }, "end": { "line": 0, "character": 7 } },
        "newText": "# ",
      },
    ]);
    assert_eq!(expected_edits, edits);
}

#[test]
fn breaking_line_between_keyword_and_more_content() {
    let mut document = document("");
    push_edit(&mut document, (0, 0), (0, 0), "if something\n");
    let edits = edits_for(&document, 1, 2, "\n", "Visual Studio Code");
    let expected_edits = json!([
      {
        "range": { "start": { "line": 1, "character": 2 }, "end": { "line": 1, "character": 2 } },
        "newText": "\n",
      },
      {
        "range": { "start": { "line": 1, "character": 2 }, "end": { "line": 1, "character": 2 } },
        "newText": "end",
      },
      {
        "range": { "start": { "line": 1, "character": 2 }, "end": { "line": 1, "character": 2 } },
        "newText": "$0",
      },
    ]);
    assert_eq!(expected_edits, edits);
}

#[test]
fn breaking_line_between_keyword_when_there_is_content_on_the_next_line() {
    let mut document = document("");
    push_edit(&mut document, (0, 0), (0, 0), "if something\n  other_thing");
    let edits = edits_for(&document, 0, 2, "\n", "Visual Studio Code");
    assert_eq!(json!([]), edits);
}

#[test]
fn breaking_line_immediately_after_keyword() {
    let mut document = document("");
    push_edit(&mut document, (0, 0), (0, 0), "  def\nfoo");
    let edits = edits_for(&document, 1, 2, "\n", "Visual Studio Code");
    let expected_edits = json!([
      {
        "range": { "start": { "line": 2, "character": 2 }, "end": { "line": 2, "character": 2 } },
        "newText": "  end\n",
      },
      {
        "range": { "start": { "line": 0, "character": 6 }, "end": { "line": 0, "character": 6 } },
        "newText": "$0",
      },
    ]);
    assert_eq!(expected_edits, edits);
}

#[test]
fn auto_indent_after_end_keyword() {
    let document = document("if foo\nbar\nend");
    let edits = edits_for(&document, 2, 2, "d", "Visual Studio Code");
    let expected_edits = json!([
      {
        "range": { "start": { "line": 1, "character": 0 }, "end": { "line": 1, "character": 0 } },
        "newText": "  ",
      },
      {
        "range": { "start": { "line": 2, "character": 2 }, "end": { "line": 2, "character": 2 } },
        "newText": "$0",
      },
    ]);
    assert_eq!(expected_edits, edits);
}

#[test]
fn auto_indent_after_end_keyword_with_complex_body() {
    let document = document("if foo\nif bar\n  baz\nend\nend");
    let edits = edits_for(&document, 4, 2, "d", "Visual Studio Code");
    let expected_edits = json!([
      {
        "range": { "start": { "line": 1, "character": 0 }, "end": { "line": 1, "character": 0 } },
        "newText": "  ",
      },
      {
        "range": { "start": { "line": 2, "character": 0 }, "end": { "line": 2, "character": 0 } },
        "newText": "  ",
      },
      {
        "range": { "start": { "line": 3, "character": 0 }, "end": { "line": 3, "character": 0 } },
        "newText": "  ",
      },
      {
        "range": { "start": { "line": 4, "character": 2 }, "end": { "line": 4, "character": 2 } },
        "newText": "$0",
      },
    ]);
    assert_eq!(expected_edits, edits);
}

#[test]
fn auto_indent_after_end_keyword_does_not_add_extra_indentation() {
    let document = document("if foo\n  bar\nend");
    let edits = edits_for(&document, 2, 2, "d", "Visual Studio Code");
    let expected_edits = json!([
      {
        "range": { "start": { "line": 2, "character": 2 }, "end": { "line": 2, "character": 2 } },
        "newText": "$0",
      },
    ]);
    assert_eq!(expected_edits, edits);
}

#[test]
fn breaking_line_if_a_keyword_is_part_of_method_call() {
    let document = document("  force({");
    let edits = edits_for(&document, 1, 2, "\n", "Visual Studio Code");
    assert_eq!(json!([]), edits);
}

#[test]
fn breaking_line_if_a_keyword_in_a_subexpression() {
    let document = document("  var = (if");
    let edits = edits_for(&document, 1, 2, "\n", "Visual Studio Code");
    let expected_edits = json!([
      {
        "range": { "start": { "line": 1, "character": 2 }, "end": { "line": 1, "character": 2 } },
        "newText": "\n",
      },
      {
        "range": { "start": { "line": 1, "character": 2 }, "end": { "line": 1, "character": 2 } },
        "newText": "  end",
      },
      {
        "range": { "start": { "line": 1, "character": 4 }, "end": { "line": 1, "character": 4 } },
        "newText": "$0",
      },
    ]);
    assert_eq!(expected_edits, edits);
}

#[test]
fn adding_heredoc_delimiter() {
    let mut document = document("");
    push_edit(&mut document, (0, 0), (0, 0), "str = <<~STR");
    let edits = edits_for(&document, 1, 2, "\n", "Visual Studio Code");
    let expected_edits = json!([
      {
        "range": { "start": { "line": 1, "character": 2 }, "end": { "line": 1, "character": 2 } },
        "newText": "\n",
      },
      {
        "range": { "start": { "line": 1, "character": 2 }, "end": { "line": 1, "character": 2 } },
        "newText": "STR",
      },
      {
        "range": { "start": { "line": 1, "character": 2 }, "end": { "line": 1, "character": 2 } },
        "newText": "$0",
      },
    ]);
    assert_eq!(expected_edits, edits);
}

#[test]
fn plain_heredoc_completion() {
    let mut document = document("");
    push_edit(&mut document, (0, 0), (0, 0), "str = <<STR");
    let edits = edits_for(&document, 1, 2, "\n", "Visual Studio Code");
    let expected_edits = json!([
      {
        "range": { "start": { "line": 1, "character": 2 }, "end": { "line": 1, "character": 2 } },
        "newText": "\n",
      },
      {
        "range": { "start": { "line": 1, "character": 2 }, "end": { "line": 1, "character": 2 } },
        "newText": "STR",
      },
      {
        "range": { "start": { "line": 1, "character": 2 }, "end": { "line": 1, "character": 2 } },
        "newText": "$0",
      },
    ]);
    assert_eq!(expected_edits, edits);
}

#[test]
fn quoted_heredoc_completion() {
    let mut document = document("");
    push_edit(&mut document, (0, 0), (0, 0), "str = <<-'STR'");
    let edits = edits_for(&document, 1, 2, "\n", "Visual Studio Code");
    let expected_edits = json!([
      {
        "range": { "start": { "line": 1, "character": 2 }, "end": { "line": 1, "character": 2 } },
        "newText": "\n",
      },
      {
        "range": { "start": { "line": 1, "character": 2 }, "end": { "line": 1, "character": 2 } },
        "newText": "STR",
      },
      {
        "range": { "start": { "line": 1, "character": 2 }, "end": { "line": 1, "character": 2 } },
        "newText": "$0",
      },
    ]);
    assert_eq!(expected_edits, edits);
}

#[test]
fn completing_end_token_inside_parameters() {
    let document = document("foo(proc do\n)");
    let edits = edits_for(&document, 1, 0, "\n", "Visual Studio Code");
    let expected_edits = json!([
      {
        "range": { "start": { "line": 1, "character": 0 }, "end": { "line": 1, "character": 0 } },
        "newText": "\n",
      },
      {
        "range": { "start": { "line": 1, "character": 0 }, "end": { "line": 1, "character": 0 } },
        "newText": "end",
      },
      {
        "range": { "start": { "line": 1, "character": 2 }, "end": { "line": 1, "character": 2 } },
        "newText": "$0",
      },
    ]);
    assert_eq!(expected_edits, edits);
}

#[test]
fn completing_end_token_inside_brackets() {
    let document = document("foo[proc do\n]");
    let edits = edits_for(&document, 1, 0, "\n", "Visual Studio Code");
    let expected_edits = json!([
      {
        "range": { "start": { "line": 1, "character": 0 }, "end": { "line": 1, "character": 0 } },
        "newText": "\n",
      },
      {
        "range": { "start": { "line": 1, "character": 0 }, "end": { "line": 1, "character": 0 } },
        "newText": "end",
      },
      {
        "range": { "start": { "line": 1, "character": 2 }, "end": { "line": 1, "character": 2 } },
        "newText": "$0",
      },
    ]);
    assert_eq!(expected_edits, edits);
}

#[test]
fn no_snippet_if_not_vs_code() {
    let mut document = document("");
    push_edit(&mut document, (0, 0), (0, 0), "class Foo");
    let edits = edits_for(&document, 1, 2, "\n", "Foo");
    let expected_edits = json!([
      {
        "range": { "start": { "line": 1, "character": 2 }, "end": { "line": 1, "character": 2 } },
        "newText": "\n",
      },
      {
        "range": { "start": { "line": 1, "character": 2 }, "end": { "line": 1, "character": 2 } },
        "newText": "end",
      },
    ]);
    assert_eq!(expected_edits, edits);
}

#[test]
fn includes_snippets_on_vscode_insiders() {
    let mut document = document("");
    push_edit(&mut document, (0, 0), (0, 0), "class Foo");
    let edits = edits_for(&document, 1, 2, "\n", "Visual Studio Code - Insiders");
    let expected_edits = json!([
      {
        "range": { "start": { "line": 1, "character": 2 }, "end": { "line": 1, "character": 2 } },
        "newText": "\n",
      },
      {
        "range": { "start": { "line": 1, "character": 2 }, "end": { "line": 1, "character": 2 } },
        "newText": "end",
      },
      {
        "range": { "start": { "line": 1, "character": 2 }, "end": { "line": 1, "character": 2 } },
        "newText": "$0",
      },
    ]);
    assert_eq!(expected_edits, edits);
}

#[test]
fn includes_snippets_on_cursor() {
    let mut document = document("");
    push_edit(&mut document, (0, 0), (0, 0), "class Foo");
    let edits = edits_for(&document, 1, 2, "\n", "Cursor");
    let expected_edits = json!([
      {
        "range": { "start": { "line": 1, "character": 2 }, "end": { "line": 1, "character": 2 } },
        "newText": "\n",
      },
      {
        "range": { "start": { "line": 1, "character": 2 }, "end": { "line": 1, "character": 2 } },
        "newText": "end",
      },
      {
        "range": { "start": { "line": 1, "character": 2 }, "end": { "line": 1, "character": 2 } },
        "newText": "$0",
      },
    ]);
    assert_eq!(expected_edits, edits);
}

#[test]
fn includes_snippets_on_vscodium() {
    let mut document = document("");
    push_edit(&mut document, (0, 0), (0, 0), "class Foo");
    let edits = edits_for(&document, 1, 2, "\n", "VSCodium");
    let expected_edits = json!([
      {
        "range": { "start": { "line": 1, "character": 2 }, "end": { "line": 1, "character": 2 } },
        "newText": "\n",
      },
      {
        "range": { "start": { "line": 1, "character": 2 }, "end": { "line": 1, "character": 2 } },
        "newText": "end",
      },
      {
        "range": { "start": { "line": 1, "character": 2 }, "end": { "line": 1, "character": 2 } },
        "newText": "$0",
      },
    ]);
    assert_eq!(expected_edits, edits);
}

#[test]
fn includes_snippets_on_windsurf() {
    let mut document = document("");
    push_edit(&mut document, (0, 0), (0, 0), "class Foo");
    let edits = edits_for(&document, 1, 2, "\n", "Windsurf");
    let expected_edits = json!([
      {
        "range": { "start": { "line": 1, "character": 2 }, "end": { "line": 1, "character": 2 } },
        "newText": "\n",
      },
      {
        "range": { "start": { "line": 1, "character": 2 }, "end": { "line": 1, "character": 2 } },
        "newText": "end",
      },
      {
        "range": { "start": { "line": 1, "character": 2 }, "end": { "line": 1, "character": 2 } },
        "newText": "$0",
      },
    ]);
    assert_eq!(expected_edits, edits);
}

#[test]
fn does_not_confuse_class_parameter_with_keyword() {
    let mut document = document("");
    push_edit(
        &mut document,
        (0, 0),
        (0, 0),
        "link_to :something,\n  class: 'foo',\n  ",
    );
    let edits = edits_for(&document, 2, 4, "\n", "Visual Studio Code");
    assert_eq!(json!([]), edits);
}

#[test]
fn allows_end_completion_when_parenthesis_are_present() {
    let mut document = document("");
    push_edit(&mut document, (0, 0), (0, 0), "if(\n  ");
    let edits = edits_for(&document, 1, 2, "\n", "Visual Studio Code");
    let expected_edits = json!([
      {
        "range": { "start": { "line": 1, "character": 2 }, "end": { "line": 1, "character": 2 } },
        "newText": "\n",
      },
      {
        "range": { "start": { "line": 1, "character": 2 }, "end": { "line": 1, "character": 2 } },
        "newText": "end",
      },
      {
        "range": { "start": { "line": 1, "character": 2 }, "end": { "line": 1, "character": 2 } },
        "newText": "$0",
      },
    ]);
    assert_eq!(expected_edits, edits);
}

#[test]
fn does_not_add_end_for_endless_method() {
    let mut document = document("");
    push_edit(&mut document, (0, 0), (0, 0), "def foo = 42");
    let edits = edits_for(&document, 1, 0, "\n", "Visual Studio Code");
    assert_eq!(json!([]), edits);
}

#[test]
fn does_not_add_end_for_multiline_endless_method() {
    let mut document = document("");
    push_edit(&mut document, (0, 0), (0, 0), "def hello = puts(");
    let edits = edits_for(&document, 1, 0, "\n", "Visual Studio Code");
    assert_eq!(json!([]), edits);
}

#[test]
fn does_not_add_end_for_endless_method_with_equals_and_parameters() {
    let mut document = document("");
    push_edit(&mut document, (0, 0), (0, 0), "def foo=(bar) = 42");
    let edits = edits_for(&document, 1, 0, "\n", "Visual Studio Code");
    assert_eq!(json!([]), edits);
}
