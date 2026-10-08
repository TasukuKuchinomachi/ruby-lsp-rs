//! Prints the hints and diagnostics of one Ruby file, for investigating inference results.
//!
//! cargo run -p type-checker --example inspect_types -- <rbs core directory> <ruby file> [first line] [last line]

use std::path::Path;

use rbs_environment::Loader;

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let [core_directory, ruby_file, rest @ ..] = arguments.as_slice() else {
        eprintln!("usage: inspect_types <rbs core directory> <ruby file> [first line] [last line]");
        return;
    };
    let first_line: usize = rest.first().and_then(|line| line.parse().ok()).unwrap_or(1);
    let last_line: usize = rest
        .get(1)
        .and_then(|line| line.parse().ok())
        .unwrap_or(usize::MAX);
    // With `WORKSPACE=<project>` the project signatures are loaded the way the server does, with `core_directory`
    // being `<rbs gem>` itself
    let environment = match std::env::var("WORKSPACE") {
        Ok(workspace) => {
            let rbs_gem = Path::new(core_directory);
            let sources = rbs_environment::WorkspaceSources {
                core_directory: Some(rbs_gem.join("core")),
                stdlib_directory: Some(rbs_gem.join("stdlib")),
                gem_directories: std::env::var("GEM_DIRECTORIES")
                    .unwrap_or_default()
                    .split(',')
                    .filter_map(|pair| pair.split_once('='))
                    .map(|(name, path)| (name.to_string(), std::path::PathBuf::from(path)))
                    .collect(),
            };
            rbs_environment::load_workspace(Path::new(&workspace), &sources)
        }
        Err(_) => {
            let mut loader = Loader::new();
            loader.add_library_directory(Path::new(core_directory));
            loader.finish()
        }
    };
    let Ok(source) = std::fs::read(ruby_file) else {
        eprintln!("cannot read {ruby_file}");
        return;
    };
    let analysis = type_checker::analyze(&environment, &source);
    let line_of = |offset: usize| {
        source[..offset.min(source.len())]
            .iter()
            .filter(|byte| **byte == b'\n')
            .count()
            + 1
    };
    let in_range = |offset: usize| (first_line..=last_line).contains(&line_of(offset));
    for hint in analysis
        .type_hints
        .iter()
        .filter(|hint| in_range(hint.offset))
    {
        let preceding_start = hint.offset.saturating_sub(24);
        let preceding =
            String::from_utf8_lossy(&source[preceding_start..hint.offset]).replace('\n', " ");
        println!(
            "{}: {:?} …{preceding}‹{}›",
            line_of(hint.offset),
            hint.kind,
            hint.hint_type
        );
    }
    for hover in analysis.hovers.iter().filter(|hover| in_range(hover.start)) {
        let text = String::from_utf8_lossy(&source[hover.start..hover.end]);
        let description = match &hover.kind {
            type_checker::HoverKind::Type(hover_type) => hover_type.to_string(),
            type_checker::HoverKind::Method {
                return_type, owner, ..
            } => format!("{owner} method -> {return_type}"),
            type_checker::HoverKind::Constant { constant_type, .. } => constant_type.to_string(),
        };
        println!("{}: hover {text} : {description}", line_of(hover.start));
    }
    for diagnostic in analysis
        .diagnostics
        .iter()
        .filter(|diagnostic| in_range(diagnostic.start))
    {
        println!(
            "{}: {}: {}",
            line_of(diagnostic.start),
            diagnostic.code,
            diagnostic.message
        );
    }
}
