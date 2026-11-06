use std::path::{Path, PathBuf};

use rbs_definition::collections::HashMap;
use ruby_prism::Node as PrismNode;

use crate::{
    diagnostic::configuration::{Configuration, Severity},
    project::{
        CollectionConfiguration, Group, LibraryOptions, PathCache, Project, Target,
        pattern::Pattern,
    },
};

#[derive(Clone, Debug)]
enum Value {
    Nil,
    Boolean(bool),
    Symbol(String),
    String(String),
    Path(PathBuf),
    Array(Vec<Value>),
    Constant(Vec<String>),
    Configuration(Configuration),
    TargetConfiguration,
}

#[derive(Default)]
struct WithPattern {
    sources: Vec<String>,
    ignored_sources: Vec<String>,
    inline_sources: Vec<String>,
    ignored_inline_sources: Vec<String>,
    signatures: Vec<String>,
    ignored_signatures: Vec<String>,
}

impl WithPattern {
    fn source_pattern(&self) -> Pattern {
        Pattern::new(self.sources.clone(), self.ignored_sources.clone(), ".rb")
    }

    fn inline_source_pattern(&self) -> Pattern {
        Pattern::new(
            self.inline_sources.clone(),
            self.ignored_inline_sources.clone(),
            ".rb",
        )
    }

    fn signature_pattern(&self) -> Pattern {
        Pattern::new(
            self.signatures.clone(),
            self.ignored_signatures.clone(),
            ".rbs",
        )
    }
}

#[derive(Default)]
struct LibraryDsl {
    options: LibraryOptions,
    configured: bool,
}

struct TargetDsl {
    name: String,
    library: LibraryDsl,
    patterns: WithPattern,
    code_diagnostics_config: Configuration,
    unreferenced: bool,
    implicitly_returns_nil: bool,
    groups: Vec<GroupDsl>,
}

struct GroupDsl {
    name: String,
    patterns: WithPattern,
    code_diagnostics_config: Option<Configuration>,
}

enum Receiver {
    Project,
    Target(Box<TargetDsl>),
    Group(Box<GroupDsl>),
}

struct Interpreter<'source> {
    source: &'source [u8],
    base_directory: PathBuf,
    locals: HashMap<String, Value>,
    constants: HashMap<String, Value>,
    receivers: Vec<Receiver>,
    global_library: LibraryDsl,
    targets: Vec<Target>,
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn constant_path(node: &PrismNode) -> Option<Vec<String>> {
    if let Some(read) = node.as_constant_read_node() {
        return Some(vec![text(read.name().as_slice())]);
    }
    if let Some(path) = node.as_constant_path_node() {
        let mut names = match path.parent() {
            Some(parent) => constant_path(&parent)?,
            None => Vec::new(),
        };
        names.push(text(path.name()?.as_slice()));
        return Some(names);
    }
    None
}

fn is_inline_keyword(node: &PrismNode) -> bool {
    let Some(hash) = node.as_keyword_hash_node() else {
        return false;
    };
    hash.elements().iter().all(|element| {
        element.as_assoc_node().is_some_and(|assoc| {
            assoc
                .key()
                .as_symbol_node()
                .is_some_and(|key| key.unescaped() == b"inline")
                && (assoc.value().as_true_node().is_some()
                    || assoc.value().as_false_node().is_some())
        })
    })
}

impl<'source> Interpreter<'source> {
    fn error(&self, node: &PrismNode, message: &str) -> String {
        let location = node.location();
        let line = self.source[..location.start_offset()]
            .iter()
            .filter(|byte| **byte == b'\n')
            .count()
            + 1;
        format!(
            "Steepfile:{line}: {message}: `{}` (the Steepfile is read without running Ruby, and only the Steepfile DSL is supported)",
            text(location.as_slice())
        )
    }

    fn evaluate_statements(&mut self, node: Option<PrismNode>) -> Result<Value, String> {
        let Some(node) = node else {
            return Ok(Value::Nil);
        };
        if let Some(statements) = node.as_statements_node() {
            let mut value = Value::Nil;
            for statement in statements.body().iter() {
                value = self.evaluate(&statement)?;
            }
            return Ok(value);
        }
        self.evaluate(&node)
    }

