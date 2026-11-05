use std::{
    fs,
    path::{Path, PathBuf},
    rc::Rc,
};

use rbs_definition::{collections::HashMap, loader::RbsGem};
use regex::Regex;
use steep::{
    environment_loader,
    inference::{Inference, InferenceRule, InferenceRules},
    project::dsl,
    ruby::node::{Node, NodeId},
    services::type_check_service::{TypeCheckService, type_check_inferring},
    source::Source,
    steep_version::SteepVersion,
    strict::EnabledChecks,
    typing::Typing,
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

fn load_service(
    rbs_gem: &RbsGem,
    directory: &Path,
    steep_version: SteepVersion,
) -> TypeCheckService {
    let steepfile =
        fs::read_to_string(directory.join("Steepfile")).expect("the Steepfile is readable");
    let project = dsl::parse(directory, &steepfile).expect("the Steepfile is read");
    let mut service =
        TypeCheckService::load(project, rbs_gem, steep_version, EnabledChecks::default(), 1)
            .expect("the signatures load");
    service.load_files().expect("the files are read");
    service
}

fn ruby_paths(service: &TypeCheckService) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = service
        .source_files
        .keys()
        .filter(|path| path.extension().is_some_and(|extension| extension == "rb"))
        .cloned()
        .collect();
    paths.sort();
    paths
}

struct Checked {
    source: Source,
    typing: Typing,
}

fn parse(service: &TypeCheckService, path: &Path) -> Option<Source> {
    let target = service.source_target(path)?;
    let checker = service.checker(&target.name)?;
    Source::parse(
        &path.to_string_lossy(),
        &service.source_files[path].content,
        checker.builder.steep_version,
    )
    .ok()
}

fn check(
    service: &TypeCheckService,
    path: &Path,
    source: &Source,
    rules: InferenceRules,
) -> Checked {
    let target = service.source_target(path).expect("the file has a target");
    let checker = service.checker(&target.name).expect("the signatures load");
    let (source, inference) = Inference::prepare(&checker, rules, source.clone());
    let typing = type_check_inferring(&checker, &source, None, &inference);
    Checked { source, typing }
}

fn node_ids(node: &Rc<Node>, ids: &mut Vec<NodeId>) {
    ids.push(node.id);
    for child in node.child_nodes() {
        node_ids(child, ids);
    }
}

// Why not reset the count of fresh type variables before each check instead: the checker
// caches subtyping results by type, and a variable name used again after a reset hits
// the results of the earlier check.
fn without_fresh_variable_numbers(text: &str) -> String {
    Regex::new(r"\(\d+\)")
        .expect("the pattern is valid")
        .replace_all(text, "(_)")
        .into_owned()
}

fn node_types(checked: &Checked) -> HashMap<NodeId, String> {
    let mut ids = Vec::new();
    if let Some(node) = &checked.source.node {
        node_ids(node, &mut ids);
    }
    ids.into_iter()
        .filter_map(|id| {
            let type_ = checked.typing.type_of(id)?;
            Some((id, without_fresh_variable_numbers(&type_.to_string())))
        })
        .collect()
}

fn differs(
    types: &HashMap<NodeId, String>,
    steep_types: &HashMap<NodeId, String>,
    id: NodeId,
) -> bool {
    types.get(&id) != steep_types.get(&id)
}

fn describe(source: &Source, id: NodeId) -> String {
    fn find(node: &Rc<Node>, id: NodeId) -> Option<Rc<Node>> {
        if node.id == id {
            return Some(node.clone());
        }
        node.child_nodes().find_map(|child| find(child, id))
    }
    source
        .node
        .as_ref()
        .and_then(|node| find(node, id))
        .and_then(|node| node.location.expression.map(|range| (node.type_, range)))
        .map(|(type_, range)| {
            format!(
                "{type_} `{}` at line {}",
                source.buffer.source_of(range),
                source.buffer.line(range.begin_pos)
            )
        })
        .unwrap_or_else(|| format!("node {id}"))
}

struct Comparison {
    differences: usize,
    failures: Vec<String>,
}

