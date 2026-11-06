use std::{
    env, fs,
    path::{Path, PathBuf},
};

use rbs_definition::rbs_version::release_segments;

use crate::steep_version::SteepVersion;

use crate::environment_loader::gem_directories;

fn lockfile_of(gemfile: &Path) -> PathBuf {
    if gemfile.file_name().is_some_and(|name| name == "gems.rb") {
        gemfile.with_file_name("gems.locked")
    } else {
        let mut path = gemfile.as_os_str().to_owned();
        path.push(".lock");
        PathBuf::from(path)
    }
}

fn find_gemfile(base_directory: &Path) -> Option<PathBuf> {
    if let Some(gemfile) = env::var_os("BUNDLE_GEMFILE").filter(|value| !value.is_empty()) {
        return Some(base_directory.join(gemfile));
    }
    base_directory.ancestors().find_map(|directory| {
        ["Gemfile", "gems.rb"]
            .into_iter()
            .map(|name| directory.join(name))
            .find(|path| path.is_file())
    })
}

fn locked_version<'lockfile>(lockfile: &'lockfile str, gem_name: &str) -> Option<&'lockfile str> {
    let prefix = format!("    {gem_name} (");
    lockfile.lines().find_map(|line| {
        line.strip_prefix(prefix.as_str())?
            .strip_suffix(')')
            .map(|version| version.split('-').next().unwrap_or(version))
    })
}

fn project_lockfile(base_directory: &Path) -> Option<(String, PathBuf)> {
    let lockfile = lockfile_of(&find_gemfile(base_directory)?);
    let content = fs::read_to_string(&lockfile).ok()?;
    Some((content, lockfile))
}

pub fn locked_rbs_release(base_directory: &Path) -> Option<(String, PathBuf)> {
    let (content, lockfile) = project_lockfile(base_directory)?;
    let release = locked_version(&content, "rbs")?.to_string();
    Some((release, lockfile))
}

fn newest_installed_steep(gem_directory: &Path) -> Option<String> {
    fs::read_dir(gem_directory.join("specifications"))
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let version = name.strip_prefix("steep-")?.strip_suffix(".gemspec")?;
            let segments = release_segments(version)?;
            Some((segments, version.to_string()))
        })
        .max()
        .map(|(_, version)| version)
}

fn supported(version: &str, found_in: &str) -> Result<SteepVersion, String> {
    SteepVersion::parse(version).ok_or_else(|| {
        format!(
            "{found_in} has Steep {version}, but only Steep {} are supported; pass --steep-version to choose one",
            SteepVersion::supported_names()
        )
    })
}

pub fn detect_steep_version(
    base_directory: &Path,
    ruby: Option<&Path>,
) -> Result<SteepVersion, String> {
    if let Some((content, lockfile)) = project_lockfile(base_directory)
        && let Some(version) = locked_version(&content, "steep")
    {
        return supported(version, &lockfile.display().to_string());
    }
    for gem_directory in gem_directories(ruby)? {
        if let Some(version) = newest_installed_steep(&gem_directory) {
            return supported(&version, &gem_directory.display().to_string());
        }
    }
    Ok(SteepVersion::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_locked_versions_of_steep_and_rbs() {
        let lockfile = "\
GEM
  remote: https://rubygems.org/
  specs:
    rbs (4.1.3)
      logger
    steep (2.0.0)
      rbs (~> 4.0)

DEPENDENCIES
  steep (~> 2.0)
";
        assert_eq!(locked_version(lockfile, "steep"), Some("2.0.0"));
        assert_eq!(locked_version(lockfile, "rbs"), Some("4.1.3"));
    }

    #[test]
    fn ignores_steep_as_a_dependency_of_another_gem() {
        let lockfile = "\
GEM
  specs:
    steep-plugin (1.0.0)
      steep (>= 2.0)
";
        assert_eq!(locked_version(lockfile, "steep"), None);
    }

    #[test]
    fn compares_versions_by_their_numbers() {
        assert!(release_segments("2.10.0") > release_segments("2.9.1"));
        assert_eq!(release_segments("2.1.0.pre.1"), None);
    }
}
