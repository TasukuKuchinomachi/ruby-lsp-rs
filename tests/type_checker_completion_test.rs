mod support;

use std::path::Path;
use std::sync::{Arc, OnceLock};

use rbs_environment::{Environment, Loader};
use serde_json::{Value, json};
use support::{DEFAULT_URI, TestOptions, TestServer, rbs_core_path, with_server};

const SIGNATURES: &str = r#"
class Foo
  # Returns the name of the foo
  def name: () -> String
  def size: () -> Integer
  def format: (Integer width) -> String
            | (Integer width, String padding) -> String
  def configure: (String name, ?verbose: bool) -> void
  def self.build: () -> Foo

  private

  def secret: () -> String
end

class Bar
  def name: () -> String
  def bar_only: () -> Integer
  def maybe_foo: () -> Foo?
  def use: (Foo foo) -> void
  def choose: (bool flag) -> void
end
"#;

fn environment() -> Option<Arc<Environment>> {
    static ENVIRONMENT: OnceLock<Option<Arc<Environment>>> = OnceLock::new();
    ENVIRONMENT
        .get_or_init(|| {
            let core_directory = rbs_core_path()?;
            let mut loader = Loader::new();
            loader.add_library_directory(&core_directory);
            loader.add_signature_source(Path::new("sig/fixture.rbs"), SIGNATURES);
            Some(Arc::new(loader.finish()))
        })
        .clone()
}

fn type_checked_server(source: &str) -> Option<TestServer> {
    let Some(environment) = environment() else {
        eprintln!("Skipping: the rbs gem was not found");
        return None;
    };
    let options = TestOptions {
        stub_no_typechecker: true,
        type_checking_environment: Some(environment),
        ..TestOptions::default()
    };
    Some(with_server(Some(source), DEFAULT_URI, options))
}

fn completion(server: &mut TestServer, line: u32, character: u32, trigger_character: Option<&str>) -> Vec<Value> {
    let mut params = json!({ "textDocument": { "uri": DEFAULT_URI }, "position": { "line": line, "character": character } });
    if let Some(trigger_character) = trigger_character {
        params["context"] = json!({ "triggerKind": 2, "triggerCharacter": trigger_character });
    }
    server.result("textDocument/completion", params).as_array().cloned().unwrap_or_default()
}

fn labels(items: &[Value]) -> Vec<&str> {
    items.iter().map(|item| item["label"].as_str().unwrap()).collect()
}

fn item<'a>(items: &'a [Value], label: &str) -> &'a Value {
    items.iter().find(|item| item["label"] == label).unwrap_or_else(|| panic!("No item for {label} in {:?}", labels(items)))
}

const FOO_IMPLEMENTATION: &str = "class Foo\n  def ruby_only\n  end\nend\n";

#[test]
fn completes_methods_of_a_local_variable_assigned_from_new() {
    let source = format!("{FOO_IMPLEMENTATION}foo = Foo.new\nfoo.\n");
    let Some(mut server) = type_checked_server(&source) else { return };
    let items = completion(&mut server, 5, 4, Some("."));
    let labels = labels(&items);
    for expected in ["name", "size", "format", "configure", "ruby_only", "frozen?"] {
        assert!(labels.contains(&expected), "{expected} is missing from {labels:?}");
    }
    assert!(!labels.contains(&"secret"), "{labels:?}");
    assert!(!labels.contains(&"build"), "{labels:?}");
    assert_eq!(labels.iter().filter(|label| **label == "name").count(), 1);

    let name = item(&items, "name");
    assert_eq!(name["kind"], 2);
    assert_eq!(name["labelDetails"], json!({ "detail": "() -> String", "description": "Foo" }));
    assert_eq!(name["textEdit"]["range"], support::range_json(5, 4, 5, 4));
    assert_eq!(name["data"]["receiver_type"], "Foo");
    assert_eq!(item(&items, "ruby_only")["data"]["receiver_type"], "Foo");
}