fn compare_file(service: &TypeCheckService, path: &Path) -> Comparison {
    let mut failures = Vec::new();
    let Some(source) = parse(service, path) else {
        return Comparison {
            differences: 0,
            failures,
        };
    };
    let steep = check(service, path, &source, InferenceRules::STEEP);
    if !steep.typing.inference_rule_applications().is_empty() {
        failures.push(format!(
            "{}: the Steep compatible check applied an inference rule",
            path.display()
        ));
    }
    let steep_types = node_types(&steep);
    let more_than_steep = node_types(&check(
        service,
        path,
        &source,
        InferenceRules::more_than_steep(),
    ));
    let mut only_rule = Vec::new();
    let mut all_but_rule = Vec::new();
    for rule in InferenceRule::ALL {
        let checked = check(service, path, &source, InferenceRules::only(rule));
        let types = node_types(&checked);
        let applications = checked.typing.inference_rule_applications();
        let changes_types = types
            .keys()
            .chain(steep_types.keys())
            .any(|id| differs(&types, &steep_types, *id));
        if changes_types && applications.is_empty() {
            let changed: Vec<String> = types
                .keys()
                .chain(steep_types.keys())
                .filter(|id| differs(&types, &steep_types, **id))
                .map(|id| {
                    format!(
                        "{} `{:?}` `{:?}`",
                        describe(&source, *id),
                        types.get(id),
                        steep_types.get(id)
                    )
                })
                .collect();
            failures.push(format!(
                "{}: types change with only {rule:?} enabled, but the rule recorded no application: {}",
                path.display(),
                changed.join(", ")
            ));
        }
        if let Some((id, other)) = applications.iter().find(|(_, applied)| *applied != rule) {
            failures.push(format!(
                "{}: {other:?} was applied at {} with only {rule:?} enabled",
                path.display(),
                describe(&checked.source, *id)
            ));
        }
        only_rule.push((rule, types));
        all_but_rule.push((
            rule,
            node_types(&check(
                service,
                path,
                &source,
                InferenceRules::more_than_steep().without(rule),
            )),
        ));
    }
    let mut ids: Vec<NodeId> = more_than_steep
        .keys()
        .chain(steep_types.keys())
        .copied()
        .collect();
    ids.sort();
    ids.dedup();
    let mut differences = 0;
    for id in ids {
        if !differs(&more_than_steep, &steep_types, id) {
            continue;
        }
        differences += 1;
        let explained = only_rule
            .iter()
            .any(|(_, types)| differs(types, &steep_types, id))
            || all_but_rule
                .iter()
                .any(|(_, types)| !differs(types, &steep_types, id));
        if !explained {
            failures.push(format!(
                "{}: {} is `{}` instead of `{}`, which no inference rule explains",
                path.display(),
                describe(&source, id),
                more_than_steep.get(&id).map_or("(none)", String::as_str),
                steep_types.get(&id).map_or("(none)", String::as_str),
            ));
        }
    }
    Comparison {
        differences,
        failures,
    }
}

