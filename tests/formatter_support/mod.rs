#![allow(dead_code)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock, PoisonError};

use serde_json::{Value, json};

/// RuboCop and Syntax Tree are installed into a throwaway bundle in this directory, never into the repository
pub const TEST_DIRECTORY_VARIABLE: &str = "RUBY_LSP_RS_FORMATTER_TEST_DIRECTORY";
pub const TEST_RUBY_VARIABLE: &str = "RUBY_LSP_RS_TEST_RUBY";

#[derive(Clone)]
pub struct FormatterWorkspace {
    pub path: PathBuf,
    pub ruby_command: Vec<String>,
}

impl FormatterWorkspace {
    pub fn file_path(&self, relative_path: &str) -> PathBuf {
        self.path.join(relative_path)
    }

    pub fn uri_for(&self, relative_path: &str) -> String {
        ruby_lsp_rs::uri::from_path_buf(&self.file_path(relative_path))
    }

    pub fn server_options(&self, initialization_options: Value) -> Value {
        let mut initialization_options = initialization_options;
        initialization_options["rubyCommand"] = json!(self.ruby_command);
        json!({
            "workspaceFolders": [{ "uri": ruby_lsp_rs::uri::from_path_buf(&self.path) }],
            "initializationOptions": initialization_options,
        })
    }
}

fn template_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/formatter_workspace")
}

fn ruby_command() -> Vec<String> {
    vec![std::env::var(TEST_RUBY_VARIABLE).unwrap_or_else(|_| "ruby".to_string())]
}

fn test_directory() -> Option<PathBuf> {
    match std::env::var_os(TEST_DIRECTORY_VARIABLE) {
        Some(directory) => Some(PathBuf::from(directory)),
        None => {
            eprintln!("Skipping: set {TEST_DIRECTORY_VARIABLE} to a scratch directory to run the RuboCop and Syntax Tree tests");
            None
        }
    }
}

fn copy_directory(source: &Path, destination: &Path) {
    std::fs::create_dir_all(destination).unwrap();
    for entry in std::fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let target = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_directory(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn bundle(workspace: &Path, ruby_command: &[String], arguments: &[&str]) -> bool {
    let mut command = Command::new(&ruby_command[0]);
    command.arg("-S").arg("bundle").args(arguments).current_dir(workspace);
    if let Some(ruby_directory) = Path::new(&ruby_command[0]).parent().filter(|directory| !directory.as_os_str().is_empty()) {
        let path = std::env::var("PATH").unwrap_or_default();
        command.env("PATH", format!("{}:{path}", ruby_directory.display()));
    }
    command.output().is_ok_and(|output| {
        if !output.status.success() {
            eprintln!("bundle {arguments:?} failed: {}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        }
        output.status.success()
    })
}

fn prepared_workspaces() -> &'static Mutex<HashMap<String, Option<FormatterWorkspace>>> {
    static WORKSPACES: OnceLock<Mutex<HashMap<String, Option<FormatterWorkspace>>>> = OnceLock::new();
    WORKSPACES.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn workspace(name: &str, extra_files: &[(&str, &str)]) -> Option<FormatterWorkspace> {
    let mut workspaces = prepared_workspaces().lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(workspace) = workspaces.get(name) {
        return workspace.clone();
    }

    let prepared = test_directory().and_then(|directory| {
        let path = directory.join("workspaces").join(name);
        let _ = std::fs::remove_dir_all(&path);
        copy_directory(&template_path(), &path);
        for (relative_path, contents) in extra_files {
            let file = path.join(relative_path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, contents).unwrap();
        }
        std::fs::create_dir_all(path.join(".bundle")).unwrap();
        std::fs::write(path.join(".bundle/config"), format!("---\nBUNDLE_PATH: \"{}\"\n", directory.join("bundle").display())).unwrap();

        let ruby_command = ruby_command();
        if !bundle(&path, &ruby_command, &["check"]) && !bundle(&path, &ruby_command, &["install"]) {
            eprintln!("Skipping: could not install the formatter test bundle");
            return None;
        }
        Some(FormatterWorkspace { path, ruby_command })
    });
    workspaces.insert(name.to_string(), prepared.clone());
    prepared
}

pub fn workspace_without_bundle(name: &str) -> Option<FormatterWorkspace> {
    let directory = test_directory()?;
    let path = directory.join("workspaces").join(name);
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    Some(FormatterWorkspace { path, ruby_command: ruby_command() })
}

pub fn sanitize_code_action_uris(diagnostics: &mut Value) {
    let Some(diagnostics) = diagnostics.as_array_mut() else { return };
    for diagnostic in diagnostics {
        let Some(code_actions) =
            diagnostic.get_mut("data").and_then(|data| data.get_mut("code_actions")).and_then(Value::as_array_mut)
        else {
            continue;
        };
        for code_action in code_actions {
            let Some(document_changes) = code_action["edit"]["documentChanges"].as_array_mut() else { continue };
            for document_change in document_changes {
                document_change["textDocument"] = json!({ "uri": "file:///fake", "version": null });
            }
        }
    }
}
