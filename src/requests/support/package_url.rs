use std::fmt;

use indexmap::IndexMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QualifierValue {
    Text(String),
    Checksums(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageUrl {
    pub package_type: String,
    pub namespace: Option<String>,
    pub name: String,
    pub version: Option<String>,
    pub qualifiers: Option<IndexMap<String, QualifierValue>>,
    pub subpath: Option<String>,
}

/// Ruby LSP raises `InvalidPackageURL` for malformed URLs, but a plain `ArgumentError` for invalid percent encodings
/// and missing names, which is not rescued by the document link listener
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackageUrlError {
    InvalidPackageUrl(String),
    Argument(String),
}

impl PackageUrl {
    pub fn new(
        package_type: &str,
        name: &str,
        namespace: Option<String>,
        version: Option<String>,
        qualifiers: Option<IndexMap<String, QualifierValue>>,
        subpath: Option<String>,
    ) -> Result<PackageUrl, PackageUrlError> {
        if package_type.is_empty() {
            return Err(PackageUrlError::Argument("type is required".to_string()));
        }
        if name.is_empty() {
            return Err(PackageUrlError::Argument("name is required".to_string()));
        }
        Ok(PackageUrl {
            package_type: package_type.to_lowercase(),
            namespace,
            name: name.to_string(),
            version,
            qualifiers,
            subpath,
        })
    }

    pub fn scheme(&self) -> &'static str {
        "pkg"
    }

    pub fn parse(string: &str) -> Result<PackageUrl, PackageUrlError> {
        let mut string = string.to_string();

        let subpath = match string.rsplit_once('#') {
            Some((remainder, subpath)) => {
                let mut segments = Vec::new();
                for segment in subpath.split('/') {
                    if segment.is_empty() || segment == "." || segment == ".." {
                        continue;
                    }
                    segments.push(decode_www_form_component(segment)?);
                }
                let subpath = segments.join("/");
                string = remainder.to_string();
                Some(subpath)
            }
            None => None,
        };

        let qualifiers = match string.rsplit_once('?') {
            Some((remainder, qualifiers)) => {
                let mut parsed = IndexMap::new();
                for pair in qualifiers.split('&') {
                    let Some((key, value)) = pair.split_once('=') else { continue };
                    let key = key.to_lowercase();
                    let value = decode_www_form_component(value)?;
                    if value.is_empty() {
                        continue;
                    }
                    let value = if key == "checksums" {
                        QualifierValue::Checksums(ruby_split(&value, ',').into_iter().map(str::to_string).collect())
                    } else {
                        QualifierValue::Text(value)
                    };
                    parsed.insert(key, value);
                }
                string = remainder.to_string();
                Some(parsed)
            }
            None => None,
        };

        string = match string.split_once(':') {
            Some(("pkg", remainder)) => remainder.to_string(),
            _ => return Err(PackageUrlError::InvalidPackageUrl("invalid or missing \"pkg:\" URL scheme".to_string())),
        };

        let trimmed = string.trim_start_matches('/');
        let trimmed = trimmed.strip_suffix('/').unwrap_or(trimmed);
        let (package_type, mut string) = match trimmed.split_once('/') {
            Some((package_type, remainder)) => (package_type.to_string(), remainder.to_string()),
            None => return Err(PackageUrlError::InvalidPackageUrl("invalid or missing package type".to_string())),
        };

        let version = match string.rsplit_once('@') {
            Some((remainder, version)) => {
                let version = decode_www_form_component(version)?;
                string = remainder.to_string();
                Some(version)
            }
            None => None,
        };

        let (name, namespace) = match string.rsplit_once('/') {
            Some((remainder, name)) => {
                let segments = ruby_split(remainder, '/')
                    .into_iter()
                    .map(decode_www_form_component)
                    .collect::<Result<Vec<String>, PackageUrlError>>()?;
                (decode_www_form_component(name)?, Some(segments.join("/")))
            }
            None => (decode_www_form_component(&string)?, None),
        };

        PackageUrl::new(&package_type, &name, namespace, version, qualifiers, subpath)
    }
}

impl fmt::Display for PackageUrl {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut purl = format!("pkg:{}/", self.package_type);

        match &self.namespace {
            Some(namespace) if !namespace.is_empty() => {
                let namespace = namespace.strip_prefix('/').unwrap_or(namespace);
                let namespace = namespace.strip_suffix('/').unwrap_or(namespace);
                let segments: Vec<String> =
                    namespace.split('/').filter(|segment| !segment.is_empty()).map(encode_www_form_component).collect();
                purl.push_str(&segments.join("/"));
                purl.push('/');
                let name = self.name.strip_prefix('/').unwrap_or(&self.name);
                let name = name.strip_suffix('/').unwrap_or(name);
                purl.push_str(&encode_www_form_component(name));
            }
            _ => purl.push_str(&encode_www_form_component(&self.name)),
        }

        if let Some(version) = self.version.as_ref().filter(|version| !version.is_empty()) {
            purl.push('@');
            purl.push_str(&encode_www_form_component(version));
        }

        if let Some(qualifiers) = self.qualifiers.as_ref().filter(|qualifiers| !qualifiers.is_empty()) {
            let mut list: Vec<String> = qualifiers
                .iter()
                .filter_map(|(key, value)| match value {
                    QualifierValue::Text(text) if text.is_empty() => None,
                    QualifierValue::Checksums(checksums) if checksums.is_empty() => None,
                    QualifierValue::Text(text) => Some(format!("{}={}", key.to_lowercase(), encode_www_form_component(text))),
                    QualifierValue::Checksums(checksums) => Some(format!("{}={}", key.to_lowercase(), checksums.join(","))),
                })
                .collect();
            if !list.is_empty() {
                list.sort();
                purl.push('?');
                purl.push_str(&list.join("&"));
            }
        }

        if let Some(subpath) = self.subpath.as_ref().filter(|subpath| !subpath.is_empty()) {
            let subpath = subpath.strip_prefix('/').unwrap_or(subpath);
            let subpath = subpath.strip_suffix('/').unwrap_or(subpath);
            let segments: Vec<String> = subpath
                .split('/')
                .filter(|segment| !segment.is_empty() && *segment != "." && *segment != "..")
                .map(encode_fragment_segment)
                .collect();
            if !segments.is_empty() {
                purl.push('#');
                purl.push_str(&segments.join("/"));
            }
        }

        formatter.write_str(&purl)
    }
}

