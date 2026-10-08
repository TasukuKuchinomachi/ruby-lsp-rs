#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde_json::{Value, json};

use crate::support::{TestOptions, TestServer, with_server};

/// Ruby LSP's own test suite runs inside its bundle, so test commands start with `bundle exec ruby`
pub fn bundled_workspace() -> &'static Path {
    static WORKSPACE: OnceLock<PathBuf> = OnceLock::new();
    WORKSPACE.get_or_init(|| {
        let workspace = std::env::temp_dir().join(format!("ruby-lsp-rs-bundled-workspace-{}", std::process::id()));
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(workspace.join("Gemfile"), "source \"https://rubygems.org\"\n").unwrap();
        workspace
    })
}

pub fn workspace_options(workspace: &Path, initialization_options: Value) -> Value {
    json!({
        "workspaceFolders": [{ "uri": ruby_lsp_rs::uri::from_path_buf(workspace), "name": "workspace" }],
        "initializationOptions": initialization_options,
    })
}

pub fn bundled_server(source: Option<&str>, uri: &str, test_library: &str, initialization_options: Value) -> TestServer {
    with_server(
        source,
        uri,
        TestOptions {
            stub_no_typechecker: true,
            test_library: Some(test_library.to_string()),
            initialization_options: Some(workspace_options(bundled_workspace(), initialization_options)),
            ..TestOptions::default()
        },
    )
}

pub fn fixture_uri(fixture_name: &str) -> String {
    format!("file://test/fixtures/{fixture_name}.rb")
}