    fn evaluate(&mut self, node: &PrismNode) -> Result<Value, String> {
        if let Some(string) = node.as_string_node() {
            return Ok(Value::String(text(string.unescaped())));
        }
        if let Some(symbol) = node.as_symbol_node() {
            return Ok(Value::Symbol(text(symbol.unescaped())));
        }
        if node.as_nil_node().is_some() {
            return Ok(Value::Nil);
        }
        if node.as_true_node().is_some() {
            return Ok(Value::Boolean(true));
        }
        if node.as_false_node().is_some() {
            return Ok(Value::Boolean(false));
        }
        if let Some(array) = node.as_array_node() {
            let mut values = Vec::new();
            for element in array.elements().iter() {
                values.push(self.evaluate(&element)?);
            }
            return Ok(Value::Array(values));
        }
        if let Some(read) = node.as_local_variable_read_node() {
            let name = text(read.name().as_slice());
            return self
                .locals
                .get(&name)
                .cloned()
                .ok_or_else(|| self.error(node, "unknown local variable"));
        }
        if let Some(write) = node.as_local_variable_write_node() {
            let value = self.evaluate(&write.value())?;
            self.locals
                .insert(text(write.name().as_slice()), value.clone());
            return Ok(value);
        }
        if let Some(write) = node.as_constant_write_node() {
            let value = self.evaluate(&write.value())?;
            self.constants
                .insert(text(write.name().as_slice()), value.clone());
            return Ok(value);
        }
        if let Some(path) = constant_path(node) {
            return Ok(self.resolve_constant(path));
        }
        if let Some(call) = node.as_call_node() {
            return self.evaluate_call(node, &call);
        }
        Err(self.error(node, "unsupported syntax"))
    }

    fn resolve_constant(&self, path: Vec<String>) -> Value {
        if let Some((first, rest)) = path.split_first()
            && let Some(Value::Constant(prefix)) = self.constants.get(first)
        {
            let mut resolved = prefix.clone();
            resolved.extend(rest.iter().cloned());
            return Value::Constant(resolved);
        }
        Value::Constant(path)
    }

    fn arguments(&mut self, call: &ruby_prism::CallNode) -> Result<Vec<Value>, String> {
        let mut values = Vec::new();
        if let Some(arguments) = call.arguments() {
            for argument in arguments.arguments().iter() {
                if is_inline_keyword(&argument) {
                    continue;
                }
                values.push(self.evaluate(&argument)?);
            }
        }
        Ok(values)
    }

    fn keyword_inline(&self, call: &ruby_prism::CallNode) -> bool {
        let Some(arguments) = call.arguments() else {
            return false;
        };
        arguments.arguments().iter().any(|argument| {
            let Some(hash) = argument.as_keyword_hash_node() else {
                return false;
            };
            hash.elements().iter().any(|element| {
                element.as_assoc_node().is_some_and(|assoc| {
                    assoc.key().as_symbol_node().is_some_and(|key| {
                        key.unescaped() == b"inline" && assoc.value().as_true_node().is_some()
                    })
                })
            })
        })
    }

    fn strings(&self, node: &PrismNode, values: &[Value]) -> Result<Vec<String>, String> {
        values
            .iter()
            .filter(|value| !matches!(value, Value::Nil))
            .map(|value| match value {
                Value::String(string) | Value::Symbol(string) => Ok(string.clone()),
                Value::Path(path) => Ok(path.to_string_lossy().into_owned()),
                _ => Err(self.error(node, "expected a string")),
            })
            .collect()
    }

    fn configuration_of(&self, node: &PrismNode, value: &Value) -> Result<Configuration, String> {
        match value {
            Value::Configuration(configuration) => Ok(configuration.clone()),
            _ => Err(self.error(node, "expected a diagnostics configuration")),
        }
    }

    fn evaluate_block(
        &mut self,
        call: &ruby_prism::CallNode,
        parameter_values: Vec<Value>,
    ) -> Result<(), String> {
        let Some(block) = call.block() else {
            return Ok(());
        };
        let Some(block) = block.as_block_node() else {
            return Ok(());
        };
        let mut names = Vec::new();
        if let Some(parameters) = block.parameters()
            && let Some(parameters) = parameters.as_block_parameters_node()
            && let Some(parameters) = parameters.parameters()
        {
            for required in parameters.requireds().iter() {
                if let Some(required) = required.as_required_parameter_node() {
                    names.push(text(required.name().as_slice()));
                }
            }
        }
        for (name, value) in names.iter().zip(parameter_values) {
            self.locals.insert(name.clone(), value);
        }
        self.evaluate_statements(block.body())?;
        Ok(())
    }

