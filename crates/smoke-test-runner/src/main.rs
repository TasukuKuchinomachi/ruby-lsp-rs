mod checker;
mod fetch;

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{self, ExitCode},
};

use checker::Checker;
use std::collections::BTreeSet;

use rbs_definition::rbs_version::RbsVersion;
use steep::expectations::{Diagnostic, Expectations, Outcome};
use steep_interface::steep_version::SteepVersion;

const EXPECTATIONS_FILE_NAME: &str = "test_expectations.yml";

const STRICT_EXPECTATIONS_FILE_NAME: &str = "strict_expectations.yml";
const STRICT_ARGUMENTS_FILE_NAME: &str = "strict_arguments";

const UNEXPECTED_ERROR_CODE: &str = "Ruby::UnexpectedError";

const DEFAULT_RBS_RELEASE: &str = "4.2.0";

const RBS_GEM_DIRECTORY_PLACEHOLDER: &str = "<rbs gem directory>";

struct AllowedFailure {
    name: &'static str,
    only_with_steep: bool,
    reason: &'static str,
}

const STEEP_ALLOWED_FAILURES: &[AllowedFailure] = &[
    AllowedFailure {
        name: "diagnostics-ruby-unsat",
        only_with_steep: false,
        reason: "Steep's bin/output_test.rb allows it to fail",
    },
    AllowedFailure {
        name: "enumerator",
        only_with_steep: true,
        reason: "with Steep, the numbers of the type variables in its messages depend on which worker process checks each file, and Steep picks the worker from the MD5 of the absolute path",
    },
];

const USAGE: &str = "\
Usage: smoke-test-runner [--checker=ruby-type-check|steep] [--steep-version=VERSION] [--rbs-version=VERSION] [TEST ...]

Runs `check --with-expectations=test_expectations.yml` of the checker in every smoke test
directory, as `rake test:output` of Steep does. A test passes when the checker exits with 0,
or when the saved diagnostics differ from the expectations only in the lines after the
first of the messages of Ruby::UnexpectedError, which are the backtrace of the Ruby exception.

The tests are of the Steep VERSION (2.1.0 without --steep-version), checked with the rbs gem
of VERSION (4.2.0 without --rbs-version). A test uses the first of these files it has, in its
directory for e2e/cases and e2e/strict, and in e2e/steep-smoke/NAME for the smoke tests of
Steep (strict_expectations instead of test_expectations for the runs with --strict):
    test_expectations.steep-STEEP.rbs-RBS.yml   the diagnostics of Steep STEEP with rbs RBS
    test_expectations.steep-STEEP.yml           the diagnostics of Steep STEEP with rbs 4.2.0
    test_expectations.yml
The versioned files are there only where the diagnostics differ from the next file: the
diagnostics of Steep 2.0.0 differ from those of Steep 2.1.0, the smoke tests of Steep 2.0.0
expect the rbs gem Steep 2.0.0 was released with, and rbs 4.1.x builds some definitions
differently from rbs 4.2.0.

`<rbs gem directory>` in expectations stands for the directory of the rbs gem the checker
loads, which Steep writes in the messages of diagnostics about the core signatures.

Tests come from three roots:
    steep/NAME   the smoke tests of Steep VERSION, fetched into target/steep-smoke/VERSION/smoke
    e2e/NAME     e2e/cases of this repository
    strict/NAME  e2e/strict of this repository, run twice: without options against
                 test_expectations.yml (saved by Steep), and with `--strict` against
                 strict_expectations.yml, only with ruby-type-check because Steep has no such option;
                 options in the file strict_arguments of the test follow `--strict`

Options:
    --checker=ruby-type-check
                         Run target/<profile>/ruby-type-check (default)
    --checker=steep      Run Steep VERSION with the rbs gem of VERSION through `ruby` in PATH
    --steep-version=VERSION
                         Compare with Steep VERSION, 2.0.0 or 2.1.0 (default); ruby-type-check runs
                         with --steep-version=VERSION
    --rbs-version=VERSION
                         Compare with Steep using the rbs gem of VERSION, a release of 4.1.x or
                         4.2.x (4.2.0 by default); ruby-type-check runs with --rbs-version=VERSION.
                         Steep 2.1.0 requires rbs 4.2.x
    TEST                 Run only the tests named TEST, as `steep/hello`, `e2e/NAME`, `strict/NAME` or `hello`
";

struct TestCase {
    name: String,
    directory: PathBuf,
    version_expectations_directory: PathBuf,
    allowed_failure_reason: Option<&'static str>,
    strict: bool,
}

