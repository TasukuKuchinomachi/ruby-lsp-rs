//! Analyzes Ruby files against RBS core and prints timings and diagnostics.
//!
//! cargo run --release -p type-checker --example analyze_files -- <rbs core directory> <ruby file or directory>...

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rbs_environment::Loader;

fn collect_ruby_files(path: &Path, files: &mut Vec<PathBuf>) {
    if path.is_file() {
        if path.extension().is_some_and(|extension| extension == "rb") {
            files.push(path.to_path_buf());
        }
        return;
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        collect_ruby_files(&entry.path(), files);
    }
}

fn main() {
    let mut arguments = std::env::args().skip(1);
    let Some(core_directory) = arguments.next() else {
        eprintln!("usage: analyze_files <rbs core directory> <ruby file or directory>...");
        return;
    };
    let mut loader = Loader::new();
    loader.add_library_directory(Path::new(&core_directory));
    let started_at = Instant::now();
    let environment = loader.finish();
    eprintln!("loaded core in {:?}", started_at.elapsed());

    let mut files = Vec::new();
    for path in arguments {
        collect_ruby_files(Path::new(&path), &mut files);
    }
    files.sort();
    let verbose = std::env::var("VERBOSE").is_ok();
    let mut total_duration = Duration::ZERO;
    let mut total_lines = 0;
    let mut slowest: Vec<(Duration, usize, PathBuf)> = Vec::new();
    let mut counts_by_code: BTreeMap<&'static str, usize> = BTreeMap::new();
    for file in &files {
        let Ok(source) = std::fs::read(file) else {
            continue;
        };
        let lines = source.iter().filter(|byte| **byte == b'\n').count();
        let started_at = Instant::now();
        let analysis = type_checker::analyze(&environment, &source);
        let duration = started_at.elapsed();
        total_duration += duration;
        total_lines += lines;
        slowest.push((duration, lines, file.clone()));
        for diagnostic in &analysis.diagnostics {
            *counts_by_code.entry(diagnostic.code).or_default() += 1;
            if verbose {
                let line = source[..diagnostic.start.min(source.len())]
                    .iter()
                    .filter(|byte| **byte == b'\n')
                    .count()
                    + 1;
                println!(
                    "{}:{}: {}: {}",
                    file.display(),
                    line,
                    diagnostic.code,
                    diagnostic.message
                );
            }
        }
    }
    slowest.sort_by_key(|(duration, _, _)| std::cmp::Reverse(*duration));
    eprintln!(
        "analyzed {} files, {} lines in {:?}",
        files.len(),
        total_lines,
        total_duration
    );
    for (duration, lines, file) in slowest.iter().take(5) {
        eprintln!("  {duration:?} {lines} lines {}", file.display());
    }
    for (code, count) in counts_by_code {
        eprintln!("{code}: {count}");
    }
}
