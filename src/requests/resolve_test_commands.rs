use serde_json::{Value, json};

use super::{RequestContext, RequestResult};
use crate::listeners::test_style;

pub fn handle(context: &mut RequestContext<'_>, params: Value) -> RequestResult {
    let items = params["items"].as_array().cloned().unwrap_or_default();
    let commands = test_style::resolve_test_commands(&items, &context.global_state.workspace_path());
    Ok(json!({ "commands": commands }))
}
