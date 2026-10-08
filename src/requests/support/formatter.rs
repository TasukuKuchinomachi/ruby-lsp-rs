use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

use serde_json::{Value, json};

use crate::global_state::{GlobalState, message_type};
use crate::requests::Outgoing;

const WORKER_SCRIPT: &str = include_str!("../../../ruby/formatter_worker.rb");

pub const RUBOCOP_INTERNAL: &str = "rubocop_internal";
pub const SYNTAX_TREE: &str = "syntax_tree";
const WORKER_FORMATTERS: &[&str] = &[RUBOCOP_INTERNAL, SYNTAX_TREE];

#[derive(Debug, Clone, PartialEq)]
pub struct FormatterError {
    pub message: String,
    pub full_message: String,
}

impl FormatterError {
    fn new(message: String) -> FormatterError {
        FormatterError { full_message: message.clone(), message }
    }
}

struct WorkerProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_request_id: u64,
    formatters: Vec<String>,
    rubocop_loaded: bool,
}

impl WorkerProcess {
    fn spawn(workspace_path: &str, ruby_command: &[String], outgoing: &Outgoing) -> Result<WorkerProcess, String> {
        let (program, arguments) = ruby_command.split_first().ok_or("Ruby command is empty")?;
        if !Path::new(workspace_path).is_dir() {
            return Err(format!("Workspace {workspace_path} is not a directory"));
        }
        let mut child = Command::new(program)
            .args(arguments)
            .arg("-e")
            .arg(WORKER_SCRIPT)
            .arg(workspace_path)
            .current_dir(workspace_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|error| format!("Failed to run {program}: {error}"))?;
        let stdin = child.stdin.take().ok_or("Formatter worker has no stdin")?;
        let stdout = BufReader::new(child.stdout.take().ok_or("Formatter worker has no stdout")?);

        let mut worker =
            WorkerProcess { child, stdin, stdout, next_request_id: 1, formatters: Vec::new(), rubocop_loaded: false };
        let ready = worker.read_message().map_err(|error| format!("Formatter worker did not start: {}", error.message))?;
        worker.formatters = string_array(&ready["formatters"]);
        worker.rubocop_loaded = ready["rubocop_loaded"].as_bool().unwrap_or(false);
        for message in string_array(&ready["show_messages"]) {
            outgoing.show_message(&message, message_type::ERROR);
        }
        for error in string_array(&ready["errors"]) {
            outgoing.log_message(&error, message_type::WARNING);
        }
        Ok(worker)
    }

    fn read_message(&mut self) -> Result<Value, FormatterError> {
        let mut line = String::new();
        loop {
            line.clear();
            let read = self
                .stdout
                .read_line(&mut line)
                .map_err(|error| FormatterError::new(format!("Formatter worker failed: {error}")))?;
            if read == 0 {
                return Err(FormatterError::new("Formatter worker exited unexpectedly".to_string()));
            }
            if line.trim().is_empty() {
                continue;
            }
            return serde_json::from_str(&line)
                .map_err(|error| FormatterError::new(format!("Invalid formatter worker output: {error}: {line}")));
        }
    }

    fn request(&mut self, method: &str, mut params: Value) -> Result<Result<Value, FormatterError>, FormatterError> {
        let id = self.next_request_id;
        self.next_request_id += 1;
        params["id"] = json!(id);
        params["method"] = json!(method);

        let mut line = serde_json::to_string(&params).map_err(|error| FormatterError::new(error.to_string()))?;
        line.push('\n');
        self.stdin
            .write_all(line.as_bytes())
            .and_then(|()| self.stdin.flush())
            .map_err(|error| FormatterError::new(format!("Formatter worker failed: {error}")))?;

        loop {
            let message = self.read_message()?;
            if message["id"].as_u64() != Some(id) {
                continue;
            }
            if let Some(error) = message.get("error") {
                let error_message = error["message"].as_str().unwrap_or_default().to_string();
                let full_message = error["full_message"].as_str().map(str::to_string).unwrap_or_else(|| error_message.clone());
                return Ok(Err(FormatterError { message: error_message, full_message }));
            }
            return Ok(Ok(message["result"].clone()));
        }
    }
}

impl Drop for WorkerProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

enum WorkerSlot {
    NotStarted,
    Running(WorkerProcess),
    Unavailable,
}

type WorkerKey = (String, Vec<String>);

/// Workers are shared per workspace and Ruby command instead of being owned by the server, so that loading RuboCop
/// happens once per process even when several servers run in it (as in the test suite)
fn workers() -> &'static Mutex<HashMap<WorkerKey, Arc<Mutex<WorkerSlot>>>> {
    static WORKERS: OnceLock<Mutex<HashMap<WorkerKey, Arc<Mutex<WorkerSlot>>>>> = OnceLock::new();
    WORKERS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn slot_for(global_state: &GlobalState) -> Arc<Mutex<WorkerSlot>> {
    let key = (global_state.workspace_path(), global_state.ruby_command.clone());
    let mut workers = workers().lock().unwrap_or_else(PoisonError::into_inner);
    workers.entry(key).or_insert_with(|| Arc::new(Mutex::new(WorkerSlot::NotStarted))).clone()
}

fn lock(slot: &Mutex<WorkerSlot>) -> MutexGuard<'_, WorkerSlot> {
    slot.lock().unwrap_or_else(PoisonError::into_inner)
}

