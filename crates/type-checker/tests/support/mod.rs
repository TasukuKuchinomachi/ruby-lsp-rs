#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use rbs_environment::{Environment, Loader};
use type_checker::{Analysis, HoverKind, TypeHintKind, analyze};

pub const MARKER: char = '‸';

const FIXTURE_SIGNATURES: &str = r#"
class Greeter
  attr_reader name: String

  # Greets somebody
  def greet: (String target) -> String
  def initialize: (String name) -> void
  def maybe_name: () -> String?
  def tags: () -> Array[String]
  def count_for: (Integer value) -> Integer
  def wrong_body: () -> Integer
  def with_keywords: (name: String, ?age: Integer) -> String
  def each_tag: () { (String tag) -> void } -> void
  def loud: () -> void
  def self.default: () -> Greeter
  def self.with_scope: (^() [self: Greeter] -> void body) -> void
  type conditions[T] = Symbol | ^(T) [self: T] -> bool
  def self.check_if: (*untyped, ?if: conditions[instance], **untyped) -> void

  @cache: Hash[Symbol, Integer]
end

class Dynamic
  def method_missing: (Symbol name, *untyped arguments) -> untyped
end

module Shape
  def area: () -> Float
end

class Box
  include Shape

  def self.build: (Integer size) -> Box
  def size: () -> Integer
end

class Container[T]
  def initialize: (T value) -> void
  def value: () -> T
  def map_value: [U] () { (T) -> U } -> Container[U]
end

LIMIT: Integer

class Object
  def present?: () -> bool
  def blank?: () -> bool
end

class NilClass
  def present?: () -> false
  def blank?: () -> true
end

type summary = { name: String, tags: Array[String], note: String? }

class Report
  DEFAULT_TITLE: String
  def summary: () -> summary
end

class DailyReport < Report
end

class Uploader
  %a{pure}
  def small_file: () -> Uploader?
  def unstable_file: () -> Uploader?
  attr_reader parent: Uploader?
  def file: () -> String
end
"#;

pub fn core_directory() -> Option<PathBuf> {
    if let Ok(directory) = std::env::var("RBS_CORE_DIRECTORY") {
        return Some(PathBuf::from(directory));
    }
    let output = Command::new("ruby")
        .args(["-e", "print Gem::Specification.find_by_name('rbs').gem_dir"])
        .output()
        .ok()?;
    let directory = PathBuf::from(String::from_utf8(output.stdout).ok()?).join("core");
    (output.status.success() && directory.is_dir()).then_some(directory)
}

pub fn environment() -> Option<&'static Environment> {
    static ENVIRONMENT: OnceLock<Option<Environment>> = OnceLock::new();
    ENVIRONMENT
        .get_or_init(|| {
            let core_directory = core_directory()?;
            let mut loader = Loader::new();
            loader.add_library_directory(&core_directory);
            loader.add_source(Path::new("sig/fixture.rbs"), FIXTURE_SIGNATURES);
            Some(loader.finish())
        })
        .as_ref()
}

pub fn environment_with(signatures: &str) -> Option<Environment> {
    let core_directory = core_directory()?;
    let mut loader = Loader::new();
    loader.add_library_directory(&core_directory);
    loader.add_source(Path::new("sig/library.rbs"), signatures);
    Some(loader.finish())
}

#[macro_export]
macro_rules! environment_or_skip {
    () => {
        match support::environment() {
            Some(environment) => environment,
            None => {
                eprintln!("skipped: RBS core signatures are not found");
                return;
            }
        }
    };
}

/// Source with `‸` markers removed, and the byte offsets where the markers were
pub struct MarkedSource {
    pub source: String,
    pub markers: Vec<usize>,
}

pub fn marked(source_with_markers: &str) -> MarkedSource {
    let mut source = String::new();
    let mut markers = Vec::new();
    for character in source_with_markers.chars() {
        if character == MARKER {
            markers.push(source.len());
        } else {
            source.push(character);
        }
    }
    MarkedSource { source, markers }
}

pub struct Checked {
    pub analysis: Analysis,
    pub markers: Vec<usize>,
    pub source: String,
}

pub fn check(environment: &Environment, source_with_markers: &str) -> Checked {
    let MarkedSource { source, markers } = marked(source_with_markers);
    let analysis = analyze(environment, source.as_bytes());
    Checked {
        analysis,
        markers,
        source,
    }
}

impl Checked {
    pub fn marker(&self, index: usize) -> usize {
        self.markers[index]
    }

    pub fn hint(&self, marker_index: usize, kind: TypeHintKind) -> Option<String> {
        let offset = self.marker(marker_index);
        self.analysis
            .type_hints
            .iter()
            .find(|hint| hint.offset == offset && hint.kind == kind)
            .map(|hint| hint.hint_type.to_string())
    }

    pub fn is_obvious_hint(&self, marker_index: usize) -> bool {
        let offset = self.marker(marker_index);
        self.analysis
            .type_hints
            .iter()
            .any(|hint| hint.offset == offset && hint.is_obvious)
    }

    /// Type of the hover on the name that ends at the marker: the variable type, the return type of a method or the
    /// type of a constant
    pub fn hover_type(&self, marker_index: usize) -> Option<String> {
        let entry = self
            .analysis
            .hover_at(self.marker(marker_index).checked_sub(1)?)?;
        Some(match &entry.kind {
            HoverKind::Type(hover_type) => hover_type.to_string(),
            HoverKind::Method { return_type, .. } => return_type.to_string(),
            HoverKind::Constant { constant_type, .. } => constant_type.to_string(),
        })
    }

    pub fn diagnostic_codes(&self) -> Vec<&'static str> {
        self.analysis
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect()
    }

    pub fn messages_with_code(&self, code: &str) -> Vec<String> {
        self.analysis
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.code == code)
            .map(|diagnostic| diagnostic.message.clone())
            .collect()
    }

    pub fn diagnostic_messages(&self) -> Vec<String> {
        self.analysis
            .diagnostics
            .iter()
            .map(|diagnostic| format!("{}: {}", diagnostic.code, diagnostic.message))
            .collect()
    }

    /// Source text covered by the diagnostic with `code`
    pub fn diagnostic_text(&self, code: &str) -> Option<&str> {
        let diagnostic = self
            .analysis
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == code)?;
        self.source.get(diagnostic.start..diagnostic.end)
    }
}
