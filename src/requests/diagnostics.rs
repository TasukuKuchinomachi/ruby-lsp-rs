use serde_json::{Value, json};

use super::support::formatter::{self, FormatterError};
use super::{RequestContext, RequestResult, TextDocumentParams, parse_params};
use crate::document::{Document, ParsedDocument};
use crate::global_state::{GlobalState, message_type};
use crate::requests::Outgoing;

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

    let global_state = &*context.global_state;
    let outgoing = context.outgoing;
    let document = context.store.get(&uri)?;

    let diagnostics = match document.cache_get(CACHE_KEY) {
        Some(cached) => cached.clone(),
        None => {
            let computed = if document.is_ruby() { perform(global_state, outgoing, document).map(Value::Array) } else { Ok(Value::Null) };
            match computed {
                Ok(diagnostics) => document.cache_set(CACHE_KEY, diagnostics),
                Err(error) => {
                    outgoing.show_message(&format!("Error running diagnostics: {}", error.message), message_type::ERROR);
                    outgoing.log_message(&format!("Diagnostics failed with\r\n: {}", error.full_message), message_type::ERROR);
                    return Ok(Value::Null);
                }
            }
        }
    };

    if diagnostics.is_null() {
        return Ok(Value::Null);
    }
    Ok(json!({ "kind": "full", "items": diagnostics }))
}

pub fn perform(global_state: &GlobalState, outgoing: &Outgoing, document: &Document) -> Result<Vec<Value>, FormatterError> {
    let parsed = document.parse();
    let mut diagnostics = syntax_error_diagnostics(&parsed);
    diagnostics.extend(syntax_warning_diagnostics(&parsed));

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
