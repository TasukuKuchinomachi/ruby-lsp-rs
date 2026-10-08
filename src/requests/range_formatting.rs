use serde_json::{Value, json};

use super::support::formatter;
use super::{RequestContext, RequestError, RequestResult, TextDocumentRangeParams, parse_params};
use crate::document::Document;
use crate::global_state::GlobalState;
use crate::position::Range;
use crate::requests::Outgoing;

pub fn handle(context: &mut RequestContext<'_>, params: Value) -> RequestResult {
    if context.global_state.formatter == "none" {
        return Ok(Value::Null);
    }

    let params: TextDocumentRangeParams = parse_params(params)?;
    let uri = params.text_document.uri;
    if let Some(path) = crate::uri::to_path(&uri)
        && !path.starts_with(&context.global_state.workspace_path())
    {
        return Ok(Value::Null);
    }

    let global_state = &*context.global_state;
    let outgoing = context.outgoing;
    let document = context.store.get(&uri)?;
    if !document.is_ruby() {
        return Ok(Value::Null);
    }

    perform(global_state, outgoing, document, params.range)
}

pub fn perform(global_state: &GlobalState, outgoing: &Outgoing, document: &Document, range: Range) -> RequestResult {
    let Some(active_formatter) = formatter::active_formatter(global_state, outgoing) else { return Ok(Value::Null) };
    let parsed = document.parse();
    if parsed.syntax_error() {
        return Ok(Value::Null);
    }

    let Some(target) = parsed.locate_first_within_range(range, &[])? else { return Ok(Value::Null) };
    let start_offset = target.location().start_offset();
    let end_offset = target.location().end_offset();
    let start_column = parsed.byte_position(start_offset).character as usize;
    let slice = String::from_utf8_lossy(&document.source().as_bytes()[start_offset..end_offset]).into_owned();

    let path = formatter::path_for_uri(document.uri());
    let formatted_text =
        formatter::run_range_formatting(global_state, outgoing, &active_formatter, path.as_deref(), &slice, start_column / 2)
            .map_err(|error| RequestError::Internal(error.full_message))?;
    let Some(formatted_text) = formatted_text else { return Ok(Value::Null) };

    Ok(json!([{
        "range": parsed.code_units_range(start_offset, end_offset),
        "newText": ruby_strip(&formatted_text),
    }]))
}

fn ruby_strip(text: &str) -> &str {
    text.trim_matches(|character| matches!(character, '\0' | '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' '))
}
