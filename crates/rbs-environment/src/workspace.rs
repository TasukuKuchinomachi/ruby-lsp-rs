use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::diagnostic_configuration::{
    DiagnosticConfiguration, DiagnosticSeverity, DiagnosticTemplate, ruby_diagnostic_codes,
};
use crate::environment::{Environment, LoadError};
use crate::loader::Loader;

/// Locations that only Ruby can tell, collected by the language server from its Ruby environment detection
#[derive(Debug, Clone, Default)]
pub struct WorkspaceSources {
    /// `<rbs gem>/core`
    pub core_directory: Option<PathBuf>,
    /// `<rbs gem>/stdlib`
    pub stdlib_directory: Option<PathBuf>,
    /// Gem name to `full_gem_path`, used for `rubygems` sources in `rbs_collection.lock.yaml`
    pub gem_directories: HashMap<String, PathBuf>,
}

/// The parts of a `Steepfile` that matter here. Without a `Steepfile` the defaults are `signature "sig"` and `check "."`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SteepfileConfiguration {
    /// Directories, files or globs such as `*.rbs`
    pub signature_patterns: Vec<PathBuf>,
    pub check_paths: Vec<PathBuf>,
    pub ignore_paths: Vec<PathBuf>,
    pub libraries: Vec<String>,
    pub collection_lockfile: Option<PathBuf>,
    /// In `Steepfile` order, which is the order Steep looks a file's target up in
    pub targets: Vec<SteepfileTarget>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SteepfileTarget {
    pub name: String,
    pub check_paths: Vec<PathBuf>,
    pub ignore_paths: Vec<PathBuf>,
    pub code_diagnostics: DiagnosticConfiguration,
    /// Looked up before the target itself, as Steep does
    pub groups: Vec<SteepfileGroup>,
    /// `implicitly_returns_nil!`, which takes `true` without an argument
    pub implicitly_returns_nil: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SteepfileGroup {
    pub name: String,
    pub check_paths: Vec<PathBuf>,
    pub ignore_paths: Vec<PathBuf>,
    pub code_diagnostics: DiagnosticConfiguration,
}

/// Reads `Steepfile` in `workspace_path` (all targets merged, and each target in `targets`). Returns `None` when there is
/// no `Steepfile`. Paths are joined with `workspace_path`
pub fn read_steepfile(workspace_path: &Path) -> Option<SteepfileConfiguration> {
    let source = std::fs::read(workspace_path.join("Steepfile")).ok()?;
    let parse_result = ruby_prism::parse(&source);
    let mut reader = SteepfileReader {
        workspace_path,
        configuration: SteepfileConfiguration {
            signature_patterns: Vec::new(),
            check_paths: Vec::new(),
            ignore_paths: Vec::new(),
            libraries: Vec::new(),
            collection_lockfile: None,
            targets: Vec::new(),
        },
        constant_aliases: HashMap::new(),
    };
    if let Some(program) = parse_result.node().as_program_node() {
        reader.read_statements(&program.statements(), &mut Scope::TopLevel);
    }
    Some(reader.configuration)
}

struct GroupBuilder {
    name: String,
    check_paths: Vec<PathBuf>,
    ignore_paths: Vec<PathBuf>,
    code_diagnostics: Option<DiagnosticConfiguration>,
}

struct TargetBuilder {
    target: SteepfileTarget,
    groups: Vec<GroupBuilder>,
}

enum Scope<'scope> {
    TopLevel,
    Target(&'scope mut TargetBuilder),
    Group {
        group: &'scope mut GroupBuilder,
        target_code_diagnostics: &'scope DiagnosticConfiguration,
    },
}

impl Scope<'_> {
    fn reborrow(&mut self) -> Scope<'_> {
        match self {
            Scope::TopLevel => Scope::TopLevel,
            Scope::Target(target) => Scope::Target(target),
            Scope::Group {
                group,
                target_code_diagnostics,
            } => Scope::Group {
                group,
                target_code_diagnostics: target_code_diagnostics,
            },
        }
    }

    fn pattern_lists(&mut self) -> Option<(&mut Vec<PathBuf>, &mut Vec<PathBuf>)> {
        match self {
            Scope::TopLevel => None,
            Scope::Target(builder) => Some((
                &mut builder.target.check_paths,
                &mut builder.target.ignore_paths,
            )),
            Scope::Group { group, .. } => Some((&mut group.check_paths, &mut group.ignore_paths)),
        }
    }
}

