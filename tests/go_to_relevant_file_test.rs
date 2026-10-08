mod support;
mod test_discovery_support;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use ruby_lsp_rs::requests::go_to_relevant_file::find_relevant_paths;
use serde_json::json;
use support::{DEFAULT_URI, TestOptions, with_server};

struct Workspace {
    root: PathBuf,
}

impl Workspace {
    fn new() -> Workspace {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "ruby-lsp-rs-go-to-relevant-file-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&root).unwrap();
        Workspace { root }
    }

    fn path(&self, relative_path: &str) -> String {
        self.root.join(relative_path).to_string_lossy().into_owned()
    }

    fn touch(&self, relative_path: &str) -> String {
        let path = self.path(relative_path);
        std::fs::create_dir_all(Path::new(&path).parent().unwrap()).unwrap();
        std::fs::write(&path, "").unwrap();
        path
    }

    fn mkdir(&self, relative_path: &str) {
        std::fs::create_dir_all(self.path(relative_path)).unwrap();
    }

    fn relevant_paths(&self, path: &str) -> Vec<String> {
        find_relevant_paths(path, &self.root.to_string_lossy())
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn sorted(mut paths: Vec<String>) -> Vec<String> {
    paths.sort();
    paths
}

#[test]
fn when_input_is_test_file_returns_array_of_implementation_file_locations() {
    let workspace = Workspace::new();
    let implementation_file = workspace.touch("lib/ruby_lsp/requests/go_to_relevant_file.rb");
    let test_file = workspace.touch("test/requests/go_to_relevant_file_test.rb");

    assert_eq!(workspace.relevant_paths(&test_file), [implementation_file]);
}

#[test]
fn when_input_is_implementation_file_returns_array_of_test_file_locations() {
    let workspace = Workspace::new();
    let implementation_file = workspace.touch("lib/ruby_lsp/requests/go_to_relevant_file.rb");
    let test_file = workspace.touch("test/requests/go_to_relevant_file_test.rb");

    assert_eq!(workspace.relevant_paths(&implementation_file), [test_file]);
}

#[test]
fn return_empty_array_when_no_filename_matches() {
    let workspace = Workspace::new();
    let implementation_file = workspace.touch("lib/ruby_lsp/requests/nonexistent_file.rb");

    assert!(workspace.relevant_paths(&implementation_file).is_empty());
}

#[test]
fn it_finds_multiple_matching_tests() {
    let workspace = Workspace::new();
    workspace.mkdir("test");
    let implementation_file = workspace.touch("lib/ruby_lsp/requests/some_feature.rb");
    let unit_test = workspace.touch("test/unit/some_feature_test.rb");
    let integration_test = workspace.touch("test/integration/some_feature_test.rb");

    assert_eq!(sorted(workspace.relevant_paths(&implementation_file)), sorted(vec![unit_test, integration_test]));
}

#[test]
fn search_within_implementation_test_root() {
    let workspace = Workspace::new();
    let implementation_a_file = workspace.touch("a/implementation.rb");
    let unit_a_test = workspace.touch("a/test/implementation_test.rb");
    workspace.touch("b/implementation.rb");
    workspace.touch("b/test/implementation_test.rb");

    assert_eq!(workspace.relevant_paths(&implementation_a_file), [unit_a_test]);
}

#[test]
fn finds_tests_in_matching_subdirectory() {
    let workspace = Workspace::new();
    let implementation_file = workspace.touch("lib/user.rb");
    let test_file_1 = workspace.touch("test/user/create_user_test.rb");
    let test_file_2 = workspace.touch("test/user/test_update_user.rb");

    assert_eq!(sorted(workspace.relevant_paths(&implementation_file)), sorted(vec![test_file_1, test_file_2]));
}

#[test]
fn finds_implementation_from_nested_test_file() {
    let workspace = Workspace::new();
    let implementation_file = workspace.touch("lib/go_to_relevant_file.rb");
    let test_file = workspace.touch("test/go_to_relevant_file/go_to_relevant_file_a_test.rb");

    assert_eq!(workspace.relevant_paths(&test_file), [implementation_file]);
}

#[test]
fn request_returns_locations_for_files_inside_the_workspace() {
    let workspace = Workspace::new();
    let implementation_file = workspace.touch("lib/foo.rb");
    let test_file = workspace.touch("test/foo_test.rb");
    let mut server = with_server(
        None,
        DEFAULT_URI,
        TestOptions {
            initialization_options: Some(test_discovery_support::workspace_options(&workspace.root, json!({}))),
            ..TestOptions::default()
        },
    );

    let result = server.result(
        "experimental/goToRelevantFile",
        json!({ "textDocument": { "uri": ruby_lsp_rs::uri::from_path(&implementation_file) } }),
    );
    assert_eq!(result, json!({ "locations": [test_file] }));

    let outside_workspace =
        server.result("experimental/goToRelevantFile", json!({ "textDocument": { "uri": "file:///other/foo.rb" } }));
    assert_eq!(outside_workspace, serde_json::Value::Null);
}
