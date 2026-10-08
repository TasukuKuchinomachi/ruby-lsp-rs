use std::path::PathBuf;
use std::sync::OnceLock;

const STATIC_DOCS: &[(&str, &str)] =
    &[("break.md", include_str!("../../../static_docs/break.md")), ("yield.md", include_str!("../../../static_docs/yield.md"))];

pub const GUESSED_TYPES_URL: &str = "https://shopify.github.io/ruby-lsp/#guessed-types";

/// The documents are embedded so that the binary keeps working after being moved away from the source tree; editors
/// still need a file to open, so they are written to the temporary directory once
pub fn static_docs_path() -> PathBuf {
    static PATH: OnceLock<PathBuf> = OnceLock::new();
    PATH.get_or_init(|| {
        let directory = std::env::temp_dir().join(format!("ruby-lsp-rs-{}-static-docs", env!("CARGO_PKG_VERSION")));
        if std::fs::create_dir_all(&directory).is_ok() {
            for (file_name, contents) in STATIC_DOCS {
                let path = directory.join(file_name);
                if std::fs::read_to_string(&path).ok().as_deref() != Some(*contents) {
                    let _ = std::fs::write(&path, contents);
                }
            }
        }
        directory
    })
    .clone()
}

pub fn keyword_documentation(keyword: &str) -> Option<&'static str> {
    match keyword {
        "break" => Some("Terminates the execution of a block or loop"),
        "yield" => Some("Invokes the passed block with the given arguments"),
        _ => None,
    }
}

pub fn keyword_documentation_uri(keyword: &str) -> String {
    crate::uri::from_path_buf(&static_docs_path().join(format!("{keyword}.md")))
}