const STEEP_RUBY_DIAGNOSTIC_NAMESPACE: [&str; 3] = ["Steep", "Diagnostic", "Ruby"];

struct SteepfileReader<'a> {
    workspace_path: &'a Path,
    configuration: SteepfileConfiguration,
    /// `D = Steep::Diagnostic`, the alias `steep init` suggests, maps `D` to `["Steep", "Diagnostic"]`
    constant_aliases: HashMap<String, Vec<String>>,
}

impl SteepfileReader<'_> {
    fn read_statements(&mut self, statements: &ruby_prism::StatementsNode, scope: &mut Scope) {
        for statement in statements.body().iter() {
            if let Some(constant_write) = statement.as_constant_write_node() {
                if let Some(value) = self.constant_path(&constant_write.value()) {
                    self.constant_aliases
                        .insert(constant_name(constant_write.name()), value);
                }
                continue;
            }
            let Some(call) = statement.as_call_node() else {
                continue;
            };
            if call.receiver().is_some() {
                continue;
            }
            let arguments = string_arguments(&call);
            let workspace_path = self.workspace_path;
            let paths = || {
                arguments
                    .iter()
                    .map(|argument| workspace_path.join(argument))
            };
            let block_statements = call
                .block()
                .and_then(|block| block.as_block_node())
                .and_then(|block| block.body())
                .and_then(|body| body.as_statements_node());
            match call.name().as_slice() {
                b"signature" => self.configuration.signature_patterns.extend(paths()),
                b"check" => {
                    self.configuration.check_paths.extend(paths());
                    if let Some((check_paths, _)) = scope.pattern_lists() {
                        check_paths.extend(paths());
                    }
                }
                b"ignore" => {
                    self.configuration.ignore_paths.extend(paths());
                    if let Some((_, ignore_paths)) = scope.pattern_lists() {
                        ignore_paths.extend(paths());
                    }
                }
                b"library" => self
                    .configuration
                    .libraries
                    .extend(arguments.iter().cloned()),
                b"collection_config" => {
                    if let Some(config_path) = arguments.first() {
                        self.configuration.collection_lockfile =
                            Some(collection_lockfile_path(&workspace_path.join(config_path)));
                    }
                }
                b"configure_code_diagnostics" => {
                    self.configure_code_diagnostics(&call, scope);
                    continue;
                }
                b"implicitly_returns_nil!" => {
                    if let Scope::Target(builder) = scope {
                        let argument = call.arguments().and_then(|arguments| arguments.arguments().iter().next());
                        builder.target.implicitly_returns_nil =
                            argument.is_none_or(|argument| argument.as_false_node().is_none() && argument.as_nil_node().is_none());
                    }
                    continue;
                }
                b"target" if matches!(scope, Scope::TopLevel) => {
                    let mut builder = TargetBuilder {
                        target: SteepfileTarget {
                            name: arguments.first().cloned().unwrap_or_default(),
                            check_paths: Vec::new(),
                            ignore_paths: Vec::new(),
                            code_diagnostics: DiagnosticConfiguration::default(),
                            groups: Vec::new(),
                            implicitly_returns_nil: false,
                        },
                        groups: Vec::new(),
                    };
                    if let Some(block_statements) = &block_statements {
                        self.read_statements(block_statements, &mut Scope::Target(&mut builder));
                    }
                    let TargetBuilder { mut target, groups } = builder;
                    target.groups = groups
                        .into_iter()
                        .map(|group| SteepfileGroup {
                            name: group.name,
                            check_paths: group.check_paths,
                            ignore_paths: group.ignore_paths,
                            code_diagnostics: group
                                .code_diagnostics
                                .unwrap_or_else(|| target.code_diagnostics.clone()),
                        })
                        .collect();
                    self.configuration.targets.push(target);
                    continue;
                }
                b"group" if matches!(scope, Scope::Target(_)) => {
                    let Scope::Target(builder) = scope else {
                        continue;
                    };
                    let mut group = GroupBuilder {
                        name: arguments.first().cloned().unwrap_or_default(),
                        check_paths: Vec::new(),
                        ignore_paths: Vec::new(),
                        code_diagnostics: None,
                    };
                    if let Some(block_statements) = &block_statements {
                        let mut group_scope = Scope::Group {
                            group: &mut group,
                            target_code_diagnostics: &builder.target.code_diagnostics,
                        };
                        self.read_statements(block_statements, &mut group_scope);
                    }
                    builder.groups.push(group);
                    continue;
                }
                _ => {}
            }
            if let Some(block_statements) = block_statements {
                self.read_statements(&block_statements, &mut scope.reborrow());
            }
        }
    }

    /// Follows `TargetDSL#configure_code_diagnostics` and `GroupDSL#configure_code_diagnostics` in steep 2.0.0
    /// `lib/steep/project/dsl.rb`: a target merges the argument into its configuration (which starts as `Ruby.default`),
    /// while a group without a block replaces its configuration with the argument
    fn configure_code_diagnostics(&self, call: &ruby_prism::CallNode, scope: &mut Scope) {
        let argument = call
            .arguments()
            .and_then(|arguments| arguments.arguments().iter().next());
        let argument_configuration = argument
            .as_ref()
            .and_then(|argument| self.diagnostic_hash(argument));
        let block = call.block().and_then(|block| block.as_block_node());
        match scope {
            Scope::TopLevel => {}
            Scope::Target(builder) => {
                if let Some(argument_configuration) = &argument_configuration {
                    builder
                        .target
                        .code_diagnostics
                        .merge(argument_configuration);
                }
                if let Some(block) = &block {
                    self.apply_diagnostic_block(block, &mut builder.target.code_diagnostics);
                }
            }
            Scope::Group {
                group,
                target_code_diagnostics,
            } => match &block {
                Some(block) => {
                    let code_diagnostics = match &mut group.code_diagnostics {
                        Some(existing) => {
                            if let Some(argument_configuration) = &argument_configuration {
                                existing.merge(argument_configuration);
                            }
                            existing
                        }
                        None => group.code_diagnostics.insert(
                            argument_configuration
                                .unwrap_or_else(|| (*target_code_diagnostics).clone()),
                        ),
                    };
                    self.apply_diagnostic_block(block, code_diagnostics);
                }
                None => group.code_diagnostics = argument_configuration,
            },
        }
    }

    fn apply_diagnostic_block(
        &self,
        block: &ruby_prism::BlockNode,
        configuration: &mut DiagnosticConfiguration,
    ) {
        let Some(parameter_name) = block_parameter_name(block) else {
            return;
        };
        let Some(statements) = block.body().and_then(|body| body.as_statements_node()) else {
            return;
        };
        for statement in statements.body().iter() {
            let Some(call) = statement.as_call_node() else {
                continue;
            };
            let receiver_is_parameter = call
                .receiver()
                .and_then(|receiver| receiver.as_local_variable_read_node())
                .is_some_and(|receiver| constant_name(receiver.name()) == parameter_name);
            if !receiver_is_parameter {
                continue;
            }
            let arguments: Vec<ruby_prism::Node> = call
                .arguments()
                .map(|arguments| arguments.arguments().iter().collect())
                .unwrap_or_default();
            match (call.name().as_slice(), arguments.as_slice()) {
                (b"[]=", [key, value]) => {
                    if let (Some(code), Some(severity)) =
                        (self.diagnostic_code(key), severity_value(value))
                    {
                        configuration.set(&code, severity);
                    }
                }
                (b"merge!" | b"update", [hash]) => {
                    if let Some(hash_configuration) = self.diagnostic_hash(hash) {
                        configuration.merge(&hash_configuration);
                    }
                }
                _ => {}
            }
        }
    }

    /// Evaluates `D::Ruby.strict`, `D::Ruby.default.merge({ ... })` and `{ D::Ruby::NoMethod => :hint }`
    fn diagnostic_hash(&self, node: &ruby_prism::Node) -> Option<DiagnosticConfiguration> {
        if let Some(call) = node.as_call_node() {
            let receiver = call.receiver()?;
            let method_name = String::from_utf8_lossy(call.name().as_slice()).into_owned();
            if self
                .constant_path(&receiver)
                .is_some_and(|path| path == STEEP_RUBY_DIAGNOSTIC_NAMESPACE)
            {
                return DiagnosticTemplate::from_method_name(&method_name)
                    .map(DiagnosticConfiguration::from_template);
            }
            if method_name == "merge" {
                let mut configuration = self.diagnostic_hash(&receiver)?;
                let argument = call.arguments()?.arguments().iter().next()?;
                configuration.merge(&self.diagnostic_hash(&argument)?);
                return Some(configuration);
            }
            return None;
        }
        let elements = match (node.as_hash_node(), node.as_keyword_hash_node()) {
            (Some(hash), _) => hash.elements(),
            (None, Some(keyword_hash)) => keyword_hash.elements(),
            (None, None) => return None,
        };
        Some(
            elements
                .iter()
                .filter_map(|element| element.as_assoc_node())
                .filter_map(|assoc| {
                    Some((
                        self.diagnostic_code(&assoc.key())?,
                        severity_value(&assoc.value())?,
                    ))
                })
                .collect(),
        )
    }

    fn diagnostic_code(&self, node: &ruby_prism::Node) -> Option<String> {
        let path = self.constant_path(node)?;
        let (name, namespace) = path.split_last()?;
        let code = format!("Ruby::{name}");
        (namespace == STEEP_RUBY_DIAGNOSTIC_NAMESPACE
            && ruby_diagnostic_codes().any(|known| known == code))
        .then_some(code)
    }

    fn constant_path(&self, node: &ruby_prism::Node) -> Option<Vec<String>> {
        if let Some(read) = node.as_constant_read_node() {
            let name = constant_name(read.name());
            return Some(
                self.constant_aliases
                    .get(&name)
                    .cloned()
                    .unwrap_or_else(|| vec![name]),
            );
        }
        let path = node.as_constant_path_node()?;
        let mut segments = match path.parent() {
            Some(parent) => self.constant_path(&parent)?,
            None => Vec::new(),
        };
        segments.push(constant_name(path.name()?));
        Some(segments)
    }
}

