use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rbs_environment::{
    DiagnosticConfiguration, Environment, Literal, Location, MethodType, SignatureDiagnostic, SourcePosition,
    SteepfileConfiguration, Type, TypeName, WorkspaceSources,
};
use ruby_type_check::project::{self, LoadedEnvironment, code_diagnostics_for, configured_diagnostics, is_signature_path};
use serde_json::{Value, json};
use type_checker::{Analysis, AnalysisOptions, HoverEntry, HoverKind, Severity, TypeHint, TypeHintKind};

use crate::document::Document;
use crate::position::{Encoding, LineIndex, Position, Range, range_covers};
use crate::requests::support::interface::LocationLink;

pub const DIAGNOSTIC_SOURCE: &str = "ruby-lsp-rs";
pub const INLAY_HINT_KIND_TYPE: u8 = 1;
const DEFAULT_MAXIMUM_LENGTH: usize = 30;
const ELLIPSIS: &str = "…";

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
        let Some(configuration) = configuration.as_object() else { return };
        merge_boolean(&mut self.method_return_types, configuration.get("methodReturnTypes"));
        merge_boolean(&mut self.attribute_types, configuration.get("attributeTypes"));
        merge_boolean(&mut self.variable_types, configuration.get("variableTypes"));
        merge_boolean(&mut self.block_parameter_types, configuration.get("blockParameterTypes"));
        merge_boolean(&mut self.method_parameter_types, configuration.get("methodParameterTypes"));
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

    pub fn shows(&self, hint: &TypeHint) -> bool {
        let kind_enabled = match hint.kind {
            TypeHintKind::MethodReturnType => self.method_return_types,
            TypeHintKind::AttributeType => self.attribute_types,
            TypeHintKind::VariableType => self.variable_types,
            TypeHintKind::BlockParameterType => self.block_parameter_types,
            TypeHintKind::MethodParameterType | TypeHintKind::KeywordParameterType => self.method_parameter_types,
        };
        kind_enabled
            && !(self.hide_obvious_types && hint.is_obvious)
            && !(self.hide_untyped && hint.hint_type.is_untyped())
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
        let Some(object) = configuration.as_object() else { return };
        merge_boolean(&mut self.enabled, object.get("enabled"));
        let diagnostics = configuration["diagnostics"].as_object();
        merge_boolean(&mut self.diagnostics_enabled, diagnostics.and_then(|diagnostics| diagnostics.get("enabled")));
        self.inlay_hints.merge(&configuration["inlayHints"]);
        if let Some(definition_target) = configuration["definition"]["target"].as_str().and_then(DefinitionTarget::from_name) {
            self.definition_target = definition_target;
        }
    }
}

fn merge_boolean(target: &mut bool, value: Option<&Value>) {
    if let Some(boolean) = value.and_then(Value::as_bool) {
        *target = boolean;
    }
}

pub type EnvironmentLoadOutcome = Result<LoadedEnvironment, String>;

pub fn load_environment(workspace_path: &Path, sources: &WorkspaceSources) -> EnvironmentLoadOutcome {
    catch_unwind(AssertUnwindSafe(|| project::load_environment(workspace_path, sources)))
        .map_err(|payload| crate::server::panic_message(&payload))
}

pub fn is_rbs_related_file(file_path: &str) -> bool {
    let file_name = Path::new(file_path).file_name().and_then(|name| name.to_str()).unwrap_or_default();
    file_path.ends_with(".rbs") || matches!(file_name, "Steepfile" | "rbs_collection.lock.yaml")
}

/// A syntax error is taken from the unsaved `source`, since it is what the user is looking at. The other problems come
/// from the environment, which reads the saved file and is reloaded when it is saved
pub fn signature_diagnostics(environment: &Environment, path: &Path, source: &str, encoding: Encoding) -> Vec<Value> {
    let diagnostics: Vec<SignatureDiagnostic> = match rbs_environment::signature_syntax_error(path, source) {
        Some(syntax_error) => vec![syntax_error],
        None => environment
            .signature_diagnostics_for(path)
            .filter(|diagnostic| diagnostic.code != rbs_environment::signature_validation::SYNTAX_ERROR)
            .cloned()
            .collect(),
    };
    diagnostics
        .iter()
        .map(|diagnostic| {
            json!({
                "range": Range::new(source_position(diagnostic.start, encoding), source_position(diagnostic.end, encoding)),
                "severity": severity_number(Severity::Error),
                "source": DIAGNOSTIC_SOURCE,
                "code": diagnostic.code,
                "message": diagnostic.message,
            })
        })
        .collect()
}

