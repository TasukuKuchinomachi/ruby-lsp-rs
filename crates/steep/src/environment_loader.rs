use std::{
    env, fs,
    path::{Path, PathBuf},
};

use rbs_definition::{
    collection::{self, Lockfile},
    environment_loader::EnvironmentLoader,
    loader::RbsGem,
};

use crate::project::{CollectionConfiguration, LibraryOptions, Project};

pub const RBS_VERSION: &str = "4.2.0";

pub const RBS_DIRECTORY_VARIABLE: &str = "RBS_GEM_DIRECTORY";

fn rbs_directory_in(gems_directory: &Path) -> Option<PathBuf> {
    let directory = gems_directory.join(format!("gems/rbs-{RBS_VERSION}"));
    directory.join("core").is_dir().then_some(directory)
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

pub fn rbs_gem_directory(ruby: Option<&Path>) -> Result<PathBuf, String> {
    if let Some(directory) = env::var_os(RBS_DIRECTORY_VARIABLE) {
        let directory = PathBuf::from(directory);
        if directory.join("core").is_dir() {
            return Ok(directory);
        }
        return Err(format!(
            "{RBS_DIRECTORY_VARIABLE}={} is not the directory of the rbs gem (it has no core/)",
            directory.display()
        ));
    }
    for candidate in gem_directories(ruby)? {
        if let Some(directory) = rbs_directory_in(&candidate) {
            return Ok(directory);
        }
    }
    match ruby {
        Some(ruby) => Err(format!(
            "the Ruby {} has no rbs {RBS_VERSION} gem: install it, or set {RBS_DIRECTORY_VARIABLE} to its directory",
            ruby.display()
        )),
        None => Err(format!(
            "cannot find the rbs {RBS_VERSION} gem: install it, or set {RBS_DIRECTORY_VARIABLE} to its directory"
        )),
    }
}

pub fn rbs_gem(ruby: Option<&Path>) -> Result<RbsGem, String> {
    let directory = rbs_gem_directory(ruby)?;
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
