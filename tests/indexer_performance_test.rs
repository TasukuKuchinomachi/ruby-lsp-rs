mod indexer_support;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use ruby_lsp_rs::indexer::configuration::IndexablePath;
use ruby_lsp_rs::indexer::indexing::index_all;
use ruby_lsp_rs::indexer::{Configuration, Index};

fn ruby_library_directory() -> PathBuf {
    if let Ok(path) = std::env::var("RUBY_LSP_RUBYLIBDIR") {
        return PathBuf::from(path);
    }
    let output = std::process::Command::new("ruby")
        .args(["-e", "print RbConfig::CONFIG['rubylibdir']"])
        .output()
        .expect("ruby must be available to locate rubylibdir");
    PathBuf::from(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[test]
#[ignore = "performance sanity check, run with `cargo test --release --test indexer_performance_test -- --ignored --nocapture`"]
fn indexes_the_ruby_standard_library_and_core_signatures() {
    let library_directory = ruby_library_directory();
    let paths: Vec<IndexablePath> = walkdir::WalkDir::new(&library_directory)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file() && entry.path().extension().is_some_and(|extension| extension == "rb"))
        .map(|entry| {
            let path = entry.into_path();
            let require_path = Some(ruby_lsp_rs::uri::require_path(&path.to_string_lossy(), &library_directory.to_string_lossy()));
            IndexablePath { path, require_path }
        })
        .collect();

    let rbs_core_path = indexer_support::rbs_core_path();
    let mut index = Index::new(Arc::new(Configuration::default()));
    let started = Instant::now();
    index_all(&mut index, &paths, Some(&rbs_core_path.to_string_lossy()), &|_| {});
    let elapsed = started.elapsed();

    let entry_count = index.all_entry_ids().count();
    println!(
        "indexed {} files from {} plus RBS core in {:?}: {} names, {} entries, {} require paths",
        paths.len(),
        library_directory.display(),
        elapsed,
        index.len(),
        entry_count,
        index.search_require_paths("").len()
    );
    assert!(index.initial_indexing_completed);
    assert!(index.len() > 1000);
}
