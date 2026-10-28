use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use rbs_environment::{
    DiagnosticConfiguration, DiagnosticSeverity, Environment, SteepfileConfiguration, SteepfileTarget, WorkspaceSources,
};
use ruby_environment::RubyEnvironment;
use ruby_file_glob::fnmatch;
use type_checker::{Analysis, AnalysisOptions, Severity, SteepVersion, TypeDiagnostic};

pub struct LoadedEnvironment {
    /// One for each target of the `Steepfile`, as Steep loads one environment for each target. A single one without
    /// a name serves every target when the environments would be the same, or when there is no target
    pub targets: Vec<LoadedTarget>,
    pub steepfile: Option<SteepfileConfiguration>,
    pub duration: Duration,
}

pub struct LoadedTarget {
    pub name: Option<String>,
    pub environment: Arc<Environment>,
}

impl LoadedEnvironment {
    /// The environment of the target named `target_name`, or the shared one
    pub fn environment_for(&self, target_name: Option<&str>) -> Option<&Arc<Environment>> {
        self.targets
            .iter()
            .find(|target| target.name.is_none() || target.name.as_deref() == target_name)
            .map(|target| &target.environment)
    }
}

pub fn workspace_sources(ruby_environment: &RubyEnvironment) -> WorkspaceSources {
    let core_directory = ruby_environment.rbs_core_path.as_ref().map(PathBuf::from);
    let stdlib_directory = core_directory.as_ref().and_then(|core| core.parent()).map(|rbs_gem| rbs_gem.join("stdlib"));
    let mut gem_directories = HashMap::new();
    for gem in &ruby_environment.locked_gems {
        gem_directories.entry(gem.name.clone()).or_insert_with(|| PathBuf::from(&gem.full_gem_path));
    }
    for gem in &ruby_environment.gem_stubs {
        gem_directories.entry(gem.name.clone()).or_insert_with(|| PathBuf::from(&gem.full_gem_path));
    }
    WorkspaceSources { core_directory, stdlib_directory, gem_directories }
}

fn locked_steep_version(ruby_environment: &RubyEnvironment) -> Option<&str> {
    ruby_environment
        .locked_gems
        .iter()
        .find(|gem| gem.name == "steep")
        .map(|gem| gem.version.as_str())
}

/// The Steep version `Gemfile.lock` locks, since the type checking of Steep differs between its minor versions. The
/// latest supported one when the bundle has no steep or locks a version that is not supported
pub fn steep_version(ruby_environment: &RubyEnvironment) -> SteepVersion {
    locked_steep_version(ruby_environment)
        .and_then(SteepVersion::from_version)
        .unwrap_or(SteepVersion::LATEST)
}

pub fn unsupported_steep_version_warning(ruby_environment: &RubyEnvironment) -> Option<String> {
    let locked_version = locked_steep_version(ruby_environment)?;
    SteepVersion::from_version(locked_version).is_none().then(|| {
        format!(
            "Steep {locked_version} in Gemfile.lock is not supported, so Steep {} is followed (supported: {})",
            SteepVersion::LATEST,
            SteepVersion::SUPPORTED.map(SteepVersion::name).join(", ")
        )
    })
}

pub fn missing_core_warning(sources: &WorkspaceSources) -> Option<String> {
    let found = sources.core_directory.as_ref().is_some_and(|core_directory| core_directory.is_dir());
    (!found).then(|| "The rbs gem was not found, so core signatures are not loaded and most types will be untyped".to_string())
}

/// The `Steepfile` as Ruby runs it, or as `rbs_environment::read_steepfile` reads it when Ruby cannot
pub fn read_steepfile(workspace_path: &Path, ruby_command: &[String]) -> Option<SteepfileConfiguration> {
    rbs_environment::evaluate_steepfile(workspace_path, ruby_command)
        .unwrap_or_else(|_| rbs_environment::read_steepfile(workspace_path))
}