#[derive(Default)]
pub struct TypeCheckingState {
    pub settings: TypeCheckerSettings,
    pub environment: Option<Arc<Environment>>,
    pub steepfile: Option<SteepfileConfiguration>,
    analyses: HashMap<String, (u64, Arc<Analysis>)>,
}

impl TypeCheckingState {
    pub fn is_active(&self) -> bool {
        self.settings.enabled && self.environment.is_some()
    }

    pub fn set_environment(&mut self, loaded: LoadedEnvironment) {
        self.environment = Some(loaded.environment);
        self.steepfile = loaded.steepfile;
        self.analyses.clear();
    }

    pub fn forget(&mut self, uri: &str) {
        self.analyses.remove(uri);
    }

    pub fn clear_analyses(&mut self) {
        self.analyses.clear();
    }

    pub fn code_diagnostics_for(&self, workspace_path: &Path, path: &Path) -> Option<&DiagnosticConfiguration> {
        code_diagnostics_for(workspace_path, self.steepfile.as_ref(), path)
    }

    pub fn is_signature_path(&self, workspace_path: &Path, path: &Path) -> bool {
        is_signature_path(workspace_path, self.steepfile.as_ref(), path)
    }

    pub fn analysis_for(&mut self, document: &Document) -> Option<Arc<Analysis>> {
        if !document.is_ruby() || !self.settings.enabled {
            return None;
        }
        let environment = self.environment.as_ref()?;
        let fingerprint = source_fingerprint(document);
        if let Some((cached_fingerprint, analysis)) = self.analyses.get(document.uri())
            && *cached_fingerprint == fingerprint
        {
            return Some(analysis.clone());
        }
        let analysis = analyze(environment, document.source().as_bytes())?;
        self.analyses.insert(document.uri().to_string(), (fingerprint, analysis.clone()));
        Some(analysis)
    }
}

/// Versions alone can't identify contents: documents read from disk all have version 0 and reopened documents restart
/// their numbering
fn source_fingerprint(document: &Document) -> u64 {
    let mut hasher = DefaultHasher::new();
    document.version().hash(&mut hasher);
    document.source().hash(&mut hasher);
    hasher.finish()
}

pub fn analyze(environment: &Environment, source: &[u8]) -> Option<Arc<Analysis>> {
    project::analyze_with_options(environment, source, AnalysisOptions::default())
}

pub fn truncate_type(type_text: &str, maximum_length: Option<usize>) -> Option<String> {
    let maximum_length = maximum_length?;
    if type_text.chars().count() <= maximum_length {
        return None;
    }
    let kept: String = type_text.chars().take(maximum_length.saturating_sub(ELLIPSIS.chars().count())).collect();
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
            TypeHintLabel { label: format!(": {shown}"), tooltip, padding_left: false, padding_right: false }
        }
        TypeHintKind::MethodReturnType => {
            TypeHintLabel { label: format!("→ {shown}"), tooltip, padding_left: true, padding_right: false }
        }
        TypeHintKind::AttributeType => {
            TypeHintLabel { label: shown.to_string(), tooltip, padding_left: false, padding_right: true }
        }
        TypeHintKind::KeywordParameterType => {
            TypeHintLabel { label: shown.to_string(), tooltip, padding_left: true, padding_right: false }
        }
    }
}

pub fn inlay_hints(analysis: &Analysis, settings: &InlayHintSettings, line_index: &LineIndex<'_>) -> Vec<Value> {
    analysis
        .type_hints
        .iter()
        .filter(|hint| settings.shows(hint))
        .map(|hint| {
            let label = type_hint_label(hint.kind, &hint.hint_type.to_string(), settings.maximum_length);
            let mut inlay_hint = json!({
                "position": line_index.position_of_offset(hint.offset),
                "label": label.label,
                "kind": INLAY_HINT_KIND_TYPE,
                "paddingLeft": label.padding_left,
                "paddingRight": label.padding_right,
            });
            if let Some(tooltip) = label.tooltip {
                inlay_hint["tooltip"] = json!(tooltip);
            }
            inlay_hint
        })
        .collect()
}

