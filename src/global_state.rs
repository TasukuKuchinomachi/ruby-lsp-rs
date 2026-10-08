use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde_json::{Value, json};

use crate::indexer::Index;
use crate::position::Encoding;
use crate::ruby_environment::RubyEnvironment;

pub mod message_type {
    pub const ERROR: u8 = 1;
    pub const WARNING: u8 = 2;
    pub const INFO: u8 = 3;
    pub const LOG: u8 = 4;
}

#[derive(Debug, Clone, PartialEq)]
pub struct PendingNotification {
    pub method: String,
    pub params: Value,
}

impl PendingNotification {
    pub fn window_log_message(message: &str, message_type: u8) -> PendingNotification {
        PendingNotification {
            method: "window/logMessage".to_string(),
            params: json!({ "type": message_type, "message": message }),
        }
    }
}

pub struct DetectionResult {
    pub value: String,
    pub reason: String,
}

impl DetectionResult {
    fn new(value: &str, reason: &str) -> DetectionResult {
        DetectionResult { value: value.to_string(), reason: reason.to_string() }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ClientCapabilities {
    pub supports_watching_files: bool,
    pub supports_request_delegation: bool,
    pub window_show_message_supports_extra_properties: bool,
    pub supported_resource_operations: Vec<String>,
    pub supports_progress: bool,
    pub supports_diagnostic_refresh: bool,
    pub supports_code_lens_refresh: bool,
}

impl ClientCapabilities {
    pub fn apply_client_capabilities(&mut self, capabilities: &Value) {
        let workspace = &capabilities["workspace"];
        let file_watching = &workspace["didChangeWatchedFiles"];
        if truthy(&file_watching["dynamicRegistration"]) && truthy(&file_watching["relativePatternSupport"]) {
            self.supports_watching_files = true;
        }

        self.supports_request_delegation = truthy(&capabilities["experimental"]["requestDelegation"]);
        if let Some(operations) = workspace["workspaceEdit"]["resourceOperations"].as_array() {
            self.supported_resource_operations =
                operations.iter().filter_map(|operation| operation.as_str().map(str::to_string)).collect();
        }

        self.window_show_message_supports_extra_properties =
            truthy(&capabilities["window"]["showMessage"]["messageActionItem"]["additionalPropertiesSupport"]);

        if truthy(&capabilities["window"]["workDoneProgress"]) {
            self.supports_progress = true;
        }

        self.supports_diagnostic_refresh = truthy(&workspace["diagnostics"]["refreshSupport"]);
        self.supports_code_lens_refresh = truthy(&workspace["codeLens"]["refreshSupport"]);
    }

    pub fn supports_rename(&self) -> bool {
        self.supported_resource_operations.iter().any(|operation| operation == "rename")
    }
}

fn truthy(value: &Value) -> bool {
    !matches!(value, Value::Null | Value::Bool(false))
}

#[derive(Debug, Clone, Default)]
pub struct RequestConfig {
    configuration: serde_json::Map<String, Value>,
}

impl RequestConfig {
    pub fn new(configuration: Value) -> RequestConfig {
        RequestConfig { configuration: configuration.as_object().cloned().unwrap_or_default() }
    }

    pub fn is_enabled(&self, feature: &str) -> bool {
        self.configuration.get("enableAll").is_some_and(truthy) || self.configuration.get(feature).is_some_and(truthy)
    }

    pub fn merge(&mut self, configuration: &Value) {
        if let Some(object) = configuration.as_object() {
            for (key, value) in object {
                self.configuration.insert(key.clone(), value.clone());
            }
        }
    }
}

pub struct GlobalState {
    pub workspace_uri: String,
    pub encoding: Encoding,
    pub formatter: String,
    pub linters: Vec<String>,
    pub test_library: String,
    pub has_type_checker: bool,
    pub index: Arc<Mutex<Index>>,
    pub client_capabilities: ClientCapabilities,
    pub enabled_feature_flags: HashMap<String, bool>,
    pub addon_settings: serde_json::Map<String, Value>,
    pub telemetry_machine_id: Option<String>,
    pub feature_configuration: HashMap<String, RequestConfig>,
    pub top_level_bundle: bool,
    pub ruby_environment: Arc<RubyEnvironment>,
    pub dependency_paths: Vec<String>,
    pub rubylibdir: Option<String>,
    pub ruby_command: Vec<String>,
}

impl Default for GlobalState {
    fn default() -> Self {
        GlobalState::new()
    }
}

impl GlobalState {
    pub fn new() -> GlobalState {
        let current_directory = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
        let mut feature_configuration = HashMap::new();
        feature_configuration.insert(
            "inlayHint".to_string(),
            RequestConfig::new(json!({ "enableAll": false, "implicitRescue": false, "implicitHashValue": false })),
        );
        feature_configuration
            .insert("codeLens".to_string(), RequestConfig::new(json!({ "enableAll": false, "enableTestCodeLens": true })));

        GlobalState {
            workspace_uri: crate::uri::from_path_buf(&current_directory),
            encoding: Encoding::Utf8,
            formatter: "auto".to_string(),
            linters: Vec::new(),
            test_library: "minitest".to_string(),
            has_type_checker: true,
            index: Arc::new(Mutex::new(Index::default())),
            client_capabilities: ClientCapabilities::default(),
            enabled_feature_flags: HashMap::new(),
            addon_settings: serde_json::Map::new(),
            telemetry_machine_id: None,
            feature_configuration,
            top_level_bundle: false,
            ruby_environment: Arc::new(RubyEnvironment::default()),
            dependency_paths: Vec::new(),
            rubylibdir: None,
            ruby_command: vec!["ruby".to_string()],
        }
    }

    /// Recovers from poisoning instead of unwrapping, since panics are caught per request and must not break later ones
    pub fn index(&self) -> MutexGuard<'_, Index> {
        self.index.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn workspace_path(&self) -> String {
        crate::uri::to_path(&self.workspace_uri).unwrap_or_else(|| self.workspace_uri.clone())
    }

    pub fn encoding_name(&self) -> &'static str {
        self.encoding.kind_name()
    }

    pub fn feature_configuration(&self, feature_name: &str) -> Option<&RequestConfig> {
        self.feature_configuration.get(feature_name)
    }

    pub fn is_feature_enabled(&self, flag: &str) -> bool {
        self.enabled_feature_flags.get("all").copied().unwrap_or(false)
            || self.enabled_feature_flags.get(flag).copied().unwrap_or(false)
    }

    pub fn settings_for_addon(&self, addon_name: &str) -> Option<&Value> {
        self.addon_settings.get(addon_name)
    }

    pub fn supports_watching_files(&self) -> bool {
        self.client_capabilities.supports_watching_files
    }

    pub fn not_in_dependencies(&self, file_path: &str) -> bool {
        if self.dependency_paths.is_empty() {
            return false;
        }
        !self.dependency_paths.iter().any(|path| file_path.starts_with(path.as_str()))
            && !self.rubylibdir.as_ref().is_some_and(|rubylibdir| file_path.starts_with(rubylibdir.as_str()))
    }

    pub fn workspace_path_from_options(options: &Value) -> Option<PathBuf> {
        options["workspaceFolders"][0]["uri"]
            .as_str()
            .or_else(|| options["rootUri"].as_str())
            .and_then(crate::uri::to_path_buf)
            .or_else(|| options["rootPath"].as_str().map(PathBuf::from))
    }

    pub fn apply_options(&mut self, options: &Value, environment: Arc<RubyEnvironment>) -> Vec<PendingNotification> {
        let mut notifications = Vec::new();
        self.ruby_environment = environment;
        let direct_dependencies = self.ruby_environment.direct_dependencies.clone();
        let all_dependencies = self.ruby_environment.all_dependencies.clone();
        self.top_level_bundle = !self.ruby_environment.locked_gems.is_empty() || !all_dependencies.is_empty();
        self.rubylibdir = self.ruby_environment.rubylibdir.clone();
        self.dependency_paths = dependency_paths(&self.ruby_environment);

        if let Some(workspace_uri) = options["workspaceFolders"][0]["uri"].as_str() {
            self.workspace_uri = workspace_uri.to_string();
        }

        let initialization_options = &options["initializationOptions"];
        if let Some(ruby_command) = initialization_options["rubyCommand"].as_array() {
            let ruby_command: Vec<String> =
                ruby_command.iter().filter_map(|part| part.as_str().map(str::to_string)).collect();
            if !ruby_command.is_empty() {
                self.ruby_command = ruby_command;
            }
        }
        let rubocop_has_addon = self.ruby_environment.rubocop_version.as_deref().is_some_and(|version| version_at_least(version, &[1, 70, 0]));

        let specified_formatter = initialization_options["formatter"].as_str();
        if let Some(specified_formatter) = specified_formatter {
            self.formatter = specified_formatter.to_string();
            if specified_formatter != "auto" {
                notifications.push(PendingNotification::window_log_message(
                    &format!("Using formatter specified by user: {}", self.formatter),
                    message_type::LOG,
                ));
            }
            if specified_formatter == "rubocop" && !rubocop_has_addon {
                self.formatter = "rubocop_internal".to_string();
            }
        }

        if self.formatter == "auto" {
            let formatter_result = self.detect_formatter(&direct_dependencies, &all_dependencies);
            self.formatter = formatter_result.value;
            notifications.push(PendingNotification::window_log_message(
                &format!("Auto detected formatter: {} ({})", self.formatter, formatter_result.reason),
                message_type::LOG,
            ));
        }

        let mut specified_linters: Option<Vec<String>> = initialization_options["linters"]
            .as_array()
            .map(|linters| linters.iter().filter_map(|linter| linter.as_str().map(str::to_string)).collect());
        let linters_include_rubocop =
            specified_linters.as_ref().is_some_and(|linters| linters.iter().any(|linter| linter == "rubocop"));

        if specified_formatter == Some("rubocop") || linters_include_rubocop {
            notifications.push(PendingNotification::window_log_message(
                "Formatter is configured to be `rubocop`. As of RuboCop v1.70.0, this identifier activates the add-on\nimplemented in the rubocop gem itself instead of the internal integration provided by the Ruby LSP.\n\nIf you wish to use the internal integration, please configure the formatter as `rubocop_internal`.\n",
                message_type::WARNING,
            ));
        }

        if linters_include_rubocop && !rubocop_has_addon
            && let Some(linters) = specified_linters.as_mut()
        {
            linters.retain(|linter| linter != "rubocop");
            linters.push("rubocop_internal".to_string());
        }

        match specified_linters {
            Some(linters) => {
                self.linters = linters;
                notifications.push(PendingNotification::window_log_message(
                    &format!("Using linters specified by user: {}", self.linters.join(", ")),
                    message_type::LOG,
                ));
            }
            None => {
                let linter_results = self.detect_linters(&direct_dependencies, &all_dependencies);
                self.linters = linter_results.iter().map(|result| result.value.clone()).collect();
                let linter_messages: Vec<String> =
                    linter_results.iter().map(|result| format!("{} ({})", result.value, result.reason)).collect();
                notifications.push(PendingNotification::window_log_message(
                    &format!("Auto detected linters: {}", linter_messages.join(", ")),
                    message_type::LOG,
                ));
            }
        }

        let test_library_result = self.detect_test_library(&direct_dependencies);
        self.test_library = test_library_result.value;
        notifications.push(PendingNotification::window_log_message(
            &format!("Detected test library: {} ({})", self.test_library, test_library_result.reason),
            message_type::LOG,
        ));

        let typechecker_result = detect_typechecker(&all_dependencies);
        self.has_type_checker = typechecker_result.is_some();
        if let Some(result) = typechecker_result {
            notifications.push(PendingNotification::window_log_message(
                &format!(
                    "Ruby LSP detected this is a Sorbet project ({}) and will defer to the Sorbet LSP for some functionality",
                    result.reason
                ),
                message_type::LOG,
            ));
        }

        let encodings: Vec<&str> = options["capabilities"]["general"]["positionEncodings"]
            .as_array()
            .map(|encodings| encodings.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        self.encoding = if encodings.is_empty() {
            Encoding::Utf16
        } else if encodings.contains(&"utf-8") {
            Encoding::Utf8
        } else if encodings.contains(&"utf-16") {
            Encoding::Utf16
        } else {
            Encoding::Utf32
        };
        {
            let mut index = self.index();
            std::sync::Arc::make_mut(&mut index.configuration).encoding = self.encoding;
        }

        if options["capabilities"].is_object() {
            self.client_capabilities.apply_client_capabilities(&options["capabilities"]);
        }

        if let Some(addon_settings) = initialization_options["addonSettings"].as_object() {
            for (key, value) in addon_settings {
                self.addon_settings.insert(key.clone(), value.clone());
            }
        }

        if let Some(flags) = initialization_options["enabledFeatureFlags"].as_object() {
            self.enabled_feature_flags = flags.iter().map(|(key, value)| (key.clone(), truthy(value))).collect();
        }

        self.telemetry_machine_id = initialization_options["telemetryMachineId"].as_str().map(str::to_string);

        if let Some(features) = initialization_options["featuresConfiguration"].as_object() {
            for (feature_name, configuration) in features {
                if let Some(request_config) = self.feature_configuration.get_mut(feature_name) {
                    request_config.merge(configuration);
                }
            }
        }

        notifications
    }

    fn detect_formatter(&self, direct_dependencies: &[String], all_dependencies: &[String]) -> DetectionResult {
        if direct_dependencies.iter().any(|dependency| dependency.starts_with("rubocop")) {
            return DetectionResult::new("rubocop_internal", "direct dependency matching /^rubocop/");
        }
        if direct_dependencies.iter().any(|dependency| dependency == "syntax_tree") {
            return DetectionResult::new("syntax_tree", "direct dependency");
        }
        if all_dependencies.iter().any(|dependency| dependency == "rubocop") && self.dot_rubocop_yml_present() {
            return DetectionResult::new("rubocop_internal", "transitive dependency with .rubocop.yml present");
        }
        DetectionResult::new("none", "no formatter detected")
    }

    fn detect_linters(&self, direct_dependencies: &[String], all_dependencies: &[String]) -> Vec<DetectionResult> {
        let mut linters = Vec::new();
        if direct_dependencies.iter().any(|dependency| dependency.starts_with("rubocop")) {
            linters.push(DetectionResult::new("rubocop_internal", "direct dependency matching /^rubocop/"));
        } else if all_dependencies.iter().any(|dependency| dependency == "rubocop") && self.dot_rubocop_yml_present() {
            linters.push(DetectionResult::new("rubocop_internal", "transitive dependency with .rubocop.yml present"));
        }
        linters
    }

    fn detect_test_library(&self, dependencies: &[String]) -> DetectionResult {
        if dependencies.iter().any(|dependency| dependency.starts_with("rspec")) {
            DetectionResult::new("rspec", "direct dependency matching /^rspec/")
        } else if self.bin_rails_present() {
            DetectionResult::new("rails", "bin/rails present")
        } else if dependencies.iter().any(|dependency| dependency == "minitest") {
            DetectionResult::new("minitest", "direct dependency matching /^minitest$/")
        } else if dependencies.iter().any(|dependency| dependency.starts_with("test-unit")) {
            DetectionResult::new("test-unit", "direct dependency matching /^test-unit/")
        } else {
            DetectionResult::new("unknown", "no test library detected")
        }
    }

    fn bin_rails_present(&self) -> bool {
        Path::new(&self.workspace_path()).join("bin/rails").exists()
    }

    fn dot_rubocop_yml_present(&self) -> bool {
        Path::new(&self.workspace_path()).join(".rubocop.yml").exists()
    }
}

fn detect_typechecker(dependencies: &[String]) -> Option<DetectionResult> {
    if std::env::var_os("RUBY_LSP_BYPASS_TYPECHECKER").is_some() {
        return None;
    }
    if !dependencies.iter().any(|dependency| dependency.starts_with("sorbet-static")) {
        return None;
    }
    Some(DetectionResult::new("sorbet", "sorbet-static in dependencies"))
}

fn version_at_least(version: &str, minimum: &[u64]) -> bool {
    let parts: Vec<u64> = version.split('.').map(|part| part.parse().unwrap_or(0)).collect();
    for (index, minimum_part) in minimum.iter().enumerate() {
        let part = parts.get(index).copied().unwrap_or(0);
        if part != *minimum_part {
            return part > *minimum_part;
        }
    }
    true
}

/// Gems are installed in `<bundle path>/gems/<name>-<version>`, so the bundle path is two levels above each gem
fn dependency_paths(environment: &RubyEnvironment) -> Vec<String> {
    let mut paths: Vec<String> = Vec::new();
    for gem in &environment.locked_gems {
        let gem_path = Path::new(&gem.full_gem_path);
        let Some(gems_directory) = gem_path.parent() else { continue };
        if gems_directory.file_name().is_none_or(|name| name != "gems") {
            continue;
        }
        let Some(bundle_path) = gems_directory.parent() else { continue };
        let bundle_path = bundle_path.to_string_lossy().into_owned();
        if !paths.contains(&bundle_path) {
            paths.push(bundle_path);
        }
    }
    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    fn environment_with(direct: &[&str], all: &[&str]) -> Arc<RubyEnvironment> {
        Arc::new(RubyEnvironment {
            direct_dependencies: direct.iter().map(|name| name.to_string()).collect(),
            all_dependencies: all.iter().map(|name| name.to_string()).collect(),
            ..RubyEnvironment::default()
        })
    }

    #[test]
    fn detects_formatter_and_linters() {
        let mut state = GlobalState::new();
        state.apply_options(&json!({}), environment_with(&["rubocop-shopify"], &["rubocop-shopify", "rubocop"]));
        assert_eq!(state.formatter, "rubocop_internal");
        assert_eq!(state.linters, vec!["rubocop_internal".to_string()]);

        let mut state = GlobalState::new();
        state.apply_options(&json!({}), environment_with(&["syntax_tree"], &["syntax_tree"]));
        assert_eq!(state.formatter, "syntax_tree");

        let mut state = GlobalState::new();
        state.apply_options(&json!({ "initializationOptions": { "formatter": "none", "linters": ["rubocop"] } }), environment_with(&[], &[]));
        assert_eq!(state.formatter, "none");
        assert_eq!(state.linters, vec!["rubocop_internal".to_string()]);
    }

    #[test]
    fn detects_test_library_and_type_checker() {
        let mut state = GlobalState::new();
        state.apply_options(&json!({}), environment_with(&["rspec-rails"], &["sorbet-static-and-runtime", "sorbet-static"]));
        assert_eq!(state.test_library, "rspec");
        assert!(state.has_type_checker);

        let mut state = GlobalState::new();
        state.apply_options(&json!({}), environment_with(&["minitest-reporters"], &[]));
        assert_eq!(state.test_library, "unknown");
        assert!(!state.has_type_checker);
    }

    #[test]
    fn negotiates_position_encoding() {
        let mut state = GlobalState::new();
        state.apply_options(&json!({ "capabilities": { "general": { "positionEncodings": ["utf-16", "utf-8"] } } }), environment_with(&[], &[]));
        assert_eq!(state.encoding, Encoding::Utf8);
        assert_eq!(state.index().configuration.encoding, Encoding::Utf8);

        let mut state = GlobalState::new();
        state.apply_options(&json!({ "capabilities": { "general": { "positionEncodings": ["utf-32"] } } }), environment_with(&[], &[]));
        assert_eq!(state.encoding, Encoding::Utf32);

        let mut state = GlobalState::new();
        state.apply_options(&json!({}), environment_with(&[], &[]));
        assert_eq!(state.encoding, Encoding::Utf16);
    }

    #[test]
    fn applies_feature_configuration_and_flags() {
        let mut state = GlobalState::new();
        state.apply_options(
            &json!({ "initializationOptions": {
                "featuresConfiguration": { "inlayHint": { "implicitRescue": true } },
                "enabledFeatureFlags": { "all": true }
            } }),
            environment_with(&[], &[]),
        );
        assert!(state.feature_configuration("inlayHint").unwrap().is_enabled("implicitRescue"));
        assert!(!state.feature_configuration("inlayHint").unwrap().is_enabled("implicitHashValue"));
        assert!(state.is_feature_enabled("anything"));
    }

    #[test]
    fn applies_client_capabilities() {
        let mut state = GlobalState::new();
        state.apply_options(
            &json!({ "capabilities": {
                "workspace": { "didChangeWatchedFiles": { "dynamicRegistration": true, "relativePatternSupport": true } },
                "window": { "workDoneProgress": true }
            } }),
            environment_with(&[], &[]),
        );
        assert!(state.supports_watching_files());
        assert!(state.client_capabilities.supports_progress);
    }
}
