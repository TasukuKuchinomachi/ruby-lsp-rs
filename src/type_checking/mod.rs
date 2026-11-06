pub mod inlay_hints;
pub mod lsp_formatter;
pub mod worker;

use std::path::{Path, PathBuf};
use std::sync::Once;
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender};
use serde_json::{Value, json};

use crate::position::Position;
use inlay_hints::TypeHintKind;
use worker::{Answer, Command, CompletionAnswer, Event, GotoAnswer, GotoKind, HoverAnswer, Query, TypeHint};

const DEFAULT_MAXIMUM_LENGTH: usize = 30;
const ELLIPSIS: &str = "…";
const INLAY_HINT_KIND_TYPE: u8 = 1;
const QUERY_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlayHintSettings {
    pub method_return_types: bool,
    pub attribute_types: bool,
    pub variable_types: bool,
    pub block_parameter_types: bool,
    pub method_parameter_types: bool,
    pub hide_obvious_types: bool,
    pub hide_untyped: bool,
    pub maximum_length: Option<usize>,
}

impl Default for InlayHintSettings {
    fn default() -> Self {
        InlayHintSettings {
            method_return_types: true,
            attribute_types: true,
            variable_types: true,
            block_parameter_types: true,
            method_parameter_types: true,
            hide_obvious_types: true,
            hide_untyped: true,
            maximum_length: Some(DEFAULT_MAXIMUM_LENGTH),
        }
    }
}

