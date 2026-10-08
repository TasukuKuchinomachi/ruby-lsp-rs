use serde_json::Value;

use super::{RequestContext, RequestResult};

pub fn handle(_context: &mut RequestContext<'_>, _params: Value) -> RequestResult {
    Ok(Value::Null)
}
