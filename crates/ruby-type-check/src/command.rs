use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rayon::prelude::*;
use rbs_environment::{DiagnosticConfiguration, DiagnosticSeverity, Environment, SignatureDiagnostic, SteepfileConfiguration};
use ruby_environment::RubyEnvironment;
use type_checker::{AnalysisOptions, Severity, SteepVersion};

use crate::project;
use crate::report::{OutputFormat, Renderer, ReportedDiagnostic, ReportedLocation};

pub const USAGE: &str = "Usage: ruby-type-check [--ruby-command \"mise exec -- ruby\"] [--severity-level error|warning|information|hint] [--steep-compatible] [--steep-version 2.0|2.1] [--format pretty|line] [paths...]";

const UNEXPECTED_ERROR_CODE: &str = "Ruby::UnexpectedError";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeCheckOptions {
    pub ruby_command: Vec<String>,
    /// Diagnostics less severe than this are neither printed nor counted, as `steep check --severity-level` (which
    /// defaults to `warning`)
    pub severity_level: DiagnosticSeverity,
    /// Report what `steep check` reports, so that the command can replace it in CI
    pub steep_compatible: bool,
    /// The Steep version to follow instead of the one `Gemfile.lock` locks
    pub steep_version: Option<SteepVersion>,
    pub format: OutputFormat,
    pub paths: Vec<PathBuf>,
}

fn option_value<'a>(
    name: &str,
    argument: &'a str,
    remaining: &mut impl Iterator<Item = &'a String>,
) -> Result<Option<&'a str>, String> {
    if argument == name {
        return remaining.next().map(|value| Some(value.as_str())).ok_or(format!("{name} requires a value"));
    }
    Ok(argument.strip_prefix(name).and_then(|rest| rest.strip_prefix('=')))
}

pub fn parse_arguments(arguments: &[String]) -> Result<TypeCheckOptions, String> {
    let mut ruby_command = vec!["ruby".to_string()];
    let mut severity_level = DiagnosticSeverity::Warning;
    let mut steep_compatible = false;
    let mut steep_version = None;
    let mut format = OutputFormat::default();
    let mut paths = Vec::new();
    let mut remaining = arguments.iter();
    while let Some(argument) = remaining.next() {
        if let Some(command_text) = option_value("--ruby-command", argument, &mut remaining)? {
            ruby_command = command_text.split_whitespace().map(str::to_string).collect();
            if ruby_command.is_empty() {
                return Err("--ruby-command must not be empty".to_string());
            }
        } else if let Some(level) = option_value("--severity-level", argument, &mut remaining)? {
            severity_level = DiagnosticSeverity::from_name(level)
                .ok_or(format!("--severity-level must be error, warning, information, or hint: {level}"))?;
        } else if let Some(version) = option_value("--steep-version", argument, &mut remaining)? {
            steep_version = Some(SteepVersion::from_version(version).ok_or(format!(
                "--steep-version must be one of {}: {version}",
                SteepVersion::SUPPORTED.map(SteepVersion::name).join(", ")
            ))?);
        } else if let Some(name) = option_value("--format", argument, &mut remaining)? {
            format = OutputFormat::from_name(name).ok_or(format!("--format must be pretty or line: {name}"))?;
        } else if argument == "--steep-compatible" {
            steep_compatible = true;
        } else if argument.starts_with("--") {
            return Err(format!("Unknown option: {argument}"));
        } else {
            paths.push(PathBuf::from(argument));
        }
    }
    Ok(TypeCheckOptions {
        ruby_command,
        severity_level,
        steep_compatible,
        steep_version,
        format,
        paths,
    })
}

pub fn run(arguments: &[String]) -> i32 {
    if arguments.iter().any(|argument| argument == "--help" || argument == "-h") {
        println!("{USAGE}");
        return 0;
    }
    let options = match parse_arguments(arguments) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("{message}\n{USAGE}");
            return 2;
        }
    };
    let workspace_path = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
    let WorkspaceResult {
        results,
        signature_result,
    } = check_workspace(&workspace_path, &options);
    let unexpected_error_file_count = results.iter().filter(|result| result.has_unexpected_error).count();
    if let Some(warning) = unexpected_error_warning(unexpected_error_file_count) {
        eprintln!("{warning}");
    }

    let renderer = Renderer { format: options.format, colored: use_color() };
    let mut error_count = 0;
    let mut diagnostic_count = 0;
    for result in results.iter().chain(std::iter::once(&signature_result)) {
        for diagnostic in &result.diagnostics {
            println!("{}", renderer.render(diagnostic));
            if options.format == OutputFormat::Pretty {
                println!();
            }
        }
        error_count += result.error_count;
        diagnostic_count += result.diagnostics.len();
    }
    eprintln!("Checked {} files: {diagnostic_count} diagnostics, {error_count} errors", results.len());
    exit_code(diagnostic_count)
}

