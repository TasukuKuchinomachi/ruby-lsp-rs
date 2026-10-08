#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceUri {
    pub gem_name: String,
    pub gem_version: Option<String>,
    pub path: Option<String>,
    pub line_number: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidUriError(pub String);

impl SourceUri {
    pub fn parse(uri: &str) -> Result<SourceUri, InvalidUriError> {
        let invalid = || InvalidUriError(format!("bad URI(is not URI?): {uri:?}"));
        if !uri.is_ascii() {
            return Err(InvalidUriError(format!("URI must be ascii only {uri:?}")));
        }
        let rest = uri.strip_prefix("source://").ok_or_else(invalid)?;

        let (rest, fragment) = match rest.split_once('#') {
            Some((rest, fragment)) => (rest, Some(fragment)),
            None => (rest, None),
        };
        let (rest, query) = match rest.split_once('?') {
            Some((rest, query)) => (rest, Some(query)),
            None => (rest, None),
        };
        let (authority, path) = match rest.find('/') {
            Some(slash) => (&rest[..slash], &rest[slash..]),
            None => (rest, ""),
        };

        let host = parse_authority(authority).ok_or_else(invalid)?;
        if !path.split('/').all(|segment| valid_characters(segment, ":@"))
            || !query.is_none_or(|query| valid_characters(query, ":@/?"))
            || !fragment.is_none_or(|fragment| valid_characters(fragment, ":@/?"))
        {
            return Err(invalid());
        }

        let gem_name = if host == "localhost" { String::new() } else { host.to_string() };
        let (gem_version, path) = split_version_and_path(path);
        Ok(SourceUri { gem_name, gem_version, path, line_number: fragment.map(str::to_string) })
    }
}

fn parse_authority(authority: &str) -> Option<&str> {
    let host_and_port = match authority.rsplit_once('@') {
        Some((userinfo, host_and_port)) => {
            if !valid_characters(userinfo, ":") {
                return None;
            }
            host_and_port
        }
        None => authority,
    };

    if host_and_port.starts_with('[') {
        let closing = host_and_port.find(']')?;
        let port = &host_and_port[closing + 1..];
        if !port.is_empty() && !port.strip_prefix(':').is_some_and(|port| port.bytes().all(|byte| byte.is_ascii_digit())) {
            return None;
        }
        return Some(&host_and_port[..=closing]);
    }

    let (host, port) = match host_and_port.split_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (host_and_port, None),
    };
    if !valid_characters(host, "") || !port.is_none_or(|port| port.bytes().all(|byte| byte.is_ascii_digit())) {
        return None;
    }
    Some(host)
}

fn valid_characters(text: &str, extra_allowed: &str) -> bool {
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'%' {
            let valid_escape = bytes.get(index + 1).is_some_and(u8::is_ascii_hexdigit)
                && bytes.get(index + 2).is_some_and(u8::is_ascii_hexdigit);
            if !valid_escape {
                return false;
            }
            index += 3;
            continue;
        }
        let unreserved = byte.is_ascii_alphanumeric() || b"-._~".contains(&byte);
        let sub_delimiter = b"!$&'()*+,;=".contains(&byte);
        if !(unreserved || sub_delimiter || extra_allowed.as_bytes().contains(&byte)) {
            return false;
        }
        index += 1;
    }
    true
}

fn split_version_and_path(path: &str) -> (Option<String>, Option<String>) {
    let path = path.strip_prefix('/').unwrap_or(path);
    if path.is_empty() {
        return (None, None);
    }
    match path.split_once('/') {
        Some((gem_version, path)) => (Some(gem_version.to_string()), Some(path.to_string())),
        None => (Some(path.to_string()), None),
    }
}

/// Mirrors `CGI.unescape`, which also turns `+` into spaces, unlike URI percent decoding
pub fn cgi_unescape(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => decoded.push(b' '),
            b'%' if bytes.get(index + 1).is_some_and(u8::is_ascii_hexdigit)
                && bytes.get(index + 2).is_some_and(u8::is_ascii_hexdigit) =>
            {
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).expect("hex digits are ASCII");
                decoded.push(u8::from_str_radix(hex, 16).expect("hex digits were checked"));
                index += 3;
                continue;
            }
            byte => decoded.push(byte),
        }
        index += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(uri: &str) -> SourceUri {
        SourceUri::parse(uri).unwrap()
    }

    #[test]
    fn parses_gem_name_version_path_and_line_number() {
        let uri = parse("source://syntax_tree/6.2.0/lib/syntax_tree.rb#39");
        assert_eq!(uri.gem_name, "syntax_tree");
        assert_eq!(uri.gem_version.as_deref(), Some("6.2.0"));
        assert_eq!(uri.path.as_deref(), Some("lib/syntax_tree.rb"));
        assert_eq!(uri.line_number.as_deref(), Some("39"));
    }

    #[test]
    fn parses_links_without_version() {
        let uri = parse("source://open3//open3.rb#1");
        assert_eq!(uri.gem_name, "open3");
        assert_eq!(uri.gem_version.as_deref(), Some(""));
        assert_eq!(uri.path.as_deref(), Some("open3.rb"));
    }

    #[test]
    fn parses_links_without_host_or_path() {
        let uri = parse("source://#123");
        assert_eq!(uri.gem_name, "");
        assert_eq!(uri.gem_version, None);
        assert_eq!(uri.path, None);

        let uri = parse("source:///#123");
        assert_eq!(uri.gem_name, "");
        assert_eq!(uri.path, None);

        let uri = parse("source://erb/#1");
        assert_eq!(uri.gem_name, "erb");
        assert_eq!(uri.gem_version, None);
        assert_eq!(uri.path, None);
    }

    #[test]
    fn rejects_invalid_uris() {
        assert!(SourceUri::parse("source://some_file /#123").is_err());
        assert!(SourceUri::parse("source://foo/1.0/a.rb#1#2").is_err());
        assert!(SourceUri::parse("source://foo:bar/1.0/a.rb#1").is_err());
        assert!(SourceUri::parse("source://foo/1.0/%zz.rb#1").is_err());
        assert!(SourceUri::parse("source://foo/1.0/é.rb#1").is_err());
    }

    #[test]
    fn cgi_unescape_decodes_plus_and_percent_escapes() {
        assert_eq!(cgi_unescape("lib/a+b%2Fc%zz.rb"), "lib/a b/c%zz.rb");
    }
}
