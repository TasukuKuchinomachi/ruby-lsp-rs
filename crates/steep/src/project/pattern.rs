use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Pattern {
    pub patterns: Vec<String>,
    pub ignores: Vec<String>,
    pub prefixes: Vec<String>,
    pub ignore_prefixes: Vec<String>,
    pub ext: String,
}

fn prefix_of(pattern: &str) -> String {
    if pattern == "." || pattern == "./" {
        return String::new();
    }
    let trimmed = pattern.strip_prefix("./").unwrap_or(pattern);
    let trimmed = trimmed.strip_suffix('/').unwrap_or(trimmed);
    format!("{trimmed}/")
}

impl Pattern {
    pub fn new(patterns: Vec<String>, ignores: Vec<String>, ext: &str) -> Pattern {
        Pattern {
            prefixes: patterns.iter().map(|pattern| prefix_of(pattern)).collect(),
            ignore_prefixes: ignores.iter().map(|pattern| prefix_of(pattern)).collect(),
            patterns,
            ignores,
            ext: ext.to_string(),
        }
    }

    pub fn matches(&self, path: &Path) -> bool {
        self.is_match(path) && !self.is_ignored(path)
    }

    pub fn is_match(&self, path: &Path) -> bool {
        self.test_string(path, &self.patterns, &self.prefixes)
    }

    pub fn is_ignored(&self, path: &Path) -> bool {
        self.test_string(path, &self.ignores, &self.ignore_prefixes)
    }

    fn test_string(&self, path: &Path, patterns: &[String], prefixes: &[String]) -> bool {
        let string = path.to_string_lossy();
        patterns.iter().any(|pattern| fnmatch(pattern, &string))
            || prefixes
                .iter()
                .any(|prefix| fnmatch(&format!("{prefix}**/*{}", self.ext), &string))
    }

    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }
}

pub fn fnmatch(pattern: &str, path: &str) -> bool {
    let pattern_segments: Vec<&str> = pattern.split('/').collect();
    let path_segments: Vec<&str> = path.split('/').collect();
    match_segments(&pattern_segments, &path_segments)
}

fn match_segments(patterns: &[&str], segments: &[&str]) -> bool {
    match patterns.split_first() {
        None => segments.is_empty(),
        Some((&"**", rest)) if !rest.is_empty() => {
            if match_segments(rest, segments) {
                return true;
            }
            for index in 0..segments.len() {
                if segments[index].starts_with('.') {
                    return false;
                }
                if match_segments(rest, &segments[index + 1..]) {
                    return true;
                }
            }
            false
        }
        Some((pattern, rest)) => match segments.split_first() {
            Some((segment, segments)) => {
                match_segment(pattern.as_bytes(), segment.as_bytes(), true)
                    && match_segments(rest, segments)
            }
            None => false,
        },
    }
}

fn match_segment(pattern: &[u8], text: &[u8], at_start: bool) -> bool {
    match pattern.split_first() {
        None => text.is_empty(),
        Some((b'*', rest)) => {
            if at_start && text.first() == Some(&b'.') {
                return false;
            }
            (0..=text.len()).any(|index| match_segment(rest, &text[index..], false))
        }
        Some((b'?', rest)) => {
            if at_start && text.first() == Some(&b'.') {
                return false;
            }
            !text.is_empty() && match_segment(rest, &text[1..], false)
        }
        Some((b'[', rest)) => {
            let Some(close) = rest.iter().skip(1).position(|byte| *byte == b']') else {
                return text.first() == Some(&b'[') && match_segment(rest, &text[1..], false);
            };
            let close = close + 1;
            let Some((character, text_rest)) = text.split_first() else {
                return false;
            };
            let (negated, set) = match rest[..close].split_first() {
                Some((b'!' | b'^', set)) => (true, set),
                _ => (false, &rest[..close]),
            };
            let mut matched = false;
            let mut index = 0;
            while index < set.len() {
                if index + 2 < set.len() && set[index + 1] == b'-' {
                    if set[index] <= *character && *character <= set[index + 2] {
                        matched = true;
                    }
                    index += 3;
                } else {
                    if set[index] == *character {
                        matched = true;
                    }
                    index += 1;
                }
            }
            matched != negated && match_segment(&rest[close + 1..], text_rest, false)
        }
        Some((b'\\', rest)) if !rest.is_empty() => {
            text.first() == Some(&rest[0]) && match_segment(&rest[1..], &text[1..], false)
        }
        Some((byte, rest)) => text.first() == Some(byte) && match_segment(rest, &text[1..], false),
    }
}

