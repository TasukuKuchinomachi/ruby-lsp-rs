use std::{
    cmp::Ordering,
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, PartialEq, Eq)]
enum Segment {
    Number(String),
    String(String),
}

impl Segment {
    fn is_zero(&self) -> bool {
        matches!(self, Segment::Number(digits) if digits.is_empty())
    }
}

fn compare_numbers(left: &str, right: &str) -> Ordering {
    left.len().cmp(&right.len()).then_with(|| left.cmp(right))
}

#[derive(Clone, Debug)]
pub struct Version {
    version: String,
}

impl Version {
    pub fn is_correct(string: &str) -> bool {
        let string = string.trim();
        if string.is_empty() {
            return true;
        }
        let (release, prerelease) = match string.split_once('-') {
            Some((release, prerelease)) => (release, Some(prerelease)),
            None => (string, None),
        };
        let mut parts = release.split('.');
        let first_is_number = parts.next().is_some_and(|first| {
            !first.is_empty() && first.bytes().all(|byte| byte.is_ascii_digit())
        });
        let rest_are_alphanumeric = parts
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_alphanumeric()));
        let prerelease_is_valid = prerelease.is_none_or(|prerelease| {
            prerelease.split('.').all(|part| {
                !part.is_empty()
                    && part
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            })
        });
        first_is_number && rest_are_alphanumeric && prerelease_is_valid
    }

    pub fn new(string: &str) -> Option<Version> {
        if !Version::is_correct(string) {
            return None;
        }
        let trimmed = string.trim();
        let version = if trimmed.is_empty() {
            "0".to_string()
        } else {
            trimmed.replace('-', ".pre.")
        };
        Some(Version { version })
    }

    pub fn as_str(&self) -> &str {
        &self.version
    }

    fn segments(&self) -> Vec<Segment> {
        let mut segments = Vec::new();
        let mut characters = self.version.chars().peekable();
        while let Some(&character) = characters.peek() {
            if character.is_ascii_digit() {
                let mut digits = String::new();
                while let Some(&digit) = characters.peek().filter(|digit| digit.is_ascii_digit()) {
                    digits.push(digit);
                    characters.next();
                }
                segments.push(Segment::Number(digits.trim_start_matches('0').to_string()));
            } else if character.is_ascii_alphabetic() {
                let mut letters = String::new();
                while let Some(&letter) = characters
                    .peek()
                    .filter(|letter| letter.is_ascii_alphabetic())
                {
                    letters.push(letter);
                    characters.next();
                }
                segments.push(Segment::String(letters));
            } else {
                characters.next();
            }
        }
        segments
    }

    fn canonical_segments(&self) -> Vec<Segment> {
        let segments = self.segments();
        let string_start = segments
            .iter()
            .position(|segment| matches!(segment, Segment::String(_)))
            .unwrap_or(segments.len());
        let (numeric, strings) = segments.split_at(string_start);
        let drop_trailing_zeros = |segments: &[Segment]| {
            let length = segments
                .iter()
                .rposition(|segment| !segment.is_zero())
                .map_or(0, |position| position + 1);
            segments[..length].to_vec()
        };
        let mut canonical = drop_trailing_zeros(numeric);
        canonical.extend(drop_trailing_zeros(strings));
        canonical
    }

    pub fn is_prerelease(&self) -> bool {
        self.version
            .chars()
            .any(|character| character.is_ascii_alphabetic())
    }

    pub fn release(&self) -> Version {
        if !self.is_prerelease() {
            return self.clone();
        }
        let mut segments = self.segments();
        while segments
            .iter()
            .any(|segment| matches!(segment, Segment::String(_)))
        {
            segments.pop();
        }
        Version::from_segments(&segments)
    }

    fn from_segments(segments: &[Segment]) -> Version {
        let strings: Vec<String> = segments
            .iter()
            .map(|segment| match segment {
                Segment::Number(digits) if digits.is_empty() => "0".to_string(),
                Segment::Number(digits) | Segment::String(digits) => digits.clone(),
            })
            .collect();
        Version::new(&strings.join(".")).expect("segments make a correct version")
    }

    fn bump(&self) -> Version {
        let mut segments = self.segments();
        while segments
            .iter()
            .any(|segment| matches!(segment, Segment::String(_)))
        {
            segments.pop();
        }
        if segments.len() > 1 {
            segments.pop();
        }
        if let Some(Segment::Number(digits)) = segments.last_mut() {
            *digits = increment(digits);
        }
        Version::from_segments(&segments)
    }
}

fn increment(digits: &str) -> String {
    let mut bytes: Vec<u8> = if digits.is_empty() {
        vec![b'0']
    } else {
        digits.bytes().collect()
    };
    for byte in bytes.iter_mut().rev() {
        if *byte == b'9' {
            *byte = b'0';
        } else {
            *byte += 1;
            return String::from_utf8(bytes).expect("digits are ASCII");
        }
    }
    bytes.insert(0, b'1');
    String::from_utf8(bytes).expect("digits are ASCII")
}

