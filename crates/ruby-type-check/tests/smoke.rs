//! Checks the `smoke` directories of Steep's repository and compares the diagnostics with their
//! `test_expectations.yml`, as Steep's `bin/output_test.rb` runs `steep check --with-expectations` on them

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use rbs_environment::DiagnosticSeverity;
use ruby_type_check::command::{TypeCheckOptions, WorkspaceResult, check_workspace};
use ruby_type_check::project::severity_name;
use ruby_type_check::report::{OutputFormat, ReportedDiagnostic};

const STEEP_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION_MAJOR"),
    ".",
    env!("CARGO_PKG_VERSION_MINOR"),
    ".",
    env!("CARGO_PKG_VERSION_PATCH")
);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ExpectedDiagnostic {
    /// 1-based line and 0-based character, as `test_expectations.yml` writes them
    start: (u64, u64),
    end: (u64, u64),
    severity: String,
    message: String,
    code: String,
}

impl std::fmt::Display for ExpectedDiagnostic {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{}:{}-{}:{} [{}] {} ({})",
            self.start.0, self.start.1, self.end.0, self.end.1, self.severity, self.message, self.code
        )
    }
}

fn steep_smoke_directory() -> PathBuf {
    let steep_directory = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("steep-{STEEP_VERSION}"));
    if !steep_directory.is_dir() {
        let downloading_directory = steep_directory.with_extension("downloading");
        let _ = std::fs::remove_dir_all(&downloading_directory);
        std::fs::create_dir_all(&downloading_directory).unwrap();
        let url = format!("https://codeload.github.com/soutaro/steep/tar.gz/refs/tags/v{STEEP_VERSION}");
        let status = Command::new("sh")
            .arg("-c")
            .arg(format!("curl --silent --show-error --fail --location {url} | tar --extract --gzip --strip-components 1"))
            .current_dir(&downloading_directory)
            .status()
            .expect("curl and tar are needed to download Steep");
        assert!(status.success(), "Failed to download Steep {STEEP_VERSION} from {url}");
        std::fs::rename(&downloading_directory, &steep_directory).unwrap();
    }
    steep_directory.join("smoke")
}

fn rbs_is_installed() -> bool {
    Command::new("ruby")
        .args(["-e", "Gem::Specification.find_by_name('rbs')"])
        .output()
        .is_ok_and(|output| output.status.success())
}

fn yaml_number(value: &serde_yaml::Value, keys: &[&str]) -> u64 {
    keys.iter().fold(value, |value, key| &value[*key]).as_u64().unwrap()
}

fn expected_diagnostics(smoke_case_directory: &Path) -> BTreeMap<String, Vec<ExpectedDiagnostic>> {
    let content = std::fs::read_to_string(smoke_case_directory.join("test_expectations.yml")).unwrap();
    let files: Vec<serde_yaml::Value> = serde_yaml::from_str(&content).unwrap();
    files
        .iter()
        .map(|file| {
            let mut diagnostics: Vec<ExpectedDiagnostic> = file["diagnostics"]
                .as_sequence()
                .unwrap()
                .iter()
                .map(|diagnostic| ExpectedDiagnostic {
                    start: (
                        yaml_number(diagnostic, &["range", "start", "line"]),
                        yaml_number(diagnostic, &["range", "start", "character"]),
                    ),
                    end: (
                        yaml_number(diagnostic, &["range", "end", "line"]),
                        yaml_number(diagnostic, &["range", "end", "character"]),
                    ),
                    severity: diagnostic["severity"].as_str().unwrap_or("ERROR").to_string(),
                    message: diagnostic["message"].as_str().unwrap().to_string(),
                    code: diagnostic["code"].as_str().unwrap().to_string(),
                })
                .collect();
            diagnostics.sort();
            (file["file"].as_str().unwrap().to_string(), diagnostics)
        })
        .collect()
}

fn reported_diagnostic(diagnostic: &ReportedDiagnostic) -> ExpectedDiagnostic {
    let location = diagnostic.location.as_ref().unwrap();
    ExpectedDiagnostic {
        start: (location.line as u64, location.column as u64 - 1),
        end: (location.end_line as u64, location.end_column as u64 - 1),
        severity: severity_name(diagnostic.severity).to_uppercase(),
        message: diagnostic.message.clone(),
        code: diagnostic.code.unwrap_or_default().to_string(),
    }
}

fn reported_diagnostics(smoke_case_directory: &Path) -> BTreeMap<String, Vec<ExpectedDiagnostic>> {
    let options = TypeCheckOptions {
        ruby_command: vec!["ruby".to_string()],
        severity_level: DiagnosticSeverity::Warning,
        steep_compatible: true,
        format: OutputFormat::Line,
        paths: Vec::new(),
    };
    let WorkspaceResult { results, signature_result } = check_workspace(smoke_case_directory, &options);
    let mut files: BTreeMap<String, Vec<ExpectedDiagnostic>> = BTreeMap::new();
    for diagnostic in results.iter().chain(std::iter::once(&signature_result)).flat_map(|result| &result.diagnostics) {
        files.entry(diagnostic.path.clone()).or_default().push(reported_diagnostic(diagnostic));
    }
    for diagnostics in files.values_mut() {
        diagnostics.sort();
    }
    files
}

/// Lines describing the diagnostics only one side has, as Steep's `Expectations::TestResult` tells missing and
/// unexpected ones apart
fn differences(
    expected: &BTreeMap<String, Vec<ExpectedDiagnostic>>,
    reported: &BTreeMap<String, Vec<ExpectedDiagnostic>>,
) -> Vec<String> {
    let mut paths: Vec<&String> = expected.keys().chain(reported.keys()).collect();
    paths.sort();
    paths.dedup();
    let mut lines = Vec::new();
    for path in paths {
        let expected_diagnostics = expected.get(path).map(Vec::as_slice).unwrap_or_default();
        let reported_diagnostics = reported.get(path).map(Vec::as_slice).unwrap_or_default();
        for diagnostic in expected_diagnostics.iter().filter(|diagnostic| !reported_diagnostics.contains(diagnostic)) {
            lines.push(format!("  missing    {path}:{diagnostic}"));
        }
        for diagnostic in reported_diagnostics.iter().filter(|diagnostic| !expected_diagnostics.contains(diagnostic)) {
            lines.push(format!("  unexpected {path}:{diagnostic}"));
        }
    }
    lines
}

#[test]
fn reports_what_steep_expects_for_its_smoke_tests() {
    if !rbs_is_installed() {
        eprintln!("skipped: the rbs gem is not installed");
        return;
    }
    let smoke_directory = steep_smoke_directory();
    let mut smoke_case_directories: Vec<PathBuf> = std::fs::read_dir(&smoke_directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.join("test_expectations.yml").is_file())
        .collect();
    smoke_case_directories.sort();
    assert!(!smoke_case_directories.is_empty(), "No smoke tests are found in {}", smoke_directory.display());

    let failures: Vec<String> = smoke_case_directories
        .iter()
        .filter_map(|directory| {
            let lines = differences(&expected_diagnostics(directory), &reported_diagnostics(directory));
            let name = directory.file_name().unwrap().to_string_lossy();
            (!lines.is_empty()).then(|| format!("{name}\n{}", lines.join("\n")))
        })
        .collect();
    assert!(
        failures.is_empty(),
        "{} of {} smoke tests failed\n{}",
        failures.len(),
        smoke_case_directories.len(),
        failures.join("\n")
    );
}