pub struct WorkspaceResult {
    /// One for each Ruby file checked, in path order
    pub results: Vec<FileResult>,
    pub signature_result: FileResult,
}

/// Loads the workspace as `run` does, reporting how it went on stderr, and checks the Ruby files and signatures
pub fn check_workspace(workspace_path: &Path, options: &TypeCheckOptions) -> WorkspaceResult {
    let ruby_environment = RubyEnvironment::detect(workspace_path, &options.ruby_command).unwrap_or_else(|error| {
        eprintln!("Failed to detect the Ruby environment: {error}");
        RubyEnvironment::default()
    });
    let sources = project::workspace_sources(&ruby_environment);
    if let Some(warning) = project::missing_core_warning(&sources) {
        eprintln!("{warning}");
    }
    let steep_version = match options.steep_version {
        Some(steep_version) => steep_version,
        None => {
            if let Some(warning) = project::unsupported_steep_version_warning(&ruby_environment) {
                eprintln!("{warning}");
            }
            project::steep_version(&ruby_environment)
        }
    };
    let loaded = project::load_environment(workspace_path, &sources, &options.ruby_command);
    eprintln!("{}", project::load_summary(&loaded));
    if let Some(report) = project::load_error_report(&loaded) {
        eprintln!("{report}");
    }

    let steepfile = loaded.steepfile.as_ref();
    let files = collect_files(workspace_path, steepfile, &options.paths);
    let default_code_diagnostics = DiagnosticConfiguration::default();
    let analysis_options = AnalysisOptions {
        steep_compatible: options.steep_compatible,
        steep_version,
    };
    let results = files
        .par_iter()
        .map(|path| {
            let target_name = project::target_for(workspace_path, steepfile, path).map(|target| target.name.as_str());
            let Some(environment) = loaded.environment_for(target_name) else {
                return FileResult::empty();
            };
            if options.steep_compatible && environment.steep_fails_to_load_signatures() {
                return FileResult::empty();
            }
            let code_diagnostics =
                project::code_diagnostics_for(workspace_path, steepfile, path).unwrap_or(&default_code_diagnostics);
            check_file(
                environment,
                workspace_path,
                path,
                code_diagnostics,
                options.severity_level,
                analysis_options,
            )
        })
        .collect();
    let mut signature_result = FileResult::empty();
    for target in &loaded.targets {
        let own_target = target.name.as_deref().and_then(|name| {
            steepfile?
                .targets
                .iter()
                .find(|steepfile_target| steepfile_target.name == name)
        });
        let owns = |path: &Path| {
            own_target.is_none_or(|own_target| project::is_signature_path_of(workspace_path, own_target, path))
        };
        let result = check_signatures(
            &target.environment,
            workspace_path,
            &options.paths,
            options.steep_compatible,
            &owns,
        );
        signature_result.error_count += result.error_count;
        signature_result.diagnostics.extend(result.diagnostics);
    }
    WorkspaceResult {
        results,
        signature_result,
    }
}

fn use_color() -> bool {
    use std::io::IsTerminal;
    std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none()
}

/// Steep gives up typing the nodes that need a class definition RBS cannot build and reports `Ruby::UnexpectedError`,
/// which is a hint by default, so it is easy to miss that parts of the files are not type checked
pub fn unexpected_error_warning(file_count: usize) -> Option<String> {
    (file_count > 0).then(|| {
        format!(
            "Steep compatible mode: Steep cannot type check parts of {file_count} files because RBS fails to build class definitions with duplicated methods or unknown types (see RBS::DuplicatedMethodDefinition and RBS::UnknownTypeName); they are reported as {UNEXPECTED_ERROR_CODE}"
        )
    })
}

