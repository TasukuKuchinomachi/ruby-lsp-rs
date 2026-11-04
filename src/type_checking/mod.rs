use serde_json::Value;

const DEFAULT_MAXIMUM_LENGTH: usize = 30;

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
}

impl Default for TypeCheckerSettings {
    fn default() -> Self {
        TypeCheckerSettings {
            enabled: true,
            diagnostics_enabled: true,
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

#[derive(Default)]
pub struct TypeCheckingState {
    pub settings: TypeCheckerSettings,
}

impl TypeCheckingState {
    pub fn is_active(&self) -> bool {
        false
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
