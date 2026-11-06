use std::collections::VecDeque;
use std::fs;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use indexmap::IndexMap;
use rbs_definition::ast::{Declaration, TypeName, TypeNameKind};
use rbs_definition::environment::{ConstantEntry, Environment};
use rbs_definition::loader::RbsGem;
use rbs_definition::location::Location as RbsLocation;
use serde_json::{Value, json};
use steep::annotations_helper::deprecated_type_name;
use steep::environment_loader;
use steep::expectations;
use steep::inference::{Inference, InferenceRules};
use steep::project::dsl;
use steep::ruby::node::SourceRange;
use steep::services::completion_provider::{CompletionError, CompletionItem, ConstantKind, rbs, ruby::Ruby};
use steep::services::content_change::ContentChange;
use steep::services::goto_service::{GotoLocation, GotoService, each_type_name};
use steep::services::hover_provider::content_for;
use steep::services::signature_help_provider::{SignatureHelpError, SignatureHelpProvider};
use steep::services::type_check_service::{TypeCheckService, type_check_inferring};
use steep::source::Source;
use steep::steep_gem;
use steep::steep_version::SteepVersion;
use steep::strict::EnabledChecks;
use steep_interface::types::Type;

use super::inlay_hints::{self, TypeHintKind};
use super::lsp_formatter;
use crate::global_state::message_type;
use crate::position::{Encoding, LineIndex, Position, Range};
use crate::requests::support::interface::LocationLink;

pub const SIGNATURE_RELOAD_DELAY: Duration = Duration::from_millis(300);
const SOURCE_INDEX_WAIT: Duration = Duration::from_secs(2);
const DEFAULT_STEEPFILE: &str = "target :default do\n  check \".\"\n  signature \"sig\"\nend\n";
const BUILTIN_TYPES: &[&str] = &[
    "untyped", "void", "bool", "class", "module", "instance", "nil", "top", "bot",
];
const DIAGNOSTIC_SOURCE: &str = "ruby-lsp-rs";

mod completion_item_kind {
    pub const FUNCTION: u8 = 3;
    pub const FIELD: u8 = 5;
    pub const VARIABLE: u8 = 6;
    pub const CLASS: u8 = 7;
    pub const INTERFACE: u8 = 8;
    pub const KEYWORD: u8 = 14;
    pub const SNIPPET: u8 = 15;
    pub const CONSTANT: u8 = 21;
}

const DEPRECATED_TAG: u8 = 1;
const SNIPPET_INSERT_TEXT_FORMAT: u8 = 2;

