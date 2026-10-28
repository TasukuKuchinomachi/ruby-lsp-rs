use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

use crate::diagnostic_configuration::{DiagnosticConfiguration, DiagnosticSeverity, DiagnosticTemplate};
use crate::workspace::{SteepfileConfiguration, SteepfileGroup, SteepfileTarget, collection_lockfile_path};

const STEEPFILE_SCRIPT: &str = include_str!("../ruby/steepfile.rb");

#[derive(Deserialize)]
struct EvaluatedSteepfile {
    libraries: Vec<String>,
    /// A path, or `false` for `disable_collection`
    collection_config: Option<serde_yaml::Value>,
    targets: Vec<EvaluatedTarget>,
}

#[derive(Deserialize)]
struct EvaluatedPatterns {
    check: Vec<String>,
    ignore: Vec<String>,
    inline_check: Vec<String>,
    inline_ignore: Vec<String>,
}

#[derive(Deserialize)]
struct EvaluatedTarget {
    name: String,
    #[serde(flatten)]
    patterns: EvaluatedPatterns,
    signature: Vec<String>,
    libraries: Vec<String>,
    collection_config: Option<serde_yaml::Value>,
    unreferenced: bool,
    implicitly_returns_nil: bool,
    code_diagnostics: EvaluatedDiagnostics,
    groups: Vec<EvaluatedGroup>,
}

#[derive(Deserialize)]
struct EvaluatedGroup {
    name: String,
    #[serde(flatten)]
    patterns: EvaluatedPatterns,
    code_diagnostics: Option<EvaluatedDiagnostics>,
}

#[derive(Deserialize)]
struct EvaluatedDiagnostics {
    /// `None` for a plain hash, which reports only the codes it has
    template: Option<String>,
    severities: std::collections::BTreeMap<String, Option<String>>,
}

impl EvaluatedDiagnostics {
    fn configuration(&self) -> DiagnosticConfiguration {
        let template = self
            .template
            .as_deref()
            .and_then(DiagnosticTemplate::from_method_name)
            .unwrap_or(DiagnosticTemplate::Silent);
        let mut configuration = DiagnosticConfiguration::from_template(template);
        for (code, severity) in &self.severities {
            let severity = severity.as_deref().and_then(DiagnosticSeverity::from_name);
            configuration.set(code, severity);
        }
        configuration
    }
}

/// Reads `Steepfile` in `workspace_path` by running it with Ruby, which takes a `Steepfile` that builds its targets
/// in Ruby, such as one target for each file `Pathname.glob` finds. `Ok(None)` when there is no `Steepfile`, and an
/// error when Ruby cannot run it, in which case `read_steepfile` reads it without Ruby
pub fn evaluate_steepfile(
    workspace_path: &Path,
    ruby_command: &[String],
) -> Result<Option<SteepfileConfiguration>, String> {
    let steepfile_path = workspace_path.join("Steepfile");
    if !steepfile_path.is_file() {
        return Ok(None);
    }
    let (program, arguments) = ruby_command.split_first().ok_or("Ruby command is empty")?;
    let output = Command::new(program)
        .args(arguments)
        .arg("-e")
        .arg(STEEPFILE_SCRIPT)
        .arg(&steepfile_path)
        .current_dir(workspace_path)
        .output()
        .map_err(|error| format!("Failed to run {program}: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "Steepfile evaluation failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let evaluated: EvaluatedSteepfile = serde_yaml::from_slice(&output.stdout)
        .map_err(|error| format!("Invalid Steepfile evaluation output: {error}"))?;
    Ok(Some(configuration_of(workspace_path, evaluated)))
}

fn configuration_of(workspace_path: &Path, evaluated: EvaluatedSteepfile) -> SteepfileConfiguration {
    let paths =
        |patterns: &[String]| -> Vec<PathBuf> { patterns.iter().map(|pattern| workspace_path.join(pattern)).collect() };
    let lockfile = |config: &Option<serde_yaml::Value>| match config {
        Some(serde_yaml::Value::String(path)) => Some(collection_lockfile_path(&workspace_path.join(path))),
        _ => None,
    };
    let mut configuration = SteepfileConfiguration {
        signature_patterns: Vec::new(),
        check_paths: Vec::new(),
        ignore_paths: Vec::new(),
        inline_check_paths: Vec::new(),
        inline_ignore_paths: Vec::new(),
        libraries: evaluated.libraries,
        collection_lockfile: lockfile(&evaluated.collection_config),
        targets: Vec::new(),
    };
    for target in evaluated.targets {
        let code_diagnostics = target.code_diagnostics.configuration();
        let mut groups = Vec::new();
        for group in &target.groups {
            configuration.check_paths.extend(paths(&group.patterns.check));
            configuration.ignore_paths.extend(paths(&group.patterns.ignore));
            configuration.inline_check_paths.extend(paths(&group.patterns.inline_check));
            configuration.inline_ignore_paths.extend(paths(&group.patterns.inline_ignore));
            groups.push(SteepfileGroup {
                name: group.name.clone(),
                check_paths: paths(&group.patterns.check),
                ignore_paths: paths(&group.patterns.ignore),
                inline_check_paths: paths(&group.patterns.inline_check),
                inline_ignore_paths: paths(&group.patterns.inline_ignore),
                code_diagnostics: group
                    .code_diagnostics
                    .as_ref()
                    .map_or_else(|| code_diagnostics.clone(), EvaluatedDiagnostics::configuration),
            });
        }
        configuration.signature_patterns.extend(paths(&target.signature));
        configuration.check_paths.extend(paths(&target.patterns.check));
        configuration.ignore_paths.extend(paths(&target.patterns.ignore));
        configuration.inline_check_paths.extend(paths(&target.patterns.inline_check));
        configuration.inline_ignore_paths.extend(paths(&target.patterns.inline_ignore));
        configuration.libraries.extend(target.libraries);
        if let Some(lockfile) = lockfile(&target.collection_config) {
            configuration.collection_lockfile = Some(lockfile);
        }
        configuration.targets.push(SteepfileTarget {
            name: target.name,
            check_paths: paths(&target.patterns.check),
            ignore_paths: paths(&target.patterns.ignore),
            inline_check_paths: paths(&target.patterns.inline_check),
            inline_ignore_paths: paths(&target.patterns.inline_ignore),
            code_diagnostics,
            groups,
            implicitly_returns_nil: target.implicitly_returns_nil,
            signature_patterns: paths(&target.signature),
            unreferenced: target.unreferenced,
        });
    }
    configuration
}
