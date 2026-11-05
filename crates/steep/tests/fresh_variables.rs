use std::{
    fs,
    path::{Path, PathBuf},
    rc::Rc,
};

use rbs_definition::loader::RbsGem;
use steep::{
    environment_loader,
    project::dsl,
    ruby::node::Node,
    services::type_check_service::{TypeCheckService, type_check},
    source::Source,
    steep_version::SteepVersion,
    strict::EnabledChecks,
    type_construction::Checker,
};

const STEEP_VERSIONS: [SteepVersion; 2] = [SteepVersion::Version2_0_0, SteepVersion::Version2_1_0];

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn rbs_gem() -> Option<RbsGem> {
    match environment_loader::rbs_gem(None) {
        Ok(rbs_gem) => Some(rbs_gem),
        Err(error) => {
            eprintln!(
                "skipped: the rbs gem {} is not found: {error}",
                environment_loader::RBS_VERSION
            );
            None
        }
    }
}

fn project_directories() -> Vec<PathBuf> {
    let mut directories: Vec<PathBuf> = fs::read_dir(repository().join("e2e/cases"))
        .expect("the end-to-end cases are readable")
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.join("Steepfile").is_file())
        .collect();
    directories.sort();
    directories.push(repository().join("fixtures/services"));
    directories
}

fn collect_types(node: &Rc<Node>, typing: &steep::typing::Typing, types: &mut Vec<String>) {
    if let Some(type_) = typing.type_of(node.id) {
        types.push(format!("{:?} {type_}", node.location.expression));
    }
    for child in node.child_nodes() {
        collect_types(child, typing, types);
    }
}

fn types_and_errors(checker: &Checker, source: &Source) -> Vec<String> {
    checker.builder.restart_fresh_variable_count();
    let typing = type_check(checker, source, None);
    let mut lines = Vec::new();
    if let Some(node) = &source.node {
        collect_types(node, &typing, &mut lines);
    }
    lines.extend(typing.errors().iter().map(|error| format!("{error:?}")));
    lines
}

#[test]
fn a_file_is_typed_the_same_after_the_checker_has_checked_other_files() {
    steep::ruby_exception::install_panic_hook();
    let Some(rbs_gem) = rbs_gem() else { return };
    let mut failures = Vec::new();
    for steep_version in STEEP_VERSIONS {
        for directory in project_directories() {
            let steepfile =
                fs::read_to_string(directory.join("Steepfile")).expect("the Steepfile is readable");
            let project = dsl::parse(&directory, &steepfile).expect("the Steepfile is read");
            let mut service = TypeCheckService::load(
                project,
                &rbs_gem,
                steep_version,
                EnabledChecks::default(),
                1,
            )
            .expect("the signatures load");
            service.load_files().expect("the files are read");
            let mut paths: Vec<PathBuf> = service
                .source_files
                .keys()
                .filter(|path| path.extension().is_some_and(|extension| extension == "rb"))
                .cloned()
                .collect();
            paths.sort();
            let sources: Vec<(PathBuf, Rc<Checker>, Source)> = paths
                .into_iter()
                .filter_map(|path| {
                    let target = service.source_target(&path)?;
                    let checker = service.checker(&target.name)?;
                    let source = Source::parse(
                        &path.to_string_lossy(),
                        &service.source_files[&path].content,
                        checker.builder.steep_version,
                    )
                    .ok()?;
                    Some((path, checker, source))
                })
                .collect();
            let first: Vec<Vec<String>> = sources
                .iter()
                .map(|(_, checker, source)| types_and_errors(checker, source))
                .collect();
            for ((path, checker, source), first) in sources.iter().zip(&first) {
                if types_and_errors(checker, source) != *first {
                    failures.push(format!("{steep_version:?} {}", path.display()));
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "typed differently when checked again:\n{}",
        failures.join("\n")
    );
}