#[derive(Debug, Clone)]
pub struct LoadRequest {
    pub workspace_path: PathBuf,
    pub encoding: Encoding,
    pub rbs_gem_directory: Option<PathBuf>,
    pub ruby: Option<PathBuf>,
    pub locked_rbs_directory: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GotoKind {
    Definition,
    Implementation,
    TypeDefinition,
}

#[derive(Debug, Clone)]
pub enum Query {
    Diagnostics {
        path: PathBuf,
    },
    Hover {
        path: PathBuf,
        position: Position,
    },
    Completion {
        path: PathBuf,
        position: Position,
    },
    SignatureHelp {
        path: PathBuf,
        position: Position,
    },
    Goto {
        kind: GotoKind,
        path: PathBuf,
        position: Position,
    },
    InlayHints {
        path: PathBuf,
    },
}

#[derive(Debug, Clone)]
pub struct HoverAnswer {
    pub markdown: String,
    pub range: Range,
}

#[derive(Debug, Clone)]
pub struct CompletionAnswer {
    pub items: Vec<Value>,
    pub is_incomplete: bool,
}

#[derive(Debug, Clone, Default)]
pub struct GotoAnswer {
    pub links: Vec<LocationLink>,
    pub instance_variable_links: Vec<LocationLink>,
    pub type_names: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct TypeHint {
    pub kind: TypeHintKind,
    pub position: Position,
    pub type_text: String,
    pub is_untyped: bool,
    pub is_obvious: bool,
}

#[derive(Debug, Clone)]
pub enum Answer {
    Diagnostics(Option<Vec<Value>>),
    Hover(Option<HoverAnswer>),
    Completion(Option<CompletionAnswer>),
    SignatureHelp(Option<Value>),
    Goto(GotoAnswer),
    InlayHints(Vec<TypeHint>),
}

pub enum Command {
    Load(LoadRequest),
    SetText {
        path: PathBuf,
        text: String,
    },
    RevertToDisk {
        path: PathBuf,
    },
    ProjectConfigurationChanged,
    SetInferMoreThanSteep(bool),
    Query {
        query: Query,
        reply: Sender<Option<Answer>>,
    },
}

#[derive(Debug)]
pub enum Event {
    Loading,
    Loaded { message: String },
    LoadFailed { message: String },
    SignaturesChanged,
    Log { message: String, message_type: u8 },
}

struct Loaded {
    service: TypeCheckService,
    encoding: Encoding,
    steep_version: SteepVersion,
    has_steepfile: bool,
}

struct Worker {
    events: Sender<Event>,
    load_request: Option<LoadRequest>,
    loaded: Option<Loaded>,
    open_texts: IndexMap<PathBuf, String>,
    signature_changes: IndexMap<PathBuf, String>,
    signature_deadline: Option<Instant>,
    rebuild_deadline: Option<Instant>,
    unchecked_paths: VecDeque<PathBuf>,
    last_signature_help: Option<(u32, Value)>,
    inference_rules: InferenceRules,
}

pub fn spawn(commands: Receiver<Command>, events: Sender<Event>) {
    super::install_panic_hook();
    let spawned = thread::Builder::new()
        .name("type-checker".to_string())
        .stack_size(256 * 1024 * 1024)
        .spawn(move || {
            let mut worker = Worker {
                events,
                load_request: None,
                loaded: None,
                open_texts: IndexMap::new(),
                signature_changes: IndexMap::new(),
                signature_deadline: None,
                rebuild_deadline: None,
                unchecked_paths: VecDeque::new(),
                last_signature_help: None,
                inference_rules: InferenceRules::STEEP,
            };
            worker.run(&commands);
        });
    if let Err(error) = spawned {
        eprintln!("Failed to start the type checker: {error}");
    }
}

fn normalized_text(text: &str) -> String {
    text.replace("\r\n", "\n")
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else if let Some(message) = payload.downcast_ref::<&str>() {
        message.to_string()
    } else if let Some(exception) = payload.downcast_ref::<steep::ruby_exception::RubyException>() {
        exception.to_string()
    } else {
        "Unknown panic".to_string()
    }
}

fn rbs_gem(request: &LoadRequest, events: &Sender<Event>) -> Result<RbsGem, String> {
    let ruby = request.ruby.as_deref();
    let gem_paths = || environment_loader::gem_directories(ruby).unwrap_or_default();
    if let Some(directory) = &request.rbs_gem_directory {
        return RbsGem::new(directory, &gem_paths());
    }
    if std::env::var_os(environment_loader::RBS_DIRECTORY_VARIABLE).is_none()
        && let Some(directory) = &request.locked_rbs_directory
        && directory.join("core").is_dir()
    {
        match RbsGem::new(directory, &gem_paths()) {
            Ok(rbs_gem) => return Ok(rbs_gem),
            Err(message) => {
                let _ = events.send(Event::Log {
                    message: format!("{message}; loading the newest supported rbs gem instead"),
                    message_type: message_type::WARNING,
                });
            }
        }
    }
    environment_loader::rbs_gem(ruby, None)
}

fn steep_version(request: &LoadRequest, events: &Sender<Event>) -> SteepVersion {
    match steep_gem::detect_steep_version(&request.workspace_path, request.ruby.as_deref()) {
        Ok(version) => version,
        Err(message) => {
            let _ = events.send(Event::Log {
                message: format!("{message}; type checking as Steep {}", SteepVersion::default()),
                message_type: message_type::WARNING,
            });
            SteepVersion::default()
        }
    }
}

fn load_service(request: &LoadRequest, events: &Sender<Event>) -> Result<Loaded, String> {
    let steepfile_path = request.workspace_path.join("Steepfile");
    let read_steepfile = fs::read_to_string(&steepfile_path).ok();
    let has_steepfile = read_steepfile.is_some();
    let steepfile = read_steepfile.unwrap_or_else(|| DEFAULT_STEEPFILE.to_string());
    let project = dsl::parse(&request.workspace_path, &steepfile)
        .map_err(|error| format!("{}: {error}", steepfile_path.display()))?;
    let steep_version = steep_version(request, events);
    let rbs_gem = rbs_gem(request, events)?;
    steep_version.check_rbs_gem(&rbs_gem)?;
    let jobs = thread::available_parallelism().map_or(1, usize::from);
    let mut service = TypeCheckService::load(project, &rbs_gem, steep_version, EnabledChecks::default(), jobs)?;
    if has_steepfile {
        let mut changes = Vec::new();
        for path in service.project_paths() {
            if let Ok(content) = fs::read_to_string(service.project.absolute_path(&path)) {
                changes.push((path, vec![ContentChange::string(content)]));
            }
        }
        service.update(&changes)?;
    }
    Ok(Loaded {
        service,
        encoding: request.encoding,
        steep_version,
        has_steepfile,
    })
}

impl Worker {
    fn run(&mut self, commands: &Receiver<Command>) {
        loop {
            let deadline = [self.signature_deadline, self.rebuild_deadline]
                .into_iter()
                .flatten()
                .min();
            let received = match deadline {
                Some(deadline) => commands.recv_deadline(deadline),
                None if self.loaded.is_some() && !self.unchecked_paths.is_empty() => {
                    commands.recv_deadline(Instant::now())
                }
                None => commands.recv().map_err(|_| RecvTimeoutError::Disconnected),
            };
            match received {
                Ok(command) => self.handle(command),
                Err(RecvTimeoutError::Disconnected) => return,
                Err(RecvTimeoutError::Timeout) => {
                    let now = Instant::now();
                    if self.rebuild_deadline.is_some_and(|deadline| deadline <= now) {
                        self.rebuild();
                    } else if self.signature_deadline.is_some_and(|deadline| deadline <= now) {
                        self.flush_signature_changes();
                    } else if deadline.is_none() {
                        self.check_next_unchecked_path();
                    }
                }
            }
        }
    }