/// `String#split` drops trailing empty fields, unlike `str::split`
fn ruby_split(text: &str, separator: char) -> Vec<&str> {
    let mut parts: Vec<&str> = text.split(separator).collect();
    while parts.last().is_some_and(|part| part.is_empty()) {
        parts.pop();
    }
    parts
}

pub fn decode_www_form_component(text: &str) -> Result<String, PackageUrlError> {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => decoded.push(b' '),
            b'%' => {
                let hex = bytes.get(index + 1..index + 3).filter(|hex| hex.iter().all(u8::is_ascii_hexdigit));
                let Some(hex) = hex else {
                    return Err(PackageUrlError::Argument(format!("invalid %-encoding ({text})")));
                };
                let hex = std::str::from_utf8(hex).expect("hex digits are ASCII");
                decoded.push(u8::from_str_radix(hex, 16).expect("hex digits were checked"));
                index += 3;
                continue;
            }
            byte => decoded.push(byte),
        }
        index += 1;
    }
    Ok(String::from_utf8_lossy(&decoded).into_owned())
}

pub fn encode_www_form_component(text: &str) -> String {
    let mut encoded = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'*' | b'-' | b'.' | b'_' | b'0'..=b'9' | b'A'..=b'Z' | b'a'..=b'z' => encoded.push(byte as char),
            b' ' => encoded.push('+'),
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

fn encode_fragment_segment(segment: &str) -> String {
    let mut encoded = String::with_capacity(segment.len());
    for character in segment.chars() {
        match character {
            '%' => encoded.push_str("%25"),
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '.' | '_' | '~' => encoded.push(character),
            _ => encoded.push_str(&format!("%{:02X}", character as u32)),
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_gem_package_urls() {
        let purl = PackageUrl::parse("pkg:gem/syntax_tree@6.2.0#lib/syntax_tree.rb:39").unwrap();
        assert_eq!(purl.package_type, "gem");
        assert_eq!(purl.name, "syntax_tree");
        assert_eq!(purl.namespace, None);
        assert_eq!(purl.version.as_deref(), Some("6.2.0"));
        assert_eq!(purl.qualifiers, None);
        assert_eq!(purl.subpath.as_deref(), Some("lib/syntax_tree.rb:39"));
    }

    #[test]
    fn parses_all_components() {
        let purl = PackageUrl::parse("pkg://NPM/%40scope/name@1.0%2B1?arch=x86&checksums=a,b&empty=#/./src/../a%20b/").unwrap();
        assert_eq!(purl.package_type, "npm");
        assert_eq!(purl.namespace.as_deref(), Some("@scope"));
        assert_eq!(purl.name, "name");
        assert_eq!(purl.version.as_deref(), Some("1.0+1"));
        let qualifiers = purl.qualifiers.as_ref().unwrap();
        assert_eq!(qualifiers.get("arch"), Some(&QualifierValue::Text("x86".to_string())));
        assert_eq!(qualifiers.get("checksums"), Some(&QualifierValue::Checksums(vec!["a".to_string(), "b".to_string()])));
        assert_eq!(qualifiers.get("empty"), None);
        assert_eq!(purl.subpath.as_deref(), Some("src/a b"));
        assert_eq!(purl.to_string(), "pkg:npm/%40scope/name@1.0%2B1?arch=x86&checksums=a,b#src/a%20b");
    }

    #[test]
    fn rejects_invalid_package_urls() {
        assert!(matches!(PackageUrl::parse("gem/foo"), Err(PackageUrlError::InvalidPackageUrl(_))));
        assert!(matches!(PackageUrl::parse("pkg:foo"), Err(PackageUrlError::InvalidPackageUrl(_))));
        assert!(matches!(PackageUrl::parse("pkg:gem/@1.0"), Err(PackageUrlError::Argument(_))));
        assert!(matches!(PackageUrl::parse("pkg:gem/foo#%zz"), Err(PackageUrlError::Argument(_))));
    }

    #[test]
    fn keeps_names_with_unexpected_characters() {
        let purl = PackageUrl::parse("pkg:gem/rubocop$1.78.0#:99").unwrap();
        assert_eq!(purl.name, "rubocop$1.78.0");
        assert_eq!(purl.version, None);
        assert_eq!(purl.subpath.as_deref(), Some(":99"));
    }
}
