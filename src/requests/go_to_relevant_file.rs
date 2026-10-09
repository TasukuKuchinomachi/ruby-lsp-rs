use std::path::Path;

use indexmap::IndexMap;
use serde_json::{Value, json};

use ruby_file_glob::{basename, dirname, extname, glob, split_path};
use super::{RequestContext, RequestResult, TextDocumentParams, parse_params};
use crate::listeners::definition::expand_path;

const TEST_KEYWORDS: &[&str] = &["test", "spec", "integration_test"];
const TEST_PREFIX_GLOB: &str = "test_,spec_,integration_test_";
const TEST_SUFFIX_GLOB: &str = "_test,_spec,_integration_test";

pub fn handle(context: &mut RequestContext<'_>, params: Value) -> RequestResult {
    let params: TextDocumentParams = parse_params(params)?;
    let workspace_path = context.global_state.workspace_path();
    let Some(path) = crate::uri::to_path(&params.text_document.uri) else { return Ok(Value::Null) };
    if !path.starts_with(&workspace_path) {
        return Ok(Value::Null);
    }

    Ok(json!({ "locations": find_relevant_paths(&path, &workspace_path) }))
}

/// Ruby LSP globs relative to the current directory of the server, which is the workspace. Relative paths are resolved
/// against the workspace path instead, since this process may run from another directory
pub fn find_relevant_paths(path: &str, workspace_path: &str) -> Vec<String> {
    let relative_path = path.strip_prefix(workspace_path).unwrap_or(path);
    let workspace = Path::new(workspace_path);
    let search_root = search_root(relative_path, workspace);

    let candidate_paths: Vec<String> = relevant_filename_patterns(relative_path)
        .iter()
        .flat_map(|pattern| glob(&format!("{search_root}/**/{pattern}"), workspace))
        .collect();

    if candidate_paths.is_empty() {
        return Vec::new();
    }

    find_most_similar_with_jaccard(relative_path, &candidate_paths)
        .into_iter()
        .map(|candidate| expand_path(&workspace.join(candidate)).to_string_lossy().into_owned())
        .collect()
}

fn search_root(relative_path: &str, workspace: &Path) -> String {
    let current_path = format!("./{}", relative_path.trim_start_matches('/'));
    let mut current_directory = dirname(&current_path);
    while current_directory != "." {
        let directory_basename = basename(&current_directory);
        if TEST_KEYWORDS.contains(&directory_basename.as_str()) {
            return dirname(&current_directory);
        }

        if let Ok(entries) = std::fs::read_dir(workspace.join(&current_directory)) {
            let contains_test_directory = entries.filter_map(Result::ok).any(|entry| {
                TEST_KEYWORDS.contains(&entry.file_name().to_string_lossy().as_ref()) && entry.path().is_dir()
            });
            if contains_test_directory {
                return current_directory;
            }
        }

        current_directory = dirname(&current_directory);
    }
    ".".to_string()
}

fn strip_test_affixes(basename: &str) -> Option<String> {
    let prefix = ["test_", "spec_", "integration_test_"].into_iter().find(|prefix| basename.starts_with(prefix));
    let without_prefix = prefix.map_or(basename, |prefix| &basename[prefix.len()..]);
    let suffix_start = ["_test", "_spec", "_integration_test"]
        .into_iter()
        .filter(|suffix| without_prefix.ends_with(suffix))
        .map(|suffix| without_prefix.len() - suffix.len())
        .min();

    if prefix.is_none() && suffix_start.is_none() {
        return None;
    }
    Some(without_prefix[..suffix_start.unwrap_or(without_prefix.len())].to_string())
}

fn relevant_filename_patterns(relative_path: &str) -> Vec<String> {
    let extension = extname(relative_path);
    let file_name = basename(relative_path);
    let input_basename = file_name.strip_suffix(extension.as_str()).filter(|name| !name.is_empty()).unwrap_or(&file_name);

    match strip_test_affixes(input_basename) {
        Some(base) => {
            let parent_directory = basename(&dirname(relative_path));
            if base.contains(&parent_directory) && base != parent_directory {
                vec![format!("{base}{extension}"), format!("{parent_directory}{extension}")]
            } else {
                vec![format!("{base}{extension}")]
            }
        }
        None => vec![
            format!("{{{TEST_PREFIX_GLOB}}}{input_basename}{extension}"),
            format!("{input_basename}{{{TEST_SUFFIX_GLOB}}}{extension}"),
            format!("{input_basename}/{{{TEST_PREFIX_GLOB}}}*{extension}"),
            format!("{input_basename}/*{{{TEST_SUFFIX_GLOB}}}{extension}"),
        ],
    }
}

fn directory_parts(path: &str) -> Vec<String> {
    let mut parts = split_path(&dirname(path));
    let mut seen = std::collections::HashSet::new();
    parts.retain(|part| seen.insert(part.clone()));
    parts
}

fn find_most_similar_with_jaccard(relative_path: &str, candidates: &[String]) -> Vec<String> {
    let directories = directory_parts(relative_path);
    let mut groups: IndexMap<u64, (f64, Vec<String>)> = IndexMap::new();

    for candidate in candidates {
        let other_directories = directory_parts(candidate);
        let intersection = directories.iter().filter(|part| other_directories.contains(part)).count();
        let union = directories.len() + other_directories.iter().filter(|part| !directories.contains(part)).count();
        let score = intersection as f64 / union as f64;
        groups.entry(score.to_bits()).or_insert_with(|| (score, Vec::new())).1.push(candidate.clone());
    }

    let mut best: Option<&(f64, Vec<String>)> = None;
    for group in groups.values() {
        if best.is_none_or(|best| group.0 > best.0) {
            best = Some(group);
        }
    }
    best.map(|(_, results)| results.clone()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_test_affixes_like_ruby_gsub() {
        assert_eq!(strip_test_affixes("foo_test").as_deref(), Some("foo"));
        assert_eq!(strip_test_affixes("test_foo").as_deref(), Some("foo"));
        assert_eq!(strip_test_affixes("foo_integration_test").as_deref(), Some("foo"));
        assert_eq!(strip_test_affixes("test_test").as_deref(), Some("test"));
        assert_eq!(strip_test_affixes("foo"), None);
    }
}
