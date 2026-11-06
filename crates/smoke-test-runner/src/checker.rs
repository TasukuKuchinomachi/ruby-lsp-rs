use std::{
    env,
    path::{Path, PathBuf},
    process::Command,
};

use steep_interface::steep_version::SteepVersion;

const RBS_VERSION: &str = "4.2.0";

pub enum CheckerKind {
    SteepCheck { executable: PathBuf },
    Steep,
}

pub struct Checker {
    pub kind: CheckerKind,
    pub steep_version: SteepVersion,
}

pub struct CheckOutput {
    pub success: bool,
    pub output: String,
}

fn activate_gems(steep_version: SteepVersion) -> String {
    format!(r#"gem "rbs", "{RBS_VERSION}"; gem "steep", "{steep_version}""#)
}

fn run(mut command: Command) -> Result<CheckOutput, String> {
    let description = format!("{command:?}");
    let output = command
        .output()
        .map_err(|error| format!("cannot run {description}: {error}"))?;
    Ok(CheckOutput {
        success: output.status.success(),
        output: format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ),
    })
}

impl Checker {
    pub fn parse(name: &str, steep_version: SteepVersion) -> Result<Self, String> {
        let kind = match name {
            "ruby-type-check" => {
                let executable = env::current_exe()
                    .map_err(|error| format!("cannot locate the runner itself: {error}"))?
                    .with_file_name(format!("ruby-type-check{}", env::consts::EXE_SUFFIX));
                CheckerKind::SteepCheck { executable }
            }
            "steep" => CheckerKind::Steep,
            _ => {
                return Err(format!(
                    "unknown checker `{name}`: choose `ruby-type-check` or `steep`"
                ));
            }
        };
        Ok(Self {
            kind,
            steep_version,
        })
    }

    pub fn is_steep(&self) -> bool {
        matches!(self.kind, CheckerKind::Steep)
    }

    pub fn describe(&self) -> Result<String, String> {
        let steep_version = self.steep_version;
        match &self.kind {
            CheckerKind::SteepCheck { executable } => {
                if !executable.is_file() {
                    return Err(format!(
                        "{} does not exist: build it with `cargo build -p steep` in the same profile as the runner",
                        executable.display()
                    ));
                }
                Ok(format!(
                    "ruby-type-check ({}) as Steep {steep_version}",
                    executable.display()
                ))
            }
            CheckerKind::Steep => {
                let mut command = Command::new("ruby");
                command.arg("-e").arg(format!(
                    r#"{}; puts "Steep #{{Gem.loaded_specs["steep"].version}} with rbs #{{Gem.loaded_specs["rbs"].version}} on Ruby #{{RUBY_VERSION}}""#,
                    activate_gems(steep_version)
                ));
                let result = run(command)?;
                if !result.success {
                    return Err(format!(
                        "`ruby` in PATH cannot load steep {steep_version} with rbs {RBS_VERSION}:\n{}",
                        result.output
                    ));
                }
                Ok(result.output.trim().to_string())
            }
        }
    }

    pub fn rbs_gem_directory(&self) -> Result<PathBuf, String> {
        match &self.kind {
            CheckerKind::SteepCheck { .. } => {
                steep::environment_loader::rbs_gem_directory(None, None)
            }
            CheckerKind::Steep => {
                let mut command = Command::new("ruby");
                command.arg("-e").arg(format!(
                    r#"{}; print Gem.loaded_specs["rbs"].gem_dir"#,
                    activate_gems(self.steep_version)
                ));
                let result = run(command)?;
                if !result.success {
                    return Err(format!(
                        "`ruby` in PATH cannot load rbs {RBS_VERSION}:\n{}",
                        result.output
                    ));
                }
                Ok(PathBuf::from(result.output))
            }
        }
    }

    pub fn check(
        &self,
        directory: &Path,
        expectations_option: &str,
        extra_arguments: &[String],
    ) -> Result<CheckOutput, String> {
        let steep_version = self.steep_version;
        let mut command = match &self.kind {
            CheckerKind::SteepCheck { executable } => Command::new(executable),
            CheckerKind::Steep => {
                let mut command = Command::new("ruby");
                command.arg("-e").arg(format!(
                    r#"{}; load Gem.bin_path("steep", "steep", "{steep_version}")"#,
                    activate_gems(steep_version)
                ));
                command
            }
        };
        command.arg("check").arg(expectations_option);
        if let CheckerKind::SteepCheck { .. } = self.kind {
            command.arg(format!("--steep-version={steep_version}"));
        }
        command.args(extra_arguments).current_dir(directory);
        run(command)
    }
}
