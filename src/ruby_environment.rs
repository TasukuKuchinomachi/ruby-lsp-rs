use std::path::Path;
use std::process::Command;

use serde::Deserialize;

const ENVIRONMENT_SCRIPT: &str = include_str!("../ruby/environment.rb");

#[derive(Debug, Clone, Deserialize, Default)]
pub struct LockedGem {
    pub name: String,
    pub version: String,
    pub full_gem_path: String,
    pub require_paths: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Default, serde::Serialize)]
pub struct Dependency {
    pub name: String,
    pub version: String,
    pub path: String,
    pub dependency: bool,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct RubyEnvironment {
    pub ruby_version: Option<String>,
    pub rubylibdir: Option<String>,
    pub rubylibprefix: Option<String>,
    #[serde(default)]
    pub load_path: Vec<String>,
    pub bundle_path: Option<String>,
    #[serde(default)]
    pub locked_gems: Vec<LockedGem>,
    #[serde(default)]
    pub initial_excluded_gems: Vec<String>,
    #[serde(default)]
    pub default_gems_in_bundle: Vec<String>,
    #[serde(default)]
    pub direct_dependencies: Vec<String>,
    #[serde(default)]
    pub all_dependencies: Vec<String>,
    #[serde(default)]
    pub dependencies: Vec<Dependency>,
    pub rbs_core_path: Option<String>,
    pub rubocop_version: Option<String>,
    #[serde(default)]
    pub errors: Vec<String>,
    #[serde(default)]
    pub gem_stubs: Vec<GemStub>,
    #[serde(default)]
    pub default_gem_stubs: Vec<DefaultGemStub>,
}

/// An entry of `Gem::Specification.stubs`, as seen by the server process once the bundle is set up
#[derive(Debug, Clone, Deserialize, Default)]
pub struct GemStub {
    pub name: String,
    pub version: String,
    pub full_gem_path: String,
}

/// An entry of `Gem::Specification.default_stubs`. Default gems are installed in `rubylibdir`, so their files are
/// listed instead of their directory
#[derive(Debug, Clone, Deserialize, Default)]
pub struct DefaultGemStub {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub require_paths: Vec<String>,
    #[serde(default)]
    pub files: Vec<String>,
}

impl RubyEnvironment {
    /// Runs the bundled Ruby script to collect information that only Bundler and RubyGems can answer
    pub fn detect(workspace_path: &Path, ruby_command: &[String]) -> Result<RubyEnvironment, String> {
        let (program, arguments) = ruby_command.split_first().ok_or("Ruby command is empty")?;
        let output = Command::new(program)
            .args(arguments)
            .arg("-e")
            .arg(ENVIRONMENT_SCRIPT)
            .arg(workspace_path)
            .current_dir(workspace_path)
            .output()
            .map_err(|error| format!("Failed to run {program}: {error}"))?;

        if !output.status.success() {
            return Err(format!(
                "Ruby environment detection failed ({}): {}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        serde_json::from_slice(&output.stdout)
            .map_err(|error| format!("Invalid Ruby environment output: {error}: {}", String::from_utf8_lossy(&output.stdout)))
    }
}