pub fn load_environment(
    workspace_path: &Path,
    sources: &WorkspaceSources,
    ruby_command: &[String],
) -> LoadedEnvironment {
    let started_at = Instant::now();
    let steepfile = read_steepfile(workspace_path, ruby_command);
    let load = |target: Option<&SteepfileTarget>| {
        Arc::new(rbs_environment::load_target(
            workspace_path,
            sources,
            steepfile.as_ref(),
            target,
        ))
    };
    let targets = match &steepfile {
        Some(configuration) if !has_same_environment_for_every_target(configuration) => configuration
            .targets
            .iter()
            .map(|target| LoadedTarget {
                name: Some(target.name.clone()),
                environment: load(Some(target)),
            })
            .collect(),
        _ => vec![LoadedTarget {
            name: None,
            environment: load(None),
        }],
    };
    LoadedEnvironment {
        targets,
        steepfile,
        duration: started_at.elapsed(),
    }
}

/// Without `unreferenced!`, every target loads the signatures of all the targets, so their environments differ only
/// when `implicitly_returns_nil!` does
fn has_same_environment_for_every_target(configuration: &SteepfileConfiguration) -> bool {
    let mut targets = configuration.targets.iter();
    let Some(first) = targets.next() else { return true };
    !first.unreferenced
        && targets.all(|target| !target.unreferenced && target.implicitly_returns_nil == first.implicitly_returns_nil)
}

pub fn load_summary(loaded: &LoadedEnvironment) -> String {
    let count = |measure: fn(&Environment) -> usize| {
        loaded
            .targets
            .iter()
            .map(|target| measure(&target.environment))
            .max()
            .unwrap_or_default()
    };
    format!(
        "Loaded RBS environment in {} ms ({} classes, {} interfaces, {} errors)",
        loaded.duration.as_millis(),
        count(|environment| environment.classes.len()),
        count(|environment| environment.interfaces.len()),
        count(|environment| environment.errors.len())
    )
}

/// The target `code_diagnostics_for` looks the configuration up from
pub fn target_for<'a>(
    workspace_path: &Path,
    steepfile: Option<&'a SteepfileConfiguration>,
    path: &Path,
) -> Option<&'a SteepfileTarget> {
    let relative_path = path.strip_prefix(workspace_path).ok()?;
    group_for(workspace_path, steepfile?, relative_path).map(|(target, _)| target)
}

/// Whether `path` is a signature file of `target` itself, which Steep validates for that target
pub fn is_signature_path_of(workspace_path: &Path, target: &SteepfileTarget, path: &Path) -> bool {
    let Ok(relative_path) = path.strip_prefix(workspace_path) else {
        return false;
    };
    target
        .signature_patterns
        .iter()
        .any(|pattern| pattern_matches(workspace_path, pattern, relative_path))
}

pub fn load_error_report(loaded: &LoadedEnvironment) -> Option<String> {
    let mut lines: Vec<String> = Vec::new();
    for error in loaded.targets.iter().flat_map(|target| &target.environment.errors) {
        let line = format!("{}: {}", error.path.display(), error.message);
        if !lines.contains(&line) {
            lines.push(line);
        }
    }
    (!lines.is_empty()).then(|| format!("RBS errors:\n{}", lines.join("\n")))
}

/// Without a `Steepfile` every file in the workspace is checked, like Steep's default `check "."`
pub fn is_checked_path(workspace_path: &Path, steepfile: Option<&SteepfileConfiguration>, path: &Path) -> bool {
    code_diagnostics_for(workspace_path, steepfile, path).is_some()
}

/// Without a `Steepfile` the signatures are read from `sig`, as `rbs_environment::load_workspace` does
pub fn is_signature_path(workspace_path: &Path, steepfile: Option<&SteepfileConfiguration>, path: &Path) -> bool {
    match steepfile {
        Some(steepfile) => {
            let Ok(relative_path) = path.strip_prefix(workspace_path) else { return false };
            steepfile.signature_patterns.iter().any(|pattern| pattern_matches(workspace_path, pattern, relative_path))
        }
        None => path.starts_with(workspace_path.join("sig")),
    }
}

