use serde_json::{Value, json};

use super::{RequestContext, RequestResult};

pub fn handle(_context: &mut RequestContext<'_>, params: Value) -> RequestResult {
    let mut code_lens = params;
    let arguments = code_lens["data"]["arguments"].clone();
    let command = match code_lens["data"]["kind"].as_str() {
        Some("run_test") => Some(("▶ Run", "rubyLsp.runTest")),
        Some("run_test_in_terminal") => Some(("▶ Run in terminal", "rubyLsp.runTestInTerminal")),
        Some("debug_test") => Some(("⚙ Debug", "rubyLsp.debugTest")),
        _ => None,
    };
    if let (Some((title, command_name)), Some(object)) = (command, code_lens.as_object_mut()) {
        object.insert("command".to_string(), json!({ "title": title, "command": command_name, "arguments": arguments }));
    }
    Ok(code_lens)
}