    fn handle(&mut self, command: Command) {
        match command {
            Command::Load(request) => {
                self.load_request = Some(request);
                self.rebuild();
            }
            Command::SetText { path, text } => {
                self.open_texts.insert(path.clone(), text.clone());
                self.apply_text(&path, text);
            }
            Command::RevertToDisk { path } => {
                self.open_texts.shift_remove(&path);
                let text = fs::read_to_string(&path).unwrap_or_default();
                self.apply_text(&path, text);
            }
            Command::ProjectConfigurationChanged => {
                if self.load_request.is_some() {
                    self.rebuild_deadline = Some(Instant::now() + SIGNATURE_RELOAD_DELAY);
                }
            }
            Command::SetInferMoreThanSteep(enabled) => {
                self.inference_rules = if enabled {
                    InferenceRules::more_than_steep()
                } else {
                    InferenceRules::STEEP
                };
                if let Some(loaded) = &mut self.loaded {
                    loaded.service.inference_rules = self.inference_rules;
                }
            }
            Command::Query { query, reply } => {
                self.flush_signature_changes();
                let answer = catch_unwind(AssertUnwindSafe(|| self.answer(&query))).unwrap_or_else(|payload| {
                    self.log(&format!(
                        "The type checker failed to answer {query:?}: {}",
                        panic_message(&*payload)
                    ));
                    None
                });
                let _ = reply.send(answer);
            }
        }
    }

    fn log(&self, message: &str) {
        let _ = self.events.send(Event::Log {
            message: message.to_string(),
            message_type: message_type::ERROR,
        });
    }

    fn rebuild(&mut self) {
        self.rebuild_deadline = None;
        let Some(request) = self.load_request.clone() else {
            return;
        };
        let _ = self.events.send(Event::Loading);
        self.loaded = None;
        self.signature_changes.clear();
        self.signature_deadline = None;
        let started = Instant::now();
        let outcome = catch_unwind(AssertUnwindSafe(|| load_service(&request, &self.events)))
            .unwrap_or_else(|payload| Err(panic_message(&*payload)));
        match outcome {
            Ok(mut loaded) => {
                loaded.service.inference_rules = self.inference_rules;
                let target_count = loaded.service.project.targets.len();
                let message = format!(
                    "Loaded the signatures of {target_count} target{} as Steep {} does in {:.2}s",
                    if target_count == 1 { "" } else { "s" },
                    loaded.steep_version,
                    started.elapsed().as_secs_f64()
                );
                self.loaded = Some(loaded);
                let open_texts: Vec<(PathBuf, String)> = self
                    .open_texts
                    .iter()
                    .map(|(path, text)| (path.clone(), text.clone()))
                    .collect();
                for (path, text) in open_texts {
                    self.apply_text(&path, text);
                }
                let _ = catch_unwind(AssertUnwindSafe(|| self.flush_signature_changes()));
                self.queue_project_paths();
                let _ = self.events.send(Event::Loaded { message });
            }
            Err(message) => {
                let _ = self.events.send(Event::LoadFailed { message });
            }
        }
    }

    fn queue_project_paths(&mut self) {
        let Some(loaded) = &self.loaded else { return };
        if !loaded.has_steepfile {
            return;
        }
        self.unchecked_paths = loaded
            .service
            .project_paths()
            .into_iter()
            .filter(|path| path.extension().is_some_and(|extension| extension == "rb"))
            .collect();
    }

    fn check_for_source_indexes(&mut self, path: &Path) {
        let deadline = Instant::now() + SOURCE_INDEX_WAIT;
        while !self.unchecked_paths.is_empty() && Instant::now() < deadline {
            self.check_next_unchecked_path();
        }
        let Some(relative_path) = self.relative_path(path) else {
            return;
        };
        let Some(loaded) = &mut self.loaded else { return };
        if loaded.service.source_files.contains_key(&relative_path) {
            let _ = catch_unwind(AssertUnwindSafe(|| loaded.service.typecheck_source(&relative_path)));
        }
    }

    fn check_next_unchecked_path(&mut self) {
        let Some(path) = self.unchecked_paths.pop_front() else {
            return;
        };
        let Some(loaded) = &mut self.loaded else { return };
        let _ = catch_unwind(AssertUnwindSafe(|| loaded.service.typecheck_source(&path)));
    }

    fn relative_path(&self, path: &Path) -> Option<PathBuf> {
        let base_directory = &self.loaded.as_ref()?.service.project.base_directory;
        if let Ok(relative_path) = path.strip_prefix(base_directory) {
            return Some(relative_path.to_path_buf());
        }
        let canonical_base_directory = fs::canonicalize(base_directory).ok()?;
        let canonical_path = match fs::canonicalize(path) {
            Ok(canonical_path) => canonical_path,
            Err(_) => fs::canonicalize(path.parent()?).ok()?.join(path.file_name()?),
        };
        canonical_path
            .strip_prefix(canonical_base_directory)
            .ok()
            .map(Path::to_path_buf)
    }

    fn is_signature_input(&self, relative_path: &Path) -> bool {
        let Some(loaded) = &self.loaded else { return false };
        let project = &loaded.service.project;
        relative_path.extension().is_some_and(|extension| extension == "rbs")
            || project.target_for_signature_path(relative_path).is_some()
            || project.target_for_inline_source_path(relative_path).is_some()
    }

