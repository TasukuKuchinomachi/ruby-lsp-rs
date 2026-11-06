use std::{
    env, fs,
    path::{Path, PathBuf},
    process::ExitCode,
    thread,
};

use rbs_definition::rbs_version::RbsVersion;
use steep::{
    environment_loader::project_rbs_gem,
    expectations::{Diagnostic, Expectations, Outcome, compare},
    project::dsl,
    ruby_exception,
    services::type_check_service::{check_project, to_expectations},
    steep_gem::detect_steep_version,
    strict::{EnabledChecks, GROUPS, StrictCheck},
};
use steep_interface::steep_version::SteepVersion;

// Why not the system allocator: the type checker allocates and frees small values on
// every thread, and the allocator of macOS spent most of the time contending for them.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

const DEFAULT_EXPECTATIONS_PATH: &str = "steep_expectations.yml";

const USAGE: &str = "\
Usage: ruby-type-check [check] [options]

Description:
    Type check the program, as `steep check` does.

    ruby-type-check reads the Steepfile in the current directory without running Ruby,
    and loads the core and stdlib signatures from the rbs gem in RBS_GEM_DIRECTORY,
    or, without it, from the rbs gem of the version in the Gemfile.lock that
    `bundle exec` uses, or of the newest supported version, found in the gem
    directories of the Ruby given with --ruby, or without it, of installed Rubies.

    The diagnostics are the same as those of the Steep version in the
    Gemfile.lock, or, without it, of the newest Steep in the first gem directory
    that has one (Steep 2.1.0 when none is found), with that rbs gem.
    ruby-type-check supports Steep 2.0.0 and 2.1.0, and rbs 4.1.x and 4.2.x.
    Steep 2.1.0 requires rbs 4.2.x.

Options:
        --with-expectations[=PATH]   Type check with expectations saved in PATH (or steep_expectations.yml)
        --save-expectations[=PATH]   Save expectations with current type check result to PATH (or steep_expectations.yml)
        --steep-version=VERSION      Report the diagnostics of Steep VERSION (2.0.0 or 2.1.0)
        --rbs-version=VERSION        Load the rbs gem of VERSION (4.1.x or 4.2.x, as 4.1.3)
        --ruby=PATH                  Find Steep and the rbs gem only in the gem directories of the ruby executable PATH
        --jobs=N                     Type check with N threads (the number of CPUs by default)
        --strict                     Enable every strict check except hidden-steep-diagnostics
        --check=NAME                 Enable the check NAME (applied after --strict)
        --no-check=NAME              Disable the check NAME (applied after --strict)
    -h, --help                       Show this message

    Without --strict, --check and --no-check, the output is the same as Steep's. The strict
    checks report places where Steep skips type checking without telling it, as
    diagnostics with the code Strict::<Name> and the severity error, except
    hidden-steep-diagnostics, which shows Steep's own diagnostics the Steepfile
    hides and runs only with --check=hidden-steep-diagnostics, as Steep shows
    them only when the Steepfile configures them.
";

fn usage() -> String {
    let mut text = USAGE.to_string();
    text.push_str("\nChecks:\n");
    for group in GROUPS {
        text.push_str(&format!("    {}:\n", group.title));
        for check in group.checks {
            text.push_str(&format!(
                "        {:<30} {}\n",
                check.name(),
                check.description()
            ));
        }
    }
    text
}

#[derive(Default)]
struct CheckOptions {
    help: bool,
    with_expectations_path: Option<PathBuf>,
    save_expectations_path: Option<PathBuf>,
    steep_version: Option<SteepVersion>,
    rbs_release: Option<String>,
    ruby: Option<PathBuf>,
    jobs: Option<usize>,
    enabled_checks: EnabledChecks,
}

fn parse_check_name(name: Option<&str>) -> Result<StrictCheck, String> {
    let name = name.ok_or("--check and --no-check need a check name, as --check=NAME")?;
    StrictCheck::parse(name).ok_or_else(|| format!("unknown check: {name}"))
}