#[test]
fn completes_methods_of_a_parameter_typed_by_its_signature() {
    let source = "class Bar\n  def use(foo)\n    foo.\n  end\nend\n";
    let Some(mut server) = type_checked_server(source) else { return };
    let items = completion(&mut server, 2, 8, Some("."));
    let labels = labels(&items);
    assert!(labels.contains(&"size"), "{labels:?}");
    assert!(!labels.contains(&"bar_only"), "{labels:?}");
}

#[test]
fn completes_methods_starting_with_a_partial_name() {
    let source = "foo = Foo.new\nfoo.si\n";
    let Some(mut server) = type_checked_server(source) else { return };
    let items = completion(&mut server, 1, 6, None);
    let labels = labels(&items);
    assert!(labels.contains(&"size"), "{labels:?}");
    assert!(labels.iter().all(|label| label.starts_with("si")), "{labels:?}");
    assert_eq!(item(&items, "size")["textEdit"]["range"], support::range_json(1, 4, 1, 6));
}

#[test]
fn completes_methods_of_an_optional_receiver_after_safe_navigation() {
    let source = "Bar.new.maybe_foo&.\n";
    let Some(mut server) = type_checked_server(source) else { return };
    let items = completion(&mut server, 0, 19, Some("."));
    let labels = labels(&items);
    assert!(labels.contains(&"size"), "{labels:?}");
    assert!(!labels.contains(&"rationalize"), "NilClass methods are offered: {labels:?}");
}

#[test]
fn completes_methods_of_every_member_of_a_union() {
    let source = "class Bar\n  def choose(flag)\n    value = flag ? Foo.new : Bar.new\n    value.\n  end\nend\n";
    let Some(mut server) = type_checked_server(source) else { return };
    let items = completion(&mut server, 3, 10, Some("."));
    let labels = labels(&items);
    assert!(labels.contains(&"size"), "{labels:?}");
    assert!(labels.contains(&"bar_only"), "{labels:?}");
    assert_eq!(labels.iter().filter(|label| **label == "name").count(), 1);
    assert_eq!(item(&items, "size")["data"]["receiver_type"], "Foo");
    assert_eq!(item(&items, "bar_only")["data"]["receiver_type"], "Bar");
}

#[test]
fn guesses_the_receiver_when_the_type_checker_does_not_know_it() {
    let source = "class Baz\n  def hello\n  end\nend\n\ndef greet(baz)\n  baz.\nend\n";
    let Some(mut server) = type_checked_server(source) else { return };
    let items = completion(&mut server, 6, 6, Some("."));
    assert_eq!(labels(&items), vec!["hello"]);
    assert_eq!(item(&items, "hello")["data"]["guessed_type"], "Baz");
}

#[test]
fn completes_from_the_index_when_the_type_checker_is_inactive() {
    let source = format!("{FOO_IMPLEMENTATION}Foo.new.\n");
    let mut server = with_server(Some(&source), DEFAULT_URI, TestOptions { stub_no_typechecker: true, ..TestOptions::default() });
    let items = completion(&mut server, 4, 8, Some("."));
    assert_eq!(labels(&items), vec!["ruby_only"]);
    assert!(item(&items, "ruby_only")["data"].get("receiver_type").is_none());
}

#[test]
fn completes_singleton_methods_of_a_class() {
    let source = "Foo.\n";
    let Some(mut server) = type_checked_server(source) else { return };
    let items = completion(&mut server, 0, 4, Some("."));
    let labels = labels(&items);
    assert!(labels.contains(&"build"), "{labels:?}");
    assert!(labels.contains(&"new"), "{labels:?}");
    assert!(!labels.contains(&"size"), "{labels:?}");
    assert_eq!(item(&items, "build")["data"]["receiver_type"], "singleton(Foo)");
}

