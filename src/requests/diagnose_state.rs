use serde_json::{Map, Value, json};

use super::{RequestContext, RequestResult};

pub fn handle(context: &mut RequestContext<'_>, _params: Value) -> RequestResult {
    let documents: Map<String, Value> =
        context.store.iter().map(|(uri, document)| (uri.clone(), Value::String(document.source().to_string()))).collect();
    Ok(json!({
        "workerAlive": true,
        "backtrace": [],
        "documents": documents,
        "incomingQueueSize": context.pending_message_count,
    }))
}