fn constant_name(name: ruby_prism::ConstantId) -> String {
    String::from_utf8_lossy(name.as_slice()).into_owned()
}

fn block_parameter_name(block: &ruby_prism::BlockNode) -> Option<String> {
    let parameters = block.parameters()?;
    if parameters.as_numbered_parameters_node().is_some() {
        return Some("_1".to_string());
    }
    let first = parameters
        .as_block_parameters_node()?
        .parameters()?
        .requireds()
        .iter()
        .next()?;
    Some(constant_name(first.as_required_parameter_node()?.name()))
}

/// `Some(None)` is `nil`, which disables the diagnostic
fn severity_value(node: &ruby_prism::Node) -> Option<Option<DiagnosticSeverity>> {
    if node.as_nil_node().is_some() {
        return Some(None);
    }
    let symbol = node.as_symbol_node()?;
    DiagnosticSeverity::from_name(&String::from_utf8_lossy(symbol.unescaped())).map(Some)
}

fn string_arguments(call: &ruby_prism::CallNode) -> Vec<String> {
    let Some(arguments) = call.arguments() else {
        return Vec::new();
    };
    let mut result = Vec::new();
    for argument in arguments.arguments().iter() {
        let elements: Vec<ruby_prism::Node> = match argument.as_array_node() {
            Some(array) => array.elements().iter().collect(),
            None => vec![argument],
        };
        for element in elements {
            if let Some(string) = element.as_string_node() {
                result.push(String::from_utf8_lossy(string.unescaped()).into_owned());
            } else if let Some(symbol) = element.as_symbol_node() {
                result.push(String::from_utf8_lossy(symbol.unescaped()).into_owned());
            }
        }
    }
    result
}