fn parse_check_arguments(arguments: &[String]) -> Result<CheckOptions, String> {
    let mut options = CheckOptions::default();
    let mut strict = false;
    let mut check_changes = Vec::new();
    for argument in arguments {
        let (name, value) = match argument.split_once('=') {
            Some((name, value)) => (name, Some(value)),
            None => (argument.as_str(), None),
        };
        let path = PathBuf::from(value.unwrap_or(DEFAULT_EXPECTATIONS_PATH));
        match name {
            "-h" | "--help" => options.help = true,
            "--with-expectations" => options.with_expectations_path = Some(path),
            "--save-expectations" => options.save_expectations_path = Some(path),
            "--steep-version" => {
                let version =
                    value.ok_or("--steep-version needs a version, as --steep-version=2.0.0")?;
                options.steep_version = Some(SteepVersion::parse(version).ok_or_else(|| {
                    format!(
                        "unsupported Steep version {version}: choose one of {}",
                        SteepVersion::supported_names()
                    )
                })?);
            }
            "--rbs-version" => {
                let release =
                    value.ok_or("--rbs-version needs a version, as --rbs-version=4.1.3")?;
                if RbsVersion::of_release(release).is_none() {
                    return Err(format!(
                        "unsupported rbs version {release}: choose one of {}",
                        RbsVersion::supported_names()
                    ));
                }
                options.rbs_release = Some(release.to_string());
            }
            "--ruby" => {
                let ruby =
                    value.ok_or("--ruby needs the path of a ruby executable, as --ruby=PATH")?;
                options.ruby = Some(PathBuf::from(ruby));
            }
            "--jobs" => {
                let jobs = value
                    .and_then(|jobs| jobs.parse().ok())
                    .filter(|jobs| *jobs > 0)
                    .ok_or("--jobs needs a positive number, as --jobs=4")?;
                options.jobs = Some(jobs);
            }
            "--strict" if value.is_none() => strict = true,
            "--check" => check_changes.push((true, parse_check_name(value)?)),
            "--no-check" => check_changes.push((false, parse_check_name(value)?)),
            _ => return Err(format!("unsupported argument: {argument}")),
        }
    }
    if strict {
        options.enabled_checks = EnabledChecks::strict();
    }
    for (enable, check) in check_changes {
        if enable {
            options.enabled_checks.enable(check);
        } else {
            options.enabled_checks.disable(check);
        }
    }
    Ok(options)
}

fn type_check(
    enabled_checks: &EnabledChecks,
    steep_version: Option<SteepVersion>,
    rbs_release: Option<&str>,
    ruby: Option<&Path>,
    jobs: usize,
) -> Result<(Expectations, Vec<String>), String> {
    let base_directory =
        env::current_dir().map_err(|error| format!("cannot get the current directory: {error}"))?;
    let steepfile = base_directory.join("Steepfile");
    let content = fs::read_to_string(&steepfile)
        .map_err(|error| format!("cannot read {}: {error}", steepfile.display()))?;
    let project = dsl::parse(&base_directory, &content)?;
    let steep_version = match steep_version {
        Some(version) => version,
        None => detect_steep_version(&base_directory, ruby)?,
    };
    let rbs_gem = project_rbs_gem(&base_directory, ruby, rbs_release)?;
    steep_version.check_rbs_gem(&rbs_gem)?;
    let result = check_project(&project, enabled_checks, steep_version, &rbs_gem, jobs)?;
    Ok((to_expectations(&result), result.failures))
}

fn print_diagnostic(prefix: &str, file: &str, diagnostic: &Diagnostic, source: bool) {
    let mut lines = diagnostic.message.split('\n');
    println!(
        "{prefix}{file}:{}:{}: [{}] {}",
        diagnostic.range.start.line,
        diagnostic.range.start.character,
        diagnostic.severity.to_lowercase(),
        lines.next().unwrap_or_default()
    );
    let rest: Vec<&str> = lines.collect();
    for line in &rest {
        println!("{prefix}│ {line}");
    }
    if !rest.is_empty() {
        println!("{prefix}│");
    }
    println!("{prefix}│ Diagnostic ID: {}", diagnostic.code);
    println!("{prefix}│");
    let content = if source {
        fs::read_to_string(file).ok()
    } else {
        None
    };
    let Some(content) = content else {
        println!("{prefix}└ (no source code available)");
        println!("{prefix}");
        return;
    };
    let line: Vec<char> = content
        .split_inclusive('\n')
        .nth(diagnostic.range.start.line as usize - 1)
        .unwrap_or_default()
        .chars()
        .collect();
    let start = (diagnostic.range.start.character as usize).min(line.len());
    let leading: String = line[..start].iter().collect();
    let (subject, trailing): (String, String) =
        if diagnostic.range.start.line == diagnostic.range.end.line {
            let end = (diagnostic.range.end.character as usize).clamp(start, line.len());
            (
                line[start..end].iter().collect(),
                line[end..]
                    .iter()
                    .collect::<String>()
                    .trim_end_matches('\n')
                    .to_string(),
            )
        } else {
            (
                line[start..]
                    .iter()
                    .collect::<String>()
                    .trim_end_matches('\n')
                    .to_string(),
                String::new(),
            )
        };
    println!("{prefix}└ {leading}{subject}{trailing}");
    println!(
        "{prefix}  {}{}",
        " ".repeat(leading.chars().count()),
        "~".repeat(subject.chars().count())
    );
}