impl TestCase {
    fn display_name(&self) -> String {
        let mut words = vec![self.name.clone()];
        words.extend(self.extra_arguments());
        words.join(" ")
    }

    fn expectations_file_name(&self, steep_version: SteepVersion, rbs_release: &str) -> String {
        let file_name = if self.strict {
            STRICT_EXPECTATIONS_FILE_NAME
        } else {
            EXPECTATIONS_FILE_NAME
        };
        [
            format!(".steep-{steep_version}.rbs-{rbs_release}.yml"),
            format!(".steep-{steep_version}.yml"),
        ]
        .iter()
        .map(|suffix| {
            self.version_expectations_directory
                .join(file_name.replace(".yml", suffix))
        })
        .find(|version_path| version_path.is_file())
        .map_or_else(
            || file_name.to_string(),
            |version_path| version_path.display().to_string(),
        )
    }

    fn extra_arguments(&self) -> Vec<String> {
        if !self.strict {
            return Vec::new();
        }
        let mut arguments = vec!["--strict".to_string()];
        if let Ok(text) = fs::read_to_string(self.directory.join(STRICT_ARGUMENTS_FILE_NAME)) {
            arguments.extend(text.split_whitespace().map(str::to_string));
        }
        arguments
    }
}

enum TestResult {
    Passed { nothing_expected: bool },
    Failed { allowed: bool },
    Skipped,
}

struct Options {
    checker: Checker,
    test_names: Vec<String>,
}

fn parse_arguments() -> Result<Options, String> {
    let mut checker_name = "ruby-type-check".to_string();
    let mut steep_version = SteepVersion::default();
    let mut rbs_release = DEFAULT_RBS_RELEASE.to_string();
    let mut test_names = Vec::new();
    for argument in env::args().skip(1) {
        if let Some(name) = argument.strip_prefix("--checker=") {
            checker_name = name.to_string();
        } else if let Some(name) = argument.strip_prefix("--steep-version=") {
            steep_version = SteepVersion::parse(name).ok_or_else(|| {
                format!(
                    "unsupported Steep version {name}: choose one of {}",
                    SteepVersion::supported_names()
                )
            })?;
        } else if let Some(release) = argument.strip_prefix("--rbs-version=") {
            rbs_release = release.to_string();
        } else if argument == "-h" || argument == "--help" {
            print!("{USAGE}");
            process::exit(0);
        } else if argument.starts_with('-') {
            return Err(format!("unknown option: {argument}"));
        } else {
            test_names.push(argument);
        }
    }
    let rbs_version = RbsVersion::of_release(&rbs_release).ok_or_else(|| {
        format!(
            "unsupported rbs version {rbs_release}: choose a release of {}",
            RbsVersion::supported_names()
        )
    })?;
    if rbs_version < steep_version.oldest_rbs_version() {
        return Err(format!(
            "Steep {steep_version} requires rbs {} or later, not rbs {rbs_release}",
            steep_version.oldest_rbs_version()
        ));
    }
    Ok(Options {
        checker: Checker::parse(&checker_name, steep_version, rbs_release)?,
        test_names,
    })
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the repository of the runner exists")
}

fn collect_test_cases(
    prefix: &str,
    root: &Path,
    version_expectations_root: Option<&Path>,
    allowed_failures: &[AllowedFailure],
    checker: &Checker,
) -> Result<Vec<TestCase>, String> {
    if !root.is_dir() {
        return Ok(Vec::new());
    }
    let mut test_cases = Vec::new();
    for entry in
        fs::read_dir(root).map_err(|error| format!("cannot read {}: {error}", root.display()))?
    {
        let directory = entry
            .map_err(|error| format!("cannot read {}: {error}", root.display()))?
            .path();
        if !directory.is_dir() {
            continue;
        }
        let directory_name = directory
            .file_name()
            .expect("an entry of a directory has a name")
            .to_string_lossy()
            .to_string();
        let allowed_failure_reason = allowed_failures
            .iter()
            .find(|allowed_failure| {
                allowed_failure.name == directory_name
                    && (!allowed_failure.only_with_steep || checker.is_steep())
            })
            .map(|allowed_failure| allowed_failure.reason);
        let version_expectations_directory = match version_expectations_root {
            Some(root) => root.join(&directory_name),
            None => directory.clone(),
        };
        test_cases.push(TestCase {
            name: format!("{prefix}/{directory_name}"),
            allowed_failure_reason,
            directory,
            version_expectations_directory,
            strict: false,
        });
    }
    test_cases.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(test_cases)
}

