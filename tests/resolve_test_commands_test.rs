mod support;
mod test_discovery_support;

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use ruby_lsp_rs::requests::support::test_command::{shell_escape, test_command};
use serde_json::{Value, json};
use support::{DEFAULT_URI, TestOptions, with_server};
use test_discovery_support::{bundled_workspace, workspace_options};

fn resolve_test_commands(items: Value) -> Vec<String> {
    let mut server = with_server(
        None,
        DEFAULT_URI,
        TestOptions {
            initialization_options: Some(workspace_options(bundled_workspace(), json!({}))),
            ..TestOptions::default()
        },
    );
    let result = server.result("rubyLsp/resolveTestCommands", json!({ "items": items }));
    result["commands"]
        .as_array()
        .unwrap_or_else(|| panic!("Expected commands, got {result}"))
        .iter()
        .map(|command| command.as_str().unwrap().to_string())
        .collect()
}

fn commands(expected: &[&str]) -> Vec<String> {
    let command = test_command(&bundled_workspace().to_string_lossy());
    expected.iter().map(|expected| expected.replace("{COMMAND}", &command)).collect()
}

/// Stands in for Ruby LSP's `Dir.stubs(:glob)`: the stubbed paths are created below a temporary root, so that the real
/// glob finds them
struct TestDirectory {
    root: PathBuf,
    files: Vec<String>,
}

