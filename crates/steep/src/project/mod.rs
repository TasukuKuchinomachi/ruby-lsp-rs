pub mod dsl;
pub mod pattern;

use std::{
    fmt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use rbs_definition::collections::{HashMap, HashSet};

use crate::{diagnostic::configuration::Configuration, project::pattern::Pattern};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CollectionConfiguration {
    Default,
    Path(PathBuf),
    Disabled,
}

#[derive(Clone, Debug)]
pub struct LibraryOptions {
    pub libraries: Vec<String>,
    pub repo_paths: Vec<PathBuf>,
    pub core_root: Option<PathBuf>,
    pub stdlib_root: Option<PathBuf>,
    pub collection_config_path: CollectionConfiguration,
}

impl Default for LibraryOptions {
    fn default() -> LibraryOptions {
        LibraryOptions {
            libraries: Vec::new(),
            repo_paths: Vec::new(),
            core_root: None,
            stdlib_root: None,
            collection_config_path: CollectionConfiguration::Default,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Group {
    pub name: String,
    pub source_pattern: Pattern,
    pub inline_source_pattern: Pattern,
    pub signature_pattern: Pattern,
    pub code_diagnostics_config: Configuration,
}

#[derive(Clone, Debug)]
pub struct Target {
    pub name: String,
    pub source_pattern: Pattern,
    pub inline_source_pattern: Pattern,
    pub signature_pattern: Pattern,
    pub target_options: Option<LibraryOptions>,
    pub code_diagnostics_config: Configuration,
    pub unreferenced: bool,
    pub implicitly_returns_nil: bool,
    pub groups: Vec<Group>,
}

impl Target {
    pub fn is_possible_source_file(&self, path: &Path) -> bool {
        self.groups.iter().any(|group| {
            group.source_pattern.matches(path) && !group.inline_source_pattern.matches(path)
        }) || (self.source_pattern.matches(path) && !self.inline_source_pattern.matches(path))
    }

    pub fn is_possible_inline_source_file(&self, path: &Path) -> bool {
        self.groups
            .iter()
            .any(|group| group.inline_source_pattern.matches(path))
            || self.inline_source_pattern.matches(path)
    }

    pub fn is_possible_signature_file(&self, path: &Path) -> bool {
        self.groups
            .iter()
            .any(|group| group.signature_pattern.matches(path))
            || self.signature_pattern.matches(path)
    }

    pub fn code_diagnostics_config_for_source(&self, path: &Path) -> &Configuration {
        self.groups
            .iter()
            .find(|group| {
                (group.source_pattern.matches(path) && !group.inline_source_pattern.matches(path))
                    || group.inline_source_pattern.matches(path)
            })
            .map(|group| &group.code_diagnostics_config)
            .unwrap_or(&self.code_diagnostics_config)
    }
}

#[derive(Clone, Debug)]
pub struct Project {
    pub base_directory: PathBuf,
    pub targets: Vec<Target>,
    pub global_options: LibraryOptions,
    pub path_cache: PathCache,
}

// Why not walk the directories each time: a check asks for the files of the same
// patterns several times, and walking a large project takes a noticeable time.
#[derive(Default)]
pub struct PathCache(Mutex<HashMap<Pattern, Arc<Vec<PathBuf>>>>);

impl Clone for PathCache {
    fn clone(&self) -> PathCache {
        PathCache::default()
    }
}

impl fmt::Debug for PathCache {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PathCache")
    }
}

impl Project {
    pub fn options<'project>(&'project self, target: &'project Target) -> &'project LibraryOptions {
        target
            .target_options
            .as_ref()
            .unwrap_or(&self.global_options)
    }

    pub fn absolute_path(&self, path: &Path) -> PathBuf {
        self.base_directory.join(path)
    }

    pub fn target_for_source_path(&self, path: &Path) -> Option<&Target> {
        self.targets
            .iter()
            .find(|target| target.is_possible_source_file(path))
    }

    pub fn target_for_inline_source_path(&self, path: &Path) -> Option<&Target> {
        self.targets
            .iter()
            .find(|target| target.is_possible_inline_source_file(path))
    }

    pub fn target_for_signature_path(&self, path: &Path) -> Option<&Target> {
        self.targets
            .iter()
            .find(|target| target.is_possible_signature_file(path))
    }

    fn each_path_in_patterns(&self, pattern: &Pattern, paths: &mut UniquePaths) {
        let cached = self
            .path_cache
            .0
            .lock()
            .expect("no thread panics while holding the path cache")
            .get(pattern)
            .cloned();
        let pattern_paths = match cached {
            Some(pattern_paths) => pattern_paths,
            None => {
                let mut pattern_paths = UniquePaths::default();
                self.walk_patterns(pattern, &mut pattern_paths);
                let pattern_paths = Arc::new(pattern_paths.into_paths());
                self.path_cache
                    .0
                    .lock()
                    .expect("no thread panics while holding the path cache")
                    .insert(pattern.clone(), pattern_paths.clone());
                pattern_paths
            }
        };
        for path in pattern_paths.iter() {
            paths.push(path.clone());
        }
    }

    fn walk_patterns(&self, pattern: &Pattern, paths: &mut UniquePaths) {
        for pattern_string in &pattern.patterns {
            for absolute_path in pattern::glob(&self.base_directory, pattern_string) {
                if absolute_path.is_file() {
                    let relative = absolute_path
                        .strip_prefix(&self.base_directory)
                        .unwrap_or(&absolute_path)
                        .to_path_buf();
                    if pattern.matches(&relative) {
                        paths.push(relative);
                    }
                } else if absolute_path.is_dir() {
                    let directory_pattern = format!("**/*{}", pattern.ext);
                    let mut files = pattern::glob(&absolute_path, &directory_pattern);
                    files.sort();
                    for file in files {
                        if !file.is_file() {
                            continue;
                        }
                        let relative = file
                            .strip_prefix(&self.base_directory)
                            .unwrap_or(&file)
                            .to_path_buf();
                        if !pattern.is_ignored(&relative) {
                            paths.push(relative);
                        }
                    }
                }
            }
        }
    }

    pub fn source_paths(&self, pattern: &Pattern) -> Vec<PathBuf> {
        let mut paths = UniquePaths::default();
        self.each_path_in_patterns(pattern, &mut paths);
        paths.into_paths()
    }

    pub fn each_path_in_target(&self, target: &Target) -> Vec<PathBuf> {
        let mut paths = UniquePaths::default();
        for group in &target.groups {
            self.each_path_in_patterns(&group.source_pattern, &mut paths);
            self.each_path_in_patterns(&group.inline_source_pattern, &mut paths);
            self.each_path_in_patterns(&group.signature_pattern, &mut paths);
        }
        self.each_path_in_patterns(&target.source_pattern, &mut paths);
        self.each_path_in_patterns(&target.inline_source_pattern, &mut paths);
        self.each_path_in_patterns(&target.signature_pattern, &mut paths);
        paths.into_paths()
    }
}

#[derive(Default)]
pub struct UniquePaths {
    paths: Vec<PathBuf>,
    seen: HashSet<PathBuf>,
}

impl UniquePaths {
    pub fn push(&mut self, path: PathBuf) {
        if self.seen.insert(path.clone()) {
            self.paths.push(path);
        }
    }

    pub fn contains(&self, path: &Path) -> bool {
        self.seen.contains(path)
    }

    pub fn into_paths(self) -> Vec<PathBuf> {
        self.paths
    }
}