/// Signature diagnostics are always errors in Steep, so every severity level shows them. Explicitly given paths limit
/// them to the signature files under those paths, and `owns` to the signature files of the target of `environment`
pub fn check_signatures(
    environment: &Environment,
    workspace_path: &Path,
    paths: &[PathBuf],
    steep_compatible: bool,
    owns: &dyn Fn(&Path) -> bool,
) -> FileResult {
    let roots: Vec<PathBuf> = paths.iter().map(|path| workspace_path.join(path)).collect();
    let diagnostics: Vec<SignatureDiagnostic> = if steep_compatible {
        environment.steep_compatible_signature_diagnostics()
    } else {
        environment.signature_diagnostics.clone()
    };
    let mut sources: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    let reported: Vec<ReportedDiagnostic> = diagnostics
        .into_iter()
        .filter(|diagnostic| roots.is_empty() || roots.iter().any(|root| diagnostic.path.starts_with(root)))
        .filter(|diagnostic| owns(&diagnostic.path))
        .map(|diagnostic| {
            let source = sources
                .entry(diagnostic.path.clone())
                .or_insert_with(|| std::fs::read(&diagnostic.path).unwrap_or_default());
            let line_offset = |line: u32| {
                source.split_inclusive(|byte| *byte == b'\n').take(line as usize).map(<[u8]>::len).sum::<usize>()
            };
            let start = line_offset(diagnostic.start.line) + diagnostic.start.byte_column as usize;
            let end = line_offset(diagnostic.end.line) + diagnostic.end.byte_column as usize;
            let mut location =
                ReportedLocation::from_offsets(source, start, end, diagnostic.start.utf16_column as usize + 1);
            location.line = diagnostic.start.line as usize + 1;
            location.end_line = diagnostic.end.line as usize + 1;
            location.end_column = diagnostic.end.utf16_column as usize + 1;
            ReportedDiagnostic {
                path: diagnostic.path.strip_prefix(workspace_path).unwrap_or(&diagnostic.path).display().to_string(),
                severity: Severity::Error,
                code: Some(diagnostic.code),
                message: diagnostic.message.clone(),
                location: Some(location),
            }
        })
        .collect();
    FileResult { error_count: reported.len(), diagnostics: reported, has_unexpected_error: false }
}

/// steep 2.0.0 `Drivers::Check#print_result` fails when any diagnostic survives `--severity-level`, errors or not
pub fn exit_code(diagnostic_count: usize) -> i32 {
    if diagnostic_count > 0 { 1 } else { 0 }
}

fn shown_at(severity: Severity, severity_level: DiagnosticSeverity) -> bool {
    let rank = match severity {
        Severity::Error => DiagnosticSeverity::Error,
        Severity::Warning => DiagnosticSeverity::Warning,
        Severity::Information => DiagnosticSeverity::Information,
        Severity::Hint => DiagnosticSeverity::Hint,
    };
    rank <= severity_level
}

pub struct FileResult {
    pub diagnostics: Vec<ReportedDiagnostic>,
    pub error_count: usize,
    /// Whether the type checker reported `Ruby::UnexpectedError`, whatever the severity level
    pub has_unexpected_error: bool,
}

impl FileResult {
    fn empty() -> FileResult {
        FileResult {
            diagnostics: Vec::new(),
            error_count: 0,
            has_unexpected_error: false,
        }
    }

    fn failure(path: String, message: String) -> FileResult {
        let diagnostic = ReportedDiagnostic { path, severity: Severity::Error, code: None, message, location: None };
        FileResult { diagnostics: vec![diagnostic], error_count: 1, has_unexpected_error: false }
    }

    pub fn lines(&self) -> Vec<String> {
        self.diagnostics.iter().map(crate::report::render_line).collect()
    }
}

pub fn check_file(
    environment: &Environment,
    workspace_path: &Path,
    path: &Path,
    code_diagnostics: &DiagnosticConfiguration,
    severity_level: DiagnosticSeverity,
    analysis_options: AnalysisOptions,
) -> FileResult {
    let display_path = path.strip_prefix(workspace_path).unwrap_or(path).display().to_string();
    let source = match std::fs::read(path) {
        Ok(source) => source,
        Err(error) => {
            return FileResult::failure(display_path, format!("Failed to read: {error}"));
        }
    };
    let Some(analysis) = project::analyze_with_options(environment, &source, analysis_options) else {
        return FileResult::failure(display_path, "The type checker crashed".to_string());
    };
    let has_unexpected_error = analysis.diagnostics.iter().any(|diagnostic| diagnostic.code == UNEXPECTED_ERROR_CODE);
    let mut diagnostics = project::configured_diagnostics(&analysis, code_diagnostics);
    diagnostics.retain(|(_, severity)| shown_at(*severity, severity_level));
    diagnostics.sort_by_key(|(diagnostic, _)| (diagnostic.start, diagnostic.end));
    let reported = diagnostics
        .iter()
        .map(|(diagnostic, severity)| {
            let column = character_column(&source, diagnostic.start);
            ReportedDiagnostic {
                path: display_path.clone(),
                severity: *severity,
                code: Some(diagnostic.code),
                message: diagnostic.message.clone(),
                location: Some(ReportedLocation::from_offsets(&source, diagnostic.start, diagnostic.end, column)),
            }
        })
        .collect();
    let error_count = diagnostics.iter().filter(|(_, severity)| *severity == Severity::Error).count();
    FileResult { diagnostics: reported, error_count, has_unexpected_error }
}