impl TestDirectory {
    fn new(files: &[&str]) -> TestDirectory {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "ruby-lsp-rs-resolve-test-commands-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        for file in files {
            let path = root.join(file.trim_start_matches('/'));
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "").unwrap();
        }
        TestDirectory { root, files: files.iter().map(|file| file.to_string()).collect() }
    }

    fn relocated(&self, path: &str) -> String {
        self.root.join(path.trim_start_matches('/')).to_string_lossy().into_owned()
    }

    fn relocate(&self, mut items: Value) -> Value {
        for item in items.as_array_mut().unwrap() {
            let is_test_directory = item["tags"].as_array().unwrap().iter().any(|tag| tag == "test_dir");
            if is_test_directory {
                let path = ruby_lsp_rs::uri::to_path(item["uri"].as_str().unwrap()).unwrap();
                item["uri"] = Value::String(ruby_lsp_rs::uri::from_path(&self.relocated(&path)));
            }
        }
        items
    }

    fn relocate_commands(&self, expected: &[&str]) -> Vec<String> {
        commands(expected)
            .into_iter()
            .map(|command| {
                self.files.iter().fold(command, |command, file| {
                    command.replace(&shell_escape(file), &shell_escape(&self.relocated(file)))
                })
            })
            .collect()
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn minitest_resolve_test_command_specific_examples() {
    let items = json!([
        {
            "id": "ServerTest",
            "uri": "file:///test/server_test.rb",
            "label": "ServerTest",
            "range": {"start": {"line": 0, "character": 0}, "end": {"line": 30, "character": 3}},
            "tags": ["framework:minitest", "test_group"],
            "children": [
                {
                    "id": "ServerTest#test_server",
                    "uri": "file:///test/server_test.rb",
                    "label": "test_server",
                    "range": {"start": {"line": 1, "character": 2}, "end": {"line": 10, "character": 3}},
                    "tags": ["framework:minitest"],
                    "children": [],
                },
            ],
        },
        {
            "id": "StoreTest",
            "uri": "file:///test/store_test.rb",
            "label": "StoreTest",
            "range": {"start": {"line": 0, "character": 0}, "end": {"line": 30, "character": 3}},
            "tags": ["framework:minitest", "test_group"],
            "children": [
                {
                    "id": "StoreTest#test_store",
                    "uri": "file:///test/store_test.rb",
                    "label": "test_store",
                    "range": {"start": {"line": 1, "character": 2}, "end": {"line": 10, "character": 3}},
                    "tags": ["framework:minitest"],
                    "children": [],
                },
            ],
        },
    ]);
    let expected = commands(&[
        r#"{COMMAND} -Itest /test/server_test.rb --name "/^ServerTest#test_server\$/""#,
        r#"{COMMAND} -Itest /test/store_test.rb --name "/^StoreTest#test_store\$/""#,
    ]);
    assert_eq!(resolve_test_commands(items), expected);
}

#[test]
fn minitest_resolve_test_command_group_mixed_with_examples() {
    let items = json!([
        {
            "id": "ServerTest",
            "uri": "file:///test/server_test.rb",
            "label": "ServerTest",
            "range": {"start": {"line": 0, "character": 0}, "end": {"line": 30, "character": 3}},
            "tags": ["framework:minitest", "test_group"],
            "children": [],
        },
        {
            "id": "StoreTest",
            "uri": "file:///test/store_test.rb",
            "label": "StoreTest",
            "range": {"start": {"line": 0, "character": 0}, "end": {"line": 30, "character": 3}},
            "tags": ["framework:minitest", "test_group"],
            "children": [
                {
                    "id": "StoreTest#test_store",
                    "uri": "file:///test/store_test.rb",
                    "label": "test_store",
                    "range": {"start": {"line": 1, "character": 2}, "end": {"line": 10, "character": 3}},
                    "tags": ["framework:minitest"],
                    "children": [],
                },
            ],
        },
    ]);
    let expected = commands(&[
        r#"{COMMAND} -Itest /test/server_test.rb --name "/^ServerTest(#|::)/""#,
        r#"{COMMAND} -Itest /test/store_test.rb --name "/^StoreTest#test_store\$/""#,
    ]);
    assert_eq!(resolve_test_commands(items), expected);
}

#[test]
fn minitest_resolve_test_command_multiple_examples_from_same_group() {
    let items = json!([
        {
            "id": "ServerTest",
            "uri": "file:///test/server_test.rb",
            "label": "ServerTest",
            "range": {"start": {"line": 0, "character": 0}, "end": {"line": 30, "character": 3}},
            "tags": ["framework:minitest", "test_group"],
            "children": [
                {
                    "id": "ServerTest#test_server",
                    "uri": "file:///test/server_test.rb",
                    "label": "test_server",
                    "range": {"start": {"line": 1, "character": 2}, "end": {"line": 10, "character": 3}},
                    "tags": ["framework:minitest"],
                    "children": [],
                },
                {
                    "id": "ServerTest#test_server_again",
                    "uri": "file:///test/server_test.rb",
                    "label": "test_server_again",
                    "range": {"start": {"line": 12, "character": 2}, "end": {"line": 30, "character": 3}},
                    "tags": ["framework:minitest"],
                    "children": [],
                },
            ],
        },
    ]);
    let expected = commands(&[
        r#"{COMMAND} -Itest /test/server_test.rb --name "/^ServerTest#(test_server|test_server_again)\$/""#,
    ]);
    assert_eq!(resolve_test_commands(items), expected);
}

#[test]
fn minitest_resolve_test_command_entire_files() {
    let items = json!([
        {
            "id": "file:///test/server_test.rb",
            "uri": "file:///test/server_test.rb",
            "label": "/test/server_test.rb",
            "tags": ["test_file", "framework:minitest"],
            "children": [],
        },
        {
            "id": "file:///test/store_test.rb",
            "uri": "file:///test/store_test.rb",
            "label": "/test/store_test.rb",
            "tags": ["test_file", "framework:minitest"],
            "children": [],
        },
    ]);
    let expected = commands(&[
        r#"{COMMAND} -Itest -e "ARGV.each { |f| require f }" /test/server_test.rb /test/store_test.rb"#,
    ]);
    assert_eq!(resolve_test_commands(items), expected);
}

#[test]
fn minitest_resolve_test_command_entire_directories() {
    let directory = TestDirectory::new(&[
        r#"/other/test/fake_test.rb"#,
        r#"/other/test/fake_test_2_test.rb"#,
    ]);
    let items = directory.relocate(json!([
        {
            "id": "file:///other/test",
            "uri": "file:///other/test",
            "label": "/other/test",
            "tags": ["test_dir", "framework:minitest"],
            "children": [],
        },
        {
            "id": "file:///test/server_test.rb",
            "uri": "file:///test/server_test.rb",
            "label": "/test/server_test.rb",
            "tags": ["test_file", "framework:minitest"],
            "children": [],
        },
        {
            "id": "file:///test/store_test.rb",
            "uri": "file:///test/store_test.rb",
            "label": "/test/store_test.rb",
            "tags": ["test_file", "framework:minitest"],
            "children": [],
        },
    ]));
    let expected = directory.relocate_commands(&[
        r#"{COMMAND} -Itest -e "ARGV.each { |f| require f }" /other/test/fake_test.rb /other/test/fake_test_2_test.rb /test/server_test.rb /test/store_test.rb"#,
    ]);
    assert_eq!(resolve_test_commands(items), expected);
}

#[test]
fn minitest_resolve_test_command_multiple_test_groups() {
    let items = json!([
        {
            "id": "ServerTest",
            "uri": "file:///test/server_test.rb",
            "label": "ServerTest",
            "range": {"start": {"line": 0, "character": 0}, "end": {"line": 30, "character": 3}},
            "tags": ["framework:minitest", "test_group"],
            "children": [],
        },
        {
            "id": "OtherServerTest",
            "uri": "file:///test/server_test.rb",
            "label": "OtherServerTest",
            "range": {"start": {"line": 32, "character": 0}, "end": {"line": 60, "character": 3}},
            "tags": ["framework:minitest", "test_group"],
            "children": [],
        },
    ]);
    let expected = commands(&[
        r#"{COMMAND} -Itest /test/server_test.rb --name "/(^ServerTest(#|::)|^OtherServerTest(#|::))/""#,
    ]);
    assert_eq!(resolve_test_commands(items), expected);
}

#[test]
fn minitest_resolve_test_command_complex_case() {
    let items = json!([
        {
            "id": "ServerTest",
            "uri": "file:///test/server_test.rb",
            "label": "ServerTest",
            "range": {"start": {"line": 0, "character": 0}, "end": {"line": 30, "character": 3}},
            "tags": ["framework:minitest", "test_group"],
            "children": [
                {
                    "id": "ServerTest#test_server",
                    "uri": "file:///test/server_test.rb",
                    "label": "test_server",
                    "range": {"start": {"line": 1, "character": 2}, "end": {"line": 10, "character": 3}},
                    "tags": ["framework:minitest"],
                    "children": [],
                },
                {
                    "id": "ServerTest#test_server_again",
                    "uri": "file:///test/server_test.rb",
                    "label": "test_server_again",
                    "range": {"start": {"line": 1, "character": 2}, "end": {"line": 10, "character": 3}},
                    "tags": ["framework:minitest"],
                    "children": [],
                },
            ],
        },
        {
            "id": "OtherServerTest",
            "uri": "file:///test/server_test.rb",
            "label": "OtherServerTest",
            "range": {"start": {"line": 0, "character": 0}, "end": {"line": 30, "character": 3}},
            "tags": ["framework:minitest", "test_group"],
            "children": [],
        },
        {
            "id": "StoreTest",
            "uri": "file:///test/store_test.rb",
            "label": "StoreTest",
            "range": {"start": {"line": 0, "character": 0}, "end": {"line": 30, "character": 3}},
            "tags": ["framework:minitest", "test_group"],
            "children": [
                {
                    "id": "StoreTest#test_store",
                    "uri": "file:///test/store_test.rb",
                    "label": "test_store",
                    "range": {"start": {"line": 1, "character": 2}, "end": {"line": 10, "character": 3}},
                    "tags": ["framework:minitest"],
                    "children": [],
                },
            ],
        },
    ]);
    let expected = commands(&[
        r#"{COMMAND} -Itest /test/server_test.rb --name "/(^ServerTest#(test_server|test_server_again)\$|^OtherServerTest(#|::))/""#,
        r#"{COMMAND} -Itest /test/store_test.rb --name "/^StoreTest#test_store\$/""#,
    ]);
    assert_eq!(resolve_test_commands(items), expected);
}

#[test]
fn minitest_resolve_test_command_examples_with_dynamic_references() {
    let items = json!([
        {
            "id": "<dynamic_reference>::ServerTest",
            "uri": "file:///test/server_test.rb",
            "label": "<dynamic_reference>::ServerTest",
            "range": {"start": {"line": 0, "character": 0}, "end": {"line": 30, "character": 3}},
            "tags": ["framework:minitest", "test_group"],
            "children": [
                {
                    "id": "<dynamic_reference>::ServerTest#test_server",
                    "uri": "file:///test/server_test.rb",
                    "label": "test_server",
                    "range": {"start": {"line": 1, "character": 2}, "end": {"line": 10, "character": 3}},
                    "tags": ["framework:minitest"],
                    "children": [],
                },
            ],
        },
        {
            "id": "<dynamic_reference>::StoreTest",
            "uri": "file:///test/store_test.rb",
            "label": "<dynamic_reference>::StoreTest",
            "range": {"start": {"line": 0, "character": 0}, "end": {"line": 30, "character": 3}},
            "tags": ["framework:minitest", "test_group"],
            "children": [],
        },
    ]);
    let expected = commands(&[
        r#"{COMMAND} -Itest /test/server_test.rb --name "/^.*::ServerTest#test_server\$/""#,
        r#"{COMMAND} -Itest /test/store_test.rb --name "/^.*::StoreTest(#|::)/""#,
    ]);
    assert_eq!(resolve_test_commands(items), expected);
}

#[test]
fn minitest_resolve_test_command_examples_with_no_parent_items() {
    let items = json!([
        {
            "id": "CompletionTest#test_with_typed_false",
            "label": "test_with_typed_false",
            "uri": "file:///test/requests/completion_test.rb",
            "tags": ["debug", "framework:minitest"],
            "children": [],
            "range": {"start": {"line": 704, "character": 2}, "end": {"line": 728, "character": 5}},
        },
        {
            "id": "CompletionTest#test_with_typed_true",
            "label": "test_with_typed_true",
            "uri": "file:///test/requests/completion_test.rb",
            "tags": ["debug", "framework:minitest"],
            "children": [],
            "range": {"start": {"line": 730, "character": 2}, "end": {"line": 754, "character": 5}},
        },
    ]);
    let expected = commands(&[
        r#"{COMMAND} -Itest /test/requests/completion_test.rb --name "/^CompletionTest#(test_with_typed_false|test_with_typed_true)\$/""#,
    ]);
    assert_eq!(resolve_test_commands(items), expected);
}

#[test]
fn minitest_resolve_test_command_nested_test_groups() {
    let items = json!([
        {
            "id": "ServerTest::MainTest",
            "uri": "file:///test/server_test.rb",
            "label": "ServerTest::MainTest",
            "range": {"start": {"line": 10, "character": 2}, "end": {"line": 20, "character": 3}},
            "tags": ["framework:minitest", "test_group"],
            "children": [
                {
                    "id": "ServerTest::MainTest::NestedTest",
                    "uri": "file:///test/server_test.rb",
                    "label": "ServerTest::MainTest::NestedTest",
                    "range": {"start": {"line": 12, "character": 4}, "end": {"line": 14, "character": 5}},
                    "tags": ["framework:minitest", "test_group"],
                    "children": [],
                },
            ],
        },
    ]);
    let expected = commands(&[
        r#"{COMMAND} -Itest /test/server_test.rb --name "/^ServerTest::MainTest::NestedTest(#|::)/""#,
    ]);
    assert_eq!(resolve_test_commands(items), expected);
}

#[test]
fn minitest_resolve_test_command_mix_of_directories_and_examples() {
    let directory = TestDirectory::new(&[
        r#"/test/unit/fake_test.rb"#,
        r#"/test/unit/fake_test_2_test.rb"#,
    ]);
    let items = directory.relocate(json!([
        {
            "id": "file:///test/unit",
            "uri": "file:///test/unit",
            "label": "/test/unit",
            "tags": ["test_dir", "framework:minitest"],
            "children": [],
        },
        {
            "id": "ServerTest#test_server",
            "uri": "file:///test/server_test.rb",
            "label": "test_server",
            "range": {"start": {"line": 1, "character": 2}, "end": {"line": 10, "character": 3}},
            "tags": ["framework:minitest"],
            "children": [],
        },
    ]));
    let expected = directory.relocate_commands(&[
        r#"{COMMAND} -Itest /test/server_test.rb --name "/^ServerTest#test_server\$/""#,
        r#"{COMMAND} -Itest -e "ARGV.each { |f| require f }" /test/unit/fake_test.rb /test/unit/fake_test_2_test.rb"#,
    ]);
    assert_eq!(resolve_test_commands(items), expected);
}

#[test]
fn minitest_resolve_test_command_for_minitest_spec() {
    let items = json!([
        {
            "id": "ServerSpec",
            "uri": "file:///spec/server_spec.rb",
            "label": "ServerSpec",
            "range": {"start": {"line": 0, "character": 0}, "end": {"line": 30, "character": 3}},
            "tags": ["framework:minitest", "test_group"],
            "children": [
                {
                    "id": "ServerSpec#test_0003_something",
                    "uri": "file:///spec/server_spec.rb",
                    "label": "test_server",
                    "range": {"start": {"line": 1, "character": 2}, "end": {"line": 10, "character": 3}},
                    "tags": ["framework:minitest"],
                    "children": [],
                },
            ],
        },
        {
            "id": "file:///spec/other_spec.rb",
            "uri": "file:///spec/other_spec.rb",
            "label": "/spec/other_spec.rb",
            "range": {"start": {"line": 0, "character": 0}, "end": {"line": 30, "character": 3}},
            "tags": ["framework:minitest", "test_file"],
            "children": [],
        },
    ]);
    let expected = commands(&[
        r#"{COMMAND} -Ispec /spec/server_spec.rb --name "/^ServerSpec#test_\d{4}_something\$/""#,
        r#"{COMMAND} -Ispec -e "ARGV.each { |f| require f }" /spec/other_spec.rb"#,
    ]);
    assert_eq!(resolve_test_commands(items), expected);
}

#[test]
fn minitest_resolve_test_command_for_minitest_spec_directory() {
    let directory = TestDirectory::new(&[
        r#"/other/spec/fake_spec.rb"#,
        r#"/other/spec/fake_spec_2_spec.rb"#,
    ]);
    let items = directory.relocate(json!([
        {
            "id": "file:///other/spec",
            "uri": "file:///other/spec",
            "label": "/other/spec",
            "tags": ["test_dir", "framework:minitest"],
            "children": [],
        },
    ]));
    let expected = directory.relocate_commands(&[
        r#"{COMMAND} -Ispec -e "ARGV.each { |f| require f }" /other/spec/fake_spec.rb /other/spec/fake_spec_2_spec.rb"#,
    ]);
    assert_eq!(resolve_test_commands(items), expected);
}

#[test]
fn minitest_resolve_test_command_for_nested_spec_in_class() {
    let items = json!([
        {
            "id": "ServerSpec",
            "uri": "file:///spec/server_spec.rb",
            "label": "ServerSpec",
            "range": {"start": {"line": 0, "character": 0}, "end": {"line": 30, "character": 3}},
            "tags": ["framework:minitest", "test_group"],
            "children": [
                {
                    "id": "ServerSpec::foo",
                    "uri": "file:///spec/server_spec.rb",
                    "label": "foo",
                    "range": {"start": {"line": 1, "character": 2}, "end": {"line": 5, "character": 19}},
                    "tags": ["framework:minitest", "test_group"],
                    "children": [],
                },
            ],
        },
    ]);
    let expected = commands(&[
        r#"{COMMAND} -Ispec /spec/server_spec.rb --name "/^ServerSpec::foo(#|::)/""#,
    ]);
    assert_eq!(resolve_test_commands(items), expected);
}

#[test]
fn minitest_resolve_test_command_for_minitest_spec_with_backticks() {
    let items = json!([
        {
            "id": "ServerSpec",
            "uri": "file:///spec/server_spec.rb",
            "label": "ServerSpec",
            "range": {"start": {"line": 0, "character": 0}, "end": {"line": 30, "character": 3}},
            "tags": ["framework:minitest", "test_group"],
            "children": [
                {
                    "id": "ServerSpec#test_0001_uses `SomeClass` to do something",
                    "uri": "file:///spec/server_spec.rb",
                    "label": "test_server",
                    "range": {"start": {"line": 1, "character": 2}, "end": {"line": 10, "character": 3}},
                    "tags": ["framework:minitest"],
                    "children": [],
                },
            ],
        },
    ]);
    let expected = commands(&[
        r#"{COMMAND} -Ispec /spec/server_spec.rb --name "/^ServerSpec#test_\d{4}_uses\ \`SomeClass\`\ to\ do\ something\$/""#,
    ]);
    assert_eq!(resolve_test_commands(items), expected);
}

#[test]
fn minitest_resolve_properly_escapes_single_file_paths() {
    let items = json!([
        {
            "id": "file:///test/server(v2)_test.rb",
            "uri": "file:///test/server(v2)_test.rb",
            "label": "/test/server(v2)_test.rb",
            "tags": ["test_file", "framework:minitest"],
            "children": [],
        },
    ]);
    let expected = commands(&[
        r#"{COMMAND} -Itest -e "ARGV.each { |f| require f }" /test/server\(v2\)_test.rb"#,
    ]);
    assert_eq!(resolve_test_commands(items), expected);
}

#[test]
fn minitest_resolve_properly_escapes_file_paths_within_directories() {
    let directory = TestDirectory::new(&[
        r#"/other/test/fake(v2)_test.rb"#,
    ]);
    let items = directory.relocate(json!([
        {
            "id": "file:///other/test",
            "uri": "file:///other/test",
            "label": "/other/test",
            "tags": ["test_dir", "framework:minitest"],
            "children": [],
        },
    ]));
    let expected = directory.relocate_commands(&[
        r#"{COMMAND} -Itest -e "ARGV.each { |f| require f }" /other/test/fake\(v2\)_test.rb"#,
    ]);
    assert_eq!(resolve_test_commands(items), expected);
}

#[test]
fn minitest_resolve_properly_escapes_file_paths_in_groups() {
    let items = json!([
        {
            "id": "ServerTest",
            "uri": "file:///test/server(v2)_test.rb",
            "label": "ServerTest",
            "range": {"start": {"line": 0, "character": 0}, "end": {"line": 30, "character": 3}},
            "tags": ["framework:minitest", "test_group"],
            "children": [],
        },
    ]);
    let expected = commands(&[
        r#"{COMMAND} -Itest /test/server\(v2\)_test.rb --name "/^ServerTest(#|::)/""#,
    ]);
    assert_eq!(resolve_test_commands(items), expected);
}

#[test]
fn test_unit_resolve_test_command_specific_examples() {
    let items = json!([
        {
            "id": "ServerTest",
            "uri": "file:///test/server_test.rb",
            "label": "ServerTest",
            "range": {"start": {"line": 0, "character": 0}, "end": {"line": 30, "character": 3}},
            "tags": ["framework:test_unit", "test_group"],
            "children": [
                {
                    "id": "ServerTest#test_server",
                    "uri": "file:///test/server_test.rb",
                    "label": "test_server",
                    "range": {"start": {"line": 1, "character": 2}, "end": {"line": 10, "character": 3}},
                    "tags": ["framework:test_unit"],
                    "children": [],
                },
            ],
        },
        {
            "id": "StoreTest",
            "uri": "file:///test/store_test.rb",
            "label": "StoreTest",
            "range": {"start": {"line": 0, "character": 0}, "end": {"line": 30, "character": 3}},
            "tags": ["framework:test_unit", "test_group"],
            "children": [
                {
                    "id": "StoreTest#test_store",
                    "uri": "file:///test/store_test.rb",
                    "label": "test_store",
                    "range": {"start": {"line": 1, "character": 2}, "end": {"line": 10, "character": 3}},
                    "tags": ["framework:test_unit"],
                    "children": [],
                },
            ],
        },
    ]);
    let expected = commands(&[
        r#"{COMMAND} -Itest /test/server_test.rb --testcase "/^ServerTest\$/" --name "/test_server\$/""#,
        r#"{COMMAND} -Itest /test/store_test.rb --testcase "/^StoreTest\$/" --name "/test_store\$/""#,
    ]);
    assert_eq!(resolve_test_commands(items), expected);
}

#[test]
fn test_unit_resolve_test_command_group_mixed_with_examples() {
    let items = json!([
        {
            "id": "ServerTest",
            "uri": "file:///test/server_test.rb",
            "label": "ServerTest",
            "range": {"start": {"line": 0, "character": 0}, "end": {"line": 30, "character": 3}},
            "tags": ["framework:test_unit", "test_group"],
            "children": [],
        },
        {
            "id": "StoreTest",
            "uri": "file:///test/store_test.rb",
            "label": "StoreTest",
            "range": {"start": {"line": 0, "character": 0}, "end": {"line": 30, "character": 3}},
            "tags": ["framework:test_unit", "test_group"],
            "children": [
                {
                    "id": "StoreTest#test_store",
                    "uri": "file:///test/store_test.rb",
                    "label": "test_store",
                    "range": {"start": {"line": 1, "character": 2}, "end": {"line": 10, "character": 3}},
                    "tags": ["framework:test_unit"],
                    "children": [],
                },
            ],
        },
    ]);
    let expected = commands(&[
        r#"{COMMAND} -Itest /test/server_test.rb --testcase "/^ServerTest\$/""#,
        r#"{COMMAND} -Itest /test/store_test.rb --testcase "/^StoreTest\$/" --name "/test_store\$/""#,
    ]);
    assert_eq!(resolve_test_commands(items), expected);
}

#[test]
fn test_unit_resolve_test_command_multiple_examples_from_same_group() {
    let items = json!([
        {
            "id": "ServerTest",
            "uri": "file:///test/server_test.rb",
            "label": "ServerTest",
            "range": {"start": {"line": 0, "character": 0}, "end": {"line": 30, "character": 3}},
            "tags": ["test_group", "framework:test_unit"],
            "children": [
                {
                    "id": "ServerTest#test_server",
                    "uri": "file:///test/server_test.rb",
                    "label": "test_server",
                    "range": {"start": {"line": 1, "character": 2}, "end": {"line": 10, "character": 3}},
                    "tags": ["framework:test_unit"],
                    "children": [],
                },
                {
                    "id": "ServerTest#test_server_again",
                    "uri": "file:///test/server_test.rb",
                    "label": "test_server_again",
                    "range": {"start": {"line": 12, "character": 2}, "end": {"line": 30, "character": 3}},
                    "tags": ["framework:test_unit"],
                    "children": [],
                },
            ],
        },
    ]);
    let expected = commands(&[
        r#"{COMMAND} -Itest /test/server_test.rb --testcase "/^ServerTest\$/" --name "/(test_server|test_server_again)\$/""#,
    ]);
    assert_eq!(resolve_test_commands(items), expected);
}

#[test]
fn test_unit_resolve_test_command_multiple_test_groups() {
    let items = json!([
        {
            "id": "ServerTest",
            "uri": "file:///test/server_test.rb",
            "label": "ServerTest",
            "range": {"start": {"line": 0, "character": 0}, "end": {"line": 30, "character": 3}},
            "tags": ["framework:test_unit", "test_group"],
            "children": [],
        },
        {
            "id": "OtherServerTest",
            "uri": "file:///test/server_test.rb",
            "label": "OtherServerTest",
            "range": {"start": {"line": 32, "character": 0}, "end": {"line": 60, "character": 3}},
            "tags": ["framework:test_unit", "test_group"],
            "children": [],
        },
    ]);
    let expected = commands(&[
        r#"{COMMAND} -Itest /test/server_test.rb --testcase "/^ServerTest\$/""#,
        r#"{COMMAND} -Itest /test/server_test.rb --testcase "/^OtherServerTest\$/""#,
    ]);
    assert_eq!(resolve_test_commands(items), expected);
}

#[test]
fn test_unit_resolve_test_command_complex_case() {
    let items = json!([
        {
            "id": "ServerTest",
            "uri": "file:///test/server_test.rb",
            "label": "ServerTest",
            "range": {"start": {"line": 0, "character": 0}, "end": {"line": 30, "character": 3}},
            "tags": ["framework:test_unit", "test_group"],
            "children": [
                {
                    "id": "ServerTest#test_server",
                    "uri": "file:///test/server_test.rb",
                    "label": "test_server",
                    "range": {"start": {"line": 1, "character": 2}, "end": {"line": 10, "character": 3}},
                    "tags": ["framework:test_unit"],
                    "children": [],
                },
                {
                    "id": "ServerTest#test_server_again",
                    "uri": "file:///test/server_test.rb",
                    "label": "test_server_again",
                    "range": {"start": {"line": 1, "character": 2}, "end": {"line": 10, "character": 3}},
                    "tags": ["framework:test_unit"],
                    "children": [],
                },
            ],
        },
        {
            "id": "OtherServerTest",
            "uri": "file:///test/server_test.rb",
            "label": "OtherServerTest",
            "range": {"start": {"line": 0, "character": 0}, "end": {"line": 30, "character": 3}},
            "tags": ["framework:test_unit", "test_group"],
            "children": [],
        },
        {
            "id": "StoreTest",
            "uri": "file:///test/store_test.rb",
            "label": "StoreTest",
            "range": {"start": {"line": 0, "character": 0}, "end": {"line": 30, "character": 3}},
            "tags": ["framework:test_unit", "test_group"],
            "children": [
                {
                    "id": "StoreTest#test_store",
                    "uri": "file:///test/store_test.rb",
                    "label": "test_store",
                    "range": {"start": {"line": 1, "character": 2}, "end": {"line": 10, "character": 3}},
                    "tags": ["framework:test_unit"],
                    "children": [],
                },
            ],
        },
    ]);
    let expected = commands(&[
        r#"{COMMAND} -Itest /test/server_test.rb --testcase "/^ServerTest\$/" --name "/(test_server|test_server_again)\$/""#,
        r#"{COMMAND} -Itest /test/server_test.rb --testcase "/^OtherServerTest\$/""#,
        r#"{COMMAND} -Itest /test/store_test.rb --testcase "/^StoreTest\$/" --name "/test_store\$/""#,
    ]);
    assert_eq!(resolve_test_commands(items), expected);
}

#[test]
fn test_unit_resolve_test_command_examples_with_dynamic_references() {
    let items = json!([
        {
            "id": "<dynamic_reference>::ServerTest",
            "uri": "file:///test/server_test.rb",
            "label": "<dynamic_reference>::ServerTest",
            "range": {"start": {"line": 0, "character": 0}, "end": {"line": 30, "character": 3}},
            "tags": ["framework:test_unit", "test_group"],
            "children": [
                {
                    "id": "<dynamic_reference>::ServerTest#test_server",
                    "uri": "file:///test/server_test.rb",
                    "label": "test_server",
                    "range": {"start": {"line": 1, "character": 2}, "end": {"line": 10, "character": 3}},
                    "tags": ["framework:test_unit"],
                    "children": [],
                },
            ],
        },
        {
            "id": "<dynamic_reference>::StoreTest",
            "uri": "file:///test/store_test.rb",
            "label": "<dynamic_reference>::StoreTest",
            "range": {"start": {"line": 0, "character": 0}, "end": {"line": 30, "character": 3}},
            "tags": ["framework:test_unit", "test_group"],
            "children": [],
        },
    ]);
    let expected = commands(&[
        r#"{COMMAND} -Itest /test/server_test.rb --testcase "/^.*::ServerTest\$/" --name "/test_server\$/""#,
        r#"{COMMAND} -Itest /test/store_test.rb --testcase "/^.*::StoreTest\$/""#,
    ]);
    assert_eq!(resolve_test_commands(items), expected);
}

#[test]
fn test_unit_resolve_test_command_examples_with_no_parent_items() {
    let items = json!([
        {
            "id": "CompletionTest#test_with_typed_false",
            "label": "test_with_typed_false",
            "uri": "file:///test/requests/completion_test.rb",
            "tags": ["debug", "framework:test_unit"],
            "children": [],
            "range": {"start": {"line": 704, "character": 2}, "end": {"line": 728, "character": 5}},
        },
        {
            "id": "CompletionTest#test_with_typed_true",
            "label": "test_with_typed_true",
            "uri": "file:///test/requests/completion_test.rb",
            "tags": ["debug", "framework:test_unit"],
            "children": [],
            "range": {"start": {"line": 730, "character": 2}, "end": {"line": 754, "character": 5}},
        },
    ]);
    let expected = commands(&[
        r#"{COMMAND} -Itest /test/requests/completion_test.rb --testcase "/^CompletionTest\$/" --name "/(test_with_typed_false|test_with_typed_true)\$/""#,
    ]);
    assert_eq!(resolve_test_commands(items), expected);
}

#[test]
fn test_unit_resolve_test_command_nested_test_groups() {
    let items = json!([
        {
            "id": "ServerTest::MainTest",
            "uri": "file:///test/server_test.rb",
            "label": "ServerTest::MainTest",
            "range": {"start": {"line": 10, "character": 2}, "end": {"line": 20, "character": 3}},
            "tags": ["framework:test_unit", "test_group"],
            "children": [
                {
                    "id": "ServerTest::MainTest::NestedTest",
                    "uri": "file:///test/server_test.rb",
                    "label": "ServerTest::MainTest::NestedTest",
                    "range": {"start": {"line": 12, "character": 4}, "end": {"line": 14, "character": 5}},
                    "tags": ["framework:test_unit", "test_group"],
                    "children": [],
                },
            ],
        },
    ]);
    let expected = commands(&[
        r#"{COMMAND} -Itest /test/server_test.rb --testcase "/^ServerTest::MainTest::NestedTest\$/""#,
    ]);
    assert_eq!(resolve_test_commands(items), expected);
}

#[test]
fn test_unit_resolve_test_command_mix_of_directories_and_examples() {
    let directory = TestDirectory::new(&[
        r#"/test/unit/fake_test.rb"#,
        r#"/test/unit/fake_test_2_test.rb"#,
    ]);
    let items = directory.relocate(json!([
        {
            "id": "file:///test/unit",
            "uri": "file:///test/unit",
            "label": "/test/unit",
            "tags": ["test_dir", "framework:test_unit"],
            "children": [],
        },
        {
            "id": "ServerTest#test_server",
            "uri": "file:///test/server_test.rb",
            "label": "test_server",
            "range": {"start": {"line": 1, "character": 2}, "end": {"line": 10, "character": 3}},
            "tags": ["framework:test_unit"],
            "children": [],
        },
    ]));
    let expected = directory.relocate_commands(&[
        r#"{COMMAND} -Itest /test/server_test.rb --testcase "/^ServerTest\$/" --name "/test_server\$/""#,
        r#"{COMMAND} -Itest -e "ARGV.each { |f| require f }" /test/unit/fake_test.rb /test/unit/fake_test_2_test.rb"#,
    ]);
    assert_eq!(resolve_test_commands(items), expected);
}

#[test]
fn test_unit_resolve_properly_escapes_single_file_paths() {
    let items = json!([
        {
            "id": "file:///test/server(v2)_test.rb",
            "uri": "file:///test/server(v2)_test.rb",
            "label": "/test/server(v2)_test.rb",
            "tags": ["test_file", "framework:test_unit"],
            "children": [],
        },
    ]);
    let expected = commands(&[
        r#"{COMMAND} -Itest -e "ARGV.each { |f| require f }" /test/server\(v2\)_test.rb"#,
    ]);
    assert_eq!(resolve_test_commands(items), expected);
}

#[test]
fn test_unit_resolve_properly_escapes_file_paths_within_directories() {
    let directory = TestDirectory::new(&[
        r#"/other/test/fake(v2)_test.rb"#,
    ]);
    let items = directory.relocate(json!([
        {
            "id": "file:///other/test",
            "uri": "file:///other/test",
            "label": "/other/test",
            "tags": ["test_dir", "framework:test_unit"],
            "children": [],
        },
    ]));
    let expected = directory.relocate_commands(&[
        r#"{COMMAND} -Itest -e "ARGV.each { |f| require f }" /other/test/fake\(v2\)_test.rb"#,
    ]);
    assert_eq!(resolve_test_commands(items), expected);
}

#[test]
fn test_unit_resolve_properly_escapes_file_paths_in_groups() {
    let items = json!([
        {
            "id": "ServerTest",
            "uri": "file:///test/server(v2)_test.rb",
            "label": "ServerTest",
            "range": {"start": {"line": 0, "character": 0}, "end": {"line": 30, "character": 3}},
            "tags": ["framework:test_unit", "test_group"],
            "children": [],
        },
    ]);
    let expected = commands(&[
        r#"{COMMAND} -Itest /test/server\(v2\)_test.rb --testcase "/^ServerTest\$/""#,
    ]);
    assert_eq!(resolve_test_commands(items), expected);
}

#[test]
fn other_frameworks_ignores_items_tagged_by_other_frameworks() {
    let items = json!([
        {
            "id": "ServerTest",
            "uri": "file:///test/server_test.rb",
            "label": "ServerTest",
            "range": {"start": {"line": 0, "character": 0}, "end": {"line": 30, "character": 3}},
            "tags": ["framework:rails", "test_group"],
            "children": [
                {
                    "id": "ServerTest#test_server",
                    "uri": "file:///test/server_test.rb",
                    "label": "test_server",
                    "range": {"start": {"line": 1, "character": 2}, "end": {"line": 10, "character": 3}},
                    "tags": ["framework:rails"],
                    "children": [],
                },
            ],
        },
        {
            "id": "StoreTest",
            "uri": "file:///test/store_test.rb",
            "label": "StoreTest",
            "range": {"start": {"line": 0, "character": 0}, "end": {"line": 30, "character": 3}},
            "tags": ["framework:rails", "test_group"],
            "children": [
                {
                    "id": "StoreTest#test_store",
                    "uri": "file:///test/store_test.rb",
                    "label": "test_store",
                    "range": {"start": {"line": 1, "character": 2}, "end": {"line": 10, "character": 3}},
                    "tags": ["framework:rails"],
                    "children": [],
                },
            ],
        },
    ]);
    let expected = commands(&[

    ]);
    assert_eq!(resolve_test_commands(items), expected);
}