pub fn severity_number(severity: Severity) -> u8 {
    match severity {
        Severity::Error => 1,
        Severity::Warning => 2,
        Severity::Information => 3,
        Severity::Hint => 4,
    }
}

pub fn diagnostics(analysis: &Analysis, configuration: &DiagnosticConfiguration, line_index: &LineIndex<'_>) -> Vec<Value> {
    configured_diagnostics(analysis, configuration)
        .into_iter()
        .map(|(diagnostic, severity)| {
            json!({
                "range": line_index.range_of(diagnostic.start, diagnostic.end),
                "severity": severity_number(severity),
                "source": DIAGNOSTIC_SOURCE,
                "code": diagnostic.code,
                "message": diagnostic.message,
            })
        })
        .collect()
}

pub fn hover_markdown(entry: &HoverEntry, source: &[u8]) -> String {
    match &entry.kind {
        HoverKind::Type(variable_type) => {
            let text = source.get(entry.start..entry.end).map(String::from_utf8_lossy).unwrap_or_default();
            if is_variable_name(&text) {
                rbs_code_block(&format!("{text}: {variable_type}"))
            } else {
                rbs_code_block(&variable_type.to_string())
            }
        }
        HoverKind::Method { receiver_type, owner, method_name, singleton, overloads, return_type, comment } => {
            let mut sections = vec![
                overloads_code_block(method_name, *singleton, overloads),
                format!("`({receiver_type}) → {return_type}`"),
                format!("Defined in `{owner}`"),
            ];
            sections.extend(comment.as_ref().map(|comment| comment.trim().to_string()).filter(|comment| !comment.is_empty()));
            sections.join("\n\n")
        }
        HoverKind::Constant { name, constant_type, comment } => {
            let mut sections = vec![rbs_code_block(&format!("{name}: {constant_type}"))];
            sections.extend(comment.as_ref().map(|comment| comment.trim().to_string()).filter(|comment| !comment.is_empty()));
            sections.join("\n\n")
        }
    }
}

/// Like the hover of a method call, without the receiver and the return type that only a call site knows
pub fn method_signature_markdown(
    owner: &str,
    method_name: &str,
    singleton: bool,
    overloads: &[MethodType],
    comment: Option<&str>,
) -> String {
    let mut sections = vec![overloads_code_block(method_name, singleton, overloads), format!("Defined in `{owner}`")];
    sections.extend(comment.map(|comment| comment.trim().to_string()).filter(|comment| !comment.is_empty()));
    sections.join("\n\n")
}

fn overloads_code_block(method_name: &str, singleton: bool, overloads: &[MethodType]) -> String {
    let prefix = format!("def {}{method_name}: ", if singleton { "self." } else { "" });
    let continuation = format!("{}| ", " ".repeat(prefix.chars().count().saturating_sub(2)));
    let signature: Vec<String> = overloads
        .iter()
        .enumerate()
        .map(|(index, overload)| format!("{}{overload}", if index == 0 { &prefix } else { &continuation }))
        .collect();
    rbs_code_block(&signature.join("\n"))
}

fn rbs_code_block(content: &str) -> String {
    format!("```rbs\n{content}\n```")
}

fn is_variable_name(text: &str) -> bool {
    let name = text.trim_start_matches(['@', '$']);
    let mut characters = name.chars();
    let Some(first) = characters.next() else { return false };
    text.len() - name.len() <= 2
        && (first.is_alphabetic() || first == '_')
        && characters.all(|character| character.is_alphanumeric() || character == '_')
}

pub fn merge_hover(type_markdown: Option<String>, existing_markdown: Option<String>) -> Option<String> {
    match (type_markdown, existing_markdown) {
        (Some(type_markdown), Some(existing_markdown)) => Some(format!("{type_markdown}\n\n---\n\n{existing_markdown}")),
        (type_markdown, existing_markdown) => type_markdown.or(existing_markdown),
    }
}

