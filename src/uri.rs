use std::path::{Path, PathBuf};

fn is_safe_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"-_.!~*'();/?@&=+$,[]".contains(&byte)
}

fn escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for byte in text.bytes() {
        if is_safe_byte(byte) {
            escaped.push(byte as char);
        } else {
            escaped.push_str(&format!("%{byte:02X}"));
        }
    }
    escaped
}

pub fn unescape(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len()
            && let Some(value) = std::str::from_utf8(&bytes[index + 1..index + 3])
                .ok()
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
            {
                decoded.push(value);
                index += 3;
                continue;
            }
        decoded.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

pub fn from_path(path: &str) -> String {
    let is_windows_drive = path.len() >= 2 && path.as_bytes()[0].is_ascii_alphabetic() && path.as_bytes()[1] == b':';
    let escaped = if is_windows_drive {
        escape(&format!("/{path}"))
    } else if let Some(stripped) = path.strip_prefix("//?") {
        escape(stripped)
    } else {
        escape(path)
    };
    format!("file://{escaped}")
}

pub fn from_path_buf(path: &Path) -> String {
    from_path(&path.to_string_lossy())
}

/// Returns `None` for URIs without a file system path, like `untitled:Untitled-1`.
pub fn to_path(uri: &str) -> Option<String> {
    let rest = uri.strip_prefix("file://")?;
    let path_part = rest.split(['#', '?']).next().unwrap_or("");
    let path_part = match path_part.find('/') {
        Some(0) => path_part,
        Some(slash) => &path_part[slash..],
        None => return None,
    };
    let unescaped = unescape(path_part);
    let bytes = unescaped.as_bytes();
    if bytes.len() >= 3 && bytes[0] == b'/' && bytes[1].is_ascii_alphabetic() && bytes[2] == b':' {
        Some(unescaped[1..].to_string())
    } else {
        Some(unescaped)
    }
}

pub fn to_path_buf(uri: &str) -> Option<PathBuf> {
    to_path(uri).map(PathBuf::from)
}

pub fn scheme(uri: &str) -> &str {
    uri.split(':').next().unwrap_or("")
}

/// Client URIs may escape characters differently from ours, so file URIs are normalized through their path.
pub fn normalize(uri: &str) -> String {
    match to_path(uri) {
        Some(path) if scheme(uri) == "file" => from_path(&path),
        _ => uri.to_string(),
    }
}

pub fn require_path(path: &str, load_path_entry: &str) -> String {
    let prefix = format!("{load_path_entry}/");
    let without_prefix = path.strip_prefix(&prefix).unwrap_or(path);
    without_prefix.strip_suffix(".rb").unwrap_or(without_prefix).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_paths() {
        let uri = from_path("/foo/bar baz:qux.rb");
        assert_eq!(uri, "file:///foo/bar%20baz%3Aqux.rb");
        assert_eq!(to_path(&uri).unwrap(), "/foo/bar baz:qux.rb");
        assert_eq!(from_path("C:/foo.rb"), "file:///C%3A/foo.rb");
        assert_eq!(to_path("file:///C%3A/foo.rb").unwrap(), "C:/foo.rb");
        assert_eq!(to_path("untitled:Untitled-1"), None);
        assert_eq!(normalize("file:///foo/%40bar.rb"), "file:///foo/@bar.rb");
    }
}
