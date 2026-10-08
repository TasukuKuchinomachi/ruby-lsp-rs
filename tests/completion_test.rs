mod support;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use ruby_lsp_rs::indexer::rbs_indexer::RbsIndexer;
use serde_json::{Value, json};
use support::{DEFAULT_URI, TestOptions, TestServer, rbs_core_path, with_server};

fn no_typechecker() -> TestOptions {
    TestOptions { stub_no_typechecker: true, ..TestOptions::default() }
}

fn completion_at(server: &mut TestServer, uri: &str, line: u32, character: u32) -> Vec<Value> {
    let result = server.result(
        "textDocument/completion",
        json!({ "textDocument": { "uri": uri }, "position": { "line": line, "character": character } }),
    );
    result.as_array().cloned().unwrap_or_else(|| panic!("Expected an array, got {result}"))
}

fn completion(server: &mut TestServer, line: u32, character: u32) -> Vec<Value> {
    let uri = server.uri.clone();
    completion_at(server, &uri, line, character)
}

fn strings(items: &[Value], pointer: &str) -> Vec<String> {
    items.iter().map(|item| item.pointer(pointer).and_then(Value::as_str).unwrap_or_default().to_string()).collect()
}

fn labels(items: &[Value]) -> Vec<String> {
    strings(items, "/label")
}

fn filter_texts(items: &[Value]) -> Vec<String> {
    strings(items, "/filterText")
}

fn new_texts(items: &[Value]) -> Vec<String> {
    strings(items, "/textEdit/newText")
}

fn descriptions(items: &[Value]) -> Vec<String> {
    strings(items, "/labelDetails/description")
}

fn index_ruby_core(server: &TestServer) -> bool {
    let Some(rbs_core_path) = rbs_core_path() else { return false };
    RbsIndexer::new(&mut server.index()).index_core_directory(&rbs_core_path);
    true
}

struct FileStructure {
    directory: PathBuf,
}