pub fn definition_links(analysis: &Analysis, offset: usize, encoding: Encoding) -> Vec<LocationLink> {
    let Some(entry) = analysis.definition_at(offset) else { return Vec::new() };
    entry.targets.iter().map(|target| signature_location_link(target, encoding)).collect()
}

pub fn signature_location_link(location: &Location, encoding: Encoding) -> LocationLink {
    let range = signature_range(location, encoding);
    LocationLink { target_uri: crate::uri::from_path_buf(&location.path), target_range: range, target_selection_range: range }
}

pub fn signature_location_covers(location: &Location, position: Position, encoding: Encoding) -> bool {
    range_covers(&signature_range(location, encoding), position)
}

fn signature_range(location: &Location, encoding: Encoding) -> Range {
    Range::new(source_position(location.start, encoding), source_position(location.end, encoding))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NamedType {
    Class { name: TypeName, singleton: bool },
    Interface(TypeName),
    Alias(TypeName),
}

/// `nil` of an optional type is left out, since `String?` is about `String` far more often than about `NilClass`
pub fn named_types(named_type: &Type) -> Vec<NamedType> {
    let mut named_types = Vec::new();
    collect_named_types(named_type, &mut named_types);
    named_types
}

fn collect_named_types(collected_type: &Type, named_types: &mut Vec<NamedType>) {
    let mut push = |named_type: NamedType| {
        if !named_types.contains(&named_type) {
            named_types.push(named_type);
        }
    };
    let class = |name: &str| NamedType::Class { name: name.strip_prefix("::").unwrap_or(name).to_string(), singleton: false };
    match collected_type {
        Type::ClassInstance { name, .. } => push(class(name)),
        Type::ClassSingleton { name } => {
            push(NamedType::Class { name: name.strip_prefix("::").unwrap_or(name).to_string(), singleton: true })
        }
        Type::Interface { name, .. } => push(NamedType::Interface(name.strip_prefix("::").unwrap_or(name).to_string())),
        Type::Alias { name, .. } => push(NamedType::Alias(name.strip_prefix("::").unwrap_or(name).to_string())),
        Type::Nil => push(class("NilClass")),
        Type::Bool => {
            push(class("TrueClass"));
            push(class("FalseClass"));
        }
        Type::Literal(literal) => push(class(match literal {
            Literal::Integer(_) => "Integer",
            Literal::String(_) => "String",
            Literal::Symbol(_) => "Symbol",
            Literal::True => "TrueClass",
            Literal::False => "FalseClass",
        })),
        Type::Tuple(_) => push(class("Array")),
        Type::Record(_) => push(class("Hash")),
        Type::Proc(_) => push(class("Proc")),
        Type::Optional(inner) => collect_named_types(inner, named_types),
        Type::Union(members) | Type::Intersection(members) => {
            for member in members {
                collect_named_types(member, named_types);
            }
        }
        _ => {}
    }
}

impl NamedType {
    pub fn name(&self) -> &str {
        match self {
            NamedType::Class { name, .. } | NamedType::Interface(name) | NamedType::Alias(name) => name,
        }
    }
}

/// Unlike `named_types`, also takes the types inside type arguments, tuples and records, e.g. `Foo` of `Array[Foo]`
pub fn referenced_named_types(shown_type: &Type) -> Vec<NamedType> {
    let mut named_types = Vec::new();
    collect_referenced_named_types(shown_type, &mut named_types);
    named_types
}

fn collect_referenced_named_types(referenced_type: &Type, referenced: &mut Vec<NamedType>) {
    match referenced_type {
        Type::Optional(inner) => collect_referenced_named_types(inner, referenced),
        Type::Union(members) | Type::Intersection(members) => {
            for member in members {
                collect_referenced_named_types(member, referenced);
            }
        }
        _ => {
            for named_type in named_types(referenced_type) {
                if !referenced.contains(&named_type) {
                    referenced.push(named_type);
                }
            }
            match referenced_type {
                Type::ClassInstance { arguments, .. } | Type::Interface { arguments, .. } | Type::Alias { arguments, .. } => {
                    for argument in arguments {
                        collect_referenced_named_types(argument, referenced);
                    }
                }
                Type::Tuple(members) => {
                    for member in members {
                        collect_referenced_named_types(member, referenced);
                    }
                }
                Type::Record(fields) => {
                    for field in fields {
                        collect_referenced_named_types(&field.field_type, referenced);
                    }
                }
                _ => {}
            }
        }
    }
}

pub fn type_declaration<'a>(environment: &'a Environment, named_type: &NamedType) -> Option<&'a Location> {
    match named_type {
        NamedType::Class { name, .. } => primary_class_declaration(environment, name),
        NamedType::Interface(name) => environment.interfaces.get(name)?.location.as_ref(),
        NamedType::Alias(name) => environment.type_aliases.get(name)?.location.as_ref(),
    }
}