fn select_test_cases(
    test_cases: Vec<TestCase>,
    test_names: &[String],
) -> Result<Vec<TestCase>, String> {
    if test_names.is_empty() {
        return Ok(test_cases);
    }
    let matches = |test_case: &TestCase, test_name: &str| {
        test_case.name == test_name || test_case.name.split_once('/').unwrap().1 == test_name
    };
    if let Some(unknown) = test_names.iter().find(|test_name| {
        !test_cases
            .iter()
            .any(|test_case| matches(test_case, test_name))
    }) {
        return Err(format!("no test is named `{unknown}`"));
    }
    Ok(test_cases
        .into_iter()
        .filter(|test_case| {
            test_names
                .iter()
                .any(|test_name| matches(test_case, test_name))
        })
        .collect())
}

fn read_expectations(path: &Path) -> Result<Expectations, String> {
    let content = fs::read_to_string(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    Expectations::load(&content)
        .map_err(|error| format!("cannot parse {}: {error}", path.display()))
}

fn indent(text: &str, prefix: &str) -> String {
    text.lines()
        .map(|line| format!("{prefix}{line}\n"))
        .collect()
}

fn format_diagnostic(sign: char, diagnostic: &Diagnostic) -> String {
    let mut lines = diagnostic.message.lines();
    let mut text = format!(
        "      {sign} {}:{}-{}:{} {} {}: {}\n",
        diagnostic.range.start.line,
        diagnostic.range.start.character,
        diagnostic.range.end.line,
        diagnostic.range.end.character,
        diagnostic.severity,
        diagnostic.code,
        lines.next().unwrap_or_default()
    );
    for line in lines {
        text.push_str(&format!("      {sign}   {line}\n"));
    }
    text
}

fn comparison_key(diagnostic: &Diagnostic) -> Diagnostic {
    if diagnostic.code == UNEXPECTED_ERROR_CODE {
        Diagnostic {
            message: diagnostic
                .message
                .lines()
                .next()
                .unwrap_or_default()
                .to_string(),
            ..diagnostic.clone()
        }
    } else {
        diagnostic.clone()
    }
}

fn compare<'diagnostics>(
    expected: &'diagnostics [Diagnostic],
    actual: &'diagnostics [Diagnostic],
) -> Vec<(Outcome, &'diagnostics Diagnostic)> {
    let expected: BTreeSet<&Diagnostic> = expected.iter().collect();
    let actual: BTreeSet<&Diagnostic> = actual.iter().collect();
    let expected_keys: BTreeSet<Diagnostic> = expected
        .iter()
        .map(|diagnostic| comparison_key(diagnostic))
        .collect();
    let actual_keys: BTreeSet<Diagnostic> = actual
        .iter()
        .map(|diagnostic| comparison_key(diagnostic))
        .collect();
    let mut outcomes: Vec<(Outcome, &Diagnostic)> = expected
        .iter()
        .map(|diagnostic| {
            if actual_keys.contains(&comparison_key(diagnostic)) {
                (Outcome::Expected, *diagnostic)
            } else {
                (Outcome::Missing, *diagnostic)
            }
        })
        .chain(
            actual
                .iter()
                .filter(|diagnostic| !expected_keys.contains(&comparison_key(diagnostic)))
                .map(|diagnostic| (Outcome::Unexpected, *diagnostic)),
        )
        .collect();
    outcomes.sort_by_key(|(_, diagnostic)| *diagnostic);
    outcomes
}

fn format_difference(expected: &Expectations, actual: &Expectations) -> String {
    let mut files: Vec<&str> = expected.files().chain(actual.files()).collect();
    files.sort();
    files.dedup();
    let mut text = String::new();
    let mut matched_count = 0;
    let mut missing_count = 0;
    let mut unexpected_count = 0;
    for file in files {
        let mut file_text = String::new();
        for (outcome, diagnostic) in
            compare(expected.diagnostics_of(file), actual.diagnostics_of(file))
        {
            match outcome {
                Outcome::Expected => matched_count += 1,
                Outcome::Missing => {
                    missing_count += 1;
                    file_text.push_str(&format_diagnostic('-', diagnostic));
                }
                Outcome::Unexpected => {
                    unexpected_count += 1;
                    file_text.push_str(&format_diagnostic('+', diagnostic));
                }
            }
        }
        if !file_text.is_empty() {
            text.push_str(&format!("    {file}\n{file_text}"));
        }
    }
    if missing_count == 0 && unexpected_count == 0 {
        return String::new();
    }
    format!(
        "    {matched_count} matched, - {missing_count} missing (expected but not reported), + {unexpected_count} unexpected (reported but not expected)\n{text}"
    )
}