/// `rbs_collection.yaml` locks into `rbs_collection.lock.yaml` next to it
fn collection_lockfile_path(config_path: &Path) -> PathBuf {
    let stem = config_path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    config_path.with_file_name(format!("{stem}.lock.yaml"))
}

#[derive(Deserialize)]
struct CollectionLockfile {
    path: Option<String>,
    #[serde(default)]
    gems: Vec<LockedGem>,
}

#[derive(Deserialize)]
struct LockedGem {
    name: String,
    version: Option<serde_yaml::Value>,
    source: Option<LockedGemSource>,
}

#[derive(Deserialize)]
struct LockedGemSource {
    #[serde(rename = "type")]
    source_type: String,
}

fn yaml_scalar_string(value: &serde_yaml::Value) -> Option<String> {
    match value {
        serde_yaml::Value::String(string) => Some(string.clone()),
        serde_yaml::Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

struct WorkspaceLoader<'a> {
    loader: Loader,
    sources: &'a WorkspaceSources,
    loaded_libraries: HashSet<String>,
}

impl WorkspaceLoader<'_> {
    fn error(&mut self, path: &Path, message: String) {
        self.loader.add_error(LoadError {
            path: path.to_path_buf(),
            message,
        });
    }

    /// Loads `<stdlib>/<name>/<version>` and the dependencies listed in its `manifest.yaml`. Without a version the
    /// newest one is used
    fn add_stdlib_library(&mut self, name: &str, version: Option<&str>) {
        if !self.loaded_libraries.insert(name.to_string()) {
            return;
        }
        let Some(stdlib_directory) = &self.sources.stdlib_directory else {
            self.error(
                Path::new(name),
                format!("cannot load library {name}: stdlib directory is unknown"),
            );
            return;
        };
        let library_directory = stdlib_directory.join(name);
        let version_directory = match version {
            Some(version) => {
                Some(library_directory.join(version)).filter(|directory| directory.is_dir())
            }
            None => newest_version_directory(&library_directory),
        };
        let Some(version_directory) = version_directory else {
            self.error(
                &library_directory,
                format!("library {name} is not found in stdlib"),
            );
            return;
        };
        self.loader.add_library_directory(&version_directory);
        for dependency in manifest_dependencies(&version_directory.join("manifest.yaml")) {
            self.add_stdlib_library(&dependency, None);
        }
    }

    fn add_collection(&mut self, workspace_path: &Path, lockfile_path: &Path) {
        let Ok(content) = std::fs::read_to_string(lockfile_path) else {
            return;
        };
        let lockfile: CollectionLockfile = match serde_yaml::from_str(&content) {
            Ok(lockfile) => lockfile,
            Err(error) => {
                self.error(lockfile_path, error.to_string());
                return;
            }
        };
        let lockfile_directory = lockfile_path.parent().unwrap_or(workspace_path);
        let collection_directory =
            lockfile_directory.join(lockfile.path.as_deref().unwrap_or(".gem_rbs_collection"));
        for gem in lockfile.gems {
            let version = gem.version.as_ref().and_then(yaml_scalar_string);
            let source_type = gem
                .source
                .as_ref()
                .map(|source| source.source_type.as_str())
                .unwrap_or("git");
            match source_type {
                "git" => {
                    let Some(version) = version else {
                        self.error(lockfile_path, format!("gem {} has no version", gem.name));
                        continue;
                    };
                    let directory = collection_directory.join(&gem.name).join(version);
                    if directory.is_dir() {
                        self.loader.add_library_directory(&directory);
                    } else {
                        self.error(
                            &directory,
                            format!(
                                "gem {} is not installed; run `rbs collection install`",
                                gem.name
                            ),
                        );
                    }
                }
                "stdlib" => self.add_stdlib_library(&gem.name, version.as_deref()),
                "rubygems" => match self.sources.gem_directories.get(&gem.name) {
                    Some(gem_directory) => {
                        let signature_directory = gem_directory.join("sig");
                        if signature_directory.is_dir() {
                            self.loader.add_library_directory(&signature_directory);
                        } else {
                            self.error(
                                &signature_directory,
                                format!("gem {} has no sig directory", gem.name),
                            );
                        }
                    }
                    None => self.error(lockfile_path, format!("gem {} is not installed", gem.name)),
                },
                other => self.error(
                    lockfile_path,
                    format!("gem {} has unsupported source type {other}", gem.name),
                ),
            }
        }
    }
}

