use std::path::{Path, PathBuf};

use globset::{GlobBuilder, GlobMatcher};
use ruby_environment::RubyEnvironment;

use crate::position::Encoding;

#[derive(Debug, Clone)]
pub struct IndexablePath {
    pub path: PathBuf,
    pub require_path: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Configuration {
    pub workspace_path: PathBuf,
    pub encoding: Encoding,
    pub excluded_gems: Vec<String>,
    pub included_gems: Vec<String>,
    pub excluded_patterns: Vec<String>,
    pub included_patterns: Vec<String>,
    pub excluded_magic_comments: Vec<String>,
}

impl Default for Configuration {
    fn default() -> Self {
        Configuration {
            workspace_path: std::env::current_dir().unwrap_or_default(),
            encoding: Encoding::Utf8,
            excluded_gems: Vec::new(),
            included_gems: Vec::new(),
            excluded_patterns: vec![
                "**/{test,spec}/**/{*_test.rb,test_*.rb,*_spec.rb}".to_string(),
                "**/fixtures/**/*".to_string(),
            ],
            included_patterns: Vec::new(),
            excluded_magic_comments: [
                "frozen_string_literal:",
                "typed:",
                "compiled:",
                "encoding:",
                "shareable_constant_value:",
                "warn_indent:",
                "rubocop:",
                "nodoc:",
                "doc:",
                "coding:",
                "warn_past_scope:",
            ]
            .iter()
            .map(|comment| comment.to_string())
            .collect(),
        }
    }
}

fn glob_matcher(pattern: &str) -> Option<GlobMatcher> {
    GlobBuilder::new(pattern).literal_separator(true).build().ok().map(|glob| glob.compile_matcher())
}

impl Configuration {
    /// Ruby LSP builds `/^#\s*a:|b:|c:/`, so only the first magic comment is anchored to the start of the comment
    pub fn matches_magic_comment(&self, comment: &str) -> bool {
        let mut alternatives = self.excluded_magic_comments.iter();
        let Some(first) = alternatives.next() else { return false };
        let anchored = comment
            .strip_prefix('#')
            .is_some_and(|rest| rest.trim_start_matches([' ', '\t', '\n', '\r', '\x0c', '\x0b']).starts_with(first.as_str()));
        anchored || alternatives.any(|alternative| comment.contains(alternative.as_str()))
    }

