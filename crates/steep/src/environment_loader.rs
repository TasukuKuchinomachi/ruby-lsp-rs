use std::{
    env, fs,
    path::{Path, PathBuf},
};

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
