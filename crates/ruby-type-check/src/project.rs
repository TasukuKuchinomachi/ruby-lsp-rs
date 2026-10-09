use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use rbs_environment::{DiagnosticConfiguration, DiagnosticSeverity, Environment, SteepfileConfiguration, WorkspaceSources};
use ruby_environment::RubyEnvironment;
use ruby_file_glob::fnmatch;
use type_checker::{Analysis, AnalysisOptions, Severity, TypeDiagnostic};

pub struct LoadedEnvironment {
    pub environment: Arc<Environment>,
    pub steepfile: Option<SteepfileConfiguration>,
    pub duration: Duration,
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

pub fn missing_core_warning(sources: &WorkspaceSources) -> Option<String> {
    let found = sources.core_directory.as_ref().is_some_and(|core_directory| core_directory.is_dir());
    (!found).then(|| "The rbs gem was not found, so core signatures are not loaded and most types will be untyped".to_string())
}

pub fn load_environment(workspace_path: &Path, sources: &WorkspaceSources) -> LoadedEnvironment {
    let started_at = Instant::now();
    let steepfile = rbs_environment::read_steepfile(workspace_path);
    let environment = rbs_environment::load_workspace(workspace_path, sources);
    LoadedEnvironment { environment: Arc::new(environment), steepfile, duration: started_at.elapsed() }
}

pub fn load_summary(loaded: &LoadedEnvironment) -> String {
    let environment = &loaded.environment;
    format!(
        "Loaded RBS environment in {} ms ({} classes, {} interfaces, {} errors)",
        loaded.duration.as_millis(),
        environment.classes.len(),
        environment.interfaces.len(),
        environment.errors.len()
    )
}

pub fn load_error_report(environment: &Environment) -> Option<String> {
    if environment.errors.is_empty() {
        return None;
    }
    let lines: Vec<String> =
        environment.errors.iter().map(|error| format!("{}: {}", error.path.display(), error.message)).collect();
    Some(format!("RBS errors:\n{}", lines.join("\n")))
}

/// Without a `Steepfile` every file in the workspace is checked, like Steep's default `check "."`
pub fn is_checked_path(workspace_path: &Path, steepfile: Option<&SteepfileConfiguration>, path: &Path) -> bool {
    code_diagnostics_for(workspace_path, steepfile, path).is_some()
}

/// Without a `Steepfile` the signatures are read from `sig`, as `rbs_environment::load_workspace` does
pub fn is_signature_path(workspace_path: &Path, steepfile: Option<&SteepfileConfiguration>, path: &Path) -> bool {
    match steepfile {
        Some(steepfile) => steepfile.signature_directories.iter().any(|directory| path.starts_with(directory)),
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
            signature_directories: Vec::new(),
            check_paths: targets.iter().flat_map(|target| target.check_paths.clone()).collect(),
            ignore_paths: targets.iter().flat_map(|target| target.ignore_paths.clone()).collect(),
            libraries: Vec::new(),
            collection_lockfile: None,
            targets,
        }
    }

    fn target(name: &str, check_paths: &[&str], ignore_paths: &[&str], code_diagnostics: DiagnosticConfiguration) -> SteepfileTarget {
        SteepfileTarget {
            name: name.to_string(),
            check_paths: check_paths.iter().map(PathBuf::from).collect(),
            ignore_paths: ignore_paths.iter().map(PathBuf::from).collect(),
            code_diagnostics,
            groups: Vec::new(),
        }
    }

    fn type_diagnostic(code: &'static str, severity: Severity) -> TypeDiagnostic {
        TypeDiagnostic { start: 0, end: 1, severity, code, message: String::new() }
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