fn newest_version_directory(library_directory: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(library_directory).ok()?;
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .max_by(|left, right| compare_versions(&file_name(left), &file_name(right)))
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn compare_versions(left: &str, right: &str) -> std::cmp::Ordering {
    let segments = |version: &str| -> Vec<u64> {
        version
            .split('.')
            .map(|segment| segment.parse::<u64>().unwrap_or(0))
            .collect()
    };
    segments(left)
        .cmp(&segments(right))
        .then_with(|| left.cmp(right))
}

#[derive(Deserialize)]
struct Manifest {
    #[serde(default)]
    dependencies: Vec<ManifestDependency>,
}

#[derive(Deserialize)]
struct ManifestDependency {
    name: String,
}

fn manifest_dependencies(manifest_path: &Path) -> Vec<String> {
    let Ok(content) = std::fs::read_to_string(manifest_path) else {
        return Vec::new();
    };
    serde_yaml::from_str::<Manifest>(&content)
        .map(|manifest| {
            manifest
                .dependencies
                .into_iter()
                .map(|dependency| dependency.name)
                .collect()
        })
        .unwrap_or_default()
}

/// Loads core, the libraries from the `Steepfile`, `rbs_collection.lock.yaml` and the project signatures
pub fn load_workspace(workspace_path: &Path, sources: &WorkspaceSources) -> Environment {
    let configuration = read_steepfile(workspace_path);
    let mut workspace_loader = WorkspaceLoader {
        loader: Loader::new(),
        sources,
        loaded_libraries: HashSet::new(),
    };

    if let Some(core_directory) = &sources.core_directory {
        workspace_loader
            .loader
            .add_library_directory(core_directory);
    }
    workspace_loader.loader.set_implicitly_returns_nil(
        configuration.iter().flat_map(|configuration| &configuration.targets).any(|target| target.implicitly_returns_nil),
    );
    for library in configuration
        .iter()
        .flat_map(|configuration| configuration.libraries.iter())
    {
        let (name, version) = match library.split_once(':') {
            Some((name, version)) => (name, Some(version)),
            None => (library.as_str(), None),
        };
        workspace_loader.add_stdlib_library(name, version);
    }
    let lockfile_path = configuration
        .as_ref()
        .and_then(|configuration| configuration.collection_lockfile.clone())
        .unwrap_or_else(|| workspace_path.join("rbs_collection.lock.yaml"));
    workspace_loader.add_collection(workspace_path, &lockfile_path);
    // RBS's `EnvironmentLoader#load` adds stringio whenever core is loaded, since it moved out of core
    let has_stringio = sources.stdlib_directory.as_ref().is_some_and(|directory| directory.join("stringio").is_dir());
    if sources.core_directory.is_some() && has_stringio {
        workspace_loader.add_stdlib_library("stringio", None);
    }

    match &configuration {
        Some(configuration) => {
            for pattern in &configuration.signature_patterns {
                let paths = pattern_files(workspace_path, std::slice::from_ref(pattern), &[], ".rbs");
                if paths.is_empty() {
                    workspace_loader.loader.add_signature_directory(pattern);
                }
                for path in paths {
                    workspace_loader.loader.add_signature_directory(&path);
                }
            }
        }
        None => {
            let default_signature_directory = workspace_path.join("sig");
            if default_signature_directory.is_dir() {
                workspace_loader
                    .loader
                    .add_signature_directory(&default_signature_directory);
            }
        }
    }
    workspace_loader.loader.finish()
}

/// Steep's `Project::Pattern`: a pattern matches the path by `File.fnmatch`, or as a directory that has the file with
/// the extension somewhere under it
struct SourcePattern<'a> {
    patterns: Vec<String>,
    ignores: Vec<String>,
    extension: &'a str,
}

