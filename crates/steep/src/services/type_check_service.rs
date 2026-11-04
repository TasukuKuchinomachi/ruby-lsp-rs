use std::{
    cell::RefCell,
    fs,
    panic::{self, AssertUnwindSafe},
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
};

use rbs_definition::{
    ast::TypeName,
    collections::{HashMap, HashSet, IndexMap},
    loader::RbsGem,
    parallel::run_in_parallel,
};
use steep_interface::{
    steep_version::SteepVersion,
    types::{Type, reset_fresh_variable_count},
};

use crate::{
    annotation::AnnotationError,
    diagnostic::{
        configuration::{Configuration, Severity},
        ruby::{self as diagnostics, Diagnostic},
    },
    environment_loader,
    expectations::{self, Expectations, Position, Range},
    ignore::{self, IgnoreRanges},
    index::source_index::SourceIndex,
    inline_source, locator,
    project::{CollectionConfiguration, Project, Target, UniquePaths},
    ruby::node::{Buffer, SourceRange},
    ruby_exception::{self, RubyException},
    services::{
        content_change::ContentChange,
        signature_service::{LoadedSignatures, SignatureService, signature_target_names},
    },
    signature::{diagnostic::Diagnostic as SignatureDiagnostic, validator::ValidatorCache},
    source::{Source, SourceError},
    strict::{EnabledChecks, Observation, StrictCheck},
    type_construction::{Checker, TypeConstruction, instance_type},
    type_inference::{
        constant_env::ConstantEnv,
        context::{CallContext, Context, ModuleContext, TypeVariableContext},
        type_env::TypeEnv,
    },
    typing::CursorRange,
    typing::Typing,
};

pub struct FileDiagnostics {
    pub path: PathBuf,
    pub diagnostics: Vec<expectations::Diagnostic>,
}

pub fn lsp_range(buffer: &Buffer, location: SourceRange) -> Range {
    Range {
        start: Position {
            line: buffer.line(location.begin_pos) as u32,
            character: buffer.column(location.begin_pos) as u32,
        },
        end: Position {
            line: buffer.line(location.end_pos) as u32,
            character: buffer.column(location.end_pos) as u32,
        },
    }
}

pub fn type_check(checker: &Checker, source: &Source, cursor: Option<usize>) -> Typing {
    let annotations = checker.annotations(source, source.node.as_deref(), &None);
    let object = TypeName::parse("::Object");
    let (module_name, module_type, instance_type_) = match &annotations.self_type {
        Some(Type::NameInstance { name, .. }) => (
            name.clone(),
            Type::Singleton(name.clone()),
            annotations.self_type.clone().expect("self type"),
        ),
        Some(Type::Singleton(name)) => (
            name.clone(),
            Type::Singleton(name.clone()),
            Type::Singleton(name.clone()),
        ),
        _ => (
            object.clone(),
            Type::Singleton(object.clone()),
            instance_type("::Object", Vec::new()),
        ),
    };
    let builder = &checker.builder;
    let definition = builder.definition_builder.build_instance(&module_name).ok();
    let constant_env = Rc::new(ConstantEnv {
        context: None,
        resolver: checker.constant_resolver.clone(),
    });
    let mut type_env = TypeEnv::new(constant_env);
    for (name, type_) in &annotations.const_types {
        type_env.constant_types.insert(name.clone(), type_.clone());
    }
    let mut global_names: Vec<&String> = checker.environment().global_declarations.keys().collect();
    global_names.sort();
    for name in global_names {
        let declaration = &checker.environment().global_declarations[name];
        Rc::make_mut(&mut type_env.global_types)
            .insert(name.clone(), builder.factory.type_(&declaration.type_));
    }
    if let Some(definition) = &definition {
        type_env.instance_variable_types = Rc::new(
            definition
                .instance_variables
                .iter()
                .map(|(name, variable)| (name.clone(), builder.factory.type_(&variable.type_)))
                .collect(),
        );
    }
    for (name, type_) in &annotations.ivar_types {
        Rc::make_mut(&mut type_env.instance_variable_types).insert(name.clone(), type_.clone());
    }
    for (name, type_) in &annotations.var_types {
        type_env
            .local_variable_types
            .insert(name.clone(), (type_.clone(), Some(type_.clone())));
    }
    let context = Context {
        identity: Default::default(),
        method_context: None,
        block_context: None,
        break_context: None,
        module_context: Rc::new(ModuleContext {
            instance_type: instance_type_.clone(),
            module_type,
            implement_name: None,
            nesting: None,
            class_name: module_name.clone(),
            instance_definition: builder.definition_builder.build_instance(&module_name).ok(),
            module_definition: builder
                .definition_builder
                .build_singleton(&module_name)
                .ok(),
            defined_instance_methods: Rc::default(),
            defined_module_methods: Rc::default(),
        }),
        self_type: instance_type_,
        type_env,
        call_context: CallContext::TopLevel,
        variable_context: Rc::new(TypeVariableContext::empty()),
    };
    let root_context = cursor.map(|_| {
        (
            CursorRange::new(0, source.buffer.source.len()),
            context.clone(),
        )
    });
    let typing = Typing::with_cursor(cursor, root_context);
    let construction = TypeConstruction {
        checker,
        source,
        annotations: Rc::new(annotations),
        typing: typing.clone(),
        context,
    };
    if let Some(node) = &source.node {
        construction.synthesize(node, None, false);
    }
    typing
}