/// rbs_rails reopens a model in the signatures of every model associated with it, so a class can have many declarations.
/// A hand written one is preferred over the generated `rbs_rails` directory since the methods written in Ruby are
/// declared there
pub fn primary_class_declaration<'a>(environment: &'a Environment, class_name: &str) -> Option<&'a Location> {
    let resolved_name = environment.resolve_class_name(class_name)?;
    let declaration = environment.classes.get(resolved_name)?;
    let class_path = PathBuf::from(underscored_path(resolved_name));
    let rank = |location: &Location| {
        let path = location.path.as_path();
        (
            path.with_extension("").ends_with(&class_path),
            path.file_stem() == class_path.file_name(),
            !path.components().any(|component| component.as_os_str() == "rbs_rails"),
            writes_super_class(location),
        )
    };
    declaration.locations.iter().rev().max_by_key(|location| rank(location))
}

/// Core classes are written in C, so their reopenings in gems, like `csv/core_ext/array.rb`, are extensions rather than
/// implementations
pub fn is_core_class(environment: &Environment, class_name: &str, core_directory: Option<&Path>) -> bool {
    let Some(core_directory) = core_directory else { return false };
    let declaration = environment.resolve_class_name(class_name).and_then(|name| environment.classes.get(name));
    declaration.is_some_and(|declaration| declaration.locations.iter().any(|location| location.path.starts_with(core_directory)))
}

/// Reads the header from the file, because `ClassDeclaration` keeps only the superclass of the first declaration
fn writes_super_class(location: &Location) -> bool {
    let Ok(source) = std::fs::read_to_string(location.path.as_path()) else { return false };
    let Some(line) = source.lines().nth(location.start.line as usize) else { return false };
    let header = line.get(location.start.byte_column as usize..).unwrap_or(line);
    let Some(rest) = header.strip_prefix("class") else { return false };
    let mut bracket_depth = 0i32;
    for character in rest.chars() {
        match character {
            '[' => bracket_depth += 1,
            ']' => bracket_depth -= 1,
            '<' if bracket_depth == 0 => return true,
            '#' => return false,
            _ => {}
        }
    }
    false
}

/// The path Zeitwerk and rbs_rails expect for a class, e.g. `api/order_item` for `API::OrderItem`
pub fn underscored_path(class_name: &str) -> String {
    let class_name = class_name.strip_prefix("::").unwrap_or(class_name);
    class_name.split("::").map(underscore).collect::<Vec<_>>().join("/")
}

fn underscore(segment: &str) -> String {
    let characters: Vec<char> = segment.chars().collect();
    let mut underscored = String::new();
    for (position, character) in characters.iter().enumerate() {
        if character.is_uppercase() && position > 0 {
            let previous = characters[position - 1];
            let next_is_lowercase = characters.get(position + 1).is_some_and(|next| next.is_lowercase());
            if previous.is_lowercase() || previous.is_ascii_digit() || (previous.is_uppercase() && next_is_lowercase) {
                underscored.push('_');
            }
        }
        underscored.extend(character.to_lowercase());
    }
    underscored
}

pub fn instance_variable_declaration_links(
    environment: &Environment,
    class_name: &str,
    variable_name: &str,
    singleton: bool,
    encoding: Encoding,
) -> Vec<LocationLink> {
    let declared_name = if singleton { format!("self.{variable_name}") } else { variable_name.to_string() };
    for ancestor in environment.ancestors(class_name) {
        let Some(declaration) = environment.classes.get(&ancestor.name) else { continue };
        let variables = if singleton { &declaration.class_instance_variables } else { &declaration.instance_variables };
        if !variables.contains_key(variable_name) {
            continue;
        }
        return declaration
            .locations
            .iter()
            .filter_map(|location| instance_variable_declaration(location, &declared_name))
            .map(|location| signature_location_link(&location, encoding))
            .collect();
    }
    Vec::new()
}