    fn apply_text(&mut self, path: &Path, text: String) {
        let Some(relative_path) = self.relative_path(path) else {
            return;
        };
        if self.is_signature_input(&relative_path) {
            let pending_text = self.signature_changes.get(&relative_path).cloned();
            if pending_text.or_else(|| self.signature_text(&relative_path)).as_deref() == Some(text.as_str()) {
                return;
            }
            if let Some(loaded) = &mut self.loaded
                && let Some(file) = loaded.service.source_files.get_mut(&relative_path)
            {
                file.content = text.clone();
            }
            self.signature_changes.insert(relative_path, text);
            self.signature_deadline = Some(Instant::now() + SIGNATURE_RELOAD_DELAY);
            return;
        }
        let Some(loaded) = &mut self.loaded else { return };
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            loaded
                .service
                .update(&[(relative_path, vec![ContentChange::string(text)])])
        }));
        match outcome {
            Ok(Ok(())) => {}
            Ok(Err(message)) => self.log(&message),
            Err(payload) => self.log(&panic_message(&*payload)),
        }
    }

    fn signature_text(&self, relative_path: &Path) -> Option<String> {
        self.loaded
            .as_ref()?
            .service
            .signature_services
            .values()
            .find_map(|signature| signature.files.get(relative_path))
            .map(|file| file.content().to_string())
    }

    fn flush_signature_changes(&mut self) {
        self.signature_deadline = None;
        if self.signature_changes.is_empty() {
            return;
        }
        let changes: Vec<(PathBuf, Vec<ContentChange>)> = self
            .signature_changes
            .drain(..)
            .map(|(path, text)| (path, vec![ContentChange::string(text)]))
            .collect();
        let Some(loaded) = &mut self.loaded else { return };
        let outcome = catch_unwind(AssertUnwindSafe(|| loaded.service.update(&changes)));
        match outcome {
            Ok(Ok(())) => {}
            Ok(Err(message)) => self.log(&message),
            Err(payload) => self.log(&panic_message(&*payload)),
        }
        self.queue_project_paths();
        let _ = self.events.send(Event::SignaturesChanged);
    }

    fn answer(&mut self, query: &Query) -> Option<Answer> {
        match query {
            Query::Diagnostics { path } => Some(Answer::Diagnostics(self.diagnostics(path))),
            Query::Hover { path, position } => Some(Answer::Hover(self.hover(path, *position))),
            Query::Completion { path, position } => Some(Answer::Completion(self.completion(path, *position))),
            Query::SignatureHelp { path, position } => {
                Some(Answer::SignatureHelp(self.signature_help(path, *position)))
            }
            Query::Goto { kind, path, position } => {
                self.check_for_source_indexes(path);
                Some(Answer::Goto(self.goto(*kind, path, *position)))
            }
            Query::InlayHints { path } => Some(Answer::InlayHints(self.inlay_hints(path))),
        }
    }

    fn file_text(&self, relative_path: &Path) -> Option<String> {
        let service = &self.loaded.as_ref()?.service;
        if let Some(file) = service.source_files.get(relative_path) {
            return Some(normalized_text(&file.content));
        }
        service
            .signature_services
            .values()
            .find_map(|signature| signature.files.get(relative_path))
            .map(|file| normalized_text(file.content()))
    }

    fn offset_of(&self, text: &str, position: Position) -> Option<usize> {
        let encoding = self.loaded.as_ref()?.encoding;
        LineIndex::new(text.as_bytes(), encoding).offset_of(position).ok()
    }

    fn encoding(&self) -> Encoding {
        self.loaded.as_ref().map_or(Encoding::Utf16, |loaded| loaded.encoding)
    }

    fn diagnostics(&mut self, path: &Path) -> Option<Vec<Value>> {
        let relative_path = self.relative_path(path)?;
        let encoding = self.encoding();
        let loaded = self.loaded.as_mut()?;
        let steep_version = loaded.steep_version;
        let service = &mut loaded.service;
        let is_ruby = relative_path.extension().is_some_and(|extension| extension == "rb");
        let mut diagnostics: Vec<expectations::Diagnostic> = Vec::new();
        let text = if is_ruby {
            let project = &service.project;
            if project.target_for_source_path(&relative_path).is_none()
                && project.target_for_inline_source_path(&relative_path).is_none()
            {
                return None;
            }
            diagnostics.extend(
                service
                    .typecheck_source(&relative_path)
                    .ok()
                    .flatten()
                    .unwrap_or_default(),
            );
            if let Some(target) = service.project.target_for_inline_source_path(&relative_path).cloned() {
                diagnostics.extend(
                    service
                        .validate_signature(&relative_path, &target.name)
                        .unwrap_or_default(),
                );
            }
            service
                .source_files
                .get(&relative_path)
                .map(|file| file.content.clone())
                .unwrap_or_default()
        } else {
            let targets = service.signature_file_targets(&relative_path);
            if targets.is_empty() {
                return None;
            }
            for target in targets {
                for diagnostic in service.validate_signature(&relative_path, &target).unwrap_or_default() {
                    if !diagnostics.contains(&diagnostic) {
                        diagnostics.push(diagnostic);
                    }
                }
            }
            service
                .signature_services
                .values()
                .find_map(|signature| signature.files.get(&relative_path))
                .map(|file| file.content().to_string())
                .unwrap_or_default()
        };
        let text = normalized_text(&text);
        let line_index = LineIndex::new(text.as_bytes(), encoding);
        Some(
            diagnostics
                .iter()
                .map(|diagnostic| diagnostic_value(diagnostic, &line_index, &text, steep_version))
                .collect(),
        )
    }

    fn hover(&self, path: &Path, position: Position) -> Option<HoverAnswer> {
        let relative_path = self.relative_path(path)?;
        let text = self.file_text(&relative_path)?;
        let offset = self.offset_of(&text, position)?;
        let service = &self.loaded.as_ref()?.service;
        let content = content_for(service, &relative_path, offset)?;
        let line_index = LineIndex::new(text.as_bytes(), self.encoding());
        Some(HoverAnswer {
            markdown: lsp_formatter::format_hover_content(&content),
            range: source_range(&line_index, content.location()),
        })
    }

    fn completion(&self, path: &Path, position: Position) -> Option<CompletionAnswer> {
        let relative_path = self.relative_path(path)?;
        let text = self.file_text(&relative_path)?;
        let offset = self.offset_of(&text, position)?;
        let line_index = LineIndex::new(text.as_bytes(), self.encoding());
        let service = &self.loaded.as_ref()?.service;
        let project = &service.project;
        let is_source = project.target_for_inline_source_path(&relative_path).is_some()
            || project.target_for_source_path(&relative_path).is_some()
            || service.source_target(&relative_path).is_some() && service.source_files.contains_key(&relative_path);
        if is_source {
            let provider = Ruby::new(service, &relative_path)?;
            let target_name = service.source_target(&relative_path)?.name.clone();
            let environment = service
                .signature_services
                .get(&target_name)?
                .latest_environment()?
                .clone();
            let format = |item: &CompletionItem| format_completion_item(item, &line_index, &environment);
            if let Ok(Some((prefix_size, items))) = provider.run_at_comment(offset) {
                let mut formatted: Vec<Value> = items.iter().map(format).collect();
                formatted.extend(builtin_types(&line_index, &text, offset, prefix_size));
                return Some(CompletionAnswer {
                    items: formatted,
                    is_incomplete: false,
                });
            }
            let items = match provider.run(offset) {
                Ok(items) => items,
                Err(CompletionError::SyntaxError) => Vec::new(),
                Err(_) => return None,
            };
            return Some(CompletionAnswer {
                items: items.iter().map(format).collect(),
                is_incomplete: false,
            });
        }
        let target = project.target_for_signature_path(&relative_path)?;
        let signature = service.signature_services.get(&target.name)?;
        let completion = rbs::run(service, &target.name, &relative_path, offset)?.ok()?;
        let environment = signature.latest_environment()?;
        let start = text.floor_char_boundary(offset.saturating_sub(completion.prefix_size));
        let range = line_index.range_of(start, offset);
        let mut items: Vec<Value> = completion
            .type_names
            .iter()
            .filter_map(|(absolute_name, relative_name)| {
                rbs_completion_item(environment, absolute_name, &relative_name.to_string(), range)
            })
            .collect();
        items.extend(builtin_types(&line_index, &text, offset, completion.prefix_size));
        Some(CompletionAnswer {
            items,
            is_incomplete: !matches!(
                signature.status,
                steep::services::signature_service::SignatureStatus::Loaded
            ),
        })
    }

    fn signature_help(&mut self, path: &Path, position: Position) -> Option<Value> {
        let relative_path = self.relative_path(path)?;
        let text = self.file_text(&relative_path)?;
        let offset = self.offset_of(&text, position)?;
        let service = &self.loaded.as_ref()?.service;
        let project = &service.project;
        if project.target_for_inline_source_path(&relative_path).is_none()
            && project.target_for_source_path(&relative_path).is_none()
        {
            return None;
        }
        let result = SignatureHelpProvider::new(service, &relative_path, offset).and_then(|provider| match provider {
            Some(provider) => provider.run(offset),
            None => Ok(None),
        });
        match result {
            Ok(Some(help)) => {
                let signatures: Vec<Value> = help
                    .items
                    .iter()
                    .map(|item| {
                        let parameters: Vec<Value> = item
                            .parameters()
                            .unwrap_or_default()
                            .into_iter()
                            .map(|parameter| json!({ "label": parameter }))
                            .collect();
                        let mut signature = json!({
                            "label": item.method_type.to_string(),
                            "parameters": parameters,
                        });
                        if let Some(active_parameter) = item.active_parameter {
                            signature["activeParameter"] = json!(active_parameter);
                        }
                        if let Some(comment) = &item.comment {
                            signature["documentation"] =
                                json!({ "kind": "markdown", "value": lsp_formatter::remove_html_comments(comment) });
                        }
                        signature
                    })
                    .collect();
                let mut response = json!({ "signatures": signatures });
                if let Some(active_signature) = help.active_signature {
                    response["activeSignature"] = json!(active_signature);
                }
                self.last_signature_help = Some((position.line, response.clone()));
                Some(response)
            }
            Ok(None) => None,
            Err(SignatureHelpError::SyntaxError) => self
                .last_signature_help
                .as_ref()
                .filter(|(line, _)| *line == position.line)
                .map(|(_, response)| response.clone()),
            Err(_) => None,
        }
    }

    fn goto(&self, kind: GotoKind, path: &Path, position: Position) -> GotoAnswer {
        let mut answer = GotoAnswer::default();
        let Some(relative_path) = self.relative_path(path) else {
            return answer;
        };
        let Some(text) = self.file_text(&relative_path) else {
            return answer;
        };
        let Some(offset) = self.offset_of(&text, position) else {
            return answer;
        };
        let Some(loaded) = &self.loaded else { return answer };
        let service = &loaded.service;
        let goto = GotoService::new(service);
        let locations = match kind {
            GotoKind::Definition => goto.definition(&relative_path, offset),
            GotoKind::Implementation => goto.implementation(&relative_path, offset),
            GotoKind::TypeDefinition => goto.type_definition(&relative_path, offset),
        }
        .unwrap_or_default();
        for link in locations.iter().filter_map(|location| self.goto_link(location)) {
            if !answer.links.contains(&link) {
                answer.links.push(link);
            }
        }
        match kind {
            GotoKind::Definition => {
                answer.instance_variable_links = self.instance_variable_links(&relative_path, offset)
            }
            GotoKind::TypeDefinition => answer.type_names = self.type_names_at(&relative_path, offset),
            GotoKind::Implementation => {}
        }
        answer
    }

    fn goto_link(&self, location: &GotoLocation) -> Option<LocationLink> {
        let service = &self.loaded.as_ref()?.service;
        match location {
            GotoLocation::Rbs(location) => Some(rbs_location_link(
                &service.project.absolute_path(Path::new(location.name())),
                location,
                self.encoding(),
            )),
            GotoLocation::Ruby { path, range } => {
                let text = match service.source_files.get(path) {
                    Some(file) => normalized_text(&file.content),
                    None => normalized_text(&fs::read_to_string(service.project.absolute_path(path)).ok()?),
                };
                let line_index = LineIndex::new(text.as_bytes(), self.encoding());
                let range = source_range(&line_index, *range);
                Some(LocationLink {
                    target_uri: crate::uri::from_path_buf(&service.project.absolute_path(path)),
                    target_range: range,
                    target_selection_range: range,
                })
            }
        }
    }

    fn instance_variable_links(&self, relative_path: &Path, offset: usize) -> Vec<LocationLink> {
        let Some(loaded) = &self.loaded else { return Vec::new() };
        let Some(checked) = loaded.service.type_check_at(relative_path, offset) else {
            return Vec::new();
        };
        let Some(node) = checked
            .source
            .find_nodes(offset)
            .and_then(|nodes| nodes.first().cloned())
        else {
            return Vec::new();
        };
        if !matches!(node.type_, "ivar" | "ivasgn") {
            return Vec::new();
        }
        let Some(name) = node.child_symbol(0) else {
            return Vec::new();
        };
        let cursor_context = checked.typing.cursor_context();
        let Some(context) = cursor_context.context() else {
            return Vec::new();
        };
        let definition_builder = &checked.checker.builder.definition_builder;
        let definition = match &context.self_type {
            Type::NameInstance { name, .. } => definition_builder.build_instance(name).ok(),
            Type::Singleton(name) => definition_builder.build_singleton(name).ok(),
            _ => None,
        };
        let Some(variable) = definition
            .as_ref()
            .and_then(|definition| definition.instance_variables.get(name))
        else {
            return Vec::new();
        };
        let Some(location) = &variable.source.location else {
            return Vec::new();
        };
        let path = loaded.service.project.absolute_path(Path::new(location.name()));
        vec![rbs_location_link(&path, location, loaded.encoding)]
    }

    fn type_names_at(&self, relative_path: &Path, offset: usize) -> Vec<String> {
        let Some(loaded) = &self.loaded else { return Vec::new() };
        let Some(checked) = loaded.service.type_check_at(relative_path, offset) else {
            return Vec::new();
        };
        let Some(node) = checked
            .source
            .find_nodes(offset)
            .and_then(|nodes| nodes.first().cloned())
        else {
            return Vec::new();
        };
        let Some(node_type) = checked.typing.type_of(node.id) else {
            return Vec::new();
        };
        let mut names = Vec::new();
        each_type_name(&node_type, &mut names);
        let mut class_names: Vec<String> = Vec::new();
        for name in names {
            if name.kind() != TypeNameKind::Class {
                continue;
            }
            let name = name.relative().to_string();
            if !class_names.contains(&name) {
                class_names.push(name);
            }
        }
        class_names
    }

    fn inlay_hints(&self, path: &Path) -> Vec<TypeHint> {
        let Some(relative_path) = self.relative_path(path) else {
            return Vec::new();
        };
        let Some(loaded) = &self.loaded else { return Vec::new() };
        let service = &loaded.service;
        let Some(target) = service.source_target(&relative_path) else {
            return Vec::new();
        };
        let Some(file) = service.source_files.get(&relative_path) else {
            return Vec::new();
        };
        let Some(checker) = service.checker(&target.name) else {
            return Vec::new();
        };
        let Ok(source) = Source::parse(
            &relative_path.to_string_lossy(),
            &file.content,
            checker.builder.steep_version,
        ) else {
            return Vec::new();
        };
        let (source, inference) = Inference::prepare(&checker, service.inference_rules, source);
        let typing = type_check_inferring(&checker, &source, None, &inference);
        let hints = inlay_hints::collect(&checker, &source, &typing);
        let text = normalized_text(&file.content);
        let line_index = LineIndex::new(text.as_bytes(), loaded.encoding);
        hints
            .into_iter()
            .map(|hint| TypeHint {
                kind: hint.kind,
                position: line_index.position_of_offset(hint.offset),
                type_text: hint.hint_type.to_string(),
                is_untyped: matches!(hint.hint_type, Type::Any),
                is_obvious: hint.is_obvious,
            })
            .collect()
    }
}

