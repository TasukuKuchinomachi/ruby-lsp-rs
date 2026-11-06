use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::{collections::IndexMap, rubygems::Version};

fn children(directory: &Path) -> Result<Vec<String>, String> {
    let entries =
        fs::read_dir(directory).map_err(|error| format!("{}: {error}", directory.display()))?;
    let mut names: Vec<String> = entries
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    Ok(names)
}

#[derive(Clone, Debug, Default)]
struct GemSignatures {
    paths: Vec<PathBuf>,
}

impl GemSignatures {
    fn versions(&self) -> IndexMap<String, (Version, PathBuf)> {
        let mut versions = IndexMap::default();
        for gem_path in &self.paths {
            let Ok(names) = children(gem_path) else {
                continue;
            };
            for name in names {
                let Some(version) = Version::new(&name) else {
                    continue;
                };
                if version.is_prerelease() {
                    continue;
                }
                versions.insert(
                    version.as_str().to_string(),
                    (version, gem_path.join(&name)),
                );
            }
        }
        versions
    }

    fn version_names(versions: &IndexMap<String, (Version, PathBuf)>) -> Vec<&Version> {
        let mut names: Vec<&Version> = versions.values().map(|(version, _)| version).collect();
        names.sort_by(|left, right| left.as_str().cmp(right.as_str()));
        names
    }

    fn latest_version(&self) -> Option<PathBuf> {
        let versions = self.versions();
        let latest = GemSignatures::version_names(&versions).pop()?;
        versions.get(latest.as_str()).map(|(_, path)| path.clone())
    }

    fn find_best_version(&self, version: &Version) -> Option<PathBuf> {
        let versions = self.versions();
        let candidates = GemSignatures::version_names(&versions);
        let best = Repository::find_best_version(Some(version), &candidates)?;
        versions.get(best.as_str()).map(|(_, path)| path.clone())
    }
}

#[derive(Clone, Debug, Default)]
pub struct Repository {
    pub directories: Vec<PathBuf>,
    gems: IndexMap<String, GemSignatures>,
}

impl Repository {
    pub fn new(stdlib_root: Option<&Path>) -> Result<Repository, String> {
        let mut repository = Repository::default();
        if let Some(stdlib_root) = stdlib_root {
            repository.add(stdlib_root)?;
        }
        Ok(repository)
    }

    pub fn find_best_version<'version>(
        version: Option<&Version>,
        candidates: &[&'version Version],
    ) -> Option<&'version Version> {
        let mut candidates = candidates.to_vec();
        candidates.sort();
        let Some(version) = version else {
            return candidates.last().copied();
        };
        candidates
            .iter()
            .rev()
            .find(|candidate| **candidate <= version)
            .or_else(|| candidates.first())
            .copied()
    }

    pub fn add(&mut self, directory: &Path) -> Result<(), String> {
        self.directories.push(directory.to_path_buf());
        for name in children(directory)? {
            self.gems
                .entry(name.clone())
                .or_default()
                .paths
                .push(directory.join(&name));
        }
        Ok(())
    }

    pub fn lookup(&self, gem: &str, version: Option<&str>) -> Option<PathBuf> {
        let signatures = self.gems.get(gem)?;
        match version.map(Version::new) {
            Some(Some(version)) => signatures.find_best_version(&version.release()),
            Some(None) => None,
            None => signatures.latest_version(),
        }
    }

    pub fn versions(&self, gem: &str) -> Vec<String> {
        self.gems
            .get(gem)
            .map(|signatures| signatures.versions().into_keys().collect())
            .unwrap_or_default()
    }
}