fn walk(base: &Path, relative: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(base.join(relative)) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let name = entry.file_name();
        let path = relative.join(&name);
        files.push(path.clone());
        if entry.file_type().is_ok_and(|file_type| file_type.is_dir()) {
            walk(base, &path, files);
        }
    }
}

// Why not `base.join(prefix).exists()`: the walk compares names exactly and does not
// descend into symbolic links, so a prefix has to be found in the same way.
fn literal_prefix(base: &Path, segments: &[&str]) -> Option<PathBuf> {
    let mut prefix = PathBuf::new();
    for (index, segment) in segments.iter().enumerate() {
        let entry = fs::read_dir(base.join(&prefix))
            .ok()?
            .flatten()
            .find(|entry| entry.file_name() == **segment)?;
        let is_last = index + 1 == segments.len();
        if !is_last && !entry.file_type().is_ok_and(|file_type| file_type.is_dir()) {
            return None;
        }
        prefix.push(segment);
    }
    Some(prefix)
}

pub fn glob(base: &Path, pattern: &str) -> Vec<PathBuf> {
    let pattern = pattern.strip_prefix("./").unwrap_or(pattern);
    let pattern = pattern.strip_suffix('/').unwrap_or(pattern);
    if pattern.is_empty() || pattern == "." {
        return vec![base.to_path_buf()];
    }
    let segments: Vec<&str> = pattern.split('/').collect();
    let literal_count = segments
        .iter()
        .take_while(|segment| !segment.contains(['*', '?', '[', '\\']))
        .count();
    let mut candidates = Vec::new();
    if let Some(prefix) = literal_prefix(base, &segments[..literal_count]) {
        if literal_count == segments.len() {
            candidates.push(prefix);
        } else {
            walk(base, &prefix, &mut candidates);
        }
    }
    let mut matched: Vec<PathBuf> = candidates
        .into_iter()
        .filter(|path| fnmatch(pattern, &path.to_string_lossy()))
        .map(|path| base.join(path))
        .collect();
    matched.sort();
    matched
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_a_directory_pattern_with_the_files_of_the_extension_under_it() {
        let pattern = Pattern::new(vec!["lib".to_string()], Vec::new(), ".rb");
        assert!(pattern.matches(Path::new("lib/user.rb")));
        assert!(pattern.matches(Path::new("lib/models/user.rb")));
        assert!(!pattern.matches(Path::new("lib/user.rbs")));
        assert!(!pattern.matches(Path::new("library/user.rb")));
    }

    #[test]
    fn excludes_ignored_paths() {
        let pattern = Pattern::new(
            vec!["lib".to_string()],
            vec!["lib/generated".to_string()],
            ".rb",
        );
        assert!(pattern.is_match(Path::new("lib/generated/user.rb")));
        assert!(pattern.is_ignored(Path::new("lib/generated/user.rb")));
        assert!(!pattern.matches(Path::new("lib/generated/user.rb")));
    }

    #[test]
    fn matches_wildcards_as_file_name_matching_does() {
        assert!(fnmatch("*.rb", "user.rb"));
        assert!(!fnmatch("*.rb", "lib/user.rb"));
        assert!(!fnmatch("*.rb", ".hidden.rb"));
        assert!(fnmatch("**/*.rb", "lib/models/user.rb"));
        assert!(fnmatch("**/*.rb", "user.rb"));
        assert!(!fnmatch("**/*.rb", ".bundle/user.rb"));
        assert!(fnmatch("lib/use?.rb", "lib/user.rb"));
        assert!(fnmatch("lib/[a-v]ser.rb", "lib/user.rb"));
        assert!(!fnmatch("lib/[!u]ser.rb", "lib/user.rb"));
    }
}