fn source_range(line_index: &LineIndex<'_>, range: SourceRange) -> Range {
    Range::new(
        line_index.position_of_offset(range.begin_pos),
        line_index.position_of_offset(range.end_pos),
    )
}

fn rbs_location_link(path: &Path, location: &RbsLocation, encoding: Encoding) -> LocationLink {
    let line_index = LineIndex::new(location.buffer.content.as_bytes(), encoding);
    let range = Range::new(
        line_index.position_of_offset(location.start()),
        line_index.position_of_offset(location.end()),
    );
    LocationLink {
        target_uri: crate::uri::from_path_buf(path),
        target_range: range,
        target_selection_range: range,
    }
}

fn position_of_line_and_character(line_index: &LineIndex<'_>, text: &str, line: u32, character: u32) -> Position {
    let line = line.saturating_sub(1) as usize;
    let Some(line_start) = line_index.line_start(line) else {
        return line_index.position_of_offset(text.len());
    };
    let line_text = &text[line_start..line_index.line_end(line)];
    let byte_column: usize = line_text.chars().take(character as usize).map(char::len_utf8).sum();
    line_index.position_of_offset(line_start + byte_column)
}

fn severity_number(severity: &str) -> u8 {
    match severity {
        "ERROR" => 1,
        "WARNING" => 2,
        "INFORMATION" => 3,
        _ => 4,
    }
}