impl SourcePattern<'_> {
    fn new<'a>(
        workspace_path: &Path,
        check_paths: &[PathBuf],
        ignore_paths: &[PathBuf],
        extension: &'a str,
    ) -> SourcePattern<'a> {
        let relative = |paths: &[PathBuf]| {
            paths
                .iter()
                .map(|path| {
                    path.strip_prefix(workspace_path)
                        .unwrap_or(path)
                        .to_string_lossy()
                        .into_owned()
                })
                .map(|pattern| {
                    if pattern.is_empty() {
                        ".".to_string()
                    } else {
                        pattern
                    }
                })
                .collect()
        };
        SourcePattern {
            patterns: relative(check_paths),
            ignores: relative(ignore_paths),
            extension,
        }
    }

    fn matches(&self, path: &str) -> bool {
        source_pattern_matches(&self.patterns, path, self.extension) && !self.ignores(path)
    }

    fn ignores(&self, path: &str) -> bool {
        source_pattern_matches(&self.ignores, path, self.extension)
    }
}

fn source_pattern_matches(patterns: &[String], path: &str, extension: &str) -> bool {
    patterns.iter().any(|pattern| {
        let prefix = if pattern == "." || pattern == "./" {
            String::new()
        } else {
            format!(
                "{}/",
                pattern
                    .strip_prefix("./")
                    .unwrap_or(pattern)
                    .trim_end_matches('/')
            )
        };
        ruby_file_glob::fnmatch(pattern, path)
            || ruby_file_glob::fnmatch(&format!("{prefix}**/*{extension}"), path)
    })
}