fn character_column(source: &[u8], offset: usize) -> usize {
    let offset = offset.min(source.len());
    let line_start = source[..offset].iter().rposition(|byte| *byte == b'\n').map_or(0, |index| index + 1);
    String::from_utf8_lossy(&source[line_start..offset]).chars().count() + 1
}

/// Explicitly given files skip the `Steepfile` filter so that a file outside the targets can still be checked on demand
pub fn collect_files(workspace_path: &Path, steepfile: Option<&SteepfileConfiguration>, paths: &[PathBuf]) -> Vec<PathBuf> {
    let roots: Vec<PathBuf> =
        if paths.is_empty() { vec![workspace_path.to_path_buf()] } else { paths.iter().map(|path| workspace_path.join(path)).collect() };
    let mut files = Vec::new();
    for root in roots {
        if root.is_file() {
            files.push(root);
            continue;
        }
        let entries = walkdir::WalkDir::new(&root)
            .into_iter()
            .filter_entry(|entry| entry.depth() == 0 || !is_skipped_directory(entry.path()))
            .filter_map(Result::ok);
        for entry in entries {
            let path = entry.path();
            if entry.file_type().is_file()
                && path.extension().is_some_and(|extension| extension == "rb")
                && project::is_checked_path(workspace_path, steepfile, path)
            {
                files.push(path.to_path_buf());
            }
        }
    }
    files.sort();
    files.dedup();
    files
}

fn is_skipped_directory(path: &Path) -> bool {
    let file_name = path.file_name().and_then(|name| name.to_str()).unwrap_or_default();
    file_name.starts_with('.') || file_name == "node_modules" || path.ends_with("vendor/bundle")
}

#[cfg(test)]
mod tests {
    use super::*;
    use rbs_environment::{SignatureDiagnostic, SourcePosition, SteepfileTarget};

