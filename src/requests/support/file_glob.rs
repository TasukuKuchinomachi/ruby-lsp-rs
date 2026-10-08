use std::path::{Path, PathBuf};

/// Mirrors `File.fnmatch?(pattern, path, File::FNM_PATHNAME | File::FNM_EXTGLOB)`. The `globset` crate is not used
/// because it lets wildcards match leading periods, which Ruby only allows with `File::FNM_DOTMATCH`
pub fn fnmatch(pattern: &str, path: &str) -> bool {
    expand_braces(pattern).iter().any(|alternative| {
        let pattern_segments: Vec<&str> = alternative.split('/').collect();
        let path_segments: Vec<&str> = path.split('/').collect();
        match_segments(&pattern_segments, &path_segments)
    })
}

fn match_segments(pattern: &[&str], path: &[&str]) -> bool {
    let Some((first, rest)) = pattern.split_first() else { return path.is_empty() };

    if *first == "**" && !rest.is_empty() {
        if match_segments(rest, path) {
            return true;
        }
        return path.len() > 1 && !path[0].starts_with('.') && match_segments(pattern, &path[1..]);
    }

    match path.split_first() {
        Some((segment, path_rest)) => match_segment(first, segment) && match_segments(rest, path_rest),
        None => false,
    }
}

pub fn match_segment(pattern: &str, name: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let name: Vec<char> = name.chars().collect();
    if name.first() == Some(&'.') && !starts_with_literal_period(&pattern) {
        return false;
    }
    match_characters(&pattern, &name)
}

fn starts_with_literal_period(pattern: &[char]) -> bool {
    matches!(pattern, ['.', ..] | ['\\', '.', ..])
}

fn match_characters(pattern: &[char], name: &[char]) -> bool {
    let Some((first, rest)) = pattern.split_first() else { return name.is_empty() };
    match first {
        '*' => (0..=name.len()).any(|consumed| match_characters(rest, &name[consumed..])),
        '?' => !name.is_empty() && match_characters(rest, &name[1..]),
        '[' => match bracket_match(rest, name.first().copied()) {
            Some((true, after_bracket)) => match_characters(after_bracket, &name[1..]),
            Some((false, _)) => false,
            None => name.first() == Some(&'[') && match_characters(rest, &name[1..]),
        },
        '\\' if !rest.is_empty() => name.first() == Some(&rest[0]) && match_characters(&rest[1..], &name[1..]),
        literal => name.first() == Some(literal) && match_characters(rest, &name[1..]),
    }
}

fn bracket_match(pattern: &[char], character: Option<char>) -> Option<(bool, &[char])> {
    let (negated, mut index) = match pattern.first() {
        Some('!') | Some('^') => (true, 1),
        _ => (false, 0),
    };
    let mut matched = false;
    let mut first = true;
    while index < pattern.len() {
        if pattern[index] == ']' && !first {
            let found = character.is_some_and(|_| matched != negated);
            return Some((found, &pattern[index + 1..]));
        }
        first = false;
        let mut start = pattern[index];
        if start == '\\' && index + 1 < pattern.len() {
            index += 1;
            start = pattern[index];
        }
        if index + 2 < pattern.len() && pattern[index + 1] == '-' && pattern[index + 2] != ']' {
            let end = pattern[index + 2];
            if character.is_some_and(|character| start <= character && character <= end) {
                matched = true;
            }
            index += 3;
        } else {
            if character == Some(start) {
                matched = true;
            }
            index += 1;
        }
    }
    None
}

pub fn expand_braces(pattern: &str) -> Vec<String> {
    let characters: Vec<char> = pattern.chars().collect();
    let mut depth = 0;
    let mut open = None;
    let mut index = 0;
    while index < characters.len() {
        match characters[index] {
            '\\' => index += 1,
            '{' => {
                if depth == 0 {
                    open = Some(index);
                }
                depth += 1;
            }
            '}' if depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    let open = open.expect("an opening brace was found");
                    let prefix: String = characters[..open].iter().collect();
                    let suffix: String = characters[index + 1..].iter().collect();
                    let mut expanded = Vec::new();
                    for alternative in split_alternatives(&characters[open + 1..index]) {
                        expanded.extend(expand_braces(&format!("{prefix}{alternative}{suffix}")));
                    }
                    return expanded;
                }
            }
            _ => {}
        }
        index += 1;
    }
    vec![pattern.to_string()]
}

fn split_alternatives(characters: &[char]) -> Vec<String> {
    let mut alternatives = Vec::new();
    let mut current = String::new();
    let mut depth = 0;
    let mut index = 0;
    while index < characters.len() {
        let character = characters[index];
        match character {
            '\\' if index + 1 < characters.len() => {
                current.push(character);
                current.push(characters[index + 1]);
                index += 1;
            }
            '{' => {
                depth += 1;
                current.push(character);
            }
            '}' => {
                depth -= 1;
                current.push(character);
            }
            ',' if depth == 0 => alternatives.push(std::mem::take(&mut current)),
            _ => current.push(character),
        }
        index += 1;
    }
    alternatives.push(current);
    alternatives
}

fn has_magic(segment: &str) -> bool {
    segment.contains(['*', '?', '[', '\\'])
}

fn join_display(display: &str, name: &str) -> String {
    if display.is_empty() {
        name.to_string()
    } else if display.ends_with('/') {
        format!("{display}{name}")
    } else {
        format!("{display}/{name}")
    }
}

