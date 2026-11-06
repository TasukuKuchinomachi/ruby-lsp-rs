use std::{
    env, fs,
    path::{Path, PathBuf},
};

use rbs_definition::{
    collection::{self, Lockfile},
    environment_loader::EnvironmentLoader,
    loader::RbsGem,
    rbs_version::{RbsVersion, release_of_gem_directory, release_segments},
};

use crate::{
    project::{CollectionConfiguration, LibraryOptions, Project},
    steep_gem::locked_rbs_release,
};

pub const RBS_DIRECTORY_VARIABLE: &str = "RBS_GEM_DIRECTORY";

fn rbs_directory_of_release_in(gems_directory: &Path, release: &str) -> Option<PathBuf> {
    let directory = gems_directory.join(format!("gems/rbs-{release}"));
    directory.join("core").is_dir().then_some(directory)
}

fn supported_rbs_directories_in(gems_directory: &Path) -> Vec<(Vec<u64>, PathBuf)> {
    let Ok(entries) = fs::read_dir(gems_directory.join("gems")) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let release = name.strip_prefix("rbs-")?;
            RbsVersion::of_release(release)?;
            let segments = release_segments(release)?;
            let directory = entry.path();
            directory
                .join("core")
                .is_dir()
                .then_some((segments, directory))
        })
        .collect()
}

fn newest_supported_rbs_directory(gems_directories: &[PathBuf]) -> Option<PathBuf> {
    gems_directories
        .iter()
        .flat_map(|gems_directory| supported_rbs_directories_in(gems_directory))
        // Not max_by alone: it keeps the last of equal releases, and the first in search order wins.
        .rev()
        .max_by(|left, right| left.0.cmp(&right.0))
        .map(|(_, directory)| directory)
}

fn wanted_rbs(release: Option<&str>) -> String {
    match release {
        Some(release) => format!("rbs {release}"),
        None => format!("rbs {}", RbsVersion::supported_names()),
    }
}

fn installed_ruby_gem_directories(root: &Path) -> Vec<PathBuf> {
    let mut directories = Vec::new();
    let Ok(rubies) = fs::read_dir(root) else {
        return directories;
    };
    for ruby in rubies.flatten() {
        let Ok(versions) = fs::read_dir(ruby.path().join("lib/ruby/gems")) else {
            continue;
        };
        for version in versions.flatten() {
            directories.push(version.path());
        }
    }
    directories.sort();
    directories.reverse();
    directories
}

fn gem_directories_of_ruby(ruby: &Path) -> Result<Vec<PathBuf>, String> {
    let executable = fs::canonicalize(ruby)
        .map_err(|error| format!("cannot find the Ruby {}: {error}", ruby.display()))?;
    let gems_directory = executable
        .parent()
        .and_then(Path::parent)
        .map(|prefix| prefix.join("lib/ruby/gems"))
        .filter(|directory| directory.is_dir())
        .ok_or_else(|| {
            format!(
                "the Ruby {} has no gem directory next to it (bin/../lib/ruby/gems): pass the ruby executable of an installed Ruby, not a shim of mise or rbenv",
                ruby.display()
            )
        })?;
    let mut directories: Vec<PathBuf> = fs::read_dir(&gems_directory)
        .map_err(|error| format!("cannot read {}: {error}", gems_directory.display()))?
        .flatten()
        .map(|entry| entry.path())
        .collect();
    directories.sort();
    if let Some(home) = env::var_os("HOME") {
        let user_directories: Vec<PathBuf> = directories
            .iter()
            .filter_map(|directory| directory.file_name())
            .map(|version| PathBuf::from(&home).join(".gem/ruby").join(version))
            .filter(|directory| directory.is_dir())
            .collect();
        directories.extend(user_directories);
    }
    Ok(directories)
}

