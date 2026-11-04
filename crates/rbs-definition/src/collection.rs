use std::{
    fs,
    path::{Path, PathBuf},
};

use serde_yaml::{Mapping, Value};

use crate::collections::IndexMap;

pub const METADATA_FILENAME: &str = ".rbs_meta.yaml";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    Git {
        name: Option<String>,
        revision: Option<String>,
        remote: Option<String>,
        repository_directory: String,
    },
    Local {
        path: String,
        full_path: PathBuf,
    },
    Stdlib,
    Rubygems,
}

fn optional_string(value: &Value) -> Option<String> {
    match value {
        Value::String(string) => Some(string.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(boolean) => Some(boolean.to_string()),
        _ => None,
    }
}

fn optional_string_value(string: &Option<String>) -> Value {
    string
        .as_ref()
        .map_or(Value::Null, |string| Value::String(string.clone()))
}

fn mapping(entries: Vec<(&str, Value)>) -> Value {
    Value::Mapping(
        entries
            .into_iter()
            .map(|(key, value)| (Value::String(key.to_string()), value))
            .collect::<Mapping>(),
    )
}

fn is_commit_hash(revision: &str) -> bool {
    revision.len() == 40
        && revision
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

impl Source {
    pub fn from_config_entry(entry: &Value, base_directory: &Path) -> Result<Source, String> {
        let field = |name: &str| optional_string(&entry[name]);
        match field("type").as_deref() {
            Some("git") | None => Ok(Source::Git {
                name: field("name"),
                revision: field("revision"),
                remote: field("remote"),
                repository_directory: field("repo_dir").unwrap_or_else(|| "gems".to_string()),
            }),
            Some("local") => {
                let path = field("path").ok_or("a local source of rbs collection has no path")?;
                Ok(Source::Local {
                    full_path: base_directory.join(&path),
                    path,
                })
            }
            Some("stdlib") => Ok(Source::Stdlib),
            Some("rubygems") => Ok(Source::Rubygems),
            Some(source_type) => Err(format!(
                "unknown type of rbs collection source: {source_type}"
            )),
        }
    }

    fn to_lockfile(&self) -> Option<Value> {
        Some(match self {
            Source::Git {
                name,
                revision,
                remote,
                repository_directory,
            } => {
                let revision = revision
                    .as_ref()
                    .filter(|revision| is_commit_hash(revision))?;
                mapping(vec![
                    ("type", Value::String("git".to_string())),
                    ("name", optional_string_value(name)),
                    ("revision", Value::String(revision.clone())),
                    ("remote", optional_string_value(remote)),
                    ("repo_dir", Value::String(repository_directory.clone())),
                ])
            }
            Source::Local { path, .. } => mapping(vec![
                ("type", Value::String("local".to_string())),
                ("path", Value::String(path.clone())),
            ]),
            Source::Stdlib => mapping(vec![("type", Value::String("stdlib".to_string()))]),
            Source::Rubygems => mapping(vec![("type", Value::String("rubygems".to_string()))]),
        })
    }
}

#[derive(Clone, Debug)]
pub struct LockedGem {
    pub name: String,
    pub version: Option<String>,
    pub source: Source,
}

impl LockedGem {
    fn library_data(&self) -> Option<Value> {
        Some(mapping(vec![
            ("name", Value::String(self.name.clone())),
            ("version", optional_string_value(&self.version)),
            ("source", self.source.to_lockfile()?),
        ]))
    }
}

#[derive(Clone, Debug)]
pub struct Lockfile {
    pub lockfile_path: PathBuf,
    pub lockfile_directory: PathBuf,
    pub path: PathBuf,
    pub gems: IndexMap<String, LockedGem>,
}

pub fn lockfile_path(config_path: &Path) -> PathBuf {
    let extension = config_path
        .extension()
        .map(|extension| format!(".{}", extension.to_string_lossy()))
        .unwrap_or_default();
    let stem = config_path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    config_path.with_file_name(format!("{stem}.lock{extension}"))
}

impl Lockfile {
    pub fn load(lockfile_path: &Path) -> Result<Lockfile, String> {
        let content = fs::read_to_string(lockfile_path)
            .map_err(|error| format!("{}: {error}", lockfile_path.display()))?;
        let data: Value = serde_yaml::from_str(&content)
            .map_err(|error| format!("{}: {error}", lockfile_path.display()))?;
        Lockfile::from_lockfile(lockfile_path, &data)
            .map_err(|error| format!("{}: {error}", lockfile_path.display()))
    }

    pub fn from_lockfile(lockfile_path: &Path, data: &Value) -> Result<Lockfile, String> {
        let lockfile_directory = lockfile_path
            .parent()
            .unwrap_or(Path::new(""))
            .to_path_buf();
        let path = optional_string(&data["path"]).ok_or("the lockfile has no path")?;
        let mut gems = IndexMap::default();
        if let Some(entries) = data["gems"].as_sequence() {
            for entry in entries {
                let name =
                    optional_string(&entry["name"]).ok_or("a gem in the lockfile has no name")?;
                let source = Source::from_config_entry(&entry["source"], &lockfile_directory)?;
                gems.insert(
                    name.clone(),
                    LockedGem {
                        name,
                        version: optional_string(&entry["version"]),
                        source,
                    },
                );
            }
        }
        Ok(Lockfile {
            lockfile_path: lockfile_path.to_path_buf(),
            lockfile_directory,
            path: PathBuf::from(path),
            gems,
        })
    }

    pub fn full_path(&self) -> PathBuf {
        self.lockfile_directory.join(&self.path)
    }

    fn installed_path(&self, gem: &LockedGem) -> PathBuf {
        let path = self.full_path().join(&gem.name);
        match &gem.version {
            Some(version) => path.join(version),
            None => path,
        }
    }

    pub fn is_rbs_available(&self) -> bool {
        if !self.full_path().exists() {
            return false;
        }
        self.gems.values().all(|gem| match &gem.source {
            Source::Git { .. } => self.is_installed_from_git(gem),
            Source::Local { .. } => self.installed_path(gem).is_symlink(),
            Source::Stdlib | Source::Rubygems => true,
        })
    }

    fn is_installed_from_git(&self, gem: &LockedGem) -> bool {
        let metadata_path = self.installed_path(gem).join(METADATA_FILENAME);
        fs::read_to_string(metadata_path)
            .ok()
            .and_then(|content| serde_yaml::from_str::<Value>(&content).ok())
            .is_some_and(|metadata| gem.library_data() == Some(metadata))
    }

    pub fn install_without_network(&self) -> Result<(), String> {
        let install_to = self.full_path();
        fs::create_dir_all(&install_to)
            .map_err(|error| format!("{}: {error}", install_to.display()))?;
        for gem in self.gems.values() {
            match &gem.source {
                Source::Local { full_path, .. } => install_local(gem, full_path, &install_to)?,
                Source::Git { remote, .. } if !self.is_installed_from_git(gem) => {
                    return Err(format!(
                        "{}:{} from the git source {} is not installed in {}; run `rbs collection install`",
                        gem.name,
                        gem.version.as_deref().unwrap_or_default(),
                        remote.as_deref().unwrap_or_default(),
                        install_to.display()
                    ));
                }
                Source::Git { .. } | Source::Stdlib | Source::Rubygems => {}
            }
        }
        Ok(())
    }
}

fn install_local(gem: &LockedGem, full_path: &Path, install_to: &Path) -> Result<(), String> {
    let Some(version) = &gem.version else {
        return Ok(());
    };
    let from = full_path.join(&gem.name).join(version);
    if !from.is_dir() {
        return Ok(());
    }
    let gem_directory = install_to.join(&gem.name).join(version);
    let io_error = |error: std::io::Error| format!("{}: {error}", gem_directory.display());
    if gem_directory.is_symlink() {
        if fs::read_link(&gem_directory).map_err(io_error)? == from {
            return Ok(());
        }
        fs::remove_file(&gem_directory).map_err(io_error)?;
    } else if gem_directory.is_dir() {
        fs::remove_dir_all(&gem_directory).map_err(io_error)?;
    } else if gem_directory.exists() {
        return Err(format!(
            "{}: cannot install the rbs collection here",
            gem_directory.display()
        ));
    }
    if let Some(parent) = gem_directory.parent() {
        fs::create_dir_all(parent).map_err(io_error)?;
    }
    symlink(&from, &gem_directory).map_err(io_error)
}

#[cfg(unix)]
fn symlink(from: &Path, to: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(from, to)
}

#[cfg(windows)]
fn symlink(from: &Path, to: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_dir(from, to)
}