pub(crate) struct RubyDiagnostics {
    pub errors: Vec<Diagnostic>,
    pub observations: Vec<Observation>,
    pub source_index: Option<SourceIndex>,
}

impl RubyDiagnostics {
    fn errors_only(errors: Vec<Diagnostic>) -> RubyDiagnostics {
        RubyDiagnostics {
            errors,
            observations: Vec::new(),
            source_index: None,
        }
    }
}

pub(crate) fn ruby_diagnostics(
    checker: &Checker,
    path: &str,
    text: &str,
) -> (Buffer, Result<RubyDiagnostics, String>) {
    match Source::parse(path, text, checker.builder.steep_version) {
        Ok(source) => {
            let result = panic::catch_unwind(AssertUnwindSafe(|| {
                let typing = type_check(checker, &source, None);
                let mut observations = source.observations.clone();
                observations.extend(typing.observations());
                RubyDiagnostics {
                    errors: typing.errors(),
                    observations,
                    source_index: Some(typing.take_source_index()),
                }
            }));
            let buffer = Buffer::new(path, text);
            match result {
                Ok(result) => {
                    let ignores = source
                        .comments
                        .iter()
                        .filter_map(|comment| ignore::parse(comment, &buffer))
                        .collect();
                    let ignore_ranges = IgnoreRanges::new(ignores);
                    let observations = result
                        .observations
                        .into_iter()
                        .filter(|observation| !ignore_ranges.covers(observation.location, &buffer))
                        .collect();
                    let errors = ignore_ranges.filter(result.errors, &buffer);
                    (
                        buffer,
                        Ok(RubyDiagnostics {
                            errors,
                            observations,
                            source_index: result.source_index,
                        }),
                    )
                }
                Err(error) => {
                    let message = error
                        .downcast_ref::<String>()
                        .cloned()
                        .or_else(|| {
                            error
                                .downcast_ref::<&str>()
                                .map(|message| message.to_string())
                        })
                        .or_else(|| {
                            error
                                .downcast_ref::<RubyException>()
                                .map(RubyException::to_string)
                        })
                        .unwrap_or_default();
                    (buffer, Err(message))
                }
            }
        }
        Err(SourceError::Syntax(error)) => (
            Buffer::new(path, text),
            Ok(RubyDiagnostics {
                errors: vec![diagnostics::syntax_error(&error.message, error.location)],
                observations: vec![Observation::new(
                    StrictCheck::RubySyntaxError,
                    error.location,
                    format!(
                        "Steep does not type check this file because of a Ruby syntax error: {}",
                        error.message
                    ),
                )],
                source_index: None,
            }),
        ),
        Err(SourceError::Annotation(AnnotationError::Syntax(error))) => (
            Buffer::new(path, text),
            Ok(RubyDiagnostics::errors_only(vec![
                diagnostics::annotation_syntax_error(&error.message, error.location),
            ])),
        ),
        Err(SourceError::Annotation(AnnotationError::Unexpected { message, location })) => (
            Buffer::new(path, text),
            Ok(RubyDiagnostics {
                errors: Vec::new(),
                observations: vec![Observation::new(
                    StrictCheck::TypeCheckAborted,
                    location,
                    format!(
                        "Steep reports nothing for this file because parsing this annotation raised RuntimeError outside TypeConstruction#synthesize: {message}"
                    ),
                )],
                source_index: None,
            }),
        ),
    }
}