impl Ord for Version {
    fn cmp(&self, other: &Version) -> Ordering {
        let left = self.canonical_segments();
        let right = other.canonical_segments();
        let zero = Segment::Number(String::new());
        for index in 0..left.len().max(right.len()) {
            let left = left.get(index).unwrap_or(&zero);
            let right = right.get(index).unwrap_or(&zero);
            let ordering = match (left, right) {
                (Segment::Number(left), Segment::Number(right)) => compare_numbers(left, right),
                (Segment::String(left), Segment::String(right)) => left.cmp(right),
                (Segment::String(_), Segment::Number(_)) => Ordering::Less,
                (Segment::Number(_), Segment::String(_)) => Ordering::Greater,
            };
            if ordering != Ordering::Equal {
                return ordering;
            }
        }
        Ordering::Equal
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Version) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Version {
    fn eq(&self, other: &Version) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Version {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Operator {
    Equal,
    NotEqual,
    Greater,
    Less,
    GreaterOrEqual,
    LessOrEqual,
    Pessimistic,
}

#[derive(Clone, Debug)]
pub struct Requirement {
    operator: Operator,
    version: Version,
}

impl Requirement {
    pub fn parse(string: &str) -> Option<Requirement> {
        let string = string.trim();
        let operators = [
            (">=", Operator::GreaterOrEqual),
            ("<=", Operator::LessOrEqual),
            ("!=", Operator::NotEqual),
            ("~>", Operator::Pessimistic),
            ("=", Operator::Equal),
            (">", Operator::Greater),
            ("<", Operator::Less),
        ];
        let (operator, version) = operators
            .iter()
            .find_map(|(prefix, operator)| {
                string
                    .strip_prefix(prefix)
                    .map(|version| (*operator, version))
            })
            .unwrap_or((Operator::Equal, string));
        Some(Requirement {
            operator,
            version: Version::new(version)?,
        })
    }

    pub fn is_satisfied_by(&self, version: &Version) -> bool {
        let requirement = &self.version;
        match self.operator {
            Operator::Equal => version == requirement,
            Operator::NotEqual => version != requirement,
            Operator::Greater => version > requirement,
            Operator::Less => version < requirement,
            Operator::GreaterOrEqual => version >= requirement,
            Operator::LessOrEqual => version <= requirement,
            Operator::Pessimistic => {
                version >= requirement && version.release() < requirement.bump()
            }
        }
    }

    fn is_prerelease(&self) -> bool {
        self.version.is_prerelease()
    }
}

#[derive(Clone, Debug)]
pub struct Specification {
    pub name: String,
    pub version: Version,
    pub gem_directory: PathBuf,
}

#[derive(Debug, Default)]
pub struct Specifications {
    specifications: Vec<Specification>,
    activated: Vec<Specification>,
}

fn specification_files(directory: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|entry| {
            entry
                .file_name()
                .to_str()?
                .strip_suffix(".gemspec")
                .map(str::to_string)
        })
        .collect();
    names.sort();
    names
}

impl Specifications {
    pub fn new(gem_paths: &[PathBuf], activated: Vec<Specification>) -> Specifications {
        let mut specifications = Vec::new();
        for gem_path in gem_paths {
            for directory in [
                gem_path.join("specifications"),
                gem_path.join("specifications/default"),
            ] {
                for full_name in specification_files(&directory) {
                    specifications.extend(Specifications::parse_full_name(gem_path, &full_name));
                }
            }
        }
        Specifications {
            specifications,
            activated,
        }
    }

    fn parse_full_name(gem_path: &Path, full_name: &str) -> Option<Specification> {
        let parts: Vec<&str> = full_name.split('-').collect();
        let version_index = parts
            .iter()
            .position(|part| part.starts_with(|character: char| character.is_ascii_digit()))?;
        if version_index == 0 {
            return None;
        }
        let name = parts[..version_index].join("-");
        let version = Version::new(parts[version_index])?;
        Some(Specification {
            name,
            version,
            gem_directory: gem_path.join("gems").join(full_name),
        })
    }

    pub fn find_by_name(&self, name: &str, version: Option<&str>) -> Option<&Specification> {
        let requirement = match version {
            Some(version) => Some(Requirement::parse(version)?),
            None => None,
        };
        let satisfies = |specification: &&Specification| {
            specification.name == name
                && requirement
                    .as_ref()
                    .is_none_or(|requirement| requirement.is_satisfied_by(&specification.version))
        };
        if let Some(activated) = self.activated.iter().find(satisfies) {
            return Some(activated);
        }
        let matches: Vec<&Specification> = self.specifications.iter().filter(satisfies).collect();
        let prerelease = requirement.as_ref().is_some_and(Requirement::is_prerelease);
        let candidates: Vec<&Specification> = if prerelease {
            matches
        } else {
            let (prereleases, releases): (Vec<&Specification>, Vec<&Specification>) = matches
                .into_iter()
                .partition(|specification| specification.version.is_prerelease());
            if releases.is_empty() {
                prereleases
            } else {
                releases
            }
        };
        candidates.into_iter().reduce(|maximum, specification| {
            if specification.version > maximum.version {
                specification
            } else {
                maximum
            }
        })
    }
}
