use std::fmt;

use rbs_definition::{loader::RbsGem, rbs_version::RbsVersion};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SteepVersion {
    Version2_0_0,
    #[default]
    Version2_1_0,
}

impl SteepVersion {
    pub const ALL: [SteepVersion; 2] = [SteepVersion::Version2_0_0, SteepVersion::Version2_1_0];

    pub fn parse(version: &str) -> Option<SteepVersion> {
        SteepVersion::ALL
            .into_iter()
            .find(|candidate| candidate.name() == version)
    }

    pub fn name(self) -> &'static str {
        match self {
            SteepVersion::Version2_0_0 => "2.0.0",
            SteepVersion::Version2_1_0 => "2.1.0",
        }
    }

    pub fn oldest_rbs_version(self) -> RbsVersion {
        match self {
            SteepVersion::Version2_0_0 => RbsVersion::Version4_1,
            SteepVersion::Version2_1_0 => RbsVersion::Version4_2,
        }
    }

    pub fn check_rbs_gem(self, rbs_gem: &RbsGem) -> Result<(), String> {
        if rbs_gem.version >= self.oldest_rbs_version() {
            return Ok(());
        }
        Err(format!(
            "Steep {self} requires rbs {} or later, but {} is rbs {}; choose the rbs gem with --rbs-version, or Steep with --steep-version",
            self.oldest_rbs_version(),
            rbs_gem.directory.display(),
            rbs_gem.release.as_deref().unwrap_or(rbs_gem.version.name())
        ))
    }

    pub fn supported_names() -> String {
        SteepVersion::ALL
            .iter()
            .map(|version| version.name())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

impl fmt::Display for SteepVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}