static DEFAULT_CODE_DIAGNOSTICS: LazyLock<DiagnosticConfiguration> = LazyLock::new(DiagnosticConfiguration::default);

/// The diagnostics configuration of the first group or target, in `Steepfile` order, whose `check` patterns take the
/// file, as Steep's `Project#group_for_source_path` does. `None` when no target checks the file
pub fn code_diagnostics_for<'a>(
    workspace_path: &Path,
    steepfile: Option<&'a SteepfileConfiguration>,
    path: &Path,
) -> Option<&'a DiagnosticConfiguration> {
    let relative_path = path.strip_prefix(workspace_path).ok()?;
    let Some(steepfile) = steepfile else { return Some(&DEFAULT_CODE_DIAGNOSTICS) };
    group_for(workspace_path, steepfile, relative_path).map(|(_, code_diagnostics)| code_diagnostics)
}

/// The target and the diagnostics configuration of the group or target that takes the file
fn group_for<'a>(
    workspace_path: &Path,
    steepfile: &'a SteepfileConfiguration,
    relative_path: &Path,
) -> Option<(&'a SteepfileTarget, &'a DiagnosticConfiguration)> {
    let takes = |check_paths: &[PathBuf], ignore_paths: &[PathBuf]| {
        let matches = |pattern: &PathBuf| pattern_matches(workspace_path, pattern, relative_path);
        check_paths.iter().any(matches) && !ignore_paths.iter().any(matches)
    };
    steepfile.targets.iter().find_map(|target| {
        target
            .groups
            .iter()
            .find(|group| takes(&group.check_paths, &group.ignore_paths))
            .map(|group| &group.code_diagnostics)
            .or_else(|| takes(&target.check_paths, &target.ignore_paths).then_some(&target.code_diagnostics))
            .map(|code_diagnostics| (target, code_diagnostics))
    })
}

fn pattern_matches(workspace_path: &Path, pattern: &Path, relative_path: &Path) -> bool {
    let pattern = pattern.strip_prefix(workspace_path).unwrap_or(pattern);
    let pattern_components: PathBuf =
        pattern.components().filter(|component| !matches!(component, Component::CurDir)).collect();
    if pattern_components.as_os_str().is_empty() {
        return true;
    }
    if relative_path.starts_with(&pattern_components) {
        return true;
    }
    fnmatch(&pattern_components.to_string_lossy(), &relative_path.to_string_lossy())
}

/// `None` when the type checker panics, so that one file it cannot handle does not stop checking the others
pub fn analyze_with_options(environment: &Environment, source: &[u8], options: AnalysisOptions) -> Option<Arc<Analysis>> {
    catch_unwind(AssertUnwindSafe(|| type_checker::analyze_with_options(environment, source, options))).ok().map(Arc::new)
}

pub fn severity_name(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Information => "information",
        Severity::Hint => "hint",
    }
}

fn type_checker_severity(severity: DiagnosticSeverity) -> Severity {
    match severity {
        DiagnosticSeverity::Error => Severity::Error,
        DiagnosticSeverity::Warning => Severity::Warning,
        DiagnosticSeverity::Information => Severity::Information,
        DiagnosticSeverity::Hint => Severity::Hint,
    }
}

/// `None` when the configuration disables the code. A code Steep doesn't have keeps the type checker's severity
pub fn configured_severity(configuration: &DiagnosticConfiguration, diagnostic: &TypeDiagnostic) -> Option<Severity> {
    match configuration.severity(diagnostic.code) {
        Some(configured) => configured.map(type_checker_severity),
        None => Some(diagnostic.severity),
    }
}