fn pluralize(count: usize) -> &'static str {
    if count == 1 {
        "diagnostic"
    } else {
        "diagnostics"
    }
}

fn check_with_expectations(path: &PathBuf, actual: &Expectations) -> Result<ExitCode, String> {
    let content = fs::read_to_string(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let expected = Expectations::load(&content)
        .map_err(|error| format!("cannot parse {}: {error}", path.display()))?;

    let mut files: Vec<&str> = expected.files().chain(actual.files()).collect();
    files.sort();
    files.dedup();

    let mut expected_count = 0;
    let mut unexpected_count = 0;
    let mut missing_count = 0;
    for file in files {
        for (outcome, diagnostic) in
            compare(expected.diagnostics_of(file), actual.diagnostics_of(file))
        {
            match outcome {
                Outcome::Expected => expected_count += 1,
                Outcome::Unexpected => {
                    unexpected_count += 1;
                    print_diagnostic("+ ", file, diagnostic, true);
                }
                Outcome::Missing => {
                    missing_count += 1;
                    print_diagnostic("- ", file, diagnostic, false);
                }
            }
        }
    }

    if unexpected_count > 0 || missing_count > 0 {
        println!();
        println!("Expectations unsatisfied:");
        println!("  {expected_count} expected {}", pluralize(expected_count));
        println!(
            "  + {unexpected_count} unexpected {}",
            pluralize(unexpected_count)
        );
        println!("  - {missing_count} missing {}", pluralize(missing_count));
        Ok(ExitCode::FAILURE)
    } else {
        println!("Expectations satisfied:");
        println!("  {expected_count} expected {}", pluralize(expected_count));
        Ok(ExitCode::SUCCESS)
    }
}

fn save_expectations(path: &PathBuf, actual: &Expectations) -> Result<ExitCode, String> {
    fs::write(path, actual.to_yaml())
        .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    println!("Saved expectations in {}...", path.display());
    Ok(ExitCode::SUCCESS)
}

fn print_result(actual: &Expectations) -> Result<ExitCode, String> {
    let total = actual.diagnostic_count();
    if total == 0 {
        println!("No type error detected.");
        return Ok(ExitCode::SUCCESS);
    }
    let mut file_count = 0;
    for file in actual.files() {
        let diagnostics = actual.diagnostics_of(file);
        if diagnostics.is_empty() {
            continue;
        }
        file_count += 1;
        for diagnostic in diagnostics {
            print_diagnostic("", file, diagnostic, true);
            println!();
        }
    }
    let problems = if total == 1 { "problem" } else { "problems" };
    let files = if file_count == 1 { "file" } else { "files" };
    println!("Detected {total} {problems} from {file_count} {files}");
    Ok(ExitCode::FAILURE)
}

fn check(options: CheckOptions) -> Result<ExitCode, String> {
    let (actual, failures) = type_check(
        &options.enabled_checks,
        options.steep_version,
        options.rbs_release.as_deref(),
        options.ruby.as_deref(),
        options
            .jobs
            .unwrap_or_else(|| thread::available_parallelism().map_or(1, |count| count.get())),
    )?;
    let exit_code = match (
        options.with_expectations_path,
        options.save_expectations_path,
    ) {
        (Some(path), _) => check_with_expectations(&path, &actual),
        (None, Some(path)) => save_expectations(&path, &actual),
        (None, None) => print_result(&actual),
    }?;
    if failures.is_empty() {
        return Ok(exit_code);
    }
    for failure in &failures {
        eprintln!("ruby-type-check: {failure}");
    }
    Err(format!(
        "{} {} failed inside ruby-type-check, so the result above is incomplete",
        failures.len(),
        if failures.len() == 1 {
            "check"
        } else {
            "checks"
        }
    ))
}

fn main() -> ExitCode {
    ruby_exception::install_panic_hook();
    let arguments: Vec<String> = env::args().skip(1).collect();
    let options = match arguments.split_first() {
        Some((command, rest)) if command == "check" => parse_check_arguments(rest),
        Some((command, _)) if !command.starts_with('-') => Err(format!(
            "unknown command `{command}`: the only supported command is `check`"
        )),
        _ => parse_check_arguments(&arguments),
    };
    match options {
        Ok(options) if options.help => {
            print!("{}", usage());
            ExitCode::SUCCESS
        }
        Ok(options) => check(options).unwrap_or_else(|message| {
            eprintln!("ruby-type-check: {message}");
            ExitCode::FAILURE
        }),
        Err(message) => {
            eprintln!("ruby-type-check: {message}");
            eprint!("{}", usage());
            ExitCode::from(2)
        }
    }
}
