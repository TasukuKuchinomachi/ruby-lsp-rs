use std::sync::atomic::{AtomicU64, Ordering};

use serde::Deserialize;
use serde_json::Value;

use super::support::semantic_highlighting_response_builder::{
    SemanticHighlightingResponseBuilder, SemanticTokens, SemanticTokensDelta, SemanticTokensEdit, encode,
};
use super::{RequestContext, RequestResult, TextDocumentIdentifier, TextDocumentParams, TextDocumentRangeParams, parse_params, to_result};
use crate::listeners::semantic_highlighting::SemanticHighlighting;
use crate::position::Range;

static RESULT_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SemanticTokensDeltaParams {
    text_document: TextDocumentIdentifier,
    previous_result_id: Option<String>,
}

pub fn handle_full(context: &mut RequestContext<'_>, params: Value) -> RequestResult {
    let params: TextDocumentParams = parse_params(params)?;
    perform(context, &params.text_document.uri, None, None)
}

pub fn handle_delta(context: &mut RequestContext<'_>, params: Value) -> RequestResult {
    let params: SemanticTokensDeltaParams = parse_params(params)?;
    perform(context, &params.text_document.uri, params.previous_result_id.as_deref(), None)
}

pub fn handle_range(context: &mut RequestContext<'_>, params: Value) -> RequestResult {
    let params: TextDocumentRangeParams = parse_params(params)?;
    perform(context, &params.text_document.uri, None, Some(params.range))
}

pub fn next_result_id() -> u64 {
    RESULT_ID.fetch_add(1, Ordering::SeqCst) + 1
}

fn perform(context: &mut RequestContext<'_>, uri: &str, previous_result_id: Option<&str>, range: Option<Range>) -> RequestResult {
    let document = context.store.get(uri)?;
    if document.past_expensive_limit() || !document.is_ruby_or_erb() {
        return Ok(Value::Null);
    }

    let result_id = next_result_id().to_string();
    let tokens = {
        let parsed = document.parse();
        let mut response_builder = SemanticHighlightingResponseBuilder::new(document.source().as_bytes(), document.encoding());
        SemanticHighlighting::new(&mut response_builder).visit(parsed.ast());
        response_builder.into_response()
    };

    let previous_tokens: Option<SemanticTokens> = document.semantic_tokens().and_then(|previous| {
        let result_id = previous.get("resultId")?.as_str()?.to_string();
        let data = serde_json::from_value(previous.get("data")?.clone()).ok()?;
        Some(SemanticTokens { result_id, data })
    });
    let encoded_tokens = encode(&tokens);
    let full_response = SemanticTokens { result_id: result_id.clone(), data: encoded_tokens };
    document.set_semantic_tokens(serde_json::to_value(&full_response).map_err(|error| super::RequestError::Internal(error.to_string()))?);

    if let Some(range) = range {
        let tokens_within_range: Vec<_> = tokens
            .into_iter()
            .filter(|token| {
                let line = token.start_line as u32 - 1;
                range.start.line <= line && line <= range.end.line
            })
            .collect();
        return to_result(SemanticTokens { result_id, data: encode(&tokens_within_range) });
    }

    if let Some(previous_result_id) = previous_result_id {
        return match previous_tokens {
            Some(previous_tokens) if previous_tokens.result_id == previous_result_id => {
                to_result(compute_delta(&full_response.data, &previous_tokens.data, &result_id))
            }
            _ => to_result(full_response),
        };
    }

    to_result(full_response)
}

pub fn compute_delta(current_tokens: &[u32], previous_tokens: &[u32], result_id: &str) -> SemanticTokensDelta {
    let mut first_different_position =
        (0..current_tokens.len()).find(|index| previous_tokens.get(*index) != Some(&current_tokens[*index]));

    if first_different_position.is_none() && current_tokens.len() < previous_tokens.len() {
        first_different_position = Some(current_tokens.len());
    }

    let Some(first_different_position) = first_different_position else {
        return SemanticTokensDelta { result_id: result_id.to_string(), edits: Vec::new() };
    };

    let old_tokens = &previous_tokens[first_different_position..];
    let new_tokens = &current_tokens[first_different_position..];

    let first_different_token_from_end = (0..new_tokens.len())
        .find(|index| {
            let old_token = old_tokens.len().checked_sub(index + 1).map(|position| old_tokens[position]);
            old_token != Some(new_tokens[new_tokens.len() - index - 1])
        })
        .unwrap_or(0);

    let old_tokens = &old_tokens[..old_tokens.len() - first_different_token_from_end];
    let new_tokens = &new_tokens[..new_tokens.len() - first_different_token_from_end];

    SemanticTokensDelta {
        result_id: result_id.to_string(),
        edits: vec![SemanticTokensEdit {
            start: first_different_position,
            delete_count: old_tokens.len(),
            data: new_tokens.to_vec(),
        }],
    }
}