/// `ClassDeclaration` keeps only the types of instance variables, so the declaration is searched for in the file. Lines
/// of nested classes, modules and interfaces are skipped since their instance variables belong to them
fn instance_variable_declaration(class_location: &Location, declared_name: &str) -> Option<Location> {
    let source = std::fs::read_to_string(class_location.path.as_path()).ok()?;
    let first_line = class_location.start.line as usize;
    let line_count = (class_location.end.line as usize).saturating_sub(first_line) + 1;
    let mut nesting_depth = 0i32;
    for (line_number, line) in source.lines().enumerate().skip(first_line).take(line_count) {
        let trimmed = line.trim_start();
        let opens_declaration = ["class ", "module ", "interface "].iter().any(|keyword| trimmed.starts_with(keyword))
            && !trimmed.contains(" = ");
        if opens_declaration {
            nesting_depth += 1;
            continue;
        }
        if trimmed == "end" || trimmed.starts_with("end ") || trimmed.starts_with("end#") {
            nesting_depth -= 1;
            continue;
        }
        let declares = nesting_depth == 1
            && trimmed.strip_prefix(declared_name).is_some_and(|rest| rest.trim_start().starts_with(':'));
        if declares {
            let byte_column = line.len() - trimmed.len();
            let start = SourcePosition {
                line: line_number as u32,
                byte_column: byte_column as u32,
                utf16_column: line[..byte_column].encode_utf16().count() as u32,
            };
            let end = SourcePosition {
                line: line_number as u32,
                byte_column: (byte_column + declared_name.len()) as u32,
                utf16_column: start.utf16_column + declared_name.encode_utf16().count() as u32,
            };
            return Some(Location { path: class_location.path.clone(), start, end });
        }
    }
    None
}

pub fn signature_markdown_link(location: &Location, workspace_path: &Path) -> String {
    let line = location.start.line + 1;
    format!(
        "[{}:{line}]({}#L{line},{})",
        displayed_path(&location.path, workspace_path),
        crate::uri::from_path_buf(&location.path),
        location.start.utf16_column + 1
    )
}

pub fn displayed_path(path: &Path, workspace_path: &Path) -> String {
    match path.strip_prefix(workspace_path) {
        Ok(relative_path) => relative_path.to_string_lossy().into_owned(),
        Err(_) => path.file_name().map(|file_name| file_name.to_string_lossy().into_owned()).unwrap_or_default(),
    }
}

