use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub const TEST_PATH_PATTERN: &str = "**/{test,spec,features}/**/{*_test.rb,test_*.rb,*_spec.rb,*.feature}";

const REPORTER_FILES: &[(&str, &str)] = &[
    (
        "lib/ruby_lsp/test_reporters/lsp_reporter.rb",
        include_str!("../../../ruby/test_reporters/lib/ruby_lsp/test_reporters/lsp_reporter.rb"),
    ),
    (
        "lib/ruby_lsp/test_reporters/minitest_reporter.rb",
        include_str!("../../../ruby/test_reporters/lib/ruby_lsp/test_reporters/minitest_reporter.rb"),
    ),
    (
        "lib/ruby_lsp/test_reporters/test_unit_reporter.rb",
        include_str!("../../../ruby/test_reporters/lib/ruby_lsp/test_reporters/test_unit_reporter.rb"),
    ),
    (
        "lib/ruby_indexer/lib/ruby_indexer/uri.rb",
        include_str!("../../../ruby/test_reporters/lib/ruby_indexer/lib/ruby_indexer/uri.rb"),
    ),
];

pub fn shell_escape(text: &str) -> String {
    if text.is_empty() {
        return "''".to_string();
    }
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '\n' => escaped.push_str("'\n'"),
            'A'..='Z' | 'a'..='z' | '0'..='9' | '_' | '-' | '.' | ',' | ':' | '+' | '/' | '@' => escaped.push(character),
            _ => {
                escaped.push('\\');
                escaped.push(character);
            }
        }
    }
    escaped
}

/// Mirrors `Bundler.default_lockfile`, which raises when no Gemfile is found. Ruby LSP's server runs from the workspace,
/// so the search starts from the workspace path instead of the current directory of this process
pub fn base_command(workspace_path: &str) -> &'static str {
    if bundle_gemfile_exists(Path::new(workspace_path)) { "bundle exec ruby" } else { "ruby" }
}

fn bundle_gemfile_exists(workspace_path: &Path) -> bool {
    if std::env::var_os("BUNDLE_GEMFILE").is_some_and(|gemfile| !gemfile.is_empty()) {
        return true;
    }
    workspace_path
        .ancestors()
        .any(|directory| directory.join("gems.rb").is_file() || directory.join("Gemfile").is_file())
}

/// Ruby LSP requires the reporters from its installed gem. They are bundled into the binary and written to a directory
/// on first use, since the binary may run far from its source tree
pub fn reporters_directory() -> &'static Path {
    static DIRECTORY: OnceLock<PathBuf> = OnceLock::new();
    DIRECTORY.get_or_init(|| {
        let directory = std::env::temp_dir().join(format!("ruby-lsp-rs-{}-test-reporters", env!("CARGO_PKG_VERSION")));
        for (relative_path, content) in REPORTER_FILES {
            let path = directory.join(relative_path);
            if std::fs::read_to_string(&path).is_ok_and(|existing| existing == *content) {
                continue;
            }
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&path, content);
        }
        directory
    })
}

pub fn minitest_reporter_path() -> String {
    reporters_directory().join("lib/ruby_lsp/test_reporters/minitest_reporter.rb").to_string_lossy().into_owned()
}

pub fn test_unit_reporter_path() -> String {
    reporters_directory().join("lib/ruby_lsp/test_reporters/test_unit_reporter.rb").to_string_lossy().into_owned()
}

pub fn test_command(workspace_path: &str) -> String {
    format!("{} -r{} -r{}", base_command(workspace_path), minitest_reporter_path(), test_unit_reporter_path())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_like_shellwords() {
        assert_eq!(shell_escape(""), "''");
        assert_eq!(shell_escape("/test/server(v2)_test.rb"), "/test/server\\(v2\\)_test.rb");
        assert_eq!(shell_escape("Baz::#bar"), "Baz::\\#bar");
        assert_eq!(shell_escape("a b\nc"), "a\\ b'\n'c");
    }
}