fn target_signature_paths(project: &Project, target: &Target) -> Vec<PathBuf> {
    let mut paths = UniquePaths::default();
    let patterns = std::iter::once(&target.signature_pattern)
        .chain(target.groups.iter().map(|group| &group.signature_pattern));
    for pattern in patterns {
        for path in project.source_paths(pattern) {
            let belongs = project
                .target_for_signature_path(&path)
                .is_some_and(|found| found.name == target.name);
            if belongs {
                paths.push(path);
            }
        }
    }
    paths.into_paths()
}

fn library_rbs_error(error: &SignatureDiagnostic, text_length: usize) -> Diagnostic {
    let mut lines = vec![error.header_line.clone()];
    if let Some(location) = &error.location {
        lines.push(format!(
            "({}:{}:{})",
            location.name(),
            location.start_line(),
            location.start_column()
        ));
    }
    if let Some(detail) = &error.detail_lines {
        lines.push(String::new());
        lines.push(detail.clone());
    }
    Diagnostic::new(
        "LibraryRBSError",
        Some(SourceRange::new(0, text_length)),
        "Type checking failed due to error in library RBS file",
    )
    .with_detail_lines(Some(lines.join("\n")))
}

fn hidden_diagnostic_severity(error_name: &str) -> Severity {
    Configuration::strict()
        .severity_for(error_name)
        .filter(|severity| *severity <= Severity::Warning)
        .unwrap_or(Severity::Error)
}

fn source_diagnostics(
    target: &Target,
    path: &Path,
    text: &str,
    checker: Option<&Checker>,
    library_errors: &[&SignatureDiagnostic],
    enabled_checks: &EnabledChecks,
) -> (Buffer, Result<CheckedSource, String>) {
    let path_string = path.to_string_lossy().into_owned();
    let (buffer, result) = match checker {
        Some(checker) => ruby_diagnostics(checker, &path_string, text),
        None if library_errors.is_empty() => (
            Buffer::new(&path_string, text),
            Ok(RubyDiagnostics {
                errors: Vec::new(),
                observations: vec![Observation::new(
                    StrictCheck::SignatureLoadFailure,
                    SourceRange::new(0, text.len()),
                    format!(
                        "Steep does not type check this file because the signatures of target `{}` failed to load",
                        target.name
                    ),
                )],
                source_index: None,
            }),
        ),
        None => (
            Buffer::new(&path_string, text),
            Ok(RubyDiagnostics::errors_only(
                library_errors
                    .iter()
                    .map(|error| library_rbs_error(error, text.len()))
                    .collect(),
            )),
        ),
    };
    let result = match result {
        Ok(result) => result,
        Err(message) => return (buffer, Err(message)),
    };
    let configuration = target.code_diagnostics_config_for_source(path);
    let visible_severity = |error_name: &str| {
        configuration
            .severity_for(error_name)
            .filter(|severity| *severity <= Severity::Warning)
    };
    let mut diagnostics = Vec::new();
    for error in &result.errors {
        let Some(location) = error.location else {
            continue;
        };
        let severity = match visible_severity(error.error_name) {
            Some(severity) => severity,
            None if enabled_checks.contains(StrictCheck::HiddenSteepDiagnostics) => {
                hidden_diagnostic_severity(error.error_name)
            }
            None => continue,
        };
        diagnostics.push(expectations::Diagnostic {
            range: lsp_range(&buffer, location),
            severity: severity.expectation_name().to_string(),
            message: error.full_message(),
            code: error.diagnostic_code(),
        });
    }
    for observation in &result.observations {
        if !observation.is_shown(enabled_checks) {
            continue;
        }
        if observation.check == StrictCheck::RubySyntaxError
            && visible_severity("SyntaxError").is_some()
        {
            continue;
        }
        diagnostics.push(expectations::Diagnostic {
            range: lsp_range(&buffer, observation.location),
            severity: observation.severity().expectation_name().to_string(),
            message: observation.message.clone(),
            code: observation.check.code(),
        });
    }
    (
        buffer,
        Ok(CheckedSource {
            diagnostics,
            source_index: result.source_index,
        }),
    )
}