fn diagnostic_value(
    diagnostic: &expectations::Diagnostic,
    line_index: &LineIndex<'_>,
    text: &str,
    steep_version: SteepVersion,
) -> Value {
    let range = Range::new(
        position_of_line_and_character(
            line_index,
            text,
            diagnostic.range.start.line,
            diagnostic.range.start.character,
        ),
        position_of_line_and_character(
            line_index,
            text,
            diagnostic.range.end.line,
            diagnostic.range.end.character,
        ),
    );
    let mut severity = severity_number(&diagnostic.severity);
    let mut tags = Vec::new();
    if diagnostic.code == "Ruby::DeprecatedReference" {
        tags.push(2);
    }
    if diagnostic.code == "RBS::DeprecatedTypeName" {
        tags.push(2);
        severity = 2;
    }
    let mut value = json!({
        "range": range,
        "severity": severity,
        "source": DIAGNOSTIC_SOURCE,
        "code": diagnostic.code,
        "message": diagnostic.message,
    });
    if diagnostic.code.starts_with("Ruby::") {
        value["codeDescription"] = json!({
            "href": format!(
                "https://github.com/soutaro/steep/tree/v{steep_version}/manual/ruby-diagnostics.md#{}",
                diagnostic.code.replace(':', "%3A")
            ),
        });
    }
    if !tags.is_empty() {
        value["tags"] = json!(tags);
    }
    value
}

