use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use serde_yaml::Value;

use crate::{
    collection::{Lockfile, Source},
    collections::{HashSet, IndexSet},
    repository::Repository,
    rubygems::{Specification, Specifications},
};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Library {
    pub name: String,
    pub version: Option<String>,
}

#[derive(Clone, Debug)]
pub struct UnknownLibraryError {
    pub library: Library,
}

impl std::fmt::Display for UnknownLibraryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "Cannot find type definitions for library: {} ({})",
            self.library.name,
            self.library.version.as_deref().unwrap_or("[nil]")
        )
    }
}

enum DirectorySource {
    Core,
    Library,
    Path,
}

pub fn gem_signature_path<'specifications>(
    specifications: &'specifications Specifications,
    name: &str,
    version: Option<&str>,
) -> Option<(&'specifications Specification, PathBuf)> {
    let specification = specifications.find_by_name(name, version)?;
    let path = specification.gem_directory.join("sig");
    path.is_dir().then_some((specification, path))
}

fn manifest_dependencies(directory: &Path) -> Vec<String> {
    let Ok(content) = fs::read_to_string(directory.join("manifest.yaml")) else {
        return Vec::new();
    };
    let Ok(manifest) = serde_yaml::from_str::<Value>(&content) else {
        return Vec::new();
    };
    manifest["dependencies"]
        .as_sequence()
        .into_iter()
        .flatten()
        .filter_map(|dependency| dependency["name"].as_str().map(str::to_string))
        .collect()
}

fn each_file(path: &Path, skip_hidden: bool) -> Vec<PathBuf> {
    if path.is_file() {
        return vec![path.to_path_buf()];
    }
    if !path.is_dir() {
        return Vec::new();
    }
    let mut files = Vec::new();
    collect_signature_files(path, skip_hidden, &mut files);
    files.sort_by(|left, right| left.as_os_str().cmp(right.as_os_str()));
    files
}

fn collect_signature_files(directory: &Path, skip_hidden: bool, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            if !(skip_hidden && name.starts_with('_')) {
                collect_signature_files(&path, skip_hidden, files);
            }
        } else if name.ends_with(".rbs") && !path.is_dir() {
            files.push(path);
        }
    }
}

pub struct EnvironmentLoader {
    pub core_root: Option<PathBuf>,
    pub repository: Repository,
    pub libraries: IndexSet<Library>,
    pub directories: Vec<PathBuf>,
    specifications: Arc<Specifications>,
    default_repository: Arc<Repository>,
}

impl EnvironmentLoader {
    pub fn new(
        core_root: Option<PathBuf>,
        repository: Repository,
        specifications: Arc<Specifications>,
        default_repository: Arc<Repository>,
    ) -> EnvironmentLoader {
        EnvironmentLoader {
            core_root,
            repository,
            libraries: IndexSet::default(),
            directories: Vec::new(),
            specifications,
            default_repository,
        }
    }

    pub fn add_path(&mut self, path: PathBuf) {
        self.directories.push(path);
    }

    pub fn add_library(&mut self, name: &str, version: Option<&str>, resolve_dependencies: bool) {
        let library = Library {
            name: name.to_string(),
            version: version.map(str::to_string),
        };
        if self.libraries.insert(library) && resolve_dependencies {
            self.resolve_dependencies(name, version);
        }
    }

    fn resolve_dependencies(&mut self, library: &str, version: Option<&str>) {
        let specifications = Arc::clone(&self.specifications);
        let manifest_directory = if gem_signature_path(&specifications, library, version).is_some()
        {
            let version = match version {
                Some(version) => Some(version.to_string()),
                None => gem_signature_path(&specifications, library, None)
                    .map(|(specification, _)| specification.version.as_str().to_string()),
            };
            gem_signature_path(&specifications, library, version.as_deref()).map(|(_, path)| path)
        } else if self.default_repository.lookup(library, version).is_some() {
            let version = match version {
                Some(version) => Some(version.to_string()),
                None => self.default_repository.versions(library).pop(),
            };
            self.default_repository.lookup(library, version.as_deref())
        } else {
            None
        };
        let Some(manifest_directory) = manifest_directory else {
            return;
        };
        for dependency in manifest_dependencies(&manifest_directory) {
            self.add_library(&dependency, None, true);
        }
    }

    pub fn add_collection(&mut self, lockfile: &Lockfile) -> Result<(), String> {
        self.repository.add(&lockfile.full_path())?;
        for gem in lockfile.gems.values() {
            let mut locked_version = gem.version.clone();
            if gem.source == Source::Rubygems
                && gem_signature_path(&self.specifications, &gem.name, locked_version.as_deref())
                    .is_none()
                && let Some((specification, _)) =
                    gem_signature_path(&self.specifications, &gem.name, None)
            {
                locked_version = Some(specification.version.as_str().to_string());
            }
            self.add_library(&gem.name, locked_version.as_deref(), false);
        }
        Ok(())
    }

    fn has_library(&self, library: &Library) -> bool {
        gem_signature_path(
            &self.specifications,
            &library.name,
            library.version.as_deref(),
        )
        .is_some()
            || self
                .repository
                .lookup(&library.name, library.version.as_deref())
                .is_some()
    }

    fn each_directory(&self) -> Result<Vec<(DirectorySource, PathBuf)>, UnknownLibraryError> {
        let mut directories = Vec::new();
        if let Some(root) = &self.core_root {
            directories.push((DirectorySource::Core, root.clone()));
        }
        for library in &self.libraries {
            if !self.has_library(library) {
                return Err(UnknownLibraryError {
                    library: library.clone(),
                });
            }
            let version = library.version.as_deref();
            if let Some((_, path)) =
                gem_signature_path(&self.specifications, &library.name, version)
            {
                directories.push((DirectorySource::Library, path));
            } else if let Some(path) = self.repository.lookup(&library.name, version) {
                directories.push((DirectorySource::Library, path));
            }
        }
        for directory in &self.directories {
            directories.push((DirectorySource::Path, directory.clone()));
        }
        Ok(directories)
    }

    pub fn signature_files(&mut self) -> Result<Vec<PathBuf>, UnknownLibraryError> {
        if self.core_root.is_some()
            && !self
                .libraries
                .iter()
                .any(|library| library.name == "stringio")
        {
            self.add_library("stringio", None, true);
        }
        let mut seen = HashSet::default();
        let mut files = Vec::new();
        for (source, directory) in self.each_directory()? {
            let skip_hidden = !matches!(source, DirectorySource::Path);
            for path in each_file(&directory, skip_hidden) {
                if seen.insert(path.clone()) {
                    files.push(path);
                }
            }
        }
        Ok(files)
    }
}