    fn arguments(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn parses_ruby_command_and_paths() {
        let options = parse_arguments(&arguments(&["--ruby-command", "mise exec -- ruby", "lib", "app/models"])).unwrap();
        assert_eq!(options.ruby_command, arguments(&["mise", "exec", "--", "ruby"]));
        assert_eq!(options.paths, vec![PathBuf::from("lib"), PathBuf::from("app/models")]);

        let options = parse_arguments(&arguments(&["--ruby-command=ruby3.4"])).unwrap();
        assert_eq!(options.ruby_command, arguments(&["ruby3.4"]));
        assert!(options.paths.is_empty());
    }

    #[test]
    fn parses_severity_level() {
        let options = parse_arguments(&arguments(&["--severity-level", "hint"])).unwrap();
        assert_eq!(options.severity_level, DiagnosticSeverity::Hint);
        let options = parse_arguments(&arguments(&["--severity-level=error", "lib"])).unwrap();
        assert_eq!(options.severity_level, DiagnosticSeverity::Error);
        assert_eq!(options.paths, vec![PathBuf::from("lib")]);
    }

    #[test]
    fn defaults_to_ruby_and_the_warning_level() {
        let options = parse_arguments(&[]).unwrap();
        assert_eq!(options, TypeCheckOptions {
            ruby_command: arguments(&["ruby"]),
            severity_level: DiagnosticSeverity::Warning,
            steep_compatible: false,
            steep_version: None,
            format: OutputFormat::Pretty,
            paths: Vec::new()
        });
    }

    #[test]
    fn parses_the_steep_version_to_follow() {
        assert_eq!(
            parse_arguments(&arguments(&["--steep-version", "2.0"]))
                .unwrap()
                .steep_version,
            Some(SteepVersion::Version2_0)
        );
        assert_eq!(
            parse_arguments(&arguments(&["--steep-version=2.1.0"]))
                .unwrap()
                .steep_version,
            Some(SteepVersion::Version2_1)
        );
        assert!(parse_arguments(&arguments(&["--steep-version=1.10.0"])).is_err());
        assert!(parse_arguments(&arguments(&["--steep-version"])).is_err());
    }

    #[test]
    fn parses_steep_compatible() {
        let options = parse_arguments(&arguments(&["--steep-compatible", "app"])).unwrap();
        assert!(options.steep_compatible);
        assert_eq!(options.paths, vec![PathBuf::from("app")]);
        assert!(parse_arguments(&arguments(&["--steep-compatible=true"])).is_err());
    }

    #[test]
    fn warns_about_files_steep_cannot_type_check_completely() {
        assert_eq!(unexpected_error_warning(0), None);
        let warning = unexpected_error_warning(243).unwrap();
        assert!(warning.starts_with("Steep compatible mode: Steep cannot type check parts of 243 files"));
        assert!(warning.contains("RBS::DuplicatedMethodDefinition"));
        assert!(warning.contains("Ruby::UnexpectedError"));
    }

    #[test]
    fn shows_diagnostics_at_or_above_the_severity_level() {
        assert!(shown_at(Severity::Error, DiagnosticSeverity::Warning));
        assert!(shown_at(Severity::Warning, DiagnosticSeverity::Warning));
        assert!(!shown_at(Severity::Information, DiagnosticSeverity::Warning));
        assert!(!shown_at(Severity::Hint, DiagnosticSeverity::Information));
        assert!(shown_at(Severity::Hint, DiagnosticSeverity::Hint));
        assert!(!shown_at(Severity::Warning, DiagnosticSeverity::Error));
    }

    #[test]
    fn fails_when_any_diagnostic_is_shown() {
        assert_eq!(exit_code(0), 0);
        assert_eq!(exit_code(1), 1);
    }

    #[test]
    fn parses_output_format() {
        assert_eq!(parse_arguments(&arguments(&["--format", "line"])).unwrap().format, OutputFormat::Line);
        assert_eq!(parse_arguments(&arguments(&["--format=pretty"])).unwrap().format, OutputFormat::Pretty);
        assert!(parse_arguments(&arguments(&["--format=github"])).is_err());
    }

    #[test]
    fn rejects_invalid_arguments() {
        assert!(parse_arguments(&arguments(&["--ruby-command"])).is_err());
        assert!(parse_arguments(&arguments(&["--ruby-command", "  "])).is_err());
        assert!(parse_arguments(&arguments(&["--verbose"])).is_err());
        assert!(parse_arguments(&arguments(&["--severity-level"])).is_err());
        assert!(parse_arguments(&arguments(&["--severity-level=notice"])).is_err());
    }

    #[test]
    fn prints_signature_diagnostics_as_errors() {
        let workspace = Path::new("/workspace");
        let diagnostic = |path: &str, line: u32| SignatureDiagnostic {
            path: workspace.join(path),
            start: SourcePosition { line, byte_column: 4, utf16_column: 4 },
            end: SourcePosition { line, byte_column: 11, utf16_column: 11 },
            code: "RBS::UnknownTypeName",
            message: "Cannot find type `Missing`".to_string(),
        };
        let mut environment = Environment::default();
        environment.signature_diagnostics = vec![diagnostic("sig/a.rbs", 2), diagnostic("sig/models/b.rbs", 0)];

        let result = check_signatures(&environment, workspace, &[], false, &|_| true);
        assert_eq!(result.lines(), vec![
            "sig/a.rbs:3:5: [error] Cannot find type `Missing` (RBS::UnknownTypeName)".to_string(),
            "sig/models/b.rbs:1:5: [error] Cannot find type `Missing` (RBS::UnknownTypeName)".to_string(),
        ]);
        assert_eq!(result.error_count, 2);

        let result = check_signatures(&environment, workspace, &[PathBuf::from("sig/models")], false, &|_| {
            true
        });
        assert_eq!(result.diagnostics.len(), 1);
        assert!(result.lines()[0].starts_with("sig/models/b.rbs:1:5:"));
    }

    #[test]
    fn prints_signature_diagnostics_as_steep_does_in_steep_compatible_mode() {
        let workspace = std::env::temp_dir().join(format!("ruby-lsp-rs-type-check-command-signatures-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&workspace);
        let signature_path = workspace.join("sig/searcher.rbs");
        std::fs::create_dir_all(signature_path.parent().unwrap()).unwrap();
        std::fs::write(
            &signature_path,
            "class Searcher\n  def initialize: (Missing records) -> void\n  def search: () -> Missing\n  attr_reader records: Missing\nend\n",
        )
        .unwrap();
        let mut loader = rbs_environment::Loader::new();
        loader.add_signature_directory(&workspace.join("sig"));
        let environment = loader.finish();

        assert_eq!(
            check_signatures(&environment, &workspace, &[], false, &|_| true)
                .diagnostics
                .len(),
            3
        );
        assert_eq!(
            check_signatures(&environment, &workspace, &[], true, &|_| true).lines(),
            vec![
            "sig/searcher.rbs:3:21: [error] Cannot find type `Missing` (RBS::UnknownTypeName)".to_string()
        ]
        );
        std::fs::remove_dir_all(&workspace).unwrap();
    }

    #[test]
    fn reports_unexpected_errors_only_in_steep_compatible_mode() {
        let workspace = std::env::temp_dir().join(format!("ruby-lsp-rs-type-check-command-unexpected-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&workspace);
        std::fs::create_dir_all(workspace.join("lib")).unwrap();
        let ruby_path = workspace.join("lib/clock.rb");
        std::fs::write(&ruby_path, "Time.now\n").unwrap();
        let mut loader = rbs_environment::Loader::new();
        loader.add_source(
            Path::new("library.rbs"),
            "class Object\nend\nclass Time\n  def self.now: () -> Time\n  def year: () -> Integer\n  def year: () -> Integer\nend\n",
        );
        let environment = loader.finish();
        let configuration = DiagnosticConfiguration::default();
        let compatible = AnalysisOptions {
            steep_compatible: true,
            ..AnalysisOptions::default()
        };

        let result = check_file(&environment, &workspace, &ruby_path, &configuration, DiagnosticSeverity::Hint, compatible);
        assert_eq!(result.diagnostics.len(), 1);
        assert!(result.lines()[0].starts_with("lib/clock.rb:1:1: [hint] UnexpectedError: "));
        assert!(result.lines()[0].ends_with("(Ruby::UnexpectedError)"));
        assert!(result.has_unexpected_error);

        let result = check_file(&environment, &workspace, &ruby_path, &configuration, DiagnosticSeverity::Warning, compatible);
        assert!(result.diagnostics.is_empty());
        assert!(result.has_unexpected_error);

        let result =
            check_file(&environment, &workspace, &ruby_path, &configuration, DiagnosticSeverity::Hint, AnalysisOptions::default());
        assert!(result.diagnostics.is_empty());
        assert!(!result.has_unexpected_error);
        std::fs::remove_dir_all(&workspace).unwrap();
    }

    #[test]
    fn collects_ruby_files_within_check_paths() {
        let workspace = std::env::temp_dir().join(format!("ruby-lsp-rs-type-check-command-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&workspace);
        for file in ["lib/foo.rb", "lib/templates/bar.rb", "test/foo_test.rb", ".bundle/baz.rb", "lib/notes.txt"] {
            let path = workspace.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "").unwrap();
        }
        let steepfile = SteepfileConfiguration {
            signature_patterns: Vec::new(),
            check_paths: vec![PathBuf::from("lib")],
            ignore_paths: vec![PathBuf::from("lib/templates")],
            libraries: Vec::new(),
            collection_lockfile: None,
            targets: vec![SteepfileTarget {
                name: "lib".to_string(),
                check_paths: vec![PathBuf::from("lib")],
                ignore_paths: vec![PathBuf::from("lib/templates")],
                code_diagnostics: DiagnosticConfiguration::default(),
                groups: Vec::new(),
                implicitly_returns_nil: false,
                signature_patterns: Vec::new(),
                unreferenced: false,
            }],
        };

        assert_eq!(collect_files(&workspace, Some(&steepfile), &[]), vec![workspace.join("lib/foo.rb")]);
        assert_eq!(collect_files(&workspace, None, &[]), vec![
            workspace.join("lib/foo.rb"),
            workspace.join("lib/templates/bar.rb"),
            workspace.join("test/foo_test.rb"),
        ]);
        assert_eq!(collect_files(&workspace, Some(&steepfile), &[PathBuf::from("test/foo_test.rb")]), vec![
            workspace.join("test/foo_test.rb")
        ]);
        std::fs::remove_dir_all(&workspace).unwrap();
    }
}
