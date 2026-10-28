use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::support::formatter::{self, FormatterError};
use super::{RequestContext, RequestResult, TextDocumentParams, parse_params};
use crate::document::{Document, LanguageId, ParsedDocument};
use crate::global_state::{GlobalState, message_type};
use crate::requests::Outgoing;
use crate::type_checking;

pub const CACHE_KEY: &str = "textDocument/diagnostic";

pub mod diagnostic_severity {
    pub const ERROR: u8 = 1;
    pub const WARNING: u8 = 2;
}

pub fn handle(context: &mut RequestContext<'_>, params: Value) -> RequestResult {
    let params: TextDocumentParams = parse_params(params)?;
    let uri = params.text_document.uri;
    if let Some(path) = crate::uri::to_path(&uri)
        && !path.starts_with(&context.global_state.workspace_path())
    {
        return Ok(Value::Null);
    }

    let outgoing = context.outgoing;
    let document = context.store.get(&uri)?;
    let type_diagnostics =
        if document.cache_get(CACHE_KEY).is_none() { type_diagnostics(context.global_state, document) } else { Vec::new() };
    let global_state = &*context.global_state;

    let diagnostics = match document.cache_get(CACHE_KEY) {
        Some(cached) => cached.clone(),
        None => {
            let computed = if document.is_ruby() {
                let mut diagnostics = perform(global_state, outgoing, document).unwrap_or_else(|error| {
                    outgoing.show_message(&format!("Error running diagnostics: {}", error.message), message_type::ERROR);
                    outgoing.log_message(&format!("Diagnostics failed with\r\n: {}", error.full_message), message_type::ERROR);
                    syntax_diagnostics(&document.parse())
                });
                diagnostics.extend(type_diagnostics);
                Value::Array(diagnostics)
            } else if document.language_id() == LanguageId::Rbs {
                signature_diagnostics(global_state, document).map(Value::Array).unwrap_or(Value::Null)
            } else {
                Value::Null
            };
            document.cache_set(CACHE_KEY, computed)
        }
    };

    if diagnostics.is_null() {
        return Ok(Value::Null);
    }
    Ok(json!({ "kind": "full", "items": diagnostics }))
}

pub fn perform(global_state: &GlobalState, outgoing: &Outgoing, document: &Document) -> Result<Vec<Value>, FormatterError> {
    let parsed = document.parse();
    let mut diagnostics = syntax_diagnostics(&parsed);

    if parsed.syntax_error() || document.past_expensive_limit() {
        return Ok(diagnostics);
    }
    let active_linters = formatter::active_linters(global_state, outgoing);
    if active_linters.is_empty() {
        return Ok(diagnostics);
    }

    diagnostics.extend(formatter::run_diagnostic(global_state, outgoing, &active_linters, document.uri(), document.source())?);
    Ok(diagnostics)
}

fn type_diagnostics(global_state: &mut GlobalState, document: &Document) -> Vec<Value> {
    let type_checking = &global_state.type_checking;
    if !type_checking.is_active() || !type_checking.settings.diagnostics_enabled || document.past_expensive_limit() {
        return Vec::new();
    }
    let workspace_path = global_state.workspace_path();
    let Some(path) = crate::uri::to_path(document.uri()) else { return Vec::new() };
    let Some(configuration) = type_checking.code_diagnostics_for(Path::new(&workspace_path), Path::new(&path)).cloned()
    else {
        return Vec::new();
    };
    let inline_signature_diagnostics = match type_checking.environment_for(document.uri()) {
        Some(environment) if type_checking.is_inline_source_path(Path::new(&workspace_path), Path::new(&path)) => {
            type_checking::inline_signature_diagnostics(environment, Path::new(&path), document.encoding())
        }
        _ => Vec::new(),
    };
    let Some(analysis) = global_state.type_checking.analysis_for(document) else { return inline_signature_diagnostics };
    let mut diagnostics = type_checking::diagnostics(&analysis, &configuration, &document.line_index());
    diagnostics.extend(inline_signature_diagnostics);
    diagnostics
}

/// `None` for a file outside the `Steepfile` signature directories, which Steep doesn't validate
fn signature_diagnostics(global_state: &GlobalState, document: &Document) -> Option<Vec<Value>> {
    let type_checking = &global_state.type_checking;
    if !type_checking.is_active() || !type_checking.settings.diagnostics_enabled {
        return None;
    }
    let environment = type_checking.environment_for(document.uri())?;
    let path = PathBuf::from(crate::uri::to_path(document.uri())?);
    if !type_checking.is_signature_path(Path::new(&global_state.workspace_path()), &path) {
        return None;
    }
    Some(type_checking::signature_diagnostics(environment, &path, document.source(), document.encoding()))
}

fn syntax_diagnostics(parsed: &ParsedDocument<'_>) -> Vec<Value> {
    let mut diagnostics = syntax_error_diagnostics(parsed);
    diagnostics.extend(syntax_warning_diagnostics(parsed));
    diagnostics
}

fn syntax_error_diagnostics(parsed: &ParsedDocument<'_>) -> Vec<Value> {
    parsed
        .parse_result
        .errors()
        .map(|error| prism_diagnostic(parsed, &error.location(), error.message(), diagnostic_severity::ERROR))
        .collect()
}

fn syntax_warning_diagnostics(parsed: &ParsedDocument<'_>) -> Vec<Value> {
    parsed
        .parse_result
        .warnings()
        .map(|warning| prism_diagnostic(parsed, &warning.location(), warning.message(), diagnostic_severity::WARNING))
        .collect()
}

fn prism_diagnostic(parsed: &ParsedDocument<'_>, location: &ruby_prism::Location<'_>, message: &str, severity: u8) -> Value {
    json!({
        "range": parsed.range_from_location(location),
        "severity": severity,
        "source": "Prism",
        "message": message,
    })
}