    fn library(&mut self) -> Option<&mut LibraryDsl> {
        match self.receivers.last_mut() {
            Some(Receiver::Project) => Some(&mut self.global_library),
            Some(Receiver::Target(target)) => Some(&mut target.library),
            _ => None,
        }
    }

    fn patterns(&mut self) -> Option<&mut WithPattern> {
        match self.receivers.last_mut() {
            Some(Receiver::Target(target)) => Some(&mut target.patterns),
            Some(Receiver::Group(group)) => Some(&mut group.patterns),
            _ => None,
        }
    }

    fn target_configuration(&mut self) -> Option<&mut Configuration> {
        for receiver in self.receivers.iter_mut().rev() {
            match receiver {
                Receiver::Group(group) => {
                    if let Some(configuration) = group.code_diagnostics_config.as_mut() {
                        return Some(configuration);
                    }
                }
                Receiver::Target(target) => return Some(&mut target.code_diagnostics_config),
                Receiver::Project => return None,
            }
        }
        None
    }

    fn evaluate_call(
        &mut self,
        node: &PrismNode,
        call: &ruby_prism::CallNode,
    ) -> Result<Value, String> {
        let name = text(call.name().as_slice());
        if let Some(receiver) = call.receiver() {
            let receiver_value = self.evaluate(&receiver)?;
            let arguments = self.arguments(call)?;
            return self.evaluate_method(node, call, receiver_value, &name, arguments);
        }
        let arguments = self.arguments(call)?;
        let inline = self.keyword_inline(call);
        match name.as_str() {
            "target" => {
                let target_name = self.strings(node, &arguments)?.into_iter().next();
                let Some(target_name) = target_name else {
                    return Err(self.error(node, "target needs a name"));
                };
                self.receivers.push(Receiver::Target(Box::new(TargetDsl {
                    name: target_name,
                    library: LibraryDsl::default(),
                    patterns: WithPattern::default(),
                    code_diagnostics_config: Configuration::default_configuration(),
                    unreferenced: false,
                    implicitly_returns_nil: false,
                    groups: Vec::new(),
                })));
                self.evaluate_block(call, Vec::new())?;
                let Some(Receiver::Target(target)) = self.receivers.pop() else {
                    unreachable!("the target receiver is pushed above");
                };
                let target_configuration = target.code_diagnostics_config.clone();
                self.targets.push(Target {
                    name: target.name,
                    source_pattern: target.patterns.source_pattern(),
                    inline_source_pattern: target.patterns.inline_source_pattern(),
                    signature_pattern: target.patterns.signature_pattern(),
                    target_options: target.library.configured.then_some(target.library.options),
                    code_diagnostics_config: target.code_diagnostics_config,
                    unreferenced: target.unreferenced,
                    implicitly_returns_nil: target.implicitly_returns_nil,
                    groups: target
                        .groups
                        .into_iter()
                        .map(|group| Group {
                            name: group.name,
                            source_pattern: group.patterns.source_pattern(),
                            inline_source_pattern: group.patterns.inline_source_pattern(),
                            signature_pattern: group.patterns.signature_pattern(),
                            code_diagnostics_config: group
                                .code_diagnostics_config
                                .unwrap_or_else(|| target_configuration.clone()),
                        })
                        .collect(),
                });
                Ok(Value::Nil)
            }
            "group" => {
                let group_name = self.strings(node, &arguments)?.into_iter().next();
                let Some(group_name) = group_name else {
                    return Err(self.error(node, "group needs a name"));
                };
                if !matches!(self.receivers.last(), Some(Receiver::Target(_))) {
                    return Err(self.error(node, "group is allowed only in a target"));
                }
                self.receivers.push(Receiver::Group(Box::new(GroupDsl {
                    name: group_name,
                    patterns: WithPattern::default(),
                    code_diagnostics_config: None,
                })));
                self.evaluate_block(call, Vec::new())?;
                let Some(Receiver::Group(group)) = self.receivers.pop() else {
                    unreachable!("the group receiver is pushed above");
                };
                if let Some(Receiver::Target(target)) = self.receivers.last_mut() {
                    target.groups.push(*group);
                }
                Ok(Value::Nil)
            }
            "check" | "ignore" | "signature" | "ignore_signature" => {
                let strings = self.strings(node, &arguments)?;
                let Some(patterns) = self.patterns() else {
                    return Err(self.error(node, "patterns are allowed only in a target or group"));
                };
                let list = match (name.as_str(), inline) {
                    ("check", false) => &mut patterns.sources,
                    ("check", true) => &mut patterns.inline_sources,
                    ("ignore", false) => &mut patterns.ignored_sources,
                    ("ignore", true) => &mut patterns.ignored_inline_sources,
                    ("signature", _) => &mut patterns.signatures,
                    _ => &mut patterns.ignored_signatures,
                };
                list.extend(strings);
                Ok(Value::Nil)
            }
            "library" | "repo_path" | "collection_config" | "disable_collection"
            | "stdlib_path" => {
                let strings = self.strings(node, &arguments)?;
                let base_directory = self.base_directory.clone();
                let Some(library) = self.library() else {
                    return Err(self.error(node, "library options are not allowed here"));
                };
                match name.as_str() {
                    "library" => {
                        library.configured = true;
                        library.options.libraries.extend(strings);
                    }
                    "repo_path" => {
                        library.configured = true;
                        library
                            .options
                            .repo_paths
                            .extend(strings.into_iter().map(PathBuf::from));
                    }
                    "collection_config" => {
                        library.configured = true;
                        if let Some(path) = strings.into_iter().next() {
                            library.options.collection_config_path =
                                CollectionConfiguration::Path(base_directory.join(path));
                        }
                    }
                    "disable_collection" => {
                        library.configured = true;
                        library.options.collection_config_path = CollectionConfiguration::Disabled;
                    }
                    _ => return Err(self.error(node, "stdlib_path is not supported")),
                }
                Ok(Value::Nil)
            }
            "unreferenced!" | "implicitly_returns_nil!" => {
                let value = !matches!(arguments.first(), Some(Value::Boolean(false) | Value::Nil));
                let Some(Receiver::Target(target)) = self.receivers.last_mut() else {
                    return Err(self.error(node, "allowed only in a target"));
                };
                if name == "unreferenced!" {
                    target.unreferenced = value;
                } else {
                    target.implicitly_returns_nil = value;
                }
                Ok(Value::Nil)
            }
            "configure_code_diagnostics" => {
                let given = match arguments.first() {
                    Some(value) => Some(self.configuration_of(node, value)?),
                    None => None,
                };
                let has_block = call.block().is_some();
                match self.receivers.last_mut() {
                    Some(Receiver::Target(target)) => {
                        if let Some(given) = given {
                            target.code_diagnostics_config.merge(&given);
                        }
                    }
                    Some(Receiver::Group(group)) => {
                        if has_block {
                            match (&mut group.code_diagnostics_config, given) {
                                (Some(existing), Some(given)) => existing.merge(&given),
                                (Some(_), None) => {}
                                (None, given) => {
                                    let configuration = match given {
                                        Some(given) => given,
                                        None => {
                                            return Err(self.error(
                                                node,
                                                "a group needs a configuration to start from",
                                            ));
                                        }
                                    };
                                    group.code_diagnostics_config = Some(configuration);
                                }
                            }
                        } else {
                            group.code_diagnostics_config = given;
                        }
                    }
                    _ => return Err(self.error(node, "allowed only in a target or group")),
                }
                if has_block {
                    self.evaluate_block(call, vec![Value::TargetConfiguration])?;
                }
                Ok(Value::Nil)
            }
            _ => Err(self.error(node, "unsupported method call")),
        }
    }