pub struct CheckResult {
    pub files: Vec<FileDiagnostics>,
    pub all_files: Vec<PathBuf>,
    pub failures: Vec<String>,
}

enum Work {
    Signature(PathBuf),
    Source(PathBuf),
    Inline(PathBuf),
}

enum WorkResult {
    File(FileDiagnostics),
    Failure(String),
}

struct Worker {
    checker: Option<Checker>,
    validator_cache: Rc<ValidatorCache>,
}

pub fn validate_signature(
    service: &SignatureService,
    checker: Option<&Checker>,
    validator_cache: Rc<ValidatorCache>,
    path: &Path,
) -> Result<Vec<expectations::Diagnostic>, String> {
    let path_string = path.to_string_lossy().into_owned();
    let diagnostics = match (checker, service.current_signatures()) {
        (Some(checker), Some(signatures)) => ruby_exception::rescue(|| {
            signatures.signature_validation().validate_path(
                &checker.builder,
                validator_cache,
                &path_string,
            )
        })
        .map_err(|exception| {
            format!(
                "validating signatures in {} raised {exception}, which Steep does not rescue",
                path.display()
            )
        })?,
        _ => service.error_diagnostics_for(&path_string),
    };
    Ok(diagnostics
        .iter()
        .filter_map(SignatureDiagnostic::to_expectation)
        .collect())
}

pub struct CheckedSource {
    pub diagnostics: Vec<expectations::Diagnostic>,
    pub source_index: Option<SourceIndex>,
}

pub fn check_source(
    target: &Target,
    path: &Path,
    text: &str,
    checker: Option<&Checker>,
    library_errors: &[&SignatureDiagnostic],
    enabled_checks: &EnabledChecks,
    inline: bool,
) -> Result<CheckedSource, String> {
    match source_diagnostics(target, path, text, checker, library_errors, enabled_checks) {
        (buffer, Ok(mut checked)) => {
            if inline {
                checked
                    .diagnostics
                    .extend(inline_source::inline_diagnostics(&buffer));
            }
            Ok(checked)
        }
        (_, Err(message)) => Err(format!(
            "type checking {} failed: {message}",
            path.display()
        )),
    }
}

pub fn target_source_paths(project: &Project, target: &Target) -> Vec<PathBuf> {
    let mut source_paths: Vec<PathBuf> = Vec::new();
    for group in &target.groups {
        source_paths.extend(project.source_paths(&group.source_pattern));
    }
    source_paths.extend(project.source_paths(&target.source_pattern));
    let mut paths = UniquePaths::default();
    for path in source_paths {
        if paths.contains(&path) || !target.is_possible_source_file(&path) {
            continue;
        }
        if project
            .target_for_source_path(&path)
            .map(|found| &found.name)
            != Some(&target.name)
        {
            continue;
        }
        paths.push(path);
    }
    paths.into_paths()
}

