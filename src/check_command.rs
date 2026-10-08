use std::path::{Path, PathBuf};

use rayon::prelude::*;
use rbs_environment::{DiagnosticConfiguration, DiagnosticSeverity, Environment, SteepfileConfiguration};
use type_checker::Severity;

use crate::position::{Encoding, LineIndex};
use crate::ruby_environment::RubyEnvironment;
use crate::type_checking;

pub const USAGE: &str = "Usage: ruby-lsp-rs check [--ruby-command \"mise exec -- ruby\"] [--severity-level error|warning|information|hint] [paths...]";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckOptions {
    pub ruby_command: Vec<String>,
    /// Diagnostics less severe than this are neither printed nor counted, as `steep check --severity-level` (which
    /// defaults to `warning`)
    pub severity_level: DiagnosticSeverity,
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

pub fn parse_arguments(arguments: &[String]) -> Result<CheckOptions, String> {
    let mut ruby_command = vec!["ruby".to_string()];
    let mut severity_level = DiagnosticSeverity::Warning;
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
        } else if argument.starts_with("--") {
            return Err(format!("Unknown option: {argument}"));
        } else {
            paths.push(PathBuf::from(argument));
        }
    }
    Ok(CheckOptions { ruby_command, severity_level, paths })
}

pub fn run(arguments: &[String]) -> i32 {
    let options = match parse_arguments(arguments) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("{message}\n{USAGE}");
            return 2;
        }
    };
    let workspace_path = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
    let ruby_environment = RubyEnvironment::detect(&workspace_path, &options.ruby_command).unwrap_or_else(|error| {
        eprintln!("Failed to detect the Ruby environment: {error}");
        RubyEnvironment::default()
    });
    let sources = type_checking::workspace_sources(&ruby_environment);
    if let Some(warning) = type_checking::missing_core_warning(&sources) {
        eprintln!("{warning}");
    }
    let loaded = match type_checking::load_environment(&workspace_path, &sources) {
        Ok(loaded) => loaded,
        Err(message) => {
            eprintln!("Failed to load the RBS environment: {message}");
            return 2;
        }
    };
    eprintln!("{}", type_checking::load_summary(&loaded));
    if let Some(report) = type_checking::load_error_report(&loaded.environment) {
        eprintln!("{report}");
    }

    let files = collect_files(&workspace_path, loaded.steepfile.as_ref(), &options.paths);
    let default_code_diagnostics = DiagnosticConfiguration::default();
    let results: Vec<FileResult> = files
        .par_iter()
        .map(|path| {
            let code_diagnostics =
                type_checking::code_diagnostics_for(&workspace_path, loaded.steepfile.as_ref(), path)
                    .unwrap_or(&default_code_diagnostics);
            check_file(&loaded.environment, &workspace_path, path, code_diagnostics, options.severity_level)
        })
        .collect();

    let signature_result = check_signatures(&loaded.environment, &workspace_path, &options.paths);

    let mut error_count = 0;
    let mut diagnostic_count = 0;
    for result in results.iter().chain(std::iter::once(&signature_result)) {
        for line in &result.lines {
            println!("{line}");
        }
        error_count += result.error_count;
        diagnostic_count += result.lines.len();
    }
    eprintln!("Checked {} files: {diagnostic_count} diagnostics, {error_count} errors", results.len());
    exit_code(diagnostic_count)
}

/// Signature diagnostics are always errors in Steep, so every severity level shows them. Explicitly given paths limit
/// them to the signature files under those paths
pub fn check_signatures(environment: &Environment, workspace_path: &Path, paths: &[PathBuf]) -> FileResult {
    let roots: Vec<PathBuf> = paths.iter().map(|path| workspace_path.join(path)).collect();
    let lines: Vec<String> = environment
        .signature_diagnostics
        .iter()
        .filter(|diagnostic| roots.is_empty() || roots.iter().any(|root| diagnostic.path.starts_with(root)))
        .map(|diagnostic| {
            let display_path = diagnostic.path.strip_prefix(workspace_path).unwrap_or(&diagnostic.path).display();
            format!(
                "{display_path}:{}:{}: [error] {} ({})",
                diagnostic.start.line + 1,
                diagnostic.start.utf16_column + 1,
                diagnostic.message,
                diagnostic.code
            )
        })
        .collect();
    FileResult { error_count: lines.len(), lines }
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
    pub lines: Vec<String>,
    pub error_count: usize,
}

pub fn check_file(
    environment: &Environment,
    workspace_path: &Path,
    path: &Path,
    code_diagnostics: &DiagnosticConfiguration,
    severity_level: DiagnosticSeverity,
) -> FileResult {
    let display_path = path.strip_prefix(workspace_path).unwrap_or(path).display().to_string();
    let source = match std::fs::read(path) {
        Ok(source) => source,
        Err(error) => {
            return FileResult { lines: vec![format!("{display_path}: [error] Failed to read: {error}")], error_count: 1 };
        }
    };
    let Some(analysis) = type_checking::analyze(environment, &source) else {
        return FileResult { lines: vec![format!("{display_path}: [error] The type checker crashed")], error_count: 1 };
    };
    let line_index = LineIndex::new(&source, Encoding::Utf32);
    let mut diagnostics = type_checking::configured_diagnostics(&analysis, code_diagnostics);
    diagnostics.retain(|(_, severity)| shown_at(*severity, severity_level));
    diagnostics.sort_by_key(|(diagnostic, _)| (diagnostic.start, diagnostic.end));
    let lines = diagnostics
        .iter()
        .map(|(diagnostic, severity)| {
            let position = line_index.position_of_offset(diagnostic.start);
            format!(
                "{display_path}:{}:{}: [{}] {} ({})",
                position.line + 1,
                position.character + 1,
                type_checking::severity_name(*severity),
                diagnostic.message,
                diagnostic.code
            )
        })
        .collect();
    let error_count = diagnostics.iter().filter(|(_, severity)| *severity == Severity::Error).count();
    FileResult { lines, error_count }
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
                && type_checking::is_checked_path(workspace_path, steepfile, path)
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
        assert_eq!(options, CheckOptions {
            ruby_command: arguments(&["ruby"]),
            severity_level: DiagnosticSeverity::Warning,
            paths: Vec::new()
        });
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

        let result = check_signatures(&environment, workspace, &[]);
        assert_eq!(result.lines, vec![
            "sig/a.rbs:3:5: [error] Cannot find type `Missing` (RBS::UnknownTypeName)".to_string(),
            "sig/models/b.rbs:1:5: [error] Cannot find type `Missing` (RBS::UnknownTypeName)".to_string(),
        ]);
        assert_eq!(result.error_count, 2);

        let result = check_signatures(&environment, workspace, &[PathBuf::from("sig/models")]);
        assert_eq!(result.lines.len(), 1);
        assert!(result.lines[0].starts_with("sig/models/b.rbs:1:5:"));
    }

    #[test]
    fn collects_ruby_files_within_check_paths() {
        let workspace = std::env::temp_dir().join(format!("ruby-lsp-rs-check-command-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&workspace);
        for file in ["lib/foo.rb", "lib/templates/bar.rb", "test/foo_test.rb", ".bundle/baz.rb", "lib/notes.txt"] {
            let path = workspace.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "").unwrap();
        }
        let steepfile = SteepfileConfiguration {
            signature_directories: Vec::new(),
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