#[test]
fn resolves_the_signature_and_the_comment_of_a_method() {
    let source = "foo = Foo.new\nfoo.\n";
    let Some(mut server) = type_checked_server(source) else { return };
    let items = completion(&mut server, 1, 4, Some("."));
    let resolved = server.result("completionItem/resolve", item(&items, "name").clone());
    let documentation = resolved["documentation"]["value"].as_str().unwrap();
    assert!(documentation.contains("```rbs\ndef name: () -> String\n```"), "{documentation}");
    assert!(documentation.contains("Defined in `Foo`"), "{documentation}");
    assert!(documentation.contains("Returns the name of the foo"), "{documentation}");

    let resolved = server.result("completionItem/resolve", item(&items, "format").clone());
    let documentation = resolved["documentation"]["value"].as_str().unwrap();
    assert!(
        documentation.contains("def format: (Integer width) -> String\n          | (Integer width, String padding) -> String"),
        "{documentation}"
    );
}

#[test]
fn resolves_methods_implemented_only_in_ruby_from_the_index() {
    let source = format!("{FOO_IMPLEMENTATION}foo = Foo.new\nfoo.\n");
    let Some(mut server) = type_checked_server(&source) else { return };
    let items = completion(&mut server, 5, 4, Some("."));
    let resolved = server.result("completionItem/resolve", item(&items, "ruby_only").clone());
    let documentation = resolved["documentation"]["value"].as_str().unwrap();
    assert!(documentation.starts_with("```ruby\nruby_only()"), "{documentation}");
}

fn signature_help(server: &mut TestServer, line: u32, character: u32) -> Value {
    server.result(
        "textDocument/signatureHelp",
        json!({ "textDocument": { "uri": DEFAULT_URI }, "position": { "line": line, "character": character } }),
    )
}

#[test]
fn shows_every_overload_as_a_signature() {
    let source = "foo = Foo.new\nfoo.format(1, )\n";
    let Some(mut server) = type_checked_server(source) else { return };
    let result = signature_help(&mut server, 1, 14);
    let signature_labels: Vec<&str> =
        result["signatures"].as_array().unwrap().iter().map(|signature| signature["label"].as_str().unwrap()).collect();
    assert_eq!(signature_labels, vec!["format(Integer width) -> String", "format(Integer width, String padding) -> String"]);
    assert_eq!(result["activeSignature"], 1);
    assert_eq!(result["activeParameter"], 1);
    assert_eq!(result["signatures"][1]["parameters"], json!([{ "label": [7, 20] }, { "label": [22, 36] }]));
    assert!(result["signatures"][0]["documentation"]["value"].as_str().unwrap().contains("Defined in `Foo`"));
}

#[test]
fn shows_the_first_parameter_of_an_unfinished_call() {
    let source = "foo = Foo.new\nfoo.configure(\n";
    let Some(mut server) = type_checked_server(source) else { return };
    let result = signature_help(&mut server, 1, 14);
    assert_eq!(result["signatures"][0]["label"], "configure(String name, ?verbose: bool) -> void");
    assert_eq!(result["activeParameter"], 0);
}

#[test]
fn activates_the_keyword_parameter_being_written() {
    let source = "foo = Foo.new\nfoo.configure(\"a\", verbose: true)\n";
    let Some(mut server) = type_checked_server(source) else { return };
    let result = signature_help(&mut server, 1, 25);
    assert_eq!(result["activeParameter"], 1);
}

#[test]
fn falls_back_to_the_index_signature_when_the_receiver_is_unknown() {
    let source = "class Baz\n  def hello(a, b)\n  end\n\n  def greet\n    hello()\n  end\nend\n";
    let Some(mut server) = type_checked_server(source) else { return };
    let result = signature_help(&mut server, 5, 10);
    assert_eq!(result["signatures"][0]["label"], "hello(a, b)");
}

#[test]
fn activates_the_next_overload_in_an_unfinished_call_inside_a_method() {
    let source = "class Bar\n  def use(foo)\n    foo.format(1, \n  end\nend\n";
    let Some(mut server) = type_checked_server(source) else { return };
    let result = signature_help(&mut server, 2, 18);
    assert_eq!(result["activeSignature"], 1);
    assert_eq!(result["activeParameter"], 1);
}
