use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Diagnostic {
    pub range: Range,
    #[serde(default = "error_severity")]
    pub severity: String,
    pub message: String,
    pub code: String,
}

fn error_severity() -> String {
    "ERROR".to_string()
}

impl Diagnostic {
    fn sort_key(&self) -> (&Position, &Position, &str, &str, &str) {
        (
            &self.range.start,
            &self.range.end,
            &self.code,
            &self.severity,
            &self.message,
        )
    }
}

impl Ord for Diagnostic {
    fn cmp(&self, other: &Self) -> Ordering {
        self.sort_key().cmp(&other.sort_key())
    }
}

impl PartialOrd for Diagnostic {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Serialize, Deserialize)]
struct FileEntry {
    file: String,
    diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Expectations {
    pub diagnostics: BTreeMap<String, Vec<Diagnostic>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Expected,
    Missing,
    Unexpected,
}

impl Expectations {
    pub fn load(content: &str) -> Result<Self, String> {
        let entries: Vec<FileEntry> =
            serde_yaml::from_str(content).map_err(|error| error.to_string())?;
        let mut diagnostics = BTreeMap::new();
        for mut entry in entries {
            entry.diagnostics.sort();
            diagnostics.insert(entry.file, entry.diagnostics);
        }
        Ok(Self { diagnostics })
    }

    pub fn to_yaml(&self) -> String {
        let entries: Vec<FileEntry> = self
            .diagnostics
            .iter()
            .filter(|(_, diagnostics)| !diagnostics.is_empty())
            .map(|(file, diagnostics)| {
                let mut diagnostics = diagnostics.clone();
                diagnostics.sort();
                FileEntry {
                    file: file.clone(),
                    diagnostics,
                }
            })
            .collect();
        if entries.is_empty() {
            return "--- []\n".to_string();
        }
        let body = serde_yaml::to_string(&entries).expect("expectations are serializable");
        format!("---\n{body}")
    }

    pub fn diagnostic_count(&self) -> usize {
        self.diagnostics.values().map(Vec::len).sum()
    }

    pub fn files(&self) -> impl Iterator<Item = &str> {
        self.diagnostics.keys().map(String::as_str)
    }

    pub fn diagnostics_of(&self, file: &str) -> &[Diagnostic] {
        self.diagnostics.get(file).map_or(&[], Vec::as_slice)
    }
}

pub fn compare<'diagnostics>(
    expected: &'diagnostics [Diagnostic],
    actual: &'diagnostics [Diagnostic],
) -> Vec<(Outcome, &'diagnostics Diagnostic)> {
    let expected: BTreeSet<&Diagnostic> = expected.iter().collect();
    let actual: BTreeSet<&Diagnostic> = actual.iter().collect();
    expected
        .union(&actual)
        .map(|diagnostic| {
            let outcome = match (expected.contains(diagnostic), actual.contains(diagnostic)) {
                (true, true) => Outcome::Expected,
                (true, false) => Outcome::Missing,
                _ => Outcome::Unexpected,
            };
            (outcome, *diagnostic)
        })
        .collect()
}