fn ensure_started<'a>(
    slot: &'a mut WorkerSlot,
    global_state: &GlobalState,
    outgoing: &Outgoing,
) -> Option<&'a mut WorkerProcess> {
    if let WorkerSlot::NotStarted = slot {
        *slot = match WorkerProcess::spawn(&global_state.workspace_path(), &global_state.ruby_command, outgoing) {
            Ok(worker) => WorkerSlot::Running(worker),
            Err(error) => {
                outgoing.log_message(&format!("Formatters are not available: {error}"), message_type::ERROR);
                WorkerSlot::Unavailable
            }
        };
    }
    match slot {
        WorkerSlot::Running(worker) => Some(worker),
        _ => None,
    }
}

fn request(
    global_state: &GlobalState,
    outgoing: &Outgoing,
    method: &str,
    params: Value,
) -> Result<Option<Value>, FormatterError> {
    let slot = slot_for(global_state);
    let mut slot = lock(&slot);
    let mut restarted = false;
    loop {
        let Some(worker) = ensure_started(&mut slot, global_state, outgoing) else { return Ok(None) };
        match worker.request(method, params.clone()) {
            Ok(result) => return result.map(Some),
            Err(error) if restarted => {
                *slot = WorkerSlot::NotStarted;
                return Err(error);
            }
            Err(error) => {
                outgoing.log_message(&format!("{}. Restarting it", error.message), message_type::WARNING);
                *slot = WorkerSlot::NotStarted;
                restarted = true;
            }
        }
    }
}

fn supported_formatters(global_state: &GlobalState, outgoing: &Outgoing) -> Vec<String> {
    let slot = slot_for(global_state);
    let mut slot = lock(&slot);
    ensure_started(&mut slot, global_state, outgoing).map(|worker| worker.formatters.clone()).unwrap_or_default()
}

pub fn active_formatter(global_state: &GlobalState, outgoing: &Outgoing) -> Option<String> {
    let formatter = global_state.formatter.as_str();
    if !WORKER_FORMATTERS.contains(&formatter) {
        return None;
    }
    supported_formatters(global_state, outgoing).into_iter().find(|supported| supported == formatter)
}

pub fn active_linters(global_state: &GlobalState, outgoing: &Outgoing) -> Vec<String> {
    if !global_state.linters.iter().any(|linter| WORKER_FORMATTERS.contains(&linter.as_str())) {
        return Vec::new();
    }
    let supported = supported_formatters(global_state, outgoing);
    global_state.linters.iter().filter(|linter| supported.contains(linter)).cloned().collect()
}

pub fn path_for_uri(uri: &str) -> Option<String> {
    crate::uri::to_path(uri).or_else(|| {
        let (_, opaque) = uri.split_once(':')?;
        (!opaque.starts_with('/')).then(|| opaque.to_string())
    })
}

pub fn run_formatting(
    global_state: &GlobalState,
    outgoing: &Outgoing,
    formatter: &str,
    path: Option<&str>,
    source: &str,
) -> Result<Option<String>, FormatterError> {
    let params = json!({ "formatter": formatter, "path": path, "source": source });
    Ok(request(global_state, outgoing, "formatting", params)?.and_then(|result| result.as_str().map(str::to_string)))
}

pub fn run_range_formatting(
    global_state: &GlobalState,
    outgoing: &Outgoing,
    formatter: &str,
    path: Option<&str>,
    source: &str,
    base_indentation: usize,
) -> Result<Option<String>, FormatterError> {
    let params = json!({ "formatter": formatter, "path": path, "source": source, "base_indentation": base_indentation });
    Ok(request(global_state, outgoing, "range_formatting", params)?.and_then(|result| result.as_str().map(str::to_string)))
}

pub fn run_diagnostic(
    global_state: &GlobalState,
    outgoing: &Outgoing,
    linters: &[String],
    uri: &str,
    source: &str,
) -> Result<Vec<Value>, FormatterError> {
    let params = json!({
        "linters": linters,
        "path": path_for_uri(uri),
        "uri": uri,
        "source": source,
        "encoding": global_state.encoding_name(),
    });
    let result = request(global_state, outgoing, "diagnostic", params)?;
    Ok(result.and_then(|result| result.as_array().cloned()).unwrap_or_default())
}

/// Returns whether RuboCop is available. Ruby LSP loads RuboCop on boot, so a worker that hasn't started yet relies on
/// the detected environment instead of being started only to answer this
pub fn reload_rubocop_configuration(global_state: &GlobalState, outgoing: &Outgoing) -> bool {
    let slot = slot_for(global_state);
    let mut slot = lock(&slot);
    let worker = match &mut *slot {
        WorkerSlot::Running(worker) => worker,
        WorkerSlot::NotStarted => return global_state.ruby_environment.rubocop_version.is_some(),
        WorkerSlot::Unavailable => return false,
    };
    if !worker.rubocop_loaded {
        return false;
    }
    match worker.request("reload_rubocop", json!({})) {
        Ok(Ok(result)) => {
            worker.formatters = string_array(&result["formatters"]);
            if let Some(message) = result["show_message"].as_str() {
                outgoing.show_message(message, message_type::ERROR);
            }
            true
        }
        Ok(Err(error)) => {
            outgoing.log_message(&format!("Failed to reload the RuboCop configuration: {}", error.message), message_type::ERROR);
            true
        }
        Err(error) => {
            outgoing.log_message(&format!("{}. It will be restarted on the next request", error.message), message_type::WARNING);
            *slot = WorkerSlot::NotStarted;
            true
        }
    }
}

pub fn worker_process_id(global_state: &GlobalState) -> Option<u32> {
    let slot = slot_for(global_state);
    let slot = lock(&slot);
    match &*slot {
        WorkerSlot::Running(worker) => Some(worker.child.id()),
        _ => None,
    }
}

fn string_array(value: &Value) -> Vec<String> {
    value.as_array().map(|values| values.iter().filter_map(|value| value.as_str().map(str::to_string)).collect()).unwrap_or_default()
}
