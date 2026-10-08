mod support;

use serde_json::{Value, json};
use support::{DEFAULT_URI, TestOptions, repository_path, with_server};

fn show_syntax_tree(source: &str, range: Option<Value>) -> String {
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    let mut params = json!({ "textDocument": { "uri": DEFAULT_URI } });
    if let Some(range) = range {
        params["range"] = range;
    }
    let result = server.result("rubyLsp/textDocument/showSyntaxTree", params);
    result["ast"].as_str().unwrap_or_else(|| panic!("Expected an ast string, got {result}")).to_string()
}

#[test]
fn returns_partial_tree_if_document_has_syntax_error() {
    let expected = r#"@ ProgramNode (location: (1,0)-(1,6))
├── flags: ∅
├── locals: []
└── statements:
    @ StatementsNode (location: (1,0)-(1,6))
    ├── flags: ∅
    └── body: (length: 1)
        └── @ CallNode (location: (1,0)-(1,6))
            ├── flags: newline, ignore_visibility
            ├── receiver: ∅
            ├── call_operator_loc: ∅
            ├── name: :foo
            ├── message_loc: (1,0)-(1,3) = "foo"
            ├── opening_loc: ∅
            ├── arguments: ∅
            ├── closing_loc: ∅
            ├── equal_loc: ∅
            └── block:
                @ BlockNode (location: (1,4)-(1,6))
                ├── flags: ∅
                ├── locals: []
                ├── parameters: ∅
                ├── body:
                │   @ StatementsNode (location: (1,4)-(1,6))
                │   ├── flags: ∅
                │   └── body: (length: 1)
                │       └── @ MissingNode (location: (1,4)-(1,6))
                │           └── flags: newline
                ├── opening_loc: (1,4)-(1,6) = "do"
                └── closing_loc: (1,6)-(1,6) = ""
"#;
    assert_eq!(show_syntax_tree("foo do", None), expected);
}

#[test]
fn returns_ast_if_document_is_parsed() {
    let expected = r#"@ ProgramNode (location: (1,0)-(1,9))
├── flags: ∅
├── locals: [:foo]
└── statements:
    @ StatementsNode (location: (1,0)-(1,9))
    ├── flags: ∅
    └── body: (length: 1)
        └── @ LocalVariableWriteNode (location: (1,0)-(1,9))
            ├── flags: newline
            ├── name: :foo
            ├── depth: 0
            ├── name_loc: (1,0)-(1,3) = "foo"
            ├── value:
            │   @ IntegerNode (location: (1,6)-(1,9))
            │   ├── flags: static_literal, decimal
            │   └── value: 123
            └── operator_loc: (1,4)-(1,5) = "="
"#;
    assert_eq!(show_syntax_tree("foo = 123", None), expected);
}

#[test]
fn returns_ast_for_a_selection() {
    let source = "foo = 123\nbar = 456\nhello = 123\n";
    let expected = r#"@ LocalVariableWriteNode (location: (1,0)-(1,9))
├── flags: newline
├── name: :foo
├── depth: 0
├── name_loc: (1,0)-(1,3) = "foo"
├── value:
│   @ IntegerNode (location: (1,6)-(1,9))
│   ├── flags: static_literal, decimal
│   └── value: 123
└── operator_loc: (1,4)-(1,5) = "="

@ LocalVariableWriteNode (location: (2,0)-(2,9))
├── flags: newline
├── name: :bar
├── depth: 0
├── name_loc: (2,0)-(2,3) = "bar"
├── value:
│   @ IntegerNode (location: (2,6)-(2,9))
│   ├── flags: static_literal, decimal
│   └── value: 456
└── operator_loc: (2,4)-(2,5) = "="
"#;

    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    let first = server.result(
        "rubyLsp/textDocument/showSyntaxTree",
        json!({
            "textDocument": { "uri": DEFAULT_URI },
            "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 1, "character": 9 } },
        }),
    );
    assert_eq!(first["ast"], expected);

    let second = server.result(
        "rubyLsp/textDocument/showSyntaxTree",
        json!({
            "textDocument": { "uri": DEFAULT_URI },
            "range": { "start": { "line": 1, "character": 0 }, "end": { "line": 1, "character": 9 } },
        }),
    );
    assert!(!second["ast"].as_str().unwrap().is_empty());
}

#[test]
fn returns_null_for_non_ruby_documents() {
    let uri = "file:///fake.html.erb";
    let mut server = with_server(Some("<%= foo %>"), uri, TestOptions::default());
    let result = server.result("rubyLsp/textDocument/showSyntaxTree", json!({ "textDocument": { "uri": uri } }));
    assert_eq!(result, Value::Null);
}

fn ruby_prism_inspect(paths: &[std::path::PathBuf]) -> Option<Vec<String>> {
    let script = "exit 2 unless Prism::VERSION == '1.9.0'; print JSON.generate(ARGV.map { |path| Prism.parse(File.read(path)).value.inspect })";
    let output = std::process::Command::new("ruby").args(["-rprism", "-rjson", "-e", script]).args(paths).output().ok()?;
    if !output.status.success() {
        return None;
    }
    serde_json::from_slice(&output.stdout).ok()
}

fn fixtures_in(directory: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut fixtures: Vec<_> = std::fs::read_dir(directory)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "rb"))
        .collect();
    fixtures.sort();
    fixtures
}

/// Ruby LSP prints Prism's own `inspect` output, so every fixture is compared against the Prism gem matching the
/// vendored parser version when it is installed
#[test]
fn matches_prism_inspect_for_every_fixture() {
    let directory = std::env::var_os("PRISM_INSPECT_FIXTURES")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| repository_path().join("tests/fixtures"));
    let fixtures = fixtures_in(&directory);
    let Some(expectations) = ruby_prism_inspect(&fixtures) else {
        eprintln!("Skipping: Ruby with the prism 1.9.0 gem is not available");
        return;
    };

    let mut failures = Vec::new();
    for (fixture, expected) in fixtures.iter().zip(expectations) {
        let source = String::from_utf8_lossy(&std::fs::read(fixture).unwrap()).into_owned();
        let actual = show_syntax_tree(&source, None);
        if actual != expected {
            failures.push(format!(
                "{}\n########## Expected ##########\n{expected}\n##########  Actual  ##########\n{actual}",
                fixture.display()
            ));
        }
    }
    assert!(failures.is_empty(), "{} fixture(s) differ:\n\n{}", failures.len(), failures.join("\n\n"));
}
