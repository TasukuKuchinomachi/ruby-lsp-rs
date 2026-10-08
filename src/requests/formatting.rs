use serde_json::{Value, json};

use super::support::formatter::{self, FormatterError};
use super::{RequestContext, RequestResult, TextDocumentParams, parse_params};
use crate::document::Document;
use crate::global_state::{GlobalState, message_type};
use crate::requests::Outgoing;

pub fn handle(context: &mut RequestContext<'_>, params: Value) -> RequestResult {
    if context.global_state.formatter == "none" {
        return Ok(Value::Null);
    }

    let params: TextDocumentParams = parse_params(params)?;
    let uri = params.text_document.uri;
    let workspace_path = context.global_state.workspace_path();
    if let Some(path) = crate::uri::to_path(&uri)
        && !path.starts_with(&workspace_path)
    {
        context.outgoing.log(&format!(
            "Ignoring formatting request for file outside of the workspace.\nWorkspace path was set by editor as {workspace_path}.\nFile path requested for formatting was {path}\n"
        ));
        return Ok(Value::Null);
    }

    let global_state = &*context.global_state;
    let outgoing = context.outgoing;
    let document = context.store.get(&uri)?;
    if !document.is_ruby() {
        return Ok(Value::Null);
    }

    match perform(global_state, outgoing, document) {
        Ok(edits) => Ok(edits),
        Err(error) => {
            outgoing.show_message(&format!("Formatting error: {}", error.message), message_type::ERROR);
            outgoing.log_message(&format!("Formatting failed with\r\n: {}", error.full_message), message_type::ERROR);
            Ok(Value::Null)
        }
    }
}

pub fn perform(global_state: &GlobalState, outgoing: &Outgoing, document: &Document) -> Result<Value, FormatterError> {
    let Some(active_formatter) = formatter::active_formatter(global_state, outgoing) else { return Ok(Value::Null) };
    if document.syntax_error() {
        return Ok(Value::Null);
    }

    let path = formatter::path_for_uri(document.uri());
    let source = document.source();
    let Some(formatted_text) = formatter::run_formatting(global_state, outgoing, &active_formatter, path.as_deref(), source)?
    else {
        return Ok(Value::Null);
    };
    if formatted_text == source {
        return Ok(Value::Null);
    }

    Ok(json!([{
        "range": {
            "start": { "line": 0, "character": 0 },
            "end": { "line": ruby_line_count(source), "character": 0 },
        },
        "newText": formatted_text,
    }]))
}

/// `String#lines.size`, which doesn't count an empty line after the last newline
fn ruby_line_count(source: &str) -> usize {
    source.split_inclusive('\n').count()
}