pub fn configured_diagnostics<'a>(
    analysis: &'a Analysis,
    configuration: &DiagnosticConfiguration,
) -> Vec<(&'a TypeDiagnostic, Severity)> {
    analysis
        .diagnostics
        .iter()
        .filter_map(|diagnostic| Some((diagnostic, configured_severity(configuration, diagnostic)?)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rbs_environment::{DiagnosticTemplate, SteepfileGroup, SteepfileTarget};

    fn steepfile_with_targets(targets: Vec<SteepfileTarget>) -> SteepfileConfiguration {
        SteepfileConfiguration {
            signature_patterns: Vec::new(),
            check_paths: targets.iter().flat_map(|target| target.check_paths.clone()).collect(),
            ignore_paths: targets.iter().flat_map(|target| target.ignore_paths.clone()).collect(),
            libraries: Vec::new(),
            collection_lockfile: None,
            targets,
        }
    }

    fn target(
        name: &str,
        check_paths: &[&str],
        ignore_paths: &[&str],
        code_diagnostics: DiagnosticConfiguration,
    ) -> SteepfileTarget {
        SteepfileTarget {
            name: name.to_string(),
            check_paths: check_paths.iter().map(PathBuf::from).collect(),
            ignore_paths: ignore_paths.iter().map(PathBuf::from).collect(),
            code_diagnostics,
            groups: Vec::new(),
            implicitly_returns_nil: false,
            signature_patterns: Vec::new(),
            unreferenced: false,
        }
    }

    fn type_diagnostic(code: &'static str, severity: Severity) -> TypeDiagnostic {
        TypeDiagnostic { start: 0, end: 1, severity, code, message: String::new() }
    }

    fn ruby_environment_locking(gems: &[(&str, &str)]) -> RubyEnvironment {
        let locked_gems = gems
            .iter()
            .map(|(name, version)| ruby_environment::LockedGem {
                name: name.to_string(),
                version: version.to_string(),
                ..ruby_environment::LockedGem::default()
            })
            .collect();
        RubyEnvironment {
            locked_gems,
            ..RubyEnvironment::default()
        }
    }

    #[test]
    fn follows_the_steep_version_of_gemfile_lock() {
        let steep_2_0 = ruby_environment_locking(&[("rbs", "4.0.2"), ("steep", "2.0.0")]);
        assert_eq!(steep_version(&steep_2_0), SteepVersion::Version2_0);
        assert_eq!(unsupported_steep_version_warning(&steep_2_0), None);
        assert_eq!(
            steep_version(&ruby_environment_locking(&[("steep", "2.1.3")])),
            SteepVersion::Version2_1
        );

        let without_steep = ruby_environment_locking(&[("rbs", "4.2.0")]);
        assert_eq!(steep_version(&without_steep), SteepVersion::LATEST);
        assert_eq!(unsupported_steep_version_warning(&without_steep), None);

        let steep_1 = ruby_environment_locking(&[("steep", "1.10.0")]);
        assert_eq!(steep_version(&steep_1), SteepVersion::LATEST);
        assert_eq!(
            unsupported_steep_version_warning(&steep_1).as_deref(),
            Some("Steep 1.10.0 in Gemfile.lock is not supported, so Steep 2.1 is followed (supported: 2.0, 2.1)")
        );
    }

    #[test]
    fn checks_paths_without_a_steepfile() {
        let workspace = Path::new("/workspace");
        assert!(is_checked_path(workspace, None, Path::new("/workspace/lib/foo.rb")));
        assert!(!is_checked_path(workspace, None, Path::new("/elsewhere/foo.rb")));
    }

    #[test]
    fn checks_paths_from_the_steepfile() {
        let workspace = Path::new("/workspace");
        let steepfile = steepfile_with_targets(vec![target(
            "app",
            &["lib", "/workspace/app", "bin/*.rb"],
            &["lib/templates", "app/**/*_generated.rb"],
            DiagnosticConfiguration::default(),
        )]);
        let checked = |path: &str| is_checked_path(workspace, Some(&steepfile), Path::new(path));
        assert!(checked("/workspace/lib/foo.rb"));
        assert!(checked("/workspace/app/models/user.rb"));
        assert!(checked("/workspace/bin/setup.rb"));
        assert!(!checked("/workspace/library/foo.rb"));
        assert!(!checked("/workspace/test/foo_test.rb"));
        assert!(!checked("/workspace/lib/templates/foo.rb"));
        assert!(!checked("/workspace/app/models/user_generated.rb"));
    }

    #[test]
    fn checks_everything_for_the_current_directory_pattern() {
        let workspace = Path::new("/workspace");
        let steepfile = steepfile_with_targets(vec![target(".", &["."], &[], DiagnosticConfiguration::default())]);
        assert!(is_checked_path(workspace, Some(&steepfile), Path::new("/workspace/foo.rb")));
    }

    #[test]
    fn looks_up_the_first_target_or_group_that_checks_the_file() {
        let workspace = Path::new("/workspace");
        let mut app = target("app", &["app"], &["app/generated"], DiagnosticConfiguration::from_template(DiagnosticTemplate::Strict));
        app.groups.push(SteepfileGroup {
            name: "legacy".to_string(),
            check_paths: vec![PathBuf::from("app/legacy")],
            ignore_paths: Vec::new(),
            code_diagnostics: DiagnosticConfiguration::from_template(DiagnosticTemplate::Silent),
        });
        let steepfile = steepfile_with_targets(vec![
            app,
            target("everything", &["."], &[], DiagnosticConfiguration::from_template(DiagnosticTemplate::Lenient)),
        ]);
        let lookup = |path: &str| code_diagnostics_for(workspace, Some(&steepfile), Path::new(path)).cloned();
        assert_eq!(lookup("/workspace/app/models/user.rb"), Some(DiagnosticConfiguration::from_template(DiagnosticTemplate::Strict)));
        assert_eq!(lookup("/workspace/app/legacy/old.rb"), Some(DiagnosticConfiguration::from_template(DiagnosticTemplate::Silent)));
        assert_eq!(lookup("/workspace/app/generated/foo.rb"), Some(DiagnosticConfiguration::from_template(DiagnosticTemplate::Lenient)));
        assert_eq!(lookup("/workspace/test/foo_test.rb"), Some(DiagnosticConfiguration::from_template(DiagnosticTemplate::Lenient)));
        assert_eq!(code_diagnostics_for(workspace, None, Path::new("/workspace/foo.rb")), Some(&DiagnosticConfiguration::default()));
    }

    #[test]
    fn ignores_paths_only_within_their_own_target() {
        let workspace = Path::new("/workspace");
        let steepfile = steepfile_with_targets(vec![
            target("app", &["app"], &["app/jobs"], DiagnosticConfiguration::default()),
            target("jobs", &["app/jobs"], &[], DiagnosticConfiguration::default()),
        ]);
        assert!(is_checked_path(workspace, Some(&steepfile), Path::new("/workspace/app/jobs/foo_job.rb")));
    }

    #[test]
    fn replaces_severities_from_the_configuration() {
        let mut configuration = DiagnosticConfiguration::default();
        configuration.set("Ruby::IncompatibleAssignment", Some(DiagnosticSeverity::Warning));
        configuration.set("Ruby::UnannotatedEmptyCollection", None);
        let severity = |code: &'static str, severity: Severity| configured_severity(&configuration, &type_diagnostic(code, severity));
        assert_eq!(severity("Ruby::IncompatibleAssignment", Severity::Hint), Some(Severity::Warning));
        assert_eq!(severity("Ruby::UnannotatedEmptyCollection", Severity::Warning), None);
        assert_eq!(severity("Ruby::NoMethod", Severity::Warning), Some(Severity::Error));
        assert_eq!(severity("Ruby::MethodDefinitionMissing", Severity::Hint), None);
        assert_eq!(severity("RubyLspRs::Unknown", Severity::Information), Some(Severity::Information));
    }
}