pub fn check_project(
    project: &Project,
    enabled_checks: &EnabledChecks,
    steep_version: SteepVersion,
    ruby: Option<&Path>,
    jobs: usize,
) -> Result<CheckResult, String> {
    let rbs_gem = environment_loader::rbs_gem(ruby)?;
    let mut files = Vec::new();
    let mut all_files = UniquePaths::default();
    let mut failures = Vec::new();
    for target in &project.targets {
        for path in project
            .source_paths(&target.source_pattern)
            .into_iter()
            .chain(project.source_paths(&target.signature_pattern))
        {
            all_files.push(path);
        }
    }
    for (target_index, target) in project.targets.iter().enumerate() {
        let service = match SignatureService::load(&rbs_gem, project, target, steep_version, jobs) {
            Ok(service) => service,
            Err(error) => {
                failures.push(format!(
                    "cannot load signatures of target `{}`: {error}",
                    target.name
                ));
                continue;
            }
        };
        let mut works: Vec<Work> = target_signature_paths(project, target)
            .into_iter()
            .chain(inline_source::target_inline_paths(project, target))
            .map(Work::Signature)
            .collect();
        works.extend(
            target_source_paths(project, target)
                .into_iter()
                .map(Work::Source),
        );
        works.extend(
            inline_source::target_inline_paths(project, target)
                .into_iter()
                .map(Work::Inline),
        );
        let library_errors = service.library_errors();
        let signatures = service.current_signatures();
        if let Some(signatures) = signatures {
            signatures.checker_tables();
        }
        let new_worker = || Worker {
            checker: signatures.map(|signatures| signatures.checker()),
            validator_cache: Rc::default(),
        };
        let check = |worker: &Worker, path: &PathBuf, inline: bool| -> Result<WorkResult, String> {
            let text = fs::read_to_string(project.absolute_path(path))
                .map_err(|error| format!("{}: {error}", path.display()))?;
            reset_fresh_variable_count();
            Ok(
                match check_source(
                    target,
                    path,
                    &text,
                    worker.checker.as_ref(),
                    &library_errors,
                    enabled_checks,
                    inline,
                ) {
                    Ok(checked) => WorkResult::File(FileDiagnostics {
                        path: path.clone(),
                        diagnostics: checked.diagnostics,
                    }),
                    Err(message) => WorkResult::Failure(message),
                },
            )
        };
        // Why not free the states of the workers, the definitions and the service of the
        // last target: freeing them takes a noticeable part of the run just before the
        // process exits.
        let last_target = target_index + 1 == project.targets.len();
        let results = run_in_parallel(
            &works,
            jobs,
            new_worker,
            |worker, work| match work {
                Work::Signature(path) => Ok(
                    match validate_signature(
                        &service,
                        worker.checker.as_ref(),
                        worker.validator_cache.clone(),
                        path,
                    ) {
                        Ok(diagnostics) => WorkResult::File(FileDiagnostics {
                            path: path.clone(),
                            diagnostics,
                        }),
                        Err(message) => WorkResult::Failure(message),
                    },
                ),
                Work::Source(path) => check(worker, path, false),
                Work::Inline(path) => check(worker, path, true),
            },
            last_target,
        );
        for result in results {
            match result? {
                WorkResult::File(file) => files.push(file),
                WorkResult::Failure(message) => failures.push(message),
            }
        }
        if last_target {
            std::mem::forget(service);
        }
    }
    let mut all_files = all_files.into_paths();
    all_files.sort();
    Ok(CheckResult {
        files,
        all_files,
        failures,
    })
}

pub fn to_expectations(result: &CheckResult) -> Expectations {
    let mut expectations = Expectations::default();
    let all_files: HashSet<&PathBuf> = result.all_files.iter().collect();
    for file in &result.files {
        if !all_files.contains(&file.path) {
            continue;
        }
        let key = file.path.to_string_lossy().into_owned();
        expectations
            .diagnostics
            .entry(key)
            .or_default()
            .extend(file.diagnostics.iter().cloned());
    }
    expectations
}

pub struct SourceFile {
    pub path: PathBuf,
    pub content: String,
    pub diagnostics: Option<Vec<expectations::Diagnostic>>,
    pub source_index: Option<SourceIndex>,
}

impl SourceFile {
    fn no_data(path: PathBuf, content: String) -> SourceFile {
        SourceFile {
            path,
            content,
            diagnostics: None,
            source_index: None,
        }
    }
}

pub struct TypeCheckedSource {
    pub target_name: String,
    pub checker: Rc<Checker>,
    pub source: Source,
    pub typing: Typing,
}

struct CachedChecker {
    signatures: Arc<LoadedSignatures>,
    checker: Rc<Checker>,
}