    fn evaluate_method(
        &mut self,
        node: &PrismNode,
        call: &ruby_prism::CallNode,
        receiver: Value,
        name: &str,
        arguments: Vec<Value>,
    ) -> Result<Value, String> {
        match (&receiver, name) {
            (Value::Constant(path), _) if path == &["Pathname"] && name == "glob" => {
                let patterns = self.strings(node, &arguments)?;
                let mut paths = Vec::new();
                for pattern in patterns {
                    for path in super::pattern::glob(&self.base_directory, &pattern) {
                        let relative = path
                            .strip_prefix(&self.base_directory)
                            .unwrap_or(&path)
                            .to_path_buf();
                        paths.push(Value::Path(relative));
                    }
                }
                Ok(Value::Array(paths))
            }
            (Value::Constant(path), _)
                if path.len() == 3
                    && path[0] == "Steep"
                    && path[1] == "Diagnostic"
                    && path[2] == "Ruby" =>
            {
                Configuration::preset(name)
                    .map(Value::Configuration)
                    .ok_or_else(|| self.error(node, "unknown diagnostics preset"))
            }
            (Value::TargetConfiguration, "[]=") => {
                let [Value::Constant(path), severity] = arguments.as_slice() else {
                    return Err(self.error(node, "unsupported diagnostics configuration"));
                };
                let Some(class_name) = path.last().cloned() else {
                    return Err(self.error(node, "unsupported diagnostics configuration"));
                };
                let severity = match severity {
                    Value::Nil => None,
                    Value::Symbol(symbol) => Some(
                        Severity::parse(symbol)
                            .ok_or_else(|| self.error(node, "unknown severity"))?,
                    ),
                    _ => return Err(self.error(node, "unsupported severity")),
                };
                let Some(configuration) = self.target_configuration() else {
                    return Err(self.error(node, "no diagnostics configuration"));
                };
                configuration.severities.insert(class_name, severity);
                Ok(Value::Nil)
            }
            (Value::Array(values), "each") => {
                for value in values.clone() {
                    self.evaluate_block(call, vec![value])?;
                }
                Ok(receiver)
            }
            (Value::Path(path), "basename") => {
                let suffix = self.strings(node, &arguments)?.into_iter().next();
                let file_name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let base_name = match suffix {
                    Some(suffix) => file_name
                        .strip_suffix(&suffix)
                        .map(str::to_string)
                        .unwrap_or(file_name),
                    None => file_name,
                };
                Ok(Value::Path(PathBuf::from(base_name)))
            }
            (Value::Path(path), "to_s") => Ok(Value::String(path.to_string_lossy().into_owned())),
            (Value::String(string), "to_s") => Ok(Value::String(string.clone())),
            (Value::String(string) | Value::Symbol(string), "to_sym") => {
                Ok(Value::Symbol(string.clone()))
            }
            _ => Err(self.error(node, "unsupported method call")),
        }
    }
}