    pub fn initialize_from_environment(&mut self, environment: &RubyEnvironment) {
        self.excluded_gems = environment.initial_excluded_gems.clone();
        if let Some(bundle_path) = &environment.bundle_path {
            let glob = bundle_path.replace('\\', "/");
            let glob = glob.trim_end_matches('/');
            self.excluded_patterns.push(format!("{glob}/**/*.rb"));
        }

        let excluded_directories = ["tmp", "node_modules", "sorbet"];
        let mut top_level_directories: Vec<String> = std::fs::read_dir(&self.workspace_path)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .filter(|entry| entry.path().is_dir())
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .filter(|name| !name.starts_with('.') && !excluded_directories.contains(&name.as_str()))
                    .collect()
            })
            .unwrap_or_default();
        top_level_directories.sort();
        self.included_patterns = vec![format!("{{{}}}/**/*.rb", top_level_directories.join(",")), "*.rb".to_string()];
    }

    pub fn apply_config(&mut self, config: &serde_json::Value) -> Result<(), String> {
        let Some(object) = config.as_object() else { return Ok(()) };
        let mut errors = Vec::new();
        for (key, value) in object {
            let snake_key: String = key
                .chars()
                .flat_map(|character| {
                    if character.is_ascii_uppercase() {
                        vec!['_', character.to_ascii_lowercase()]
                    } else {
                        vec![character]
                    }
                })
                .collect();
            if !["excluded_gems", "included_gems", "excluded_patterns", "included_patterns", "excluded_magic_comments"]
                .contains(&snake_key.as_str())
            {
                errors.push(format!("Unknown configuration option: {snake_key}"));
                continue;
            }
            let Some(values) = value.as_array() else {
                let ruby_class = match value {
                    serde_json::Value::Null => "NilClass",
                    serde_json::Value::Bool(true) => "TrueClass",
                    serde_json::Value::Bool(false) => "FalseClass",
                    serde_json::Value::Number(number) if number.is_f64() => "Float",
                    serde_json::Value::Number(_) => "Integer",
                    serde_json::Value::String(_) => "String",
                    serde_json::Value::Array(_) => "Array",
                    serde_json::Value::Object(_) => "Hash",
                };
                errors.push(format!("Expected {snake_key} to be a Array, but got {ruby_class}"));
                continue;
            };
            let strings: Vec<String> = values.iter().filter_map(|value| value.as_str().map(str::to_string)).collect();
            match snake_key.as_str() {
                "excluded_gems" => self.excluded_gems.extend(strings),
                "included_gems" => self.included_gems.extend(strings),
                "excluded_patterns" => self.excluded_patterns.extend(strings),
                "included_patterns" => self.included_patterns.extend(strings),
                _ => self.excluded_magic_comments.extend(strings),
            }
        }
        if errors.is_empty() { Ok(()) } else { Err(errors.join("\n")) }
    }

    fn absolute_pattern(&self, pattern: &str) -> String {
        if Path::new(pattern).is_absolute() {
            pattern.to_string()
        } else {
            format!("{}/{pattern}", self.workspace_path.to_string_lossy())
        }
    }

    pub fn indexable_paths(&self, environment: &RubyEnvironment) -> Vec<IndexablePath> {
        let excluded_gems: Vec<&String> =
            self.excluded_gems.iter().filter(|gem| !self.included_gems.contains(gem)).collect();
        let mut paths: Vec<IndexablePath> = Vec::new();

        for pattern in &self.included_patterns {
            let Some(matcher) = glob_matcher(&self.absolute_pattern(pattern)) else { continue };
            let max_depth = if pattern.contains("**") { usize::MAX } else { pattern.matches('/').count() + 1 };
            for entry in walkdir::WalkDir::new(&self.workspace_path)
                .max_depth(max_depth)
                .into_iter()
                .filter_entry(|entry| {
                    entry.depth() != 1
                        || !entry.file_type().is_dir()
                        || !["node_modules", "tmp", ".git"].contains(&entry.file_name().to_string_lossy().as_ref())
                })
                .filter_map(Result::ok)
            {
                if !entry.file_type().is_file() || !matcher.is_match(entry.path()) {
                    continue;
                }
                let path = entry.path().to_path_buf();
                let path_string = path.to_string_lossy();
                let load_path_entry = environment.load_path.iter().find(|load_path| path_string.starts_with(load_path.as_str()));
                let require_path = load_path_entry.map(|entry| crate::uri::require_path(&path_string, entry));
                paths.push(IndexablePath { path, require_path });
            }
        }

        let excluded_matchers: Vec<GlobMatcher> =
            self.excluded_patterns.iter().filter_map(|pattern| glob_matcher(&self.absolute_pattern(pattern))).collect();
        let fixtures_matcher = glob_matcher("**/fixtures/**/*");
        let bundle_path = environment.bundle_path.as_ref().map(|path| path.replace('\\', "/"));
        paths.retain(|indexable| {
            let path = indexable.path.to_string_lossy();
            let file_name = indexable.path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
            let ignored_from_exclusion = (file_name == "test_case.rb" || file_name == "test_helper.rb")
                && !fixtures_matcher.as_ref().is_some_and(|matcher| matcher.is_match(path.as_ref()))
                && bundle_path.as_ref().is_none_or(|bundle| !path.starts_with(bundle.as_str()));
            if ignored_from_exclusion {
                return true;
            }
            !excluded_matchers.iter().any(|matcher| matcher.is_match(path.as_ref()))
        });

        if let Some(rubylibdir) = &environment.rubylibdir {
            let mut default_entries: Vec<PathBuf> =
                std::fs::read_dir(rubylibdir).map(|entries| entries.filter_map(Result::ok).map(|entry| entry.path()).collect()).unwrap_or_default();
            default_entries.sort();
            for default_path in default_entries {
                let file_name = default_path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
                let short_name = file_name.strip_suffix(".rb").unwrap_or(&file_name).to_string();
                if excluded_gems.iter().any(|gem| **gem == short_name) || environment.default_gems_in_bundle.contains(&short_name) {
                    continue;
                }
                if default_path.is_dir() {
                    for entry in walkdir::WalkDir::new(&default_path).into_iter().filter_map(Result::ok) {
                        if entry.file_type().is_file() && entry.path().extension().is_some_and(|extension| extension == "rb") {
                            let path = entry.path().to_path_buf();
                            let require_path = Some(crate::uri::require_path(&path.to_string_lossy(), rubylibdir));
                            paths.push(IndexablePath { path, require_path });
                        }
                    }
                } else if default_path.extension().is_some_and(|extension| extension == "rb") {
                    let require_path = Some(crate::uri::require_path(&default_path.to_string_lossy(), rubylibdir));
                    paths.push(IndexablePath { path: default_path, require_path });
                }
            }
        }

        let workspace = self.workspace_path.to_string_lossy();
        for spec in &environment.locked_gems {
            if excluded_gems.iter().any(|gem| **gem == spec.name) || spec.full_gem_path == workspace.as_ref() {
                continue;
            }
            for require_path in &spec.require_paths {
                let load_path_entry = format!("{}/{require_path}", spec.full_gem_path);
                for entry in walkdir::WalkDir::new(&load_path_entry).into_iter().filter_map(Result::ok) {
                    if entry.file_type().is_file() && entry.path().extension().is_some_and(|extension| extension == "rb") {
                        let path = entry.path().to_path_buf();
                        let require = Some(crate::uri::require_path(&path.to_string_lossy(), &load_path_entry));
                        paths.push(IndexablePath { path, require_path: require });
                    }
                }
            }
        }

        let mut seen = std::collections::HashSet::new();
        paths.retain(|indexable| seen.insert(indexable.path.clone()));
        paths
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_magic_comments_like_ruby_lsp_regex() {
        let configuration = Configuration::default();
        assert!(configuration.matches_magic_comment("# frozen_string_literal: true"));
        assert!(configuration.matches_magic_comment("# this is the doc: something"));
        assert!(!configuration.matches_magic_comment("# Hello world"));
        assert!(!configuration.matches_magic_comment("# text frozen_string_literal: true"));
    }
}
