use std::{fmt, fs, path::Path};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RbsVersion {
    Version4_1,
    #[default]
    Version4_2,
}

impl RbsVersion {
    pub const ALL: [RbsVersion; 2] = [RbsVersion::Version4_1, RbsVersion::Version4_2];

    pub fn of_release(release: &str) -> Option<RbsVersion> {
        let segments = release_segments(release)?;
        RbsVersion::ALL
            .into_iter()
            .find(|version| segments.len() == 3 && segments[..2] == version.major_and_minor())
    }

    fn major_and_minor(self) -> [u64; 2] {
        match self {
            RbsVersion::Version4_1 => [4, 1],
            RbsVersion::Version4_2 => [4, 2],
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            RbsVersion::Version4_1 => "4.1",
            RbsVersion::Version4_2 => "4.2",
        }
    }

    pub fn supported_names() -> String {
        RbsVersion::ALL
            .iter()
            .map(|version| format!("{}.x", version.name()))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

impl fmt::Display for RbsVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

pub fn release_segments(release: &str) -> Option<Vec<u64>> {
    release
        .split('.')
        .map(|segment| segment.parse().ok())
        .collect()
}

pub fn release_of_gem_directory(directory: &Path) -> Option<String> {
    let version_file = fs::read_to_string(directory.join("lib/rbs/version.rb")).ok();
    let from_version_file = version_file.as_deref().and_then(|content| {
        content.lines().find_map(|line| {
            line.trim()
                .strip_prefix("VERSION = \"")?
                .strip_suffix('"')
                .map(str::to_string)
        })
    });
    from_version_file.or_else(|| {
        directory
            .file_name()?
            .to_str()?
            .strip_prefix("rbs-")
            .map(str::to_string)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_release_belongs_to_the_version_of_its_major_and_minor() {
        assert_eq!(
            RbsVersion::of_release("4.1.3"),
            Some(RbsVersion::Version4_1)
        );
        assert_eq!(
            RbsVersion::of_release("4.1.0"),
            Some(RbsVersion::Version4_1)
        );
        assert_eq!(
            RbsVersion::of_release("4.2.0"),
            Some(RbsVersion::Version4_2)
        );
        assert_eq!(RbsVersion::of_release("4.0.3"), None);
        assert_eq!(RbsVersion::of_release("4.3.0"), None);
        assert_eq!(RbsVersion::of_release("4.2.0.pre.1"), None);
        assert_eq!(RbsVersion::of_release("4.2"), None);
    }
}