pub struct TypeCheckService {
    pub project: Project,
    pub enabled_checks: EnabledChecks,
    pub signature_services: IndexMap<String, SignatureService>,
    pub source_files: IndexMap<PathBuf, SourceFile>,
    pub signature_validation_diagnostics:
        IndexMap<String, IndexMap<PathBuf, Vec<expectations::Diagnostic>>>,
    checkers: RefCell<HashMap<String, CachedChecker>>,
    validator_cache: Rc<ValidatorCache>,
}

impl TypeCheckService {
    pub fn load(
        project: Project,
        rbs_gem: &RbsGem,
        steep_version: SteepVersion,
        enabled_checks: EnabledChecks,
        jobs: usize,
    ) -> Result<TypeCheckService, String> {
        let mut signature_services = IndexMap::default();
        let mut signature_validation_diagnostics = IndexMap::default();
        for target in &project.targets {
            signature_services.insert(
                target.name.clone(),
                SignatureService::load(rbs_gem, &project, target, steep_version, jobs)?,
            );
            signature_validation_diagnostics.insert(target.name.clone(), IndexMap::default());
        }
        Ok(TypeCheckService {
            project,
            enabled_checks,
            signature_services,
            source_files: IndexMap::default(),
            signature_validation_diagnostics,
            checkers: RefCell::default(),
            validator_cache: Rc::default(),
        })
    }

    pub fn is_project_configuration(&self, path: &Path) -> bool {
        let absolute_path = self.project.absolute_path(path);
        if absolute_path == self.project.absolute_path(Path::new("Steepfile")) {
            return true;
        }
        self.project.targets.iter().any(|target| {
            let configuration = match &self.project.options(target).collection_config_path {
                CollectionConfiguration::Path(path) => Some(self.project.absolute_path(path)),
                CollectionConfiguration::Default => {
                    Some(self.project.absolute_path(Path::new("rbs_collection.yaml")))
                }
                CollectionConfiguration::Disabled => None,
            };
            configuration.is_some_and(|configuration| {
                absolute_path == configuration
                    || absolute_path == rbs_definition::collection::lockfile_path(&configuration)
            })
        })
    }

    pub fn project_paths(&self) -> Vec<PathBuf> {
        let mut paths = UniquePaths::default();
        for target in &self.project.targets {
            for path in self.project.each_path_in_target(target) {
                paths.push(path);
            }
        }
        paths.into_paths()
    }

    pub fn load_files(&mut self) -> Result<(), String> {
        let mut changes = Vec::new();
        for path in self.project_paths() {
            let absolute_path = self.project.absolute_path(&path);
            let content = fs::read_to_string(&absolute_path)
                .map_err(|error| format!("{}: {error}", absolute_path.display()))?;
            changes.push((path, vec![ContentChange::string(content)]));
        }
        self.update(&changes)
    }

    pub fn source_target(&self, path: &Path) -> Option<&Target> {
        self.project
            .target_for_inline_source_path(path)
            .or_else(|| self.project.target_for_source_path(path))
            .or_else(|| {
                path.extension()
                    .is_some_and(|extension| extension == "rb")
                    .then(|| self.project.targets.first())
                    .flatten()
            })
    }

    pub fn is_source_file(&self, path: &Path) -> bool {
        self.source_files.contains_key(path) || self.source_target(path).is_some()
    }

    pub fn signature_file_targets(&self, path: &Path) -> Vec<String> {
        self.signature_services
            .iter()
            .filter(|(_, service)| {
                service.files.contains_key(path)
                    || service
                        .env_rbs_paths()
                        .contains(&self.project.absolute_path(path))
            })
            .map(|(name, _)| name.clone())
            .collect()
    }