fn differs_only_in_unexpected_error_backtraces(
    expected: &Expectations,
    actual: &Expectations,
) -> bool {
    expected != actual
        && expected.files().chain(actual.files()).all(|file| {
            compare(expected.diagnostics_of(file), actual.diagnostics_of(file))
                .iter()
                .all(|(outcome, _)| *outcome == Outcome::Expected)
        })
}

enum FailureExplanation {
    OnlyUnexpectedErrorBacktracesDiffer,
    Failed(String),
}

fn explain_failure(
    checker: &Checker,
    test_case: &TestCase,
    expected: &Expectations,
    with_expectations_output: &str,
) -> FailureExplanation {
    let saved_path = env::temp_dir().join(format!(
        "smoke-test-runner-{}-{}{}.yml",
        process::id(),
        test_case.name.replace('/', "-"),
        if test_case.strict { "-strict" } else { "" }
    ));
    let saved = checker
        .check(
            &test_case.directory,
            &format!("--save-expectations={}", saved_path.display()),
            &test_case.extra_arguments(),
        )
        .and_then(|output| {
            if output.success {
                read_expectations(&saved_path)
            } else {
                Err(format!(
                    "`check --save-expectations` failed:\n{}",
                    output.output
                ))
            }
        });
    let _ = fs::remove_file(&saved_path);
    if let Ok(actual) = &saved
        && differs_only_in_unexpected_error_backtraces(expected, actual)
    {
        return FailureExplanation::OnlyUnexpectedErrorBacktracesDiffer;
    }
    let difference = match &saved {
        Ok(actual) => format_difference(expected, actual),
        Err(_) => String::new(),
    };
    if difference.is_empty() {
        let mut text = format!(
            "    the checker failed without a difference in diagnostics; its output:\n{}",
            indent(with_expectations_output, "    | ")
        );
        if let Err(message) = saved {
            text.push_str(&indent(&message, "    "));
        }
        FailureExplanation::Failed(text)
    } else {
        FailureExplanation::Failed(difference)
    }
}

fn report_pass(test_case: &TestCase, expected: &Expectations, note: &str) -> TestResult {
    let nothing_expected = expected.diagnostic_count() == 0;
    if nothing_expected {
        println!(
            "PASS {} (no diagnostics expected{note})",
            test_case.display_name()
        );
    } else {
        println!(
            "PASS {} ({} diagnostics expected{note})",
            test_case.display_name(),
            expected.diagnostic_count()
        );
    }
    TestResult::Passed { nothing_expected }
}

fn report_failure(test_case: &TestCase, detail: &str) -> TestResult {
    match test_case.allowed_failure_reason {
        Some(reason) => println!("FAIL {} (allowed: {reason})", test_case.display_name()),
        None => println!("FAIL {}", test_case.display_name()),
    }
    print!("{detail}");
    TestResult::Failed {
        allowed: test_case.allowed_failure_reason.is_some(),
    }
}

fn run_test_case(checker: &Checker, test_case: &TestCase) -> TestResult {
    if test_case.strict && checker.is_steep() {
        println!("SKIP {} (Steep has no --strict)", test_case.display_name());
        return TestResult::Skipped;
    }
    let expectations_file_name =
        test_case.expectations_file_name(checker.steep_version, &checker.rbs_release);
    let expectations_path = test_case.directory.join(&expectations_file_name);
    if !expectations_path.is_file() {
        println!(
            "SKIP {} (no {expectations_file_name})",
            test_case.display_name()
        );
        return TestResult::Skipped;
    }
    let (expected, expectations_argument, temporary_path) = match prepare_expectations(
        checker,
        test_case,
        &expectations_path,
        &expectations_file_name,
    ) {
        Ok(prepared) => prepared,
        Err(message) => return report_failure(test_case, &indent(&message, "    ")),
    };
    let result = checker.check(
        &test_case.directory,
        &format!("--with-expectations={expectations_argument}"),
        &test_case.extra_arguments(),
    );
    if let Some(temporary_path) = temporary_path {
        let _ = fs::remove_file(temporary_path);
    }
    match result {
        Ok(output) if output.success => report_pass(test_case, &expected, ""),
        Ok(output) => match explain_failure(checker, test_case, &expected, &output.output) {
            FailureExplanation::OnlyUnexpectedErrorBacktracesDiffer => report_pass(
                test_case,
                &expected,
                ", backtraces of Ruby::UnexpectedError not compared",
            ),
            FailureExplanation::Failed(detail) => report_failure(test_case, &detail),
        },
        Err(message) => report_failure(test_case, &indent(&message, "    ")),
    }
}

