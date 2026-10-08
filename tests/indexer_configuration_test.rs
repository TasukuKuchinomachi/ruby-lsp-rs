use std::fs;
use std::path::{Path, PathBuf};

use ruby_lsp_rs::indexer::configuration::{Configuration, IndexablePath};
use ruby_lsp_rs::ruby_environment::{LockedGem, RubyEnvironment};

struct TemporaryDirectory {
    path: PathBuf,
}

impl TemporaryDirectory {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir()
            .join(format!("ruby_lsp_rs_configuration_test_{}_{name}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        TemporaryDirectory { path }
    }

    fn touch(&self, relative_path: &str) -> PathBuf {
        let path = self.path.join(relative_path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "").unwrap();
        path
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn rubylibdir() -> String {
    let output = std::process::Command::new("ruby")
        .args(["-e", "print RbConfig::CONFIG['rubylibdir']"])
        .output()
        .expect("ruby must be available to locate rubylibdir");
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn configuration_for(workspace_path: &Path, environment: &RubyEnvironment) -> Configuration {
    let mut configuration = Configuration { workspace_path: workspace_path.to_path_buf(), ..Configuration::default() };
    configuration.initialize_from_environment(environment);
    configuration
}

fn paths_of(indexable_paths: &[IndexablePath]) -> Vec<String> {
    indexable_paths.iter().map(|indexable| indexable.path.to_string_lossy().into_owned()).collect()
}

fn locked_gem(name: &str, full_gem_path: &Path) -> LockedGem {
    LockedGem {
        name: name.to_string(),
        version: "1.0.0".to_string(),
        full_gem_path: full_gem_path.to_string_lossy().into_owned(),
        require_paths: vec!["lib".to_string()],
    }
}

#[test]
fn test_load_configuration_executes_configure_block() {
    let workspace = TemporaryDirectory::new("configure_block");
    workspace.touch("lib/project.rb");
    workspace.touch("test/fixtures/fixture.rb");
    let gems = TemporaryDirectory::new("configure_block_gems");
    gems.touch("minitest-reporters/lib/minitest_reporters.rb");
    gems.touch("prism/lib/prism.rb");
    let environment = RubyEnvironment {
        locked_gems: vec![
            locked_gem("minitest-reporters", &gems.path.join("minitest-reporters")),
            locked_gem("prism", &gems.path.join("prism")),
        ],
        initial_excluded_gems: vec!["minitest-reporters".to_string()],
        ..RubyEnvironment::default()
    };
    let mut configuration = configuration_for(&workspace.path, &environment);
    configuration.apply_config(&serde_json::json!({ "excluded_patterns": ["**/fixtures/**/*"] })).unwrap();

    let paths = paths_of(&configuration.indexable_paths(&environment));
    assert!(paths.iter().all(|path| !path.contains("test/fixtures")));
    assert!(paths.iter().all(|path| !path.contains("minitest-reporters")));
    assert!(paths.iter().any(|path| path.contains("prism")));
}

#[test]
fn test_indexable_uris_have_expanded_full_paths() {
    let workspace = TemporaryDirectory::new("expanded_full_paths");
    workspace.touch("lib/project.rb");
    workspace.touch("nested/deeper/file.rb");
    let environment = RubyEnvironment::default();
    let mut configuration = configuration_for(&workspace.path, &environment);
    configuration.apply_config(&serde_json::json!({ "included_patterns": ["**/*.rb"] })).unwrap();

    let indexable_paths = configuration.indexable_paths(&environment);
    assert!(!indexable_paths.is_empty());
    assert!(indexable_paths.iter().all(|indexable| indexable.path.is_absolute()));
}

#[test]
fn test_indexable_uris_only_includes_gem_require_paths() {
    let workspace = TemporaryDirectory::new("gem_require_paths");
    let gems = TemporaryDirectory::new("gem_require_paths_gems");
    gems.touch("some_gem/lib/some_gem.rb");
    gems.touch("some_gem/test/some_gem_test.rb");
    gems.touch("some_gem/test/helper.rb");
    let environment = RubyEnvironment {
        locked_gems: vec![locked_gem("some_gem", &gems.path.join("some_gem"))],
        ..RubyEnvironment::default()
    };
    let configuration = configuration_for(&workspace.path, &environment);

    let paths = paths_of(&configuration.indexable_paths(&environment));
    let test_directory = gems.path.join("some_gem/test").to_string_lossy().into_owned();
    assert!(paths.iter().all(|path| !path.starts_with(&test_directory)));
    assert!(paths.iter().any(|path| path.ends_with("some_gem/lib/some_gem.rb")));
}

#[test]
fn test_indexable_uris_does_not_include_default_gem_path_when_in_bundle() {
    let workspace = TemporaryDirectory::new("default_gem_in_bundle");
    let rubylibdir = rubylibdir();
    let environment = RubyEnvironment {
        rubylibdir: Some(rubylibdir.clone()),
        default_gems_in_bundle: vec!["psych".to_string()],
        ..RubyEnvironment::default()
    };
    let configuration = configuration_for(&workspace.path, &environment);

    let paths = paths_of(&configuration.indexable_paths(&environment));
    assert!(paths.iter().all(|path| !path.starts_with(&format!("{rubylibdir}/psych"))));
}

#[test]
fn test_indexable_uris_includes_default_gems() {
    let workspace = TemporaryDirectory::new("default_gems");
    let rubylibdir = rubylibdir();
    let environment = RubyEnvironment { rubylibdir: Some(rubylibdir.clone()), ..RubyEnvironment::default() };
    let configuration = configuration_for(&workspace.path, &environment);

    let paths = paths_of(&configuration.indexable_paths(&environment));
    assert!(paths.contains(&format!("{rubylibdir}/pathname.rb")));
    assert!(paths.contains(&format!("{rubylibdir}/ipaddr.rb")));
}

#[test]
fn test_indexable_uris_includes_project_files() {
    let workspace = TemporaryDirectory::new("project_files");
    let expected = vec![
        workspace.touch("lib/project.rb"),
        workspace.touch("lib/project/nested.rb"),
        workspace.touch("lib/project/deeply/nested.rb"),
    ];
    workspace.touch("lib/project/nested_test.rb");
    let environment = RubyEnvironment::default();
    let configuration = configuration_for(&workspace.path, &environment);

    let paths = paths_of(&configuration.indexable_paths(&environment));
    for path in expected {
        assert!(paths.contains(&path.to_string_lossy().into_owned()), "{path:?} should be indexed");
    }
}

#[test]
fn test_indexable_uris_avoids_duplicates_if_bundle_path_is_inside_project() {
    let workspace = TemporaryDirectory::new("bundle_path_inside_project");
    let environment = RubyEnvironment { bundle_path: Some("vendor/bundle".to_string()), ..RubyEnvironment::default() };
    let configuration = configuration_for(&workspace.path, &environment);

    assert!(configuration.excluded_patterns.contains(&"vendor/bundle/**/*.rb".to_string()));
}

#[test]
fn test_indexable_uris_does_not_include_gems_own_installed_files() {
    let workspace = TemporaryDirectory::new("gems_own_files");
    workspace.touch("lib/my_gem.rb");
    workspace.touch("lib/my_gem/excluded_test.rb");
    let environment = RubyEnvironment {
        locked_gems: vec![locked_gem("my_gem", &workspace.path)],
        ..RubyEnvironment::default()
    };
    let mut configuration = configuration_for(&workspace.path, &environment);
    configuration.apply_config(&serde_json::json!({ "excluded_patterns": ["**/excluded_test.rb"] })).unwrap();

    let paths = paths_of(&configuration.indexable_paths(&environment));
    assert_eq!(1, paths.iter().filter(|path| path.ends_with("lib/my_gem.rb")).count());
    assert!(paths.iter().all(|path| !path.ends_with("excluded_test.rb")));
}

#[test]
fn test_indexable_uris_does_not_include_non_ruby_files_inside_rubylibdir() {
    let workspace = TemporaryDirectory::new("non_ruby_rubylibdir");
    let rubylibdir = TemporaryDirectory::new("non_ruby_rubylibdir_lib");
    let path = rubylibdir.touch("extra_file.txt");
    let environment = RubyEnvironment {
        rubylibdir: Some(rubylibdir.path.to_string_lossy().into_owned()),
        ..RubyEnvironment::default()
    };
    let configuration = configuration_for(&workspace.path, &environment);

    let indexable_paths = configuration.indexable_paths(&environment);
    assert!(indexable_paths.iter().all(|indexable| indexable.path != path));
}

#[test]
fn test_paths_are_unique() {
    let workspace = TemporaryDirectory::new("unique_paths");
    workspace.touch("lib/project.rb");
    workspace.touch("top_level.rb");
    let environment = RubyEnvironment { rubylibdir: Some(rubylibdir()), ..RubyEnvironment::default() };
    let mut configuration = configuration_for(&workspace.path, &environment);
    configuration.apply_config(&serde_json::json!({ "included_patterns": ["**/*.rb", "lib/**/*.rb"] })).unwrap();

    let paths = paths_of(&configuration.indexable_paths(&environment));
    let mut unique_paths = paths.clone();
    unique_paths.sort();
    unique_paths.dedup();
    assert_eq!(unique_paths.len(), paths.len());
}

#[test]
fn test_configuration_raises_for_unknown_keys() {
    let mut configuration = Configuration::default();
    assert!(configuration.apply_config(&serde_json::json!({ "unknown_config": 123 })).is_err());
}

#[test]
fn test_magic_comments_regex() {
    let configuration = Configuration::default();
    for comment in [
        "# frozen_string_literal:",
        "# typed:",
        "# compiled:",
        "# encoding:",
        "# shareable_constant_value:",
        "# warn_indent:",
        "# rubocop:",
        "# nodoc:",
        "# doc:",
        "# coding:",
        "# warn_past_scope:",
    ] {
        assert!(configuration.matches_magic_comment(comment), "{comment} should match");
    }
}

#[test]
fn test_indexable_uris_respect_given_workspace_path() {
    let workspace = TemporaryDirectory::new("given_workspace_path");
    workspace.touch("ignore/file0.rb");
    workspace.touch("file1.rb");
    workspace.touch("file2.rb");
    let environment = RubyEnvironment::default();
    let mut configuration = configuration_for(&workspace.path, &environment);
    configuration.apply_config(&serde_json::json!({ "excluded_patterns": ["ignore/**/*.rb"] })).unwrap();

    let indexable_paths = configuration.indexable_paths(&environment);
    assert!(indexable_paths.iter().all(|indexable| !indexable.path.starts_with(workspace.path.join("ignore"))));
    assert!(indexable_paths.iter().all(|indexable| indexable.path.starts_with(&workspace.path)));
    assert_eq!(2, indexable_paths.len());
}

#[test]
fn test_includes_top_level_files() {
    let workspace = TemporaryDirectory::new("top_level_files");
    workspace.touch("find_me.rb");
    let environment = RubyEnvironment::default();
    let configuration = configuration_for(&workspace.path, &environment);

    let indexable_paths = configuration.indexable_paths(&environment);
    assert!(indexable_paths.iter().any(|indexable| indexable.path.file_name().unwrap() == "find_me.rb"));
}

#[test]
fn test_indexables_include_non_test_files_in_test_directories() {
    let workspace = TemporaryDirectory::new("non_test_files_in_test_directories");
    workspace.touch("test/requests/support/expectations_test_runner.rb");
    workspace.touch("test/test_helper.rb");
    workspace.touch("test/test_case.rb");
    workspace.touch("test/requests/foo_test.rb");
    let environment = RubyEnvironment::default();
    let configuration = configuration_for(&workspace.path, &environment);

    let project_paths: Vec<String> = configuration
        .indexable_paths(&environment)
        .iter()
        .filter_map(|indexable| indexable.path.strip_prefix(&workspace.path).ok().map(|path| path.to_string_lossy().into_owned()))
        .collect();
    assert!(project_paths.contains(&"test/requests/support/expectations_test_runner.rb".to_string()));
    assert!(project_paths.contains(&"test/test_helper.rb".to_string()));
    assert!(project_paths.contains(&"test/test_case.rb".to_string()));
    assert!(!project_paths.contains(&"test/requests/foo_test.rb".to_string()));
}