    pub fn update(&mut self, changes: &[(PathBuf, Vec<ContentChange>)]) -> Result<(), String> {
        let mut signature_changes: IndexMap<String, Vec<(PathBuf, Vec<ContentChange>)>> =
            IndexMap::default();
        for (path, path_changes) in changes {
            for target_name in signature_target_names(&self.project, path) {
                signature_changes
                    .entry(target_name)
                    .or_default()
                    .push((path.clone(), path_changes.clone()));
            }
        }
        for (target_name, target_changes) in signature_changes {
            if let Some(service) = self.signature_services.get_mut(&target_name) {
                service.update(&self.project, &target_changes)?;
            }
        }
        for (path, path_changes) in changes {
            if !self.is_source_file(path) {
                continue;
            }
            let old_content = self
                .source_files
                .get(path)
                .map(|file| file.content.clone())
                .unwrap_or_default();
            let content = path_changes
                .iter()
                .fold(old_content, |text, change| change.apply_to(&text));
            match self.source_files.get_mut(path) {
                Some(file) => file.content = content,
                None => {
                    self.source_files
                        .insert(path.clone(), SourceFile::no_data(path.clone(), content));
                }
            }
        }
        Ok(())
    }

    pub fn checker(&self, target_name: &str) -> Option<Rc<Checker>> {
        let signatures = self
            .signature_services
            .get(target_name)?
            .current_signatures()?;
        let mut checkers = self.checkers.borrow_mut();
        if let Some(cached) = checkers.get(target_name)
            && Arc::ptr_eq(&cached.signatures, signatures)
        {
            return Some(cached.checker.clone());
        }
        let checker = Rc::new(signatures.checker());
        checkers.insert(
            target_name.to_string(),
            CachedChecker {
                signatures: signatures.clone(),
                checker: checker.clone(),
            },
        );
        Some(checker)
    }

    pub fn typecheck_source(
        &mut self,
        path: &Path,
    ) -> Result<Option<Vec<expectations::Diagnostic>>, String> {
        let Some(target) = self.source_target(path).cloned() else {
            return Ok(None);
        };
        let Some(file) = self.source_files.get(path) else {
            return Ok(None);
        };
        let content = file.content.clone();
        let checker = self.checker(&target.name);
        let service = &self.signature_services[&target.name];
        let library_errors = service.library_errors();
        let inline = self.project.target_for_inline_source_path(path).is_some();
        let checked = check_source(
            &target,
            path,
            &content,
            checker.as_deref(),
            &library_errors,
            &self.enabled_checks,
            inline,
        )?;
        let file = self
            .source_files
            .get_mut(path)
            .expect("the source file is loaded");
        file.diagnostics = Some(checked.diagnostics.clone());
        file.source_index = checked.source_index;
        Ok(Some(checked.diagnostics))
    }

    pub fn validate_signature(
        &mut self,
        path: &Path,
        target_name: &str,
    ) -> Result<Vec<expectations::Diagnostic>, String> {
        let checker = self.checker(target_name);
        let service = self
            .signature_services
            .get(target_name)
            .ok_or_else(|| format!("no target named {target_name}"))?;
        let diagnostics = validate_signature(
            service,
            checker.as_deref(),
            self.validator_cache.clone(),
            path,
        )?;
        self.signature_validation_diagnostics
            .entry(target_name.to_string())
            .or_default()
            .insert(path.to_path_buf(), diagnostics.clone());
        Ok(diagnostics)
    }

    pub fn type_check_at(&self, path: &Path, position: usize) -> Option<TypeCheckedSource> {
        let target = self.source_target(path)?;
        let file = self.source_files.get(path)?;
        let checker = self.checker(&target.name)?;
        let path_string = path.to_string_lossy().into_owned();
        let source = Source::parse(&path_string, &file.content, checker.builder.steep_version)
            .ok()?
            .without_unrelated_defs(position);
        let typing = panic::catch_unwind(AssertUnwindSafe(|| {
            type_check(&checker, &source, Some(position))
        }))
        .ok()?;
        Some(TypeCheckedSource {
            target_name: target.name.clone(),
            checker,
            source,
            typing,
        })
    }
}

impl TypeCheckService {
    pub fn inline_type_name_at(
        &self,
        path: &Path,
        position: usize,
    ) -> Option<(TypeName, SourceRange)> {
        let target = self.project.target_for_inline_source_path(path)?;
        let file = self.signature_services.get(&target.name)?.files.get(path)?;
        let checker = self.checker(&target.name)?;
        locator::inline_type_name_at(
            &path.to_string_lossy(),
            file.content(),
            position,
            &checker.type_name_resolver,
        )
    }
}