/// RBS locations only carry byte and UTF-16 columns, and reading the signature file again just for UTF-32 isn't worth it
/// for the rare non-BMP characters in signatures
fn source_position(position: SourcePosition, encoding: Encoding) -> Position {
    let character = match encoding {
        Encoding::Utf8 => position.byte_column,
        Encoding::Utf16 | Encoding::Utf32 => position.utf16_column,
    };
    Position::new(position.line, character)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rbs_environment::{DiagnosticSeverity, Function, MethodType, Parameter, Type};
    use type_checker::TypeDiagnostic;

    fn type_diagnostic(code: &'static str, severity: Severity) -> TypeDiagnostic {
        TypeDiagnostic { start: 0, end: 1, severity, code, message: String::new() }
    }

    fn hint(kind: TypeHintKind, hint_type: Type, is_obvious: bool) -> TypeHint {
        TypeHint { kind, offset: 0, hint_type, is_obvious }
    }

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
    fn collects_named_types_of_unions_and_literals() {
        let union = Type::Union(vec![
            Type::Optional(Box::new(Type::instance("Foo"))),
            Type::ClassSingleton { name: "::Bar".to_string() },
            Type::Literal(Literal::Symbol("name".to_string())),
            Type::instance("Foo"),
        ]);
        assert_eq!(
            named_types(&union),
            vec![
                NamedType::Class { name: "Foo".to_string(), singleton: false },
                NamedType::Class { name: "Bar".to_string(), singleton: true },
                NamedType::Class { name: "Symbol".to_string(), singleton: false },
            ]
        );
    }

    #[test]
    fn collects_named_types_inside_type_arguments() {
        let shown_type = Type::Union(vec![
            Type::ClassInstance {
                name: "::Array".to_string(),
                arguments: vec![Type::Optional(Box::new(Type::instance("Foo")))],
            },
            Type::Tuple(vec![Type::instance("Bar")]),
        ]);
        let names: Vec<String> = referenced_named_types(&shown_type).iter().map(|named_type| named_type.name().to_string()).collect();
        assert_eq!(names, vec!["Array", "Foo", "Bar"]);
    }

    #[test]
    fn underscores_class_names_into_paths() {
        assert_eq!(underscored_path("::API::OrderItem"), "api/order_item");
        assert_eq!(underscored_path("HTTPClient"), "http_client");
        assert_eq!(underscored_path("V2Api"), "v2_api");
    }

    #[test]
    fn keeps_defaults_for_missing_or_invalid_settings() {
        let mut settings = TypeCheckerSettings::default();
        settings.merge(&Value::Null);
        settings.merge(&json!({ "enabled": "yes", "inlayHints": { "maxLength": "long" } }));
        assert_eq!(settings, TypeCheckerSettings::default());
        assert_eq!(settings.inlay_hints.maximum_length, Some(30));
    }

    #[test]
    fn builds_labels_for_each_hint_kind() {
        let variable = type_hint_label(TypeHintKind::VariableType, "Integer", Some(30));
        assert_eq!((variable.label.as_str(), variable.padding_left, variable.padding_right), (": Integer", false, false));
        assert_eq!(variable.tooltip, None);

        let block_parameter = type_hint_label(TypeHintKind::BlockParameterType, "String", Some(30));
        assert_eq!(block_parameter.label, ": String");

        let method_return = type_hint_label(TypeHintKind::MethodReturnType, "String?", None);
        assert_eq!((method_return.label.as_str(), method_return.padding_left), ("→ String?", true));

        let attribute = type_hint_label(TypeHintKind::AttributeType, "Array[Integer]", Some(30));
        assert_eq!((attribute.label.as_str(), attribute.padding_right), ("Array[Integer]", true));
        let parameter = type_hint_label(TypeHintKind::MethodParameterType, "String", Some(30));
        assert_eq!((parameter.label.as_str(), parameter.padding_left), (": String", false));
        let keyword = type_hint_label(TypeHintKind::KeywordParameterType, "String", Some(30));
        assert_eq!((keyword.label.as_str(), keyword.padding_left), ("String", true));
    }

    #[test]
    fn truncates_long_types() {
        let full_type = "Hash[Symbol, Array[Integer | String]]";
        let label = type_hint_label(TypeHintKind::VariableType, full_type, Some(10));
        assert_eq!(label.label, ": Hash[Symb…");
        assert_eq!(label.tooltip.as_deref(), Some(full_type));
        assert_eq!(truncate_type("Integer", Some(7)), None);
        assert_eq!(truncate_type(full_type, None), None);
    }

    #[test]
    fn filters_hints_by_settings() {
        let settings = InlayHintSettings::default();
        assert!(settings.shows(&hint(TypeHintKind::VariableType, Type::instance("Integer"), false)));
        assert!(!settings.shows(&hint(TypeHintKind::VariableType, Type::instance("Integer"), true)));
        assert!(!settings.shows(&hint(TypeHintKind::VariableType, Type::Untyped, false)));

        let settings = InlayHintSettings {
            hide_obvious_types: false,
            hide_untyped: false,
            method_return_types: false,
            ..InlayHintSettings::default()
        };
        assert!(settings.shows(&hint(TypeHintKind::VariableType, Type::Untyped, true)));
        assert!(!settings.shows(&hint(TypeHintKind::MethodReturnType, Type::instance("Integer"), false)));
    }

    #[test]
    fn converts_hint_offsets_with_the_negotiated_encoding() {
        let source = "名前 = 1\n";
        let analysis = Analysis {
            type_hints: vec![TypeHint {
                kind: TypeHintKind::VariableType,
                offset: "名前".len(),
                hint_type: Type::instance("Integer"),
                is_obvious: false,
            }],
            ..Analysis::default()
        };
        let line_index = LineIndex::new(source.as_bytes(), Encoding::Utf16);
        let hints = inlay_hints(&analysis, &InlayHintSettings::default(), &line_index);
        assert_eq!(hints[0]["position"], json!({ "line": 0, "character": 2 }));
        assert_eq!(hints[0]["kind"], json!(1));
        assert_eq!(hints[0]["label"], json!(": Integer"));
    }

    #[test]
    fn publishes_configured_diagnostics() {
        let analysis = Analysis {
            diagnostics: vec![
                type_diagnostic("Ruby::IncompatibleAssignment", Severity::Hint),
                type_diagnostic("Ruby::UnannotatedEmptyCollection", Severity::Warning),
            ],
            ..Analysis::default()
        };
        let mut configuration = DiagnosticConfiguration::default();
        configuration.set("Ruby::IncompatibleAssignment", Some(DiagnosticSeverity::Warning));
        configuration.set("Ruby::UnannotatedEmptyCollection", None);
        let published = diagnostics(&analysis, &configuration, &LineIndex::new(b"x = 1\n", Encoding::Utf16));
        assert_eq!(published.len(), 1);
        assert_eq!(published[0]["code"], json!("Ruby::IncompatibleAssignment"));
        assert_eq!(published[0]["severity"], json!(2));
    }

    fn overload(parameter_type: &str, return_type: &str) -> MethodType {
        let mut function = Function::returning(Type::instance(return_type));
        function.required_positionals.push(Parameter::new(Type::instance(parameter_type)));
        MethodType { type_parameters: Vec::new(), function, block: None }
    }

    #[test]
    fn renders_method_hover_with_every_overload() {
        let overloads = vec![overload("Integer", "Integer"), overload("Float", "Float")];
        let entry = HoverEntry {
            start: 0,
            end: 3,
            kind: HoverKind::Method {
                receiver_type: Type::instance("Integer"),
                owner: "Integer".to_string(),
                method_name: "+".to_string(),
                singleton: false,
                overloads: overloads.clone(),
                return_type: Type::instance("Integer"),
                comment: Some("Performs addition.\n".to_string()),
            },
        };
        assert_eq!(
            hover_markdown(&entry, b"1+2"),
            format!(
                "```rbs\ndef +: {}\n     | {}\n```\n\n`(Integer) → Integer`\n\nDefined in `Integer`\n\nPerforms addition.",
                overloads[0], overloads[1]
            )
        );
    }

    #[test]
    fn renders_singleton_method_hover() {
        let entry = HoverEntry {
            start: 0,
            end: 3,
            kind: HoverKind::Method {
                receiver_type: Type::ClassSingleton { name: "Foo".to_string() },
                owner: "Foo".to_string(),
                method_name: "build".to_string(),
                singleton: true,
                overloads: vec![overload("String", "Foo")],
                return_type: Type::instance("Foo"),
                comment: None,
            },
        };
        assert!(hover_markdown(&entry, b"Foo.build").starts_with("```rbs\ndef self.build: (String)"));
    }

    #[test]
    fn renders_variable_hover_with_its_name() {
        let entry = HoverEntry { start: 0, end: 5, kind: HoverKind::Type(Type::instance("String")) };
        assert_eq!(hover_markdown(&entry, b"@name = 'x'"), "```rbs\n@name: String\n```");

        let entry = HoverEntry { start: 0, end: 5, kind: HoverKind::Type(Type::instance("String")) };
        assert_eq!(hover_markdown(&entry, b"'abc' + 'x'"), "```rbs\nString\n```");
    }

    #[test]
    fn merges_hover_contents() {
        assert_eq!(merge_hover(Some("a".to_string()), Some("b".to_string())), Some("a\n\n---\n\nb".to_string()));
        assert_eq!(merge_hover(None, Some("b".to_string())), Some("b".to_string()));
        assert_eq!(merge_hover(Some("a".to_string()), None), Some("a".to_string()));
        assert_eq!(merge_hover(None, None), None);
    }

    #[test]
    fn recognizes_rbs_related_files() {
        assert!(is_rbs_related_file("/workspace/sig/foo.rbs"));
        assert!(is_rbs_related_file("/workspace/Steepfile"));
        assert!(is_rbs_related_file("/workspace/rbs_collection.lock.yaml"));
        assert!(!is_rbs_related_file("/workspace/lib/foo.rb"));
    }
}