/// `foo/./bar` and `./foo` as `foo/bar` and `foo`, the way `Pathname#relative_path_from` gives them
fn normalized_relative_path(path: &str) -> String {
    Path::new(path)
        .components()
        .filter(|component| !matches!(component, std::path::Component::CurDir))
        .collect::<PathBuf>()
        .to_string_lossy()
        .into_owned()
}

/// The files with the extension that the patterns take, as Steep's `Services::FileLoader#each_path_in_patterns` lists
/// them: a pattern is globbed, and a directory it matches gives the files under it
fn pattern_files(
    workspace_path: &Path,
    check_paths: &[PathBuf],
    ignore_paths: &[PathBuf],
    extension: &str,
) -> Vec<PathBuf> {
    let pattern = SourcePattern::new(workspace_path, check_paths, ignore_paths, extension);
    let mut files = Vec::new();
    for written in &pattern.patterns {
        for matched in ruby_file_glob::glob(written, workspace_path) {
            let matched = normalized_relative_path(&matched);
            let absolute_path = workspace_path.join(&matched);
            if absolute_path.is_file() {
                if pattern.matches(&matched) {
                    files.push(absolute_path);
                }
                continue;
            }
            let directory_pattern = format!("{}/**/*{extension}", matched.trim_end_matches('/'));
            let mut found: Vec<String> = ruby_file_glob::glob(&directory_pattern, workspace_path)
                .iter()
                .map(|path| normalized_relative_path(path))
                .collect();
            found.sort();
            for path in found {
                let absolute_path = workspace_path.join(&path);
                if absolute_path.is_file() && !pattern.ignores(&path) {
                    files.push(absolute_path);
                }
            }
        }
    }
    files
}
