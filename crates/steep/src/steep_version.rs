use std::fmt;

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