pub fn gem_directories(ruby: Option<&Path>) -> Result<Vec<PathBuf>, String> {
    if let Some(ruby) = ruby {
        return gem_directories_of_ruby(ruby);
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    for variable in ["GEM_HOME", "GEM_PATH"] {
        if let Some(value) = env::var_os(variable) {
            candidates.extend(env::split_paths(&value));
        }
    }
    if let Some(path) = env::var_os("PATH") {
        for directory in env::split_paths(&path) {
            if directory.join("ruby").is_file()
                && let Some(prefix) = directory.parent()
                && let Ok(versions) = fs::read_dir(prefix.join("lib/ruby/gems"))
            {
                candidates.extend(versions.flatten().map(|version| version.path()));
                break;
            }
        }
    }
    if let Some(home) = env::var_os("HOME") {
        let home = PathBuf::from(home);
        candidates.extend(installed_ruby_gem_directories(
            &home.join(".local/share/mise/installs/ruby"),
        ));
        candidates.extend(installed_ruby_gem_directories(
            &home.join(".rbenv/versions"),
        ));
        candidates.extend(installed_ruby_gem_directories(&home.join(".rubies")));
        if let Ok(entries) = fs::read_dir(home.join(".gem/ruby")) {
            candidates.extend(entries.flatten().map(|entry| entry.path()));
        }
    }
    candidates.extend(installed_ruby_gem_directories(Path::new(
        "/opt/homebrew/lib/ruby/gems",
    )));
    Ok(candidates)
}

pub fn rbs_gem_directory(ruby: Option<&Path>, release: Option<&str>) -> Result<PathBuf, String> {
    if let Some(directory) = env::var_os(RBS_DIRECTORY_VARIABLE) {
        let directory = PathBuf::from(directory);
        if !directory.join("core").is_dir() {
            return Err(format!(
                "{RBS_DIRECTORY_VARIABLE}={} is not the directory of the rbs gem (it has no core/)",
                directory.display()
            ));
        }
        if let Some(release) = release
            && let Some(found) = release_of_gem_directory(&directory)
            && found != release
        {
            return Err(format!(
                "{RBS_DIRECTORY_VARIABLE}={} is rbs {found}, not rbs {release}",
                directory.display()
            ));
        }
        return Ok(directory);
    }
    let candidates = gem_directories(ruby)?;
    let found = match release {
        Some(release) => candidates
            .iter()
            .find_map(|candidate| rbs_directory_of_release_in(candidate, release)),
        None => newest_supported_rbs_directory(&candidates),
    };
    if let Some(directory) = found {
        return Ok(directory);
    }
    let wanted = wanted_rbs(release);
    match ruby {
        Some(ruby) => Err(format!(
            "the Ruby {} has no {wanted} gem: install it, or set {RBS_DIRECTORY_VARIABLE} to its directory",
            ruby.display()
        )),
        None => Err(format!(
            "cannot find the {wanted} gem: install it, or set {RBS_DIRECTORY_VARIABLE} to its directory"
        )),
    }
}

pub fn rbs_gem(ruby: Option<&Path>, release: Option<&str>) -> Result<RbsGem, String> {
    let directory = rbs_gem_directory(ruby, release)?;
    let gem_paths = match ruby {
        Some(ruby) => gem_directories_of_ruby(ruby)?,
        None => ["GEM_HOME", "GEM_PATH"]
            .into_iter()
            .filter_map(env::var_os)
            .flat_map(|value| env::split_paths(&value).collect::<Vec<_>>())
            .collect(),
    };
    RbsGem::new(&directory, &gem_paths)
}

pub fn supported_rbs_release(release: &str, found_in: &str) -> Result<(), String> {
    match RbsVersion::of_release(release) {
        Some(_) => Ok(()),
        None => Err(format!(
            "{found_in} has rbs {release}, but only rbs {} are supported; pass --rbs-version to choose one",
            RbsVersion::supported_names()
        )),
    }
}

pub fn project_rbs_gem(
    base_directory: &Path,
    ruby: Option<&Path>,
    release: Option<&str>,
) -> Result<RbsGem, String> {
    if release.is_some() || env::var_os(RBS_DIRECTORY_VARIABLE).is_some() {
        return rbs_gem(ruby, release);
    }
    match locked_rbs_release(base_directory) {
        Some((release, lockfile)) => {
            supported_rbs_release(&release, &lockfile.display().to_string())?;
            rbs_gem(ruby, Some(&release))
        }
        None => rbs_gem(ruby, None),
    }
}

fn load_collection_lock(config_path: &Path) -> Result<Option<Lockfile>, String> {
    let lockfile_path = collection::lockfile_path(config_path);
    if !config_path.is_file() {
        eprintln!(
            "rbs-collection configuration is missing: `{}`",
            config_path.display()
        );
        return Ok(None);
    }
    if !lockfile_path.is_file() {
        eprintln!(
            "Run `rbs collection install` to generate missing lockfile: `{}`",
            lockfile_path.display()
        );
        return Ok(None);
    }
    let lockfile = Lockfile::load(&lockfile_path)?;
    if !lockfile.is_rbs_available() {
        lockfile
            .install_without_network()
            .map_err(|error| format!("{}: {error}", lockfile_path.display()))?;
        if !lockfile.is_rbs_available() {
            return Err(format!(
                "{}: the rbs collection is not installed; run `rbs collection install`",
                lockfile_path.display()
            ));
        }
    }
    Ok(Some(lockfile))
}

pub fn construct_env_loader(
    rbs_gem: &RbsGem,
    project: &Project,
    options: &LibraryOptions,
) -> Result<EnvironmentLoader, String> {
    let mut repository = (*rbs_gem.default_repository).clone();
    for path in &options.repo_paths {
        repository.add(&project.absolute_path(path))?;
    }
    let mut loader = rbs_gem.environment_loader(Some(rbs_gem.core_root()), repository);
    for library in &options.libraries {
        let (name, version) = match library.split_once(':') {
            Some((name, version)) => (name, Some(version)),
            None => (library.as_str(), None),
        };
        loader.add_library(name, version, true);
    }
    let collection_config = match &options.collection_config_path {
        CollectionConfiguration::Path(path) => Some(path.clone()),
        CollectionConfiguration::Default => {
            let path = project.absolute_path(Path::new("rbs_collection.yaml"));
            path.is_file().then_some(path)
        }
        CollectionConfiguration::Disabled => None,
    };
    if let Some(config_path) = collection_config
        && let Some(lockfile) = load_collection_lock(&config_path)?
    {
        loader.add_collection(&lockfile)?;
    }
    Ok(loader)
}

pub fn library_files(
    rbs_gem: &RbsGem,
    project: &Project,
    options: &LibraryOptions,
) -> Result<Vec<PathBuf>, String> {
    construct_env_loader(rbs_gem, project, options)?
        .signature_files()
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gems_directory_with(root: &Path, name: &str, releases: &[&str]) -> PathBuf {
        let gems_directory = root.join(name);
        for release in releases {
            fs::create_dir_all(gems_directory.join(format!("gems/rbs-{release}/core"))).unwrap();
        }
        gems_directory
    }

    fn temporary_root(name: &str) -> PathBuf {
        let root = env::temp_dir().join(format!("ruby-type-check-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        root
    }

    #[test]
    fn newest_supported_rbs_directory_looks_at_every_gem_directory() {
        let root = temporary_root("newest-rbs");
        let gem_home = gems_directory_with(&root, "gem_home", &["4.1.3"]);
        let gem_path = gems_directory_with(&root, "gem_path", &["4.2.0", "4.3.0"]);
        assert_eq!(
            newest_supported_rbs_directory(&[gem_home, gem_path.clone()]),
            Some(gem_path.join("gems/rbs-4.2.0"))
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn newest_supported_rbs_directory_prefers_the_earlier_gem_directory_for_the_same_release() {
        let root = temporary_root("same-rbs");
        let gem_home = gems_directory_with(&root, "gem_home", &["4.2.0"]);
        let gem_path = gems_directory_with(&root, "gem_path", &["4.2.0"]);
        assert_eq!(
            newest_supported_rbs_directory(&[gem_home.clone(), gem_path]),
            Some(gem_home.join("gems/rbs-4.2.0"))
        );
        fs::remove_dir_all(&root).unwrap();
    }
}