fn text_edit(range: Range, new_text: &str) -> Value {
    json!({ "range": range, "newText": new_text })
}

fn label_details(description: &str) -> Value {
    json!({ "description": description })
}

fn markdown(value: String) -> Value {
    json!({ "kind": "markdown", "value": value })
}

fn type_name_declaration(environment: &Environment, type_name: &TypeName) -> Option<Declaration> {
    match type_name.kind() {
        TypeNameKind::Class => match environment.constant_entry(type_name)? {
            ConstantEntry::Class(entry) => Some(steep::services::hover_provider::class_declaration(
                entry.primary_declaration(),
            )),
            ConstantEntry::ClassAlias(entry) => Some(Declaration::ClassAlias(entry.declaration.clone())),
            ConstantEntry::Constant(_) => None,
        },
        TypeNameKind::Interface => environment
            .interface_declarations
            .get(type_name)
            .map(|entry| Declaration::Interface(entry.declaration.clone())),
        TypeNameKind::Alias => environment
            .type_alias_declarations
            .get(type_name)
            .map(|entry| Declaration::TypeAlias(entry.declaration.clone())),
    }
}

fn type_name_comments(environment: &Environment, type_name: &TypeName) -> Vec<String> {
    match type_name.kind() {
        TypeNameKind::Class => match environment.constant_entry(type_name) {
            Some(ConstantEntry::Class(entry)) => entry
                .declarations()
                .filter_map(|declaration| {
                    lsp_formatter::declaration_comment(&steep::services::hover_provider::class_declaration(declaration))
                        .map(str::to_string)
                })
                .collect(),
            Some(ConstantEntry::ClassAlias(entry)) => entry.declaration.comment.iter().cloned().collect(),
            _ => Vec::new(),
        },
        TypeNameKind::Interface | TypeNameKind::Alias => type_name_declaration(environment, type_name)
            .as_ref()
            .and_then(lsp_formatter::declaration_comment)
            .map(str::to_string)
            .into_iter()
            .collect(),
    }
}

fn type_name_kind_number(type_name: &TypeName) -> u8 {
    match type_name.kind() {
        TypeNameKind::Class => completion_item_kind::CLASS,
        TypeNameKind::Interface => completion_item_kind::INTERFACE,
        TypeNameKind::Alias => completion_item_kind::FIELD,
    }
}

fn deprecated_tags(deprecated: bool) -> Value {
    if deprecated { json!([DEPRECATED_TAG]) } else { json!([]) }
}

