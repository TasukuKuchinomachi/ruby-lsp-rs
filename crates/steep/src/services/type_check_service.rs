use std::{
    fs,
    panic::{self, AssertUnwindSafe},
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
};

use rbs_definition::{
    ancestor_builder::AncestorBuilder, ast::TypeName, collections::HashSet,
    definition_builder::DefinitionBuilder, parallel::run_in_parallel,
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
    inline_source,
    project::{Project, Target, UniquePaths},
    ruby::node::{Buffer, SourceRange},
    ruby_exception::{self, RubyException},
    signature::{
        diagnostic::Diagnostic as SignatureDiagnostic,
        service::SignatureService,
        validator::{SignatureValidation, ValidatorCache},
    },
    source::{Source, SourceError},
    strict::{EnabledChecks, Observation, StrictCheck},
    type_construction::{Checker, CheckerTables, TypeConstruction, instance_type},
    type_inference::{
        constant_env::ConstantEnv,
        context::{CallContext, Context, ModuleContext, TypeVariableContext},
        type_env::TypeEnv,
    },
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

fn type_check(checker: &Checker, source: &Source) -> RubyDiagnostics {
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
    let typing = Typing::new();
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
    let mut observations = source.observations.clone();
    observations.extend(typing.observations());
    RubyDiagnostics {
        errors: typing.errors(),
        observations,
    }
}

pub(crate) struct RubyDiagnostics {
    pub errors: Vec<Diagnostic>,
    pub observations: Vec<Observation>,
}

impl RubyDiagnostics {
    fn errors_only(errors: Vec<Diagnostic>) -> RubyDiagnostics {
        RubyDiagnostics {
            errors,
            observations: Vec::new(),
        }
    }
}

pub(crate) fn ruby_diagnostics(
    checker: &Checker,
    path: &str,
    text: &str,
) -> (Buffer, Result<RubyDiagnostics, String>) {
    reset_fresh_variable_count();
    match Source::parse(path, text, checker.builder.steep_version) {
        Ok(source) => {
            let result = panic::catch_unwind(AssertUnwindSafe(|| type_check(checker, &source)));
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
) -> (Buffer, Result<Vec<expectations::Diagnostic>, String>) {
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
    (buffer, Ok(diagnostics))
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
        let mut service = match SignatureService::load(&rbs_gem, project, target, jobs) {
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
        let mut source_paths: Vec<PathBuf> = Vec::new();
        for group in &target.groups {
            source_paths.extend(project.source_paths(&group.source_pattern));
        }
        source_paths.extend(project.source_paths(&target.source_pattern));
        let mut seen = HashSet::default();
        for path in source_paths {
            if seen.contains(&path) || !target.is_possible_source_file(&path) {
                continue;
            }
            if project
                .target_for_source_path(&path)
                .map(|found| &found.name)
                != Some(&target.name)
            {
                continue;
            }
            seen.insert(path.clone());
            works.push(Work::Source(path));
        }
        works.extend(
            inline_source::target_inline_paths(project, target)
                .into_iter()
                .map(Work::Inline),
        );
        let one_ancestors_cache = service.take_one_ancestors_cache();
        let library_errors = service.library_errors();
        let environment = service.environment();
        let definition_builder = environment.map(|environment| {
            Arc::new(DefinitionBuilder::with_ancestor_builder(
                AncestorBuilder::with_one_ancestors_cache(environment.clone(), one_ancestors_cache),
            ))
        });
        let signature_validation =
            environment.map(|environment| SignatureValidation::new(environment));
        let checker_tables = definition_builder.clone().map(CheckerTables::new);
        let new_worker = || Worker {
            checker: definition_builder.clone().zip(checker_tables.as_ref()).map(
                |(definition_builder, tables)| {
                    Checker::with_tables(
                        definition_builder,
                        tables,
                        target.implicitly_returns_nil,
                        steep_version,
                    )
                },
            ),
            validator_cache: Rc::default(),
        };
        let check_source =
            |worker: &Worker, path: &PathBuf, inline: bool| -> Result<WorkResult, String> {
                let text = fs::read_to_string(project.absolute_path(path))
                    .map_err(|error| format!("{}: {error}", path.display()))?;
                Ok(
                    match source_diagnostics(
                        target,
                        path,
                        &text,
                        worker.checker.as_ref(),
                        &library_errors,
                        enabled_checks,
                    ) {
                        (buffer, Ok(mut diagnostics)) => {
                            if inline {
                                diagnostics.extend(inline_source::inline_diagnostics(&buffer));
                            }
                            WorkResult::File(FileDiagnostics {
                                path: path.clone(),
                                diagnostics,
                            })
                        }
                        (_, Err(message)) => WorkResult::Failure(format!(
                            "type checking {} failed: {message}",
                            path.display()
                        )),
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
                Work::Signature(path) => {
                    let path_string = path.to_string_lossy().into_owned();
                    let signature_diagnostics = match (&worker.checker, &signature_validation) {
                        (Some(checker), Some(signature_validation)) => {
                            match ruby_exception::rescue(|| {
                                signature_validation.validate_path(
                                    &checker.builder,
                                    worker.validator_cache.clone(),
                                    &path_string,
                                )
                            }) {
                                Ok(diagnostics) => diagnostics,
                                Err(exception) => {
                                    return Ok(WorkResult::Failure(format!(
                                        "validating signatures in {} raised {exception}, which Steep does not rescue",
                                        path.display()
                                    )));
                                }
                            }
                        }
                        _ => service.error_diagnostics_for(&path_string),
                    };
                    Ok(WorkResult::File(FileDiagnostics {
                        path: path.clone(),
                        diagnostics: signature_diagnostics
                            .iter()
                            .filter_map(SignatureDiagnostic::to_expectation)
                            .collect(),
                    }))
                }
                Work::Source(path) => check_source(worker, path, false),
                Work::Inline(path) => check_source(worker, path, true),
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
            std::mem::forget(definition_builder);
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
