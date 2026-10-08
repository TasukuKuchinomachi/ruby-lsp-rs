use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use rbs_environment::{
    DiagnosticConfiguration, DiagnosticSeverity, DiagnosticTemplate, SteepfileConfiguration,
    SteepfileGroup, SteepfileTarget, WorkspaceSources, load_workspace, read_steepfile,
};

struct TemporaryDirectory {
    path: PathBuf,
}

impl TemporaryDirectory {
    fn new(name: &str) -> TemporaryDirectory {
        let path =
            std::env::temp_dir().join(format!("rbs-environment-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        TemporaryDirectory { path }
    }

    fn write(&self, relative_path: &str, content: &str) {
        let path = self.path.join(relative_path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

const STEEPFILE: &str = r#"
D = Steep::Diagnostic

target :lib do
  signature "sig", "sig-private"
  check "lib"
  ignore "lib/generated"
  library "set", "pathname"
  configure_code_diagnostics(D::Ruby.strict) do |hash|
    hash[D::Ruby::NoMethod] = :information
  end
end

target :test do
  signature "sig-test"
  check "test"
  library "json"
end

collection_config "config/rbs_collection.yaml"
"#;

#[test]
fn read_steepfile_merges_every_target() {
    let workspace = TemporaryDirectory::new("steepfile");
    workspace.write("Steepfile", STEEPFILE);
    let root = &workspace.path;
    assert_eq!(
        read_steepfile(root),
        Some(SteepfileConfiguration {
            signature_directories: vec![
                root.join("sig"),
                root.join("sig-private"),
                root.join("sig-test")
            ],
            check_paths: vec![root.join("lib"), root.join("test")],
            ignore_paths: vec![root.join("lib/generated")],
            libraries: vec![
                "set".to_string(),
                "pathname".to_string(),
                "json".to_string()
            ],
            collection_lockfile: Some(root.join("config/rbs_collection.lock.yaml")),
            targets: vec![
                SteepfileTarget {
                    name: "lib".to_string(),
                    check_paths: vec![root.join("lib")],
                    ignore_paths: vec![root.join("lib/generated")],
                    code_diagnostics: strict_with_information_no_method(),
                    groups: Vec::new(),
                },
                SteepfileTarget {
                    name: "test".to_string(),
                    check_paths: vec![root.join("test")],
                    ignore_paths: Vec::new(),
                    code_diagnostics: DiagnosticConfiguration::default(),
                    groups: Vec::new(),
                },
            ],
        })
    );
}

fn strict_with_information_no_method() -> DiagnosticConfiguration {
    let mut configuration = DiagnosticConfiguration::from_template(DiagnosticTemplate::Strict);
    configuration.set("Ruby::NoMethod", Some(DiagnosticSeverity::Information));
    configuration
}

fn target_diagnostics(steepfile: &str) -> Vec<DiagnosticConfiguration> {
    static NEXT_WORKSPACE_NUMBER: AtomicUsize = AtomicUsize::new(0);
    let workspace_number = NEXT_WORKSPACE_NUMBER.fetch_add(1, Ordering::Relaxed);
    let workspace = TemporaryDirectory::new(&format!("steepfile-diagnostics-{workspace_number}"));
    workspace.write("Steepfile", steepfile);
    read_steepfile(&workspace.path)
        .unwrap()
        .targets
        .into_iter()
        .map(|target| target.code_diagnostics)
        .collect()
}

#[test]
fn read_steepfile_defaults_to_the_default_template() {
    let configurations = target_diagnostics("target :app do\n  check \"app\"\nend\n");
    assert_eq!(configurations, vec![DiagnosticConfiguration::default()]);
    assert_eq!(
        configurations[0].severity("Ruby::IncompatibleAssignment"),
        Some(Some(DiagnosticSeverity::Hint))
    );
}

#[test]
fn read_steepfile_reads_each_template() {
    for (method_name, template) in [
        ("default", DiagnosticTemplate::Default),
        ("strict", DiagnosticTemplate::Strict),
        ("lenient", DiagnosticTemplate::Lenient),
        ("silent", DiagnosticTemplate::Silent),
        ("all_error", DiagnosticTemplate::AllError),
    ] {
        let steepfile = format!(
            "target :app do\n  configure_code_diagnostics(Steep::Diagnostic::Ruby.{method_name})\nend\n"
        );
        assert_eq!(
            target_diagnostics(&steepfile),
            vec![DiagnosticConfiguration::from_template(template)],
            "{method_name}"
        );
    }
}

#[test]
fn read_steepfile_reads_a_block_with_either_constant_spelling() {
    let configurations = target_diagnostics(
        r#"
D = Steep::Diagnostic

target :app do
  configure_code_diagnostics do |hash|
    hash[D::Ruby::NoMethod] = :information
    hash[Steep::Diagnostic::Ruby::UnannotatedEmptyCollection] = nil
    hash[::Steep::Diagnostic::Ruby::IncompatibleAssignment] = :warning
  end
end
"#,
    );
    let mut expected = DiagnosticConfiguration::default();
    expected.set("Ruby::NoMethod", Some(DiagnosticSeverity::Information));
    expected.set("Ruby::UnannotatedEmptyCollection", None);
    expected.set(
        "Ruby::IncompatibleAssignment",
        Some(DiagnosticSeverity::Warning),
    );
    assert_eq!(configurations, vec![expected]);
    assert_eq!(
        configurations[0].severity("Ruby::UnannotatedEmptyCollection"),
        Some(None)
    );
}

#[test]
fn read_steepfile_applies_the_block_after_the_template() {
    let configurations = target_diagnostics(
        r#"
D = Steep::Diagnostic

target :app do
  configure_code_diagnostics(D::Ruby.strict) do |hash|
    hash[D::Ruby::NoMethod] = :information
  end
end
"#,
    );
    assert_eq!(configurations, vec![strict_with_information_no_method()]);
}

#[test]
fn read_steepfile_merges_successive_configurations_and_hashes() {
    let configurations = target_diagnostics(
        r#"
D = Steep::Diagnostic

target :app do
  configure_code_diagnostics do |hash|
    hash[D::Ruby::NoMethod] = :hint
  end
  configure_code_diagnostics(D::Ruby.lenient.merge({ D::Ruby::FallbackAny => :warning }))
  configure_code_diagnostics(D::Ruby::UnknownConstant => nil)
end
"#,
    );
    let mut expected = DiagnosticConfiguration::from_template(DiagnosticTemplate::Lenient);
    expected.set("Ruby::FallbackAny", Some(DiagnosticSeverity::Warning));
    expected.set("Ruby::UnknownConstant", None);
    assert_eq!(configurations, vec![expected]);
}

#[test]
fn read_steepfile_keeps_each_target_separate() {
    let workspace = TemporaryDirectory::new("steepfile-targets");
    workspace.write(
        "Steepfile",
        r#"
D = Steep::Diagnostic

target :app do
  check "app"
  ignore "app/generated"
  configure_code_diagnostics(D::Ruby.strict)

  group :legacy do
    check "app/legacy"
    configure_code_diagnostics do |hash|
      hash[D::Ruby::NoMethod] = nil
    end
  end

  group :plain do
    check "app/plain"
  end
end

target :test do
  check "test"
  configure_code_diagnostics(D::Ruby.lenient)
end
"#,
    );
    let root = &workspace.path;
    let targets = read_steepfile(root).unwrap().targets;
    let mut legacy = DiagnosticConfiguration::from_template(DiagnosticTemplate::Strict);
    legacy.set("Ruby::NoMethod", None);
    assert_eq!(
        targets,
        vec![
            SteepfileTarget {
                name: "app".to_string(),
                check_paths: vec![root.join("app")],
                ignore_paths: vec![root.join("app/generated")],
                code_diagnostics: DiagnosticConfiguration::from_template(
                    DiagnosticTemplate::Strict
                ),
                groups: vec![
                    SteepfileGroup {
                        name: "legacy".to_string(),
                        check_paths: vec![root.join("app/legacy")],
                        ignore_paths: Vec::new(),
                        code_diagnostics: legacy,
                    },
                    SteepfileGroup {
                        name: "plain".to_string(),
                        check_paths: vec![root.join("app/plain")],
                        ignore_paths: Vec::new(),
                        code_diagnostics: DiagnosticConfiguration::from_template(
                            DiagnosticTemplate::Strict
                        ),
                    },
                ],
            },
            SteepfileTarget {
                name: "test".to_string(),
                check_paths: vec![root.join("test")],
                ignore_paths: Vec::new(),
                code_diagnostics: DiagnosticConfiguration::from_template(
                    DiagnosticTemplate::Lenient
                ),
                groups: Vec::new(),
            },
        ]
    );
}

#[test]
fn read_steepfile_replaces_a_group_configuration_given_without_a_block() {
    let configurations: Vec<DiagnosticConfiguration> = {
        let workspace = TemporaryDirectory::new("steepfile-group-hash");
        workspace.write(
            "Steepfile",
            r#"
target :app do
  group :partial do
    configure_code_diagnostics({ Steep::Diagnostic::Ruby::NoMethod => :hint })
  end
end
"#,
        );
        read_steepfile(&workspace.path).unwrap().targets[0]
            .groups
            .iter()
            .map(|group| group.code_diagnostics.clone())
            .collect()
    };
    assert_eq!(
        configurations[0].severity("Ruby::NoMethod"),
        Some(Some(DiagnosticSeverity::Hint))
    );
    assert_eq!(
        configurations[0].severity("Ruby::UnreachableBranch"),
        Some(Some(DiagnosticSeverity::Error))
    );
}

#[test]
fn read_steepfile_resolves_groups_against_the_target_as_steep_does() {
    let workspace = TemporaryDirectory::new("steepfile-group-order");
    workspace.write(
        "Steepfile",
        r#"
D = Steep::Diagnostic

target :app do
  group :snapshot do
    check "app/snapshot"
    configure_code_diagnostics do |hash|
      hash[D::Ruby::NoMethod] = :hint
    end
  end

  group :follows do
    check "app/follows"
  end

  group :reset do
    check "app/reset"
    configure_code_diagnostics(D::Ruby.lenient)
    configure_code_diagnostics(nil)
  end

  check "app"
  configure_code_diagnostics(D::Ruby.strict)
end
"#,
    );
    let groups: Vec<(String, DiagnosticConfiguration)> =
        read_steepfile(&workspace.path).unwrap().targets[0]
            .groups
            .iter()
            .map(|group| (group.name.clone(), group.code_diagnostics.clone()))
            .collect();
    let mut snapshot = DiagnosticConfiguration::default();
    snapshot.set("Ruby::NoMethod", Some(DiagnosticSeverity::Hint));
    let strict = DiagnosticConfiguration::from_template(DiagnosticTemplate::Strict);
    assert_eq!(
        groups,
        vec![
            ("snapshot".to_string(), snapshot),
            ("follows".to_string(), strict.clone()),
            ("reset".to_string(), strict),
        ]
    );
}

#[test]
fn read_steepfile_returns_none_without_steepfile() {
    let workspace = TemporaryDirectory::new("no-steepfile");
    assert_eq!(read_steepfile(&workspace.path), None);
}

fn write_fake_rbs_gem(rbs_gem: &TemporaryDirectory) {
    rbs_gem.write(
        "core/object.rbs",
        "class BasicObject\nend\nclass Object < BasicObject\nend\nclass String\nend\n",
    );
    rbs_gem.write("core/_hidden/ignored.rbs", "class HiddenCore\nend\n");
    rbs_gem.write("stdlib/pathname/0/pathname.rbs", "class Pathname\nend\n");
    rbs_gem.write("stdlib/set/0/set.rbs", "class OldSet\nend\n");
    rbs_gem.write("stdlib/set/1.2/set.rbs", "class Set\nend\n");
    rbs_gem.write(
        "stdlib/set/1.2/manifest.yaml",
        "dependencies:\n  - name: tsort\n",
    );
    rbs_gem.write("stdlib/tsort/0/tsort.rbs", "module TSort\nend\n");
}

#[test]
fn load_workspace_reads_core_libraries_collection_and_signatures() {
    let rbs_gem = TemporaryDirectory::new("rbs-gem");
    write_fake_rbs_gem(&rbs_gem);
    let installed_gem = TemporaryDirectory::new("installed-gem");
    installed_gem.write("sig/installed.rbs", "class InstalledGem\nend\n");

    let workspace = TemporaryDirectory::new("workspace");
    workspace.write(
        "Steepfile",
        "target :app do\n  signature \"sig\"\n  library \"set\"\nend\n",
    );
    workspace.write(
        "rbs_collection.lock.yaml",
        r#"
path: ".gem_rbs_collection"
gems:
- name: activesupport
  version: '7.0'
  source:
    type: git
    name: ruby/gem_rbs_collection
    revision: 0123456789abcdef
    remote: https://github.com/ruby/gem_rbs_collection.git
    repo_dir: gems
- name: pathname
  version: '0'
  source:
    type: stdlib
- name: installed
  version: 1.0.0
  source:
    type: rubygems
- name: missing
  version: '1.0'
  source:
    type: git
"#,
    );
    workspace.write(
        ".gem_rbs_collection/activesupport/7.0/active_support.rbs",
        "module ActiveSupport\nend\n",
    );
    workspace.write(
        ".gem_rbs_collection/activesupport/7.0/_test/test.rbs",
        "class ActiveSupportTest\nend\n",
    );
    workspace.write("sig/app.rbs", "class App\n  def name: () -> String\nend\n");

    let sources = WorkspaceSources {
        core_directory: Some(rbs_gem.path.join("core")),
        stdlib_directory: Some(rbs_gem.path.join("stdlib")),
        gem_directories: HashMap::from([("installed".to_string(), installed_gem.path.clone())]),
    };
    let environment = load_workspace(&workspace.path, &sources);

    for loaded in [
        "String",
        "Set",
        "TSort",
        "ActiveSupport",
        "Pathname",
        "InstalledGem",
        "App",
    ] {
        assert!(
            environment.classes.contains_key(loaded),
            "{loaded} is not loaded"
        );
    }
    for skipped in ["HiddenCore", "OldSet", "ActiveSupportTest"] {
        assert!(
            !environment.classes.contains_key(skipped),
            "{skipped} is loaded"
        );
    }
    let error_paths: Vec<&Path> = environment
        .errors
        .iter()
        .map(|error| error.path.as_path())
        .collect();
    assert_eq!(
        error_paths,
        vec![
            workspace
                .path
                .join(".gem_rbs_collection/missing/1.0")
                .as_path()
        ]
    );
}

#[test]
fn load_workspace_without_steepfile_reads_sig_directory() {
    let workspace = TemporaryDirectory::new("default-signature");
    workspace.write("sig/app.rbs", "class App\nend\n");
    workspace.write("sig/broken.rbs", "class Broken\n  def x: ->\nend\n");
    let environment = load_workspace(&workspace.path, &WorkspaceSources::default());
    assert!(environment.classes.contains_key("App"));
    assert_eq!(environment.errors.len(), 1);
    assert_eq!(
        environment.errors[0].path,
        workspace.path.join("sig/broken.rbs")
    );
}