fn rbs_completion_item(environment: &Environment, type_name: &TypeName, label: &str, range: Range) -> Option<Value> {
    let type_name = environment
        .normalize_type_name(type_name)
        .unwrap_or_else(|| type_name.clone());
    let declaration = type_name_declaration(environment, &type_name)?;
    let comments = type_name_comments(environment, &type_name);
    let deprecated = deprecated_type_name(&type_name, environment).is_some();
    Some(json!({
        "label": label,
        "labelDetails": label_details(&lsp_formatter::declaration_summary(&declaration)),
        "documentation": markdown(lsp_formatter::format_rbs_completion_documentation(&type_name, &declaration, &comments)),
        "textEdit": text_edit(range, label),
        "kind": type_name_kind_number(&type_name),
        "tags": deprecated_tags(deprecated),
    }))
}

fn builtin_types(line_index: &LineIndex<'_>, text: &str, offset: usize, prefix_size: usize) -> Vec<Value> {
    let start = text.floor_char_boundary(offset.saturating_sub(prefix_size));
    let range = line_index.range_of(start, offset);
    BUILTIN_TYPES
        .iter()
        .map(|name| {
            json!({
                "label": name,
                "detail": "(builtin type)",
                "textEdit": text_edit(range, name),
                "kind": completion_item_kind::KEYWORD,
                "filterText": name,
                "sortText": format!("zz__{name}"),
            })
        })
        .collect()
}

fn format_completion_item(item: &CompletionItem, line_index: &LineIndex<'_>, environment: &Environment) -> Value {
    let range = source_range(line_index, item.range());
    let documentation =
        |declaration: Option<&Declaration>| markdown(lsp_formatter::format_completion_documentation(item, declaration));
    let mut value = match item {
        CompletionItem::LocalVariable { identifier, type_, .. } => json!({
            "label": identifier,
            "kind": completion_item_kind::VARIABLE,
            "labelDetails": label_details(&type_.to_string()),
            "documentation": documentation(None),
            "insertText": identifier,
            "sortText": identifier,
        }),
        CompletionItem::Constant {
            identifier,
            kind,
            declaration,
            deprecated,
            ..
        } => json!({
            "label": identifier,
            "kind": match kind {
                ConstantKind::Class | ConstantKind::Module => completion_item_kind::CLASS,
                ConstantKind::Constant => completion_item_kind::CONSTANT,
            },
            "labelDetails": label_details(&declaration.as_ref().map(lsp_formatter::declaration_summary).unwrap_or_default()),
            "documentation": documentation(None),
            "textEdit": text_edit(range, identifier),
            "tags": deprecated_tags(*deprecated),
        }),
        CompletionItem::SimpleMethodName {
            identifier,
            method_name,
            deprecated,
            ..
        } => json!({
            "label": identifier,
            "kind": completion_item_kind::FUNCTION,
            "labelDetails": label_details(&method_name.relative().to_string()),
            "insertText": identifier,
            "documentation": documentation(None),
            "tags": deprecated_tags(*deprecated),
        }),
        CompletionItem::ComplexMethodName {
            identifier,
            method_decls,
            ..
        } => {
            let mut method_names: Vec<String> = Vec::new();
            for declaration in method_decls {
                let name = declaration.name().relative().to_string();
                if !method_names.contains(&name) {
                    method_names.push(name);
                }
            }
            json!({
                "label": identifier,
                "kind": completion_item_kind::FUNCTION,
                "labelDetails": label_details(&method_names.join(", ")),
                "insertText": identifier,
                "documentation": documentation(None),
            })
        }
        CompletionItem::GeneratedMethodName { identifier, .. } => json!({
            "label": identifier,
            "kind": completion_item_kind::FUNCTION,
            "labelDetails": label_details("(Generated)"),
            "insertText": identifier,
            "documentation": documentation(None),
        }),
        CompletionItem::InstanceVariable { identifier, type_, .. } => json!({
            "label": identifier,
            "kind": completion_item_kind::FIELD,
            "labelDetails": label_details(&type_.to_string()),
            "documentation": documentation(None),
            "textEdit": text_edit(range, identifier),
        }),
        CompletionItem::KeywordArgument { identifier, .. } => json!({
            "label": identifier,
            "kind": completion_item_kind::FIELD,
            "labelDetails": label_details("Keyword argument"),
            "documentation": documentation(None),
            "textEdit": text_edit(range, identifier),
        }),
        CompletionItem::TypeName {
            absolute_type_name,
            relative_type_name,
            ..
        } => {
            let declaration = type_name_declaration(environment, absolute_type_name);
            let deprecated = deprecated_type_name(absolute_type_name, environment).is_some();
            json!({
                "label": relative_type_name.to_string(),
                "kind": type_name_kind_number(absolute_type_name),
                "documentation": documentation(declaration.as_ref()),
                "textEdit": text_edit(range, &relative_type_name.to_string()),
                "tags": deprecated_tags(deprecated),
            })
        }
        CompletionItem::Text {
            text, help_text, label, ..
        } => json!({
            "label": label,
            "labelDetails": label_details(help_text),
            "kind": completion_item_kind::SNIPPET,
            "insertTextFormat": SNIPPET_INSERT_TEXT_FORMAT,
            "textEdit": text_edit(range, text),
        }),
    };
    if let Some(method_owner) = method_owner(item) {
        value["data"] = json!({ "type_checker": true, "owner_name": method_owner });
    } else {
        value["data"] = json!({ "skip_resolve": true });
    }
    value
}

fn method_owner(item: &CompletionItem) -> Option<String> {
    match item {
        CompletionItem::SimpleMethodName { method_name, .. } => Some(method_name.type_name().relative().to_string()),
        _ => None,
    }
}