/// Written by hand instead of using a glob crate, because the order of the results must match `Dir.glob`: braces are
/// expanded first and each alternative is globbed separately, and `**/` does not descend into hidden or symlinked
/// directories
pub fn glob(pattern: &str, base_directory: &Path) -> Vec<String> {
    let mut results = Vec::new();
    for alternative in expand_braces(pattern) {
        let (directory, display, remaining) = match alternative.strip_prefix('/') {
            Some(remaining) => (PathBuf::from("/"), "/".to_string(), remaining.to_string()),
            None => (base_directory.to_path_buf(), String::new(), alternative.clone()),
        };
        let segments: Vec<&str> = remaining.split('/').filter(|segment| !segment.is_empty()).collect();
        glob_helper(&directory, &display, &[&segments], &mut results);
    }
    results
}

fn glob_helper(directory: &Path, display: &str, cursors: &[&[&str]], results: &mut Vec<String>) {
    if cursors.iter().any(|cursor| cursor.is_empty()) && !display.is_empty() {
        results.push(display.to_string());
    }

    let pending: Vec<&[&str]> = cursors.iter().copied().filter(|cursor| !cursor.is_empty()).collect();
    if pending.is_empty() || !directory.is_dir() {
        return;
    }

    let (literal, magic): (Vec<&[&str]>, Vec<&[&str]>) =
        pending.into_iter().partition(|cursor| cursor[0] != "**" && !has_magic(cursor[0]));

    for cursor in literal {
        let child = directory.join(cursor[0]);
        if child.symlink_metadata().is_ok() {
            glob_helper(&child, &join_display(display, cursor[0]), &[&cursor[1..]], results);
        }
    }

    if magic.is_empty() {
        return;
    }

    let Ok(entries) = std::fs::read_dir(directory) else { return };
    let mut names: Vec<(String, bool)> = entries
        .filter_map(Result::ok)
        .map(|entry| {
            let is_real_directory = entry.file_type().is_ok_and(|file_type| file_type.is_dir());
            (entry.file_name().to_string_lossy().into_owned(), is_real_directory)
        })
        .collect();
    names.sort_by(|left, right| left.0.as_bytes().cmp(right.0.as_bytes()));

    for (name, is_real_directory) in names {
        let mut next_cursors: Vec<&[&str]> = Vec::new();
        for cursor in &magic {
            if cursor[0] == "**" && cursor.len() > 1 {
                if is_real_directory && !name.starts_with('.') {
                    next_cursors.push(cursor);
                }
                if match_segment(cursor[1], &name) {
                    next_cursors.push(&cursor[2..]);
                }
            } else if match_segment(cursor[0], &name) {
                next_cursors.push(&cursor[1..]);
            }
        }
        if !next_cursors.is_empty() {
            glob_helper(&directory.join(&name), &join_display(display, &name), &next_cursors, results);
        }
    }
}

pub fn dirname(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return if path.starts_with('/') { "/".to_string() } else { ".".to_string() };
    }
    match trimmed.rfind('/') {
        Some(index) => {
            let parent = trimmed[..index].trim_end_matches('/');
            if parent.is_empty() { "/".to_string() } else { parent.to_string() }
        }
        None => ".".to_string(),
    }
}

pub fn basename(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return if path.starts_with('/') { "/".to_string() } else { String::new() };
    }
    trimmed.rsplit('/').next().unwrap_or(trimmed).to_string()
}

pub fn extname(path: &str) -> String {
    let name = basename(path);
    let without_leading_periods = name.trim_start_matches('.');
    match without_leading_periods.rfind('.') {
        Some(index) => without_leading_periods[index..].to_string(),
        None => String::new(),
    }
}

/// Unlike `str::split`, Ruby's `String#split` drops trailing empty strings
pub fn split_path(path: &str) -> Vec<String> {
    let mut parts: Vec<String> = path.split('/').map(str::to_string).collect();
    while parts.last().is_some_and(String::is_empty) {
        parts.pop();
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_like_ruby_fnmatch() {
        assert!(fnmatch("**/test/**/*", "/test/c.rb"));
        assert!(fnmatch("**/test/**/*", "test/c.rb"));
        assert!(!fnmatch("**/test/**/*", "/a/.b/test/c.rb"));
        assert!(!fnmatch("**/test/**/*", "/test/.c.rb"));
        assert!(fnmatch("**/spec/**/*_spec.rb", "/spec/a/b_spec.rb"));
        assert!(fnmatch("**/{test,spec,features}/**/{*_test.rb,test_*.rb,*_spec.rb,*.feature}", "/a/test/b/c_test.rb"));
        assert!(!fnmatch("**/{test,spec,features}/**/{*_test.rb,test_*.rb,*_spec.rb,*.feature}", "/tests/something.rb"));
        assert!(fnmatch("[a-c]?.rb", "bx.rb"));
        assert!(!fnmatch("[!a-c]?.rb", "bx.rb"));
    }

    #[test]
    fn mirrors_ruby_path_helpers() {
        assert_eq!(dirname("./lib"), ".");
        assert_eq!(dirname("/lib/foo.rb"), "/lib");
        assert_eq!(dirname("/foo.rb"), "/");
        assert_eq!(dirname("foo.rb"), ".");
        assert_eq!(basename("/lib/foo.rb"), "foo.rb");
        assert_eq!(extname("/lib/foo.rb"), ".rb");
        assert_eq!(extname("/lib/.bashrc"), "");
        assert_eq!(extname("/lib/foo."), ".");
        assert_eq!(extname("/lib/..rb"), "");
        assert_eq!(split_path("/a/b"), vec!["", "a", "b"]);
        assert_eq!(split_path("./a"), vec![".", "a"]);
        assert_eq!(expand_braces("{a,b}{c,d}"), vec!["ac", "ad", "bc", "bd"]);
    }
}