pub fn parse(base_directory: &Path, source: &str) -> Result<Project, String> {
    let result = ruby_prism::parse(source.as_bytes());
    if let Some(error) = result.errors().next() {
        return Err(format!("Steepfile has a syntax error: {}", error.message()));
    }
    let mut interpreter = Interpreter {
        source: source.as_bytes(),
        base_directory: base_directory.to_path_buf(),
        locals: HashMap::default(),
        constants: HashMap::default(),
        receivers: vec![Receiver::Project],
        global_library: LibraryDsl::default(),
        targets: Vec::new(),
    };
    let program = result.node();
    let Some(program) = program.as_program_node() else {
        return Err("Steepfile is not a program".to_string());
    };
    for statement in program.statements().body().iter() {
        interpreter.evaluate(&statement)?;
    }
    Ok(Project {
        base_directory: base_directory.to_path_buf(),
        targets: interpreter.targets,
        global_options: interpreter.global_library.options,
        path_cache: PathCache::default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::configuration::Severity;

    fn project(source: &str) -> Project {
        parse(Path::new("/project"), source).expect("the Steepfile is read")
    }

    #[test]
    fn reads_the_patterns_of_a_target() {
        let project = project(
            r#"
target :application do
  check "lib"
  check "app/models/*.rb", inline: true
  ignore "lib/generated"
  signature "sig"
  ignore_signature "sig/broken"
end
"#,
        );
        let [target] = project.targets.as_slice() else {
            panic!("the Steepfile has one target");
        };
        assert_eq!(target.name, "application");
        assert_eq!(target.source_pattern.patterns, ["lib"]);
        assert_eq!(target.source_pattern.ignores, ["lib/generated"]);
        assert_eq!(target.inline_source_pattern.patterns, ["app/models/*.rb"]);
        assert_eq!(target.signature_pattern.patterns, ["sig"]);
        assert_eq!(target.signature_pattern.ignores, ["sig/broken"]);
        assert!(target.is_possible_source_file(Path::new("lib/user.rb")));
        assert!(!target.is_possible_source_file(Path::new("lib/generated/user.rb")));
        assert!(target.is_possible_inline_source_file(Path::new("app/models/user.rb")));
        assert!(target.is_possible_signature_file(Path::new("sig/user.rbs")));
        assert!(!target.is_possible_signature_file(Path::new("sig/broken/user.rbs")));
    }

    #[test]
    fn reads_library_options_of_the_project_and_of_a_target() {
        let project = project(
            r#"
library "pathname"

target :application do
  check "lib"
  repo_path "vendor/rbs"
  library "set", "json"
  collection_config "config/rbs_collection.yaml"
end

target :test do
  check "test"
  disable_collection
end

target :script do
  check "bin"
end
"#,
        );
        assert_eq!(project.global_options.libraries, ["pathname"]);
        let application = &project.targets[0];
        let options = project.options(application);
        assert_eq!(options.libraries, ["set", "json"]);
        assert_eq!(options.repo_paths, [PathBuf::from("vendor/rbs")]);
        assert_eq!(
            options.collection_config_path,
            CollectionConfiguration::Path(PathBuf::from("/project/config/rbs_collection.yaml"))
        );
        assert_eq!(
            project.options(&project.targets[1]).collection_config_path,
            CollectionConfiguration::Disabled
        );
        assert_eq!(project.options(&project.targets[2]).libraries, ["pathname"]);
    }

    #[test]
    fn reads_flags_of_a_target() {
        let project = project(
            r#"
target :application do
  check "lib"
  implicitly_returns_nil! true
end

target :test do
  check "test"
  unreferenced!
  implicitly_returns_nil! false
end
"#,
        );
        assert!(project.targets[0].implicitly_returns_nil);
        assert!(!project.targets[0].unreferenced);
        assert!(project.targets[1].unreferenced);
        assert!(!project.targets[1].implicitly_returns_nil);
    }

    #[test]
    fn reads_diagnostics_configurations_given_by_presets_and_blocks() {
        let project = project(
            r#"
D = Steep::Diagnostic

target :application do
  check "lib"

  configure_code_diagnostics(D::Ruby.strict) do |hash|
    hash[D::Ruby::NoMethod] = :information
    hash[D::Ruby::UnknownConstant] = nil
  end
end

target :test do
  check "test"
end
"#,
        );
        let configuration = &project.targets[0].code_diagnostics_config;
        assert_eq!(
            configuration.severity_for("NoMethod"),
            Some(Severity::Information)
        );
        assert_eq!(configuration.severity_for("UnknownConstant"), None);
        assert_eq!(
            configuration.severity_for("FallbackAny"),
            Configuration::strict().severity_for("FallbackAny")
        );
        assert_eq!(
            project.targets[1].code_diagnostics_config,
            Configuration::default_configuration()
        );
    }

    #[test]
    fn gives_a_group_the_configuration_of_its_target_unless_the_group_configures_its_own() {
        let project = project(
            r#"
target :application do
  check "lib"
  configure_code_diagnostics(Steep::Diagnostic::Ruby.all_error)

  group :legacy do
    check "lib/legacy"
    configure_code_diagnostics(Steep::Diagnostic::Ruby.silent)
  end

  group :models do
    check "lib/models"
  end
end
"#,
        );
        let target = &project.targets[0];
        let [legacy, models] = target.groups.as_slice() else {
            panic!("the target has two groups");
        };
        assert_eq!(legacy.name, "legacy");
        assert_eq!(legacy.code_diagnostics_config, Configuration::silent());
        assert_eq!(models.code_diagnostics_config, Configuration::all_error());
        assert_eq!(
            target.code_diagnostics_config_for_source(Path::new("lib/legacy/user.rb")),
            &Configuration::silent()
        );
        assert_eq!(
            target.code_diagnostics_config_for_source(Path::new("lib/user.rb")),
            &Configuration::all_error()
        );
    }

    #[test]
    fn evaluates_local_variables_and_each_over_an_array() {
        let project = project(
            r#"
directories = ["app", "lib"]
directories.each do |directory|
  target directory.to_sym do
    check directory
  end
end
"#,
        );
        let names: Vec<_> = project
            .targets
            .iter()
            .map(|target| target.name.as_str())
            .collect();
        assert_eq!(names, ["app", "lib"]);
        assert_eq!(project.targets[1].source_pattern.patterns, ["lib"]);
    }

    #[test]
    fn reports_a_method_call_outside_the_dsl_with_its_line() {
        let error = parse(
            Path::new("/project"),
            "target :application do\n  check \"lib\"\n  require \"steep\"\nend\n",
        )
        .expect_err("require is not a method of the DSL");
        assert!(
            error.starts_with("Steepfile:3: unsupported method call: `require \"steep\"`"),
            "{error}"
        );
    }

    #[test]
    fn reports_a_syntax_error() {
        let error = parse(Path::new("/project"), "target :application do\n")
            .expect_err("the block is not closed");
        assert!(
            error.starts_with("Steepfile has a syntax error:"),
            "{error}"
        );
    }
}