fn compare_diagnostics(service: &mut TypeCheckService, path: &Path) -> Vec<String> {
    service.inference_rules = InferenceRules::STEEP;
    let steep = without_fresh_variable_numbers(&format!("{:?}", service.typecheck_source(path)));
    service.inference_rules = InferenceRules::more_than_steep();
    let more_than_steep =
        without_fresh_variable_numbers(&format!("{:?}", service.typecheck_source(path)));
    service.inference_rules = InferenceRules::STEEP;
    if steep == more_than_steep {
        Vec::new()
    } else {
        vec![format!(
            "{}: the diagnostics differ when inferring more than Steep:\n{steep:?}\n{more_than_steep:?}",
            path.display()
        )]
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
    directories.push(repository().join("fixtures/inference_rules"));
    directories
}

#[test]
fn inferring_more_than_steep_keeps_the_diagnostics_and_changes_types_only_through_the_rules() {
    steep::ruby_exception::install_panic_hook();
    let Some(rbs_gem) = rbs_gem() else { return };
    let mut failures = Vec::new();
    let mut files = 0;
    let mut differences = 0;
    for steep_version in STEEP_VERSIONS {
        for directory in project_directories() {
            let mut service = load_service(&rbs_gem, &directory, steep_version);
            for path in ruby_paths(&service) {
                files += 1;
                let prefix = format!("Steep {steep_version} {}", directory.display());
                let comparison = compare_file(&service, &path);
                differences += comparison.differences;
                failures.extend(
                    comparison
                        .failures
                        .into_iter()
                        .map(|failure| format!("{prefix}: {failure}")),
                );
                failures.extend(
                    compare_diagnostics(&mut service, &path)
                        .into_iter()
                        .map(|failure| format!("{prefix}: {failure}")),
                );
            }
        }
    }
    eprintln!("compared {files} files, {differences} nodes typed differently");
    assert!(differences > 0);
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

fn type_of_text(
    service: &TypeCheckService,
    path: &str,
    text: &str,
    rules: InferenceRules,
) -> String {
    let path = Path::new(path);
    let source = parse(service, path).expect("the file parses");
    let begin = service.source_files[path]
        .content
        .rfind(text)
        .expect("the text is in the file");
    let checked = check(service, path, &source, rules);
    fn find(node: &Rc<Node>, begin: usize, end: usize) -> Option<NodeId> {
        if let Some(range) = node.location.expression
            && range.begin_pos == begin
            && range.end_pos == end
        {
            return Some(node.id);
        }
        node.child_nodes().find_map(|child| find(child, begin, end))
    }
    let id = checked
        .source
        .node
        .as_ref()
        .and_then(|node| find(node, begin, begin + text.len()))
        .expect("a node covers the text");
    checked
        .typing
        .type_of(id)
        .expect("the node is typed")
        .to_string()
}

fn assert_types(
    path: &str,
    text: &str,
    rule: InferenceRule,
    expected: [(SteepVersion, &str, &str); 2],
) {
    steep::ruby_exception::install_panic_hook();
    let Some(rbs_gem) = rbs_gem() else { return };
    for (steep_version, steep_type, inferred_type) in expected {
        let service = load_service(
            &rbs_gem,
            &repository().join("fixtures/inference_rules"),
            steep_version,
        );
        assert_eq!(
            type_of_text(&service, path, text, InferenceRules::STEEP),
            steep_type,
            "Steep {steep_version}, Steep compatible"
        );
        assert_eq!(
            type_of_text(&service, path, text, InferenceRules::only(rule)),
            inferred_type,
            "Steep {steep_version}, only {rule:?}"
        );
        assert_eq!(
            type_of_text(&service, path, text, InferenceRules::more_than_steep()),
            inferred_type,
            "Steep {steep_version}, all rules"
        );
    }
}

fn same_in_both_versions<'a>(
    steep_type: &'a str,
    inferred_type: &'a str,
) -> [(SteepVersion, &'a str, &'a str); 2] {
    STEEP_VERSIONS.map(|steep_version| (steep_version, steep_type, inferred_type))
}

#[test]
fn a_frozen_array_literal_assigned_to_a_constant_is_typed_with_the_constant_type() {
    assert_types(
        "collection_literal_with_hint_of_self_returning_call.rb",
        "[{ name: \"a\" }].freeze",
        InferenceRule::CollectionLiteralWithHintOfSelfReturningCall,
        same_in_both_versions(
            "::Array[::Hash[::Symbol, ::String]]",
            "::Array[{ :name => ::String }]",
        ),
    );
}

#[test]
fn a_frozen_hash_literal_returned_from_a_method_returning_a_record_is_the_record() {
    assert_types(
        "collection_literal_with_hint_of_self_returning_call.rb",
        "{ key:, label: }.freeze",
        InferenceRule::CollectionLiteralWithHintOfSelfReturningCall,
        same_in_both_versions(
            "::Hash[::Symbol, (::Symbol | ::String)]",
            "{ :key => ::Symbol, :label => ::String }",
        ),
    );
}

#[test]
fn a_frozen_array_literal_returned_from_a_method_returning_a_tuple_is_the_tuple() {
    assert_types(
        "collection_literal_with_hint_of_self_returning_call.rb",
        "[1, \"a\"].freeze",
        InferenceRule::CollectionLiteralWithHintOfSelfReturningCall,
        same_in_both_versions("::Array[(::Integer | ::String)]", "[::Integer, ::String]"),
    );
}

#[test]
fn proc_is_typed_with_the_parameters_and_the_body_of_its_block() {
    assert_types(
        "proc_type_from_block.rb",
        "proc { |value| value }",
        InferenceRule::ProcTypeFromBlock,
        same_in_both_versions("::Proc", "^(untyped) -> untyped"),
    );
    assert_types(
        "proc_type_from_block.rb",
        "proc { 1 }.call",
        InferenceRule::ProcTypeFromBlock,
        same_in_both_versions("untyped", "::Integer"),
    );
}