impl Drop for FileStructure {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

static FILE_STRUCTURE_COUNTER: AtomicUsize = AtomicUsize::new(0);

fn with_file_structure(server: &TestServer) -> FileStructure {
    let directory = std::env::temp_dir().join(format!(
        "path_completion_test_{}_{}",
        std::process::id(),
        FILE_STRUCTURE_COUNTER.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(directory.join("foo/support")).unwrap();
    let load_path_entry = directory.to_string_lossy().into_owned();
    for relative_path in ["foo/bar.rb", "foo/baz.rb", "foo/quux.rb", "foo/support/bar.rb", "foo/support/baz.rb", "foo/support/quux.rb"] {
        let path = directory.join(relative_path);
        std::fs::write(&path, "").unwrap();
        let require_path = ruby_lsp_rs::uri::require_path(&path.to_string_lossy(), &load_path_entry);
        server.index_file(&path, Some(&require_path));
    }
    FileStructure { directory }
}

fn path_completion(path: &str, start_character: u32, end_character: u32) -> Value {
    json!({
        "label": path,
        "textEdit": {
            "range": {
                "start": { "line": 0, "character": start_character },
                "end": { "line": 0, "character": end_character },
            },
            "newText": path,
        },
        "kind": 17,
    })
}

fn quote_positions(source: &str) -> (u32, u32) {
    (source.find('"').unwrap() as u32 + 1, source.rfind('"').unwrap() as u32)
}

fn open(server: &TestServer, path: &Path, source: &str) -> String {
    let uri = ruby_lsp_rs::uri::from_path_buf(path);
    server.did_open(&uri, source, "ruby");
    uri
}

#[test]
fn completion_keyword() {
    let mut server = with_server(Some("foo d\n"), DEFAULT_URI, no_typechecker());
    let _structure = with_file_structure(&server);
    let result = completion(&mut server, 0, 5);
    assert_eq!(labels(&result), vec!["def", "defined?", "do"]);
}

fn assert_require_completion(source: &str, expected_paths: &[&str]) {
    let (start_character, end_character) = quote_positions(source);
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    let _structure = with_file_structure(&server);
    let result = completion(&mut server, 0, end_character);
    let expected: Vec<Value> =
        expected_paths.iter().map(|path| path_completion(path, start_character, end_character)).collect();
    assert_eq!(result, expected);
}

const ALL_FOO_PATHS: &[&str] =
    &["foo/bar", "foo/baz", "foo/quux", "foo/support/bar", "foo/support/baz", "foo/support/quux"];

#[test]
fn completion_command() {
    assert_require_completion("require \"foo/\"\n", ALL_FOO_PATHS);
}

#[test]
fn completion_call() {
    assert_require_completion("require(\"foo/\")\n", ALL_FOO_PATHS);
}

#[test]
fn completion_command_call() {
    assert_require_completion("Kernel.require \"foo/\"\n", ALL_FOO_PATHS);
}

#[test]
fn completion_with_partial_path() {
    assert_require_completion("require \"foo/suppo\"\n", &["foo/support/bar", "foo/support/baz", "foo/support/quux"]);
}

#[test]
fn completion_does_not_fail_when_there_are_syntax_errors() {
    let source = "require \"ruby_lsp/requests/\"\n\ndef foo\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    let _structure = with_file_structure(&server);
    let end_character = source.rfind('"').unwrap() as u32;
    let response = server.request(
        "textDocument/completion",
        json!({ "textDocument": { "uri": DEFAULT_URI }, "position": { "line": 0, "character": end_character } }),
    );
    assert!(response.response_result.is_ok());
}

#[test]
fn completion_is_not_triggered_if_argument_is_not_a_string() {
    let source = "require foo\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    let _structure = with_file_structure(&server);
    assert!(completion(&mut server, 0, source.rfind('o').unwrap() as u32).is_empty());
}

#[test]
fn completion_for_fully_qualified_paths_inside_namespace() {
    let source = "module Foo\n  module Bar\n    class Baz\n    end\n\n    Foo::\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, no_typechecker());
    let _structure = with_file_structure(&server);
    assert_eq!(labels(&completion(&mut server, 5, 9)), vec!["Foo::Bar", "Foo::Bar::Baz"]);
}

#[test]
fn completion_for_constants() {
    let mut server = with_server(Some("class Foo\nend\n\nF\n"), DEFAULT_URI, no_typechecker());
    let _structure = with_file_structure(&server);
    let result = completion(&mut server, 3, 1);
    assert_eq!(labels(&result), vec!["Foo"]);
    assert_eq!(descriptions(&result), vec!["fake.rb"]);
}

#[test]
fn completion_for_constant_paths() {
    let source = "class Bar\nend\n\nclass Foo::Bar\nend\n\nmodule Foo\n  B\nend\n\nFoo::B\n";
    let mut server = with_server(Some(source), DEFAULT_URI, no_typechecker());
    let _structure = with_file_structure(&server);

    let result = completion(&mut server, 7, 3);
    assert_eq!(labels(&result), vec!["Foo::Bar", "Bar"]);
    assert_eq!(filter_texts(&result), vec!["Bar", "::Bar"]);
    assert_eq!(new_texts(&result), vec!["Bar", "::Bar"]);

    let result = completion(&mut server, 10, 6);
    assert_eq!(labels(&result), vec!["Foo::Bar"]);
    assert_eq!(filter_texts(&result), vec!["Foo::Bar"]);
    assert_eq!(new_texts(&result), vec!["Foo::Bar"]);
}

#[test]
fn completion_conflicting_constants() {
    let source = "module Foo\n  class Qux; end\n\n  module Bar\n    class Qux; end\n\n    Q\n  end\n\n  Q\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, no_typechecker());
    let _structure = with_file_structure(&server);
    let result = completion(&mut server, 6, 5);
    assert_eq!(labels(&result), vec!["Foo::Bar::Qux", "Foo::Qux"]);
    assert_eq!(filter_texts(&result), vec!["Qux", "Foo::Qux"]);
    assert_eq!(new_texts(&result), vec!["Qux", "Foo::Qux"]);
}

#[test]
fn completion_for_top_level_constants_inside_nesting() {
    let source = "class Bar\nend\n\nclass Foo::Bar\nend\n\nmodule Foo\n  ::B\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, no_typechecker());
    let _structure = with_file_structure(&server);
    let result = completion(&mut server, 7, 5);
    assert_eq!(labels(&result), vec!["Bar"]);
    assert_eq!(filter_texts(&result), vec!["::Bar"]);
    assert_eq!(new_texts(&result), vec!["::Bar"]);
}

#[test]
fn completion_private_constants_inside_the_same_namespace() {
    let source = "class A\n  CONST = 1\n  private_constant(:CONST)\n\n  C\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, no_typechecker());
    let _structure = with_file_structure(&server);
    assert_eq!(new_texts(&completion(&mut server, 3, 4)), vec!["CONST"]);
}

#[test]
fn completion_private_constants_from_different_namespace() {
    let source = "class A\n  CONST = 1\n  private_constant(:CONST)\nend\n\nA::C\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    let _structure = with_file_structure(&server);
    assert!(completion(&mut server, 4, 5).is_empty());
}

#[test]
fn completion_for_aliased_constants() {
    let source = "module A\n  module B\n    CONST = 1\n  end\nend\n\nmodule Other\n  ALIAS_NAME = A\n\n  ALIAS_NAME::B::C\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, no_typechecker());
    let _structure = with_file_structure(&server);
    let result = completion(&mut server, 9, 18);
    assert_eq!(labels(&result), vec!["ALIAS_NAME::B::CONST"]);
    assert_eq!(filter_texts(&result), vec!["ALIAS_NAME::B::CONST"]);
    assert_eq!(new_texts(&result), vec!["ALIAS_NAME::B::CONST"]);
}

#[test]
fn completion_for_aliased_complex_constants() {
    let source = "module A\n  module B\n    CONST = 1\n  end\nend\n\nmodule Other\n  ALIAS_NAME = A\nend\n\nFINAL_ALIAS = Other\nFINAL_ALIAS::ALIAS_NAME::B::C\n";
    let mut server = with_server(Some(source), DEFAULT_URI, no_typechecker());
    let _structure = with_file_structure(&server);
    let result = completion(&mut server, 11, 29);
    assert_eq!(labels(&result), vec!["FINAL_ALIAS::ALIAS_NAME::B::CONST"]);
    assert_eq!(filter_texts(&result), vec!["FINAL_ALIAS::ALIAS_NAME::B::CONST"]);
    assert_eq!(new_texts(&result), vec!["FINAL_ALIAS::ALIAS_NAME::B::CONST"]);
}

#[test]
fn completion_uses_shortest_possible_name_for_filter_text() {
    let source = "module A\n  module B\n    class Foo\n    end\n\n    F\n    A::B::F\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, no_typechecker());
    let _structure = with_file_structure(&server);

    let result = completion(&mut server, 5, 5);
    assert_eq!(labels(&result), vec!["A::B::Foo"]);
    assert_eq!(filter_texts(&result), vec!["Foo"]);
    assert_eq!(new_texts(&result), vec!["Foo"]);

    let result = completion(&mut server, 6, 11);
    assert_eq!(labels(&result), vec!["A::B::Foo"]);
    assert_eq!(filter_texts(&result), vec!["A::B::Foo"]);
    assert_eq!(new_texts(&result), vec!["Foo"]);
}

#[test]
fn completion_for_inherited_constants() {
    let source = "module Foo\n  Bar = 1\nend\n\nclass Baz\n  include Foo\n\n  # Not inherited: direct reference\n  Foo::B\n\n  # Inherited through include\n  B\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, no_typechecker());
    assert_eq!(labels(&completion(&mut server, 8, 8)), vec!["Foo::Bar"]);
    assert_eq!(labels(&completion(&mut server, 11, 3)), vec!["Foo::Bar", "Baz"]);
}

#[test]
fn completion_for_inherited_constants_with_constant_path() {
    let source = "module Foo\n  Bar = 1\nend\n\nclass Baz\n  include Foo\nend\n\nBaz::B\n";
    let mut server = with_server(Some(source), DEFAULT_URI, no_typechecker());
    let result = completion(&mut server, 8, 6);
    let item = &result[0];
    assert_eq!(item["label"], "Baz::Bar");
    assert_eq!(item["textEdit"]["newText"], "Baz::Bar");
    assert_eq!(item["textEdit"]["range"], support::range_json(8, 0, 8, 6));
}

#[test]
fn completion_for_inherited_constants_with_constant_path_but_no_name() {
    let source = "module Foo\n  Bar = 1\nend\n\nclass Baz\n  include Foo\nend\n\nBaz::\n";
    let mut server = with_server(Some(source), DEFAULT_URI, no_typechecker());
    let result = completion(&mut server, 8, 5);
    assert_eq!(labels(&result), vec!["Baz::Bar"]);
    assert_eq!(new_texts(&result), vec!["Baz::Bar"]);
}

#[test]
fn completion_for_constants_inside_aliased_namespaces() {
    let source = "module First\n  module Second\n    class Foo\n    end\n  end\nend\n\nmodule Namespace\n  Second = First::Second\n\n  Second::\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, no_typechecker());
    let result = completion(&mut server, 10, 10);
    assert_eq!(labels(&result), vec!["Second::Foo"]);
    assert_eq!(new_texts(&result), vec!["Second::Foo"]);
}

#[test]
fn completion_for_methods_invoked_on_self() {
    let source = "class Foo\n  def qux1(a, b); end\n  def qux2(c, d); end\n\n  def process\n    q\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    let _structure = with_file_structure(&server);
    let result = completion(&mut server, 5, 5);
    assert_eq!(labels(&result), vec!["qux2", "qux1"]);
    assert_eq!(filter_texts(&result), vec!["qux2", "qux1"]);
    assert_eq!(new_texts(&result), vec!["qux2", "qux1"]);
    assert_eq!(descriptions(&result), vec!["fake.rb", "fake.rb"]);
}

#[test]
fn completion_for_methods_invoked_on_explicit_self() {
    let source = "class Foo\n  def bar(a, b); end\n  def baz(c, d); end\n\n  def process\n    self.b\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    let _structure = with_file_structure(&server);
    let result = completion(&mut server, 5, 10);
    assert_eq!(labels(&result), vec!["baz", "bar"]);
    assert_eq!(filter_texts(&result), vec!["baz", "bar"]);
    assert_eq!(new_texts(&result), vec!["baz", "bar"]);
    let start_characters: Vec<u64> =
        result.iter().map(|item| item["textEdit"]["range"]["start"]["character"].as_u64().unwrap()).collect();
    assert_eq!(start_characters, vec![9, 9]);
}

#[test]
fn completion_for_methods_named_with_uppercase_characters() {
    let source = "class Kernel\n  def Array(a); end\n\n  def process\n    Array(\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    let _structure = with_file_structure(&server);
    let result = completion(&mut server, 4, 10);
    assert_eq!(labels(&result), vec!["Array"]);
    assert_eq!(filter_texts(&result), vec!["Array"]);
    assert_eq!(new_texts(&result), vec!["Array"]);
}

#[test]
fn completion_for_attributes() {
    let source = "class Foo\n  attr_accessor :qux\n\n  def bar\n    q\n    self.q\n  end\nend\n\nfoo = Foo.new\nfoo.q\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    let _structure = with_file_structure(&server);

    let result = completion(&mut server, 4, 5);
    assert_eq!(labels(&result), vec!["qux", "qux="]);
    assert_eq!(filter_texts(&result), vec!["qux", "qux="]);
    assert_eq!(new_texts(&result), vec!["qux", "self.qux = "]);

    let result = completion(&mut server, 5, 10);
    assert_eq!(labels(&result), vec!["qux", "qux="]);
    assert_eq!(filter_texts(&result), vec!["qux", "qux="]);
    assert_eq!(new_texts(&result), vec!["qux", "qux = "]);

    let result = completion(&mut server, 10, 5);
    assert_eq!(labels(&result), vec!["qux", "qux="]);
    assert_eq!(filter_texts(&result), vec!["qux", "qux="]);
    assert_eq!(new_texts(&result), vec!["qux", "qux = "]);
}

#[test]
fn with_typed_false() {
    let source = "# typed: false\nclass Foo\n  def complete_me\n  end\n\n  def you\n    comp\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    let _structure = with_file_structure(&server);
    assert_eq!(labels(&completion(&mut server, 6, 8)), vec!["complete_me"]);
}

#[test]
fn with_typed_true() {
    let source = "# typed: true\nclass Foo\n  def complete_me\n  end\n\n  def you\n    comp\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    let _structure = with_file_structure(&server);
    assert!(completion(&mut server, 6, 8).is_empty());
}

#[test]
fn completion_for_inherited_methods() {
    let source = "module Foo\n  module First\n    def method1; end\n  end\n\n  class Bar\n    def method2; end\n  end\n\n  class Baz < Bar\n    include First\n\n    def do_it\n      m\n    end\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    let _structure = with_file_structure(&server);
    assert_eq!(labels(&completion(&mut server, 13, 7)), vec!["module", "method2", "method1"]);
}

fn assert_relative_completion(source: &str, opened_relative_path: &str, expected_paths: &[&str]) {
    let (start_character, end_character) = quote_positions(source);
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    let structure = with_file_structure(&server);
    let uri = open(&server, &structure.directory.join(opened_relative_path), source);
    let result = completion_at(&mut server, &uri, 0, end_character);
    let expected: Vec<Value> =
        expected_paths.iter().map(|path| path_completion(path, start_character, end_character)).collect();
    assert_eq!(result, expected);
}

#[test]
fn relative_completion_command() {
    assert_relative_completion(
        "require_relative \"support/\"\n",
        "foo/fake.rb",
        &["support/bar", "support/baz", "support/quux"],
    );
}

#[test]
fn relative_completion_call() {
    assert_relative_completion(
        "require_relative(\"../\")\n",
        "foo/fake.rb",
        &["../foo/bar", "../foo/baz", "../foo/quux", "../foo/support/bar", "../foo/support/baz", "../foo/support/quux"],
    );
}

#[test]
fn relative_completion_command_call() {
    assert_relative_completion(
        "Kernel.require_relative \"./\"\n",
        "foo/support/fake.rb",
        &["./bar", "./baz", "./quux"],
    );
}

#[test]
fn relative_completion_command_call_without_leading_dot() {
    assert_relative_completion(
        "Kernel.require_relative \"b\"\n",
        "foo/quxx.rb",
        &["bar", "baz", "support/bar", "support/baz"],
    );
}

#[test]
fn relative_completion_with_partial_path() {
    assert_relative_completion(
        "require_relative \"../suppo\"\n",
        "foo/support/fake.rb",
        &["../support/bar", "../support/baz", "../support/quux"],
    );
}

#[test]
fn completion_for_global_variables() {
    let source = "$qar &&= 1\n$qaz += 1\n$qux ||= 1\n$quux, $qorge = 1\n$qoo = 1\n\n$q\n$LOAD\n$\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    let core_indexed = index_ruby_core(&server);

    assert_eq!(labels(&completion(&mut server, 6, 2)), vec!["$qoo", "$qorge", "$quux", "$qux", "$qaz", "$qar"]);
    if !core_indexed {
        return;
    }

    let result = completion(&mut server, 7, 5);
    assert_eq!(labels(&result), vec!["$LOAD_PATH", "$LOADED_FEATURES"]);
    assert_eq!(descriptions(&result), vec!["global_variables.rbs", "global_variables.rbs"]);

    assert!(completion(&mut server, 8, 1).len() > 40);
}

#[test]
fn completion_for_global_variables_show_only_uniq_entries() {
    let source = "$qar &&= 1\n$qar += 1\n$qar ||= 1\n$q\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    index_ruby_core(&server);
    assert_eq!(labels(&completion(&mut server, 3, 2)), vec!["$qar"]);
}

#[test]
fn completion_for_class_variables() {
    let source = "class Foo\n  def set_variables\n    @@foo = 1\n    @@foobar = 2\n  end\n\n  def bar\n    @\n  end\n\n  def baz\n    @@ = 123\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, no_typechecker());
    let result = completion(&mut server, 7, 5);
    assert_eq!(labels(&result), vec!["@@foo", "@@foobar"]);
    assert_eq!(descriptions(&result), vec!["fake.rb", "fake.rb"]);
}

#[test]
fn completion_for_class_variables_node() {
    let source = "class Foo\n  def set_variables\n    @@foo = 1\n    @@foobar = 2\n  end\n\n  def bar\n    @@\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, no_typechecker());
    assert_eq!(labels(&completion(&mut server, 7, 6)), vec!["@@foo", "@@foobar"]);
}

#[test]
fn completion_for_inherited_class_variables() {
    let source = "module Foo\n  def set_variable\n    @@bar = 9\n  end\nend\n\nclass Parent\n  def set_variable\n    @@baz = 5\n  end\nend\n\nclass Child < Parent\n  include Foo\n\n  def do_something\n    @\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, no_typechecker());
    assert_eq!(labels(&completion(&mut server, 16, 5)), vec!["@@baz", "@@bar"]);
}

#[test]
fn completion_for_class_variables_show_only_unique_entries() {
    let source = "class Foo\n  def set_variables\n    @@foo = 1\n    @@foo = 2\n  end\n\n  def bar\n    @\n  end\n\n  def baz\n    @@ = 123\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, no_typechecker());
    assert_eq!(labels(&completion(&mut server, 7, 5)), vec!["@@foo"]);
}

#[test]
fn completion_for_class_variables_in_different_context() {
    let source = "class Foo\n  @@a = 1\n\n  class << self\n    @@b = 2\n\n    def foo\n      @@c = 3\n    end\n  end\n\n  def bar\n    @\n  end\n\n  def baz\n    @@ = 4\n  end\n\n  def self.foobar\n    @@d = 5\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, no_typechecker());
    assert_eq!(labels(&completion(&mut server, 12, 5)), vec!["@@d", "@@c", "@@b", "@@a"]);
}

#[test]
fn completion_for_instance_variables() {
    let source = "class Foo\n  def initialize\n    @foo = 1\n    @foobar = 2\n  end\n\n  def bar\n    @\n  end\n\n  def baz\n    @ = 123\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, no_typechecker());
    assert_eq!(labels(&completion(&mut server, 7, 5)), vec!["@foo", "@foobar"]);
    let result = completion(&mut server, 11, 5);
    assert_eq!(labels(&result), vec!["@foo", "@foobar"]);
    assert_eq!(descriptions(&result), vec!["fake.rb", "fake.rb"]);
}

#[test]
fn completion_for_inherited_instance_variables() {
    let source = "module Foo\n  def set_ivar\n    @a = 9\n    @b = 1\n  end\nend\n\nclass Parent\n  def initialize\n    @a = 5\n  end\nend\n\nclass Child < Parent\n  include Foo\n\n  def do_something\n    @\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, no_typechecker());
    assert_eq!(labels(&completion(&mut server, 17, 5)), vec!["@b", "@a"]);
}

#[test]
fn instance_variable_completion_shows_only_uniq_entries() {
    let source = "class Foo\n  def initialize\n    @foo = 1\n  end\n\n  def other_set_foo\n    @foo = 2\n  end\n\n  def baz\n    @\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, no_typechecker());
    assert_eq!(labels(&completion(&mut server, 10, 5)), vec!["@foo"]);
}

#[test]
fn completion_for_singleton_methods() {
    let source = "class Foo\n  def self.baz; end\n\n  class << self\n    def bar\n      b\n    end\n  end\nend\n\nFoo.\n";
    let mut server = with_server(Some(source), DEFAULT_URI, no_typechecker());
    assert_eq!(labels(&completion(&mut server, 10, 4)), vec!["bar", "baz"]);
    assert_eq!(labels(&completion(&mut server, 5, 7)), vec!["begin", "break", "bar", "baz"]);
}

#[test]
fn completion_for_class_instance_variables() {
    let source = "class Foo\n  @a = 1\n\n  def self.baz\n    @b = 2\n  end\n\n  class << self\n    def bar\n      @c = 3\n    end\n\n    def qux\n      @\n    end\n  end\n\n  def self.yeah\n    @\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, no_typechecker());
    assert_eq!(labels(&completion(&mut server, 13, 7)), vec!["@c", "@b", "@a"]);
    assert_eq!(labels(&completion(&mut server, 18, 5)), vec!["@c", "@b", "@a"]);
}

#[test]
fn completion_for_aliased_methods() {
    let source = "class Parent\n  def bar(a); end\nend\n\nclass Child < Parent\n  alias baz bar\n\n  def do_something\n    b\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, no_typechecker());
    assert_eq!(labels(&completion(&mut server, 8, 5)), vec!["begin", "break", "baz", "bar"]);
}

#[test]
fn completion_for_locals() {
    let source = "class Child\n  abc0 = 42\n\n  def do_something(abc1, abc2, abc3)\n    a\n\n    [].each do |abc4, abc5|\n      a\n    end\n  end\n\n  a\nend\n\nabc = 12\na\n";
    let mut server = with_server(Some(source), DEFAULT_URI, no_typechecker());
    assert_eq!(labels(&completion(&mut server, 4, 5)), vec!["abc1", "abc2", "abc3", "alias", "and"]);
    assert_eq!(labels(&completion(&mut server, 7, 7)), vec!["abc1", "abc2", "abc3", "abc4", "abc5", "alias", "and"]);
    assert_eq!(labels(&completion(&mut server, 11, 3)), vec!["abc0", "alias", "and"]);
    assert_eq!(labels(&completion(&mut server, 15, 1)), vec!["abc", "alias", "and"]);
}

#[test]
fn completion_for_locals_only_happens_when_there_is_no_receiver() {
    let source = "class Child\n  def do_something\n    abc = 123\n\n    foo.a\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, no_typechecker());
    assert!(completion(&mut server, 4, 9).is_empty());
}

#[test]
fn self_method_completion_is_disabled_on_typed_true() {
    let source = "# typed: true\nclass Foo\n  def bar\n    b\n  end\n\n  def baz; end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    assert!(completion(&mut server, 3, 5).is_empty());
}

#[test]
fn local_completion_is_disabled_on_typed_true() {
    let source = "# typed: true\nclass Foo\n  def bar\n    abc = 123\n    a\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    assert!(completion(&mut server, 4, 5).is_empty());
}

#[test]
fn keyword_completion_is_disabled_on_typed_true() {
    let source = "# typed: true\nclass Foo\n  def bar\n    yie\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    assert!(completion(&mut server, 3, 7).is_empty());
}

#[test]
fn instance_variable_completion_is_disabled_on_typed_strict() {
    let source = "# typed: strict\nclass Foo\n  def initialize\n    @hello = 123\n  end\n\n  def bar\n    @\n  end\nend\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    assert!(completion(&mut server, 7, 5).is_empty());
}

#[test]
fn provides_constant_completion_on_type_ignore() {
    let source = "# typed: ignore\nclass Foo\nend\n\nF\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    let _structure = with_file_structure(&server);
    let result = completion(&mut server, 4, 1);
    assert_eq!(labels(&result), vec!["Foo"]);
    assert_eq!(descriptions(&result), vec!["fake.rb"]);
}

#[test]
fn guessed_type_name_is_only_included_for_guessed_types() {
    let mut server = with_server(Some("[].e\n"), DEFAULT_URI, TestOptions::default());
    if !index_ruby_core(&server) {
        return;
    }
    let items = completion(&mut server, 0, 4);
    assert!(!items.is_empty());
    for item in items {
        assert!(matches!(item["data"]["guessed_type"], Value::Null | Value::Bool(false)), "{item}");
    }
}

#[test]
fn completion_for_private_methods() {
    let source = "class Foo\n  def bar\n    b\n  end\n\n  private\n\n  def baz\n  end\nend\n\nfoo = Foo.new\nfoo.b\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    assert!(labels(&completion(&mut server, 2, 5)).contains(&"baz".to_string()));
    assert!(!labels(&completion(&mut server, 12, 5)).contains(&"baz".to_string()));
}

#[test]
fn completion_for_protected_methods() {
    let source = "class Foo\n  def bar\n    b\n  end\n\n  protected\n\n  def baz\n  end\nend\n\nfoo = Foo.new\nfoo.b\n";
    let mut server = with_server(Some(source), DEFAULT_URI, TestOptions::default());
    assert!(labels(&completion(&mut server, 2, 5)).contains(&"baz".to_string()));
    assert!(!labels(&completion(&mut server, 12, 5)).contains(&"baz".to_string()));
}

#[test]
fn require_relative_returns_empty_result_for_unsaved_files() {
    let source = "require_relative \"support/\"\n";
    let mut server = with_server(Some(source), "untitled:Untitled-1", TestOptions::default());
    let _structure = with_file_structure(&server);
    assert!(completion(&mut server, 0, source.rfind('"').unwrap() as u32).is_empty());
}