impl InlayHintSettings {
    pub fn merge(&mut self, configuration: &Value) {
        let Some(configuration) = configuration.as_object() else {
            return;
        };
        merge_boolean(&mut self.method_return_types, configuration.get("methodReturnTypes"));
        merge_boolean(&mut self.attribute_types, configuration.get("attributeTypes"));
        merge_boolean(&mut self.variable_types, configuration.get("variableTypes"));
        merge_boolean(
            &mut self.block_parameter_types,
            configuration.get("blockParameterTypes"),
        );
        merge_boolean(
            &mut self.method_parameter_types,
            configuration.get("methodParameterTypes"),
        );
        merge_boolean(&mut self.hide_obvious_types, configuration.get("hideObviousTypes"));
        merge_boolean(&mut self.hide_untyped, configuration.get("hideUntyped"));
        match configuration.get("maxLength") {
            Some(Value::Null) => self.maximum_length = None,
            Some(value) => {
                if let Some(maximum_length) = value.as_u64() {
                    self.maximum_length = Some(maximum_length as usize);
                }
            }
            None => {}
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DefinitionTarget {
    #[default]
    Implementation,
    Signature,
    Both,
}

impl DefinitionTarget {
    fn from_name(name: &str) -> Option<DefinitionTarget> {
        match name {
            "implementation" => Some(DefinitionTarget::Implementation),
            "signature" => Some(DefinitionTarget::Signature),
            "both" => Some(DefinitionTarget::Both),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeCheckerSettings {
    pub enabled: bool,
    pub diagnostics_enabled: bool,
    pub inlay_hints: InlayHintSettings,
    pub definition_target: DefinitionTarget,
    pub infer_more_than_steep: bool,
}

impl Default for TypeCheckerSettings {
    fn default() -> Self {
        TypeCheckerSettings {
            enabled: true,
            diagnostics_enabled: true,
            infer_more_than_steep: false,
            inlay_hints: InlayHintSettings::default(),
            definition_target: DefinitionTarget::default(),
        }
    }
}

impl TypeCheckerSettings {
    pub fn merge(&mut self, configuration: &Value) {
        let Some(object) = configuration.as_object() else {
            return;
        };
        merge_boolean(&mut self.enabled, object.get("enabled"));
        merge_boolean(&mut self.infer_more_than_steep, object.get("inferMoreThanSteep"));
        let diagnostics = configuration["diagnostics"].as_object();
        merge_boolean(
            &mut self.diagnostics_enabled,
            diagnostics.and_then(|diagnostics| diagnostics.get("enabled")),
        );
        self.inlay_hints.merge(&configuration["inlayHints"]);
        if let Some(definition_target) = configuration["definition"]["target"]
            .as_str()
            .and_then(DefinitionTarget::from_name)
        {
            self.definition_target = definition_target;
        }
    }
}

fn merge_boolean(target: &mut bool, value: Option<&Value>) {
    if let Some(boolean) = value.and_then(Value::as_bool) {
        *target = boolean;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ServiceStatus {
    #[default]
    NotStarted,
    Loading,
    Loaded,
    Failed,
}

pub struct TypeCheckClient {
    commands: Sender<Command>,
}

impl TypeCheckClient {
    pub fn start() -> (TypeCheckClient, Receiver<Event>) {
        let (commands, command_receiver) = crossbeam_channel::unbounded();
        let (event_sender, events) = crossbeam_channel::unbounded();
        worker::spawn(command_receiver, event_sender);
        (TypeCheckClient { commands }, events)
    }

    pub fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }

    fn query(&self, query: Query) -> Option<Answer> {
        let (reply, answer) = crossbeam_channel::bounded(1);
        self.commands.send(Command::Query { query, reply }).ok()?;
        answer.recv_timeout(QUERY_TIMEOUT).ok().flatten()
    }
}

#[derive(Default)]
pub struct TypeCheckingState {
    pub settings: TypeCheckerSettings,
    pub client: Option<TypeCheckClient>,
    pub status: ServiceStatus,
}

impl TypeCheckingState {
    pub fn is_active(&self) -> bool {
        self.settings.enabled && self.status == ServiceStatus::Loaded && self.client.is_some()
    }

    fn query(&self, query: Query) -> Option<Answer> {
        if !self.is_active() {
            return None;
        }
        self.client.as_ref()?.query(query)
    }

    pub fn notify_inference(&self) {
        if let Some(client) = &self.client {
            client.send(Command::SetInferMoreThanSteep(self.settings.infer_more_than_steep));
        }
    }

    pub fn notify_text(&self, uri: &str, text: &str) {
        if let (Some(client), Some(path)) = (&self.client, checked_path(uri)) {
            client.send(Command::SetText {
                path,
                text: text.to_string(),
            });
        }
    }

    pub fn notify_closed(&self, uri: &str) {
        if let (Some(client), Some(path)) = (&self.client, checked_path(uri)) {
            client.send(Command::RevertToDisk { path });
        }
    }

    pub fn notify_file_changed_on_disk(&self, file_path: &Path) {
        let Some(client) = &self.client else { return };
        if is_project_configuration_file(file_path) {
            client.send(Command::ProjectConfigurationChanged);
        } else if is_checked_file(file_path) {
            client.send(Command::RevertToDisk {
                path: file_path.to_path_buf(),
            });
        }
    }

    pub fn diagnostics(&self, uri: &str) -> Option<Vec<Value>> {
        if !self.settings.diagnostics_enabled {
            return None;
        }
        match self.query(Query::Diagnostics {
            path: checked_path(uri)?,
        })? {
            Answer::Diagnostics(diagnostics) => diagnostics,
            _ => None,
        }
    }

    pub fn hover(&self, uri: &str, position: Position) -> Option<HoverAnswer> {
        match self.query(Query::Hover {
            path: checked_path(uri)?,
            position,
        })? {
            Answer::Hover(hover) => hover,
            _ => None,
        }
    }

    pub fn completion(&self, uri: &str, position: Position) -> Option<CompletionAnswer> {
        match self.query(Query::Completion {
            path: checked_path(uri)?,
            position,
        })? {
            Answer::Completion(completion) => completion,
            _ => None,
        }
    }

    pub fn signature_help(&self, uri: &str, position: Position) -> Option<Value> {
        match self.query(Query::SignatureHelp {
            path: checked_path(uri)?,
            position,
        })? {
            Answer::SignatureHelp(signature_help) => signature_help,
            _ => None,
        }
    }

    pub fn goto(&self, kind: GotoKind, uri: &str, position: Position) -> Option<GotoAnswer> {
        match self.query(Query::Goto {
            kind,
            path: checked_path(uri)?,
            position,
        })? {
            Answer::Goto(goto) => Some(goto),
            _ => None,
        }
    }

    pub fn inlay_hints(&self, uri: &str) -> Vec<Value> {
        let Some(path) = checked_path(uri) else {
            return Vec::new();
        };
        match self.query(Query::InlayHints { path }) {
            Some(Answer::InlayHints(hints)) => hints
                .iter()
                .filter(|hint| self.settings.inlay_hints.shows(hint))
                .map(|hint| inlay_hint_value(hint, self.settings.inlay_hints.maximum_length))
                .collect(),
            _ => Vec::new(),
        }
    }
}

fn checked_path(uri: &str) -> Option<PathBuf> {
    let path = crate::uri::to_path_buf(uri)?;
    is_checked_file(&path).then_some(path)
}

fn is_checked_file(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension == "rb" || extension == "rbs")
}

pub fn is_project_configuration_file(path: &Path) -> bool {
    let file_name = path.file_name().and_then(|name| name.to_str()).unwrap_or_default();
    matches!(
        file_name,
        "Steepfile" | "rbs_collection.yaml" | "rbs_collection.lock.yaml"
    )
}

pub fn is_signature_related_file(path: &Path) -> bool {
    is_project_configuration_file(path) || path.extension().is_some_and(|extension| extension == "rbs")
}

pub fn install_panic_hook() {
    static INSTALLED: Once = Once::new();
    INSTALLED.call_once(steep::ruby_exception::install_panic_hook);
}

impl InlayHintSettings {
    fn shows(&self, hint: &TypeHint) -> bool {
        let kind_enabled = match hint.kind {
            TypeHintKind::MethodReturnType => self.method_return_types,
            TypeHintKind::AttributeType => self.attribute_types,
            TypeHintKind::VariableType => self.variable_types,
            TypeHintKind::BlockParameterType => self.block_parameter_types,
            TypeHintKind::MethodParameterType | TypeHintKind::KeywordParameterType => self.method_parameter_types,
        };
        kind_enabled && !(self.hide_obvious_types && hint.is_obvious) && !(self.hide_untyped && hint.is_untyped)
    }
}

pub fn truncate_type(type_text: &str, maximum_length: Option<usize>) -> Option<String> {
    let maximum_length = maximum_length?;
    if type_text.chars().count() <= maximum_length {
        return None;
    }
    let kept: String = type_text
        .chars()
        .take(maximum_length.saturating_sub(ELLIPSIS.chars().count()))
        .collect();
    Some(format!("{kept}{ELLIPSIS}"))
}

pub struct TypeHintLabel {
    pub label: String,
    pub tooltip: Option<String>,
    pub padding_left: bool,
    pub padding_right: bool,
}

pub fn type_hint_label(kind: TypeHintKind, type_text: &str, maximum_length: Option<usize>) -> TypeHintLabel {
    let truncated = truncate_type(type_text, maximum_length);
    let shown = truncated.as_deref().unwrap_or(type_text);
    let tooltip = truncated.as_ref().map(|_| type_text.to_string());
    match kind {
        TypeHintKind::VariableType | TypeHintKind::BlockParameterType | TypeHintKind::MethodParameterType => {
            TypeHintLabel {
                label: format!(": {shown}"),
                tooltip,
                padding_left: false,
                padding_right: false,
            }
        }
        TypeHintKind::MethodReturnType => TypeHintLabel {
            label: format!("→ {shown}"),
            tooltip,
            padding_left: true,
            padding_right: false,
        },
        TypeHintKind::AttributeType => TypeHintLabel {
            label: shown.to_string(),
            tooltip,
            padding_left: false,
            padding_right: true,
        },
        TypeHintKind::KeywordParameterType => TypeHintLabel {
            label: shown.to_string(),
            tooltip,
            padding_left: true,
            padding_right: false,
        },
    }
}

fn inlay_hint_value(hint: &TypeHint, maximum_length: Option<usize>) -> Value {
    let label = type_hint_label(hint.kind, &hint.type_text, maximum_length);
    let mut value = json!({
        "position": hint.position,
        "label": label.label,
        "kind": INLAY_HINT_KIND_TYPE,
        "paddingLeft": label.padding_left,
        "paddingRight": label.padding_right,
    });
    if let Some(tooltip) = label.tooltip {
        value["tooltip"] = json!(tooltip);
    }
    value
}

pub fn merge_hover(type_markdown: Option<String>, existing_markdown: Option<String>) -> Option<String> {
    match (type_markdown, existing_markdown) {
        (Some(type_markdown), Some(existing_markdown)) => {
            Some(format!("{type_markdown}\n\n---\n\n{existing_markdown}"))
        }
        (type_markdown, existing_markdown) => type_markdown.or(existing_markdown),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn parses_type_checker_settings() {
        let mut settings = TypeCheckerSettings::default();
        settings.merge(&json!({
            "enabled": false,
            "diagnostics": { "enabled": false },
            "inlayHints": { "variableTypes": false, "hideUntyped": false, "maxLength": null },
        }));
        assert!(!settings.enabled);
        assert!(!settings.diagnostics_enabled);
        assert!(!settings.inlay_hints.variable_types);
        assert!(settings.inlay_hints.method_return_types);
        assert!(!settings.inlay_hints.hide_untyped);
        assert_eq!(settings.inlay_hints.maximum_length, None);
        assert!(!settings.infer_more_than_steep);

        settings.merge(&json!({ "inferMoreThanSteep": true }));
        assert!(settings.infer_more_than_steep);

        settings.merge(&json!({ "inlayHints": { "maxLength": 12 } }));
        assert!(!settings.enabled);
        assert_eq!(settings.inlay_hints.maximum_length, Some(12));
    }

    #[test]
    fn parses_the_definition_target() {
        let mut settings = TypeCheckerSettings::default();
        assert_eq!(settings.definition_target, DefinitionTarget::Implementation);
        settings.merge(&json!({ "definition": { "target": "both" } }));
        assert_eq!(settings.definition_target, DefinitionTarget::Both);
        settings.merge(&json!({ "definition": { "target": "unknown" } }));
        assert_eq!(settings.definition_target, DefinitionTarget::Both);
        settings.merge(&json!({ "definition": { "target": "signature" } }));
        assert_eq!(settings.definition_target, DefinitionTarget::Signature);
    }

    #[test]
    fn keeps_defaults_for_missing_or_invalid_settings() {
        let mut settings = TypeCheckerSettings::default();
        settings.merge(&Value::Null);
        settings.merge(&json!({ "enabled": "yes", "inlayHints": { "maxLength": "long" } }));
        assert_eq!(settings, TypeCheckerSettings::default());
        assert_eq!(settings.inlay_hints.maximum_length, Some(30));
    }
}