fn prepare_expectations(
    checker: &Checker,
    test_case: &TestCase,
    expectations_path: &Path,
    expectations_file_name: &str,
) -> Result<(Expectations, String, Option<PathBuf>), String> {
    let content = fs::read_to_string(expectations_path)
        .map_err(|error| format!("cannot read {}: {error}", expectations_path.display()))?;
    if !content.contains(RBS_GEM_DIRECTORY_PLACEHOLDER) {
        let expected = Expectations::load(&content)
            .map_err(|error| format!("cannot parse {}: {error}", expectations_path.display()))?;
        return Ok((expected, expectations_file_name.to_string(), None));
    }
    let content = content.replace(
        RBS_GEM_DIRECTORY_PLACEHOLDER,
        &checker.rbs_gem_directory()?.display().to_string(),
    );
    let expected = Expectations::load(&content)
        .map_err(|error| format!("cannot parse {}: {error}", expectations_path.display()))?;
    let temporary_path = env::temp_dir().join(format!(
        "smoke-test-runner-{}-{}{}-expectations.yml",
        process::id(),
        test_case.name.replace('/', "-"),
        if test_case.strict { "-strict" } else { "" }
    ));
    fs::write(&temporary_path, content)
        .map_err(|error| format!("cannot write {}: {error}", temporary_path.display()))?;
    Ok((
        expected,
        temporary_path.display().to_string(),
        Some(temporary_path),
    ))
}

fn relative_path(path: &Path, base: &Path) -> String {
    path.strip_prefix(base)
        .unwrap_or(path)
        .display()
        .to_string()
}

fn run() -> Result<bool, String> {
    let options = parse_arguments()?;
    let repository_root = repository_root();
    let checker_description = options
        .checker
        .describe()?
        .replace(&format!("{}/", repository_root.display()), "");
    let steep_smoke_directory =
        fetch::steep_smoke_tests(&repository_root, options.checker.steep_version)?;
    let end_to_end_directory = repository_root.join("e2e").join("cases");

    let mut test_cases = collect_test_cases(
        "steep",
        &steep_smoke_directory,
        Some(&repository_root.join("e2e").join("steep-smoke")),
        STEEP_ALLOWED_FAILURES,
        &options.checker,
    )?;
    let steep_count = test_cases.len();
    test_cases.extend(collect_test_cases(
        "e2e",
        &end_to_end_directory,
        None,
        &[],
        &options.checker,
    )?);
    let end_to_end_count = test_cases.len() - steep_count;
    let strict_directory = repository_root.join("e2e").join("strict");
    for strict_case in collect_test_cases("strict", &strict_directory, None, &[], &options.checker)?
    {
        test_cases.push(TestCase {
            strict: true,
            name: strict_case.name.clone(),
            directory: strict_case.directory.clone(),
            version_expectations_directory: strict_case.version_expectations_directory.clone(),
            allowed_failure_reason: None,
        });
        test_cases.push(strict_case);
    }
    let strict_count = (test_cases.len() - steep_count - end_to_end_count) / 2;
    let test_cases = select_test_cases(test_cases, &options.test_names)?;

    println!("Checker: {checker_description}");
    println!(
        "Tests: {steep_count} in {}, {end_to_end_count} in {}, {strict_count} in {} (each run without and with --strict)",
        relative_path(&steep_smoke_directory, &repository_root),
        relative_path(&end_to_end_directory, &repository_root),
        relative_path(&strict_directory, &repository_root)
    );
    println!();

    let mut passed_with_diagnostics = 0;
    let mut passed_with_nothing_expected = 0;
    let mut failed = 0;
    let mut failed_but_allowed = 0;
    let mut skipped = 0;
    for test_case in &test_cases {
        match run_test_case(&options.checker, test_case) {
            TestResult::Passed {
                nothing_expected: false,
            } => passed_with_diagnostics += 1,
            TestResult::Passed {
                nothing_expected: true,
            } => passed_with_nothing_expected += 1,
            TestResult::Failed { allowed: false } => failed += 1,
            TestResult::Failed { allowed: true } => failed_but_allowed += 1,
            TestResult::Skipped => skipped += 1,
        }
    }

    println!();
    println!("Summary ({checker_description})");
    println!("  passed, with diagnostics expected     {passed_with_diagnostics:>4}");
    println!("  passed, with no diagnostics expected  {passed_with_nothing_expected:>4}");
    println!("  failed                                {failed:>4}");
    println!("  failed, but allowed to fail           {failed_but_allowed:>4}");
    println!("  skipped                               {skipped:>4}");
    println!(
        "  total                                 {:>4}",
        test_cases.len()
    );
    Ok(failed == 0)
}

fn main() -> ExitCode {
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(message) => {
            eprintln!("smoke-test-runner: {message}");
            ExitCode::from(2)
        }
    }
}
