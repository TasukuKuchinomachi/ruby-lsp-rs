#![allow(dead_code)]

use std::path::PathBuf;

use ruby_lsp_rs::document::{Document, LanguageId};
use ruby_lsp_rs::listeners::document_link::{DocumentLink, DocumentLinkItem, GemPaths};
use ruby_lsp_rs::position::Encoding;
use ruby_lsp_rs::ruby_environment::{DefaultGemStub, GemStub};

pub const SYNTAX_TREE_VERSION: &str = "9.8.7";
pub const FAKE_RUBYLIBDIR: &str = "/ruby-lsp-rs-test-rubylibdir";

/// Stands in for the bundle of Ruby LSP's own test suite, where `syntax_tree` is installed and `open3` and `erb` are
/// default gems
pub fn fake_bundle_path() -> PathBuf {
    let bundle_path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("document_link_bundle");
    let library_path = bundle_path.join(format!("gems/syntax_tree-{SYNTAX_TREE_VERSION}/lib"));
    std::fs::create_dir_all(&library_path).unwrap();
    std::fs::write(library_path.join("syntax_tree.rb"), "module SyntaxTree\nend\n").unwrap();
    bundle_path
}

pub fn fake_gem_paths() -> GemPaths {
    let bundle_path = fake_bundle_path();
    let gem_stubs = vec![GemStub {
        name: "syntax_tree".to_string(),
        version: SYNTAX_TREE_VERSION.to_string(),
        full_gem_path: bundle_path.join(format!("gems/syntax_tree-{SYNTAX_TREE_VERSION}")).to_string_lossy().into_owned(),
    }];
    let default_gem_stubs = vec![
        DefaultGemStub {
            name: "open3".to_string(),
            version: "0.1.2".to_string(),
            require_paths: vec!["lib".to_string()],
            files: vec!["lib/open3.rb".to_string(), "lib/open3/version.rb".to_string()],
        },
        DefaultGemStub {
            name: "erb".to_string(),
            version: "4.0.2".to_string(),
            require_paths: vec!["lib".to_string()],
            files: vec!["lib/erb.rb".to_string(), "lib/erb/version.rb".to_string()],
        },
    ];
    GemPaths::new(&gem_stubs, &default_gem_stubs, FAKE_RUBYLIBDIR)
}

pub fn document_links(source: &str, uri: &str) -> Vec<DocumentLinkItem> {
    let source = source.replace("SYNTAX_TREE_VERSION", SYNTAX_TREE_VERSION);
    let document = Document::new(uri, source, 1, LanguageId::Ruby, Encoding::Utf16);
    let parsed = document.parse();
    let gem_paths = fake_gem_paths();
    DocumentLink::new(&parsed, uri, &gem_paths).perform()
}

/// The reverse of the substitutions done by Ruby LSP's `DocumentLinkExpectationsTest#substitute`
pub fn with_placeholders(text: &str) -> String {
    text.replace(&fake_bundle_path().to_string_lossy().into_owned(), "BUNDLER_PATH")
        .replace(FAKE_RUBYLIBDIR, "RUBY_ROOT")
        .replace(SYNTAX_TREE_VERSION, "SYNTAX_TREE_VERSION")
}
