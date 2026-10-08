mod support;

use serde_json::{Value, json};
use support::{DEFAULT_URI, TestOptions, with_server};

fn folding_ranges(source: &str) -> Value {
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    server.result("textDocument/foldingRange", json!({ "textDocument": { "uri": DEFAULT_URI } }))
}

#[test]
fn trailing_comments_are_not_grouped() {
    let source = "a = 1 # first\n# second\n# third\n";
    assert_eq!(folding_ranges(source), json!([{ "startLine": 1, "endLine": 2, "kind": "comment" }]));
}

#[test]
fn requires_are_grouped_until_another_range() {
    let source = "require \"a\"\nKernel.require \"b\"\nrequire_relative \"c\"\nclass Foo\n  require \"d\"\nend\n";
    assert_eq!(
        folding_ranges(source),
        json!([
            { "startLine": 0, "endLine": 2, "kind": "imports" },
            { "startLine": 3, "endLine": 4, "kind": "region" },
        ])
    );
}

#[test]
fn erb_documents_fold_ruby_code() {
    let mut server = with_server(None, DEFAULT_URI, TestOptions::default());
    let uri = "file:///fake.html.erb";
    server.did_open(uri, "<ul>\n<% [1].each do |item| %>\n  <li><%= item %></li>\n<% end %>\n</ul>\n", "erb");
    let result = server.result("textDocument/foldingRange", json!({ "textDocument": { "uri": uri } }));
    assert_eq!(
        result,
        json!([
            { "startLine": 1, "endLine": 2, "kind": "region" },
            { "startLine": 1, "endLine": 2, "kind": "region" },
        ])
    );
}
