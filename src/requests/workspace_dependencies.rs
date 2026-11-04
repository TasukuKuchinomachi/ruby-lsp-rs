use serde_json::{Value, json};

use super::{RequestContext, RequestResult, to_result};

pub fn handle(context: &mut RequestContext<'_>, _params: Value) -> RequestResult {
    if !context.global_state.top_level_bundle {
        return Ok(json!([]));
    }
    to_result(&context.global_state.ruby_environment.dependencies)
}
