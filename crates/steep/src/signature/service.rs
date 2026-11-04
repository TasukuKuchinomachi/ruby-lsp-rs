use std::{
    fs,
    path::{Path, PathBuf},
};

use rbs_definition::{
    ancestor_builder::{AncestorBuilder, OneAncestorsCache},
    ast::TypeName,
    environment::{Environment, Source},
    errors::RbsError,
    inline_parser,
    loader::RbsGem,
    location::Buffer,
    parallel::map_in_parallel,
    parse::{SignatureError, parse_signature_buffer},
};

use crate::{
    environment_loader, inline_source,
    project::{Project, Target, UniquePaths},
    signature::diagnostic::{Diagnostic, from_rbs_error},
};

pub enum SignatureStatus {
    SyntaxError(Vec<Diagnostic>),
    AncestorError(Vec<Diagnostic>),
    Loaded(Box<Environment>),
}

pub struct SignatureService {
    pub status: SignatureStatus,
    pub project_paths: Vec<String>,
    one_ancestors_cache: Option<OneAncestorsCache>,
    // Why not drop it once the names are resolved: freeing it takes a noticeable time
    // before the check, so it is freed with the service after the check.
    _unresolved_environment: Option<Environment>,
}

enum Parsed {
    Source(Source),
    Error(Diagnostic),
}

fn parse_file(path: &Path, name: String) -> Result<Parsed, String> {
    let content =
        fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let buffer = Buffer::new(name, content);
    match parse_signature_buffer(&buffer) {
        Ok(signature) => Ok(Parsed::Source(Source {
            path: path.to_path_buf(),
            directives: signature.directives,
            declarations: signature.declarations,
        })),
        Err(SignatureError::Syntax(error)) => Ok(Parsed::Error(from_rbs_error(
            &RbsError::Parsing(Box::new(error)),
            None,
        )?)),
        Err(SignatureError::Unsupported(message)) => Err(format!("{}: {message}", path.display())),
    }
}

fn parse_inline_file(path: &Path, name: String) -> Result<Source, String> {
    let content =
        fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let buffer = Buffer::new(name, content);
    Ok(Source {
        path: path.to_path_buf(),
        directives: Vec::new(),
        declarations: inline_parser::parse(&buffer).declarations,
    })
}

fn library_sources(
    files: &[PathBuf],
    jobs: usize,
) -> Result<Result<Vec<Source>, Diagnostic>, String> {
    let mut sources = Vec::new();
    for parsed in map_in_parallel(files, jobs, |path| {
        parse_file(path, path.to_string_lossy().into_owned())
    }) {
        match parsed? {
            Parsed::Source(source) => sources.push(source),
            Parsed::Error(diagnostic) => return Ok(Err(diagnostic)),
        }
    }
    Ok(Ok(sources))
}

fn diagnostics(errors: Vec<RbsError>) -> Result<Vec<Diagnostic>, String> {
    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    for error in errors {
        let diagnostic = from_rbs_error(&error, None)?;
        if !diagnostics
            .iter()
            .any(|existing| existing.same_as(&diagnostic))
        {
            diagnostics.push(diagnostic);
        }
    }
    Ok(diagnostics)
}

pub fn signature_paths_for(project: &Project, target: &Target) -> Vec<PathBuf> {
    let mut paths = UniquePaths::default();
    for other in &project.targets {
        if other.name != target.name && other.unreferenced {
            continue;
        }
        for path in project.source_paths(&other.signature_pattern) {
            paths.push(path);
        }
        for group in &other.groups {
            for path in project.source_paths(&group.signature_pattern) {
                paths.push(path);
            }
        }
    }
    paths.into_paths()
}

impl SignatureService {
    pub fn load(
        rbs_gem: &RbsGem,
        project: &Project,
        target: &Target,
        jobs: usize,
    ) -> Result<SignatureService, String> {
        let library_files =
            environment_loader::library_files(rbs_gem, project, project.options(target))?;
        let (library_sources, library_error) = match library_sources(&library_files, jobs)? {
            Ok(sources) => (sources, None),
            Err(diagnostic) => {
                let core = library_sources(&rbs_gem.core_signature_files()?, jobs)?
                    .map_err(|diagnostic| diagnostic.full_message())?;
                (core, Some(diagnostic))
            }
        };

        let paths = signature_paths_for(project, target);
        let inline_paths = inline_source::signature_paths_for(project, target);
        let project_paths: Vec<String> = paths
            .iter()
            .chain(&inline_paths)
            .map(|path| path.to_string_lossy().into_owned())
            .collect();
        let service = |status| SignatureService {
            status,
            project_paths: project_paths.clone(),
            one_ancestors_cache: None,
            _unresolved_environment: None,
        };

        let mut environment = Environment::default();
        for source in library_sources {
            environment.add_source(source)?;
        }

        if paths.is_empty() && inline_paths.is_empty() {
            return Ok(service(match library_error {
                Some(diagnostic) => SignatureStatus::SyntaxError(vec![diagnostic]),
                None => SignatureStatus::Loaded(Box::new(environment.resolve_type_names(jobs)?)),
            }));
        }

        let mut project_sources = Vec::new();
        let mut syntax_errors = Vec::new();
        for parsed in map_in_parallel(&paths, jobs, |path| {
            parse_file(
                &project.absolute_path(path),
                path.to_string_lossy().into_owned(),
            )
        }) {
            match parsed? {
                Parsed::Source(source) => project_sources.push(source),
                Parsed::Error(diagnostic) => syntax_errors.push(diagnostic),
            }
        }
        if !syntax_errors.is_empty() {
            return Ok(service(SignatureStatus::SyntaxError(syntax_errors)));
        }
        for source in map_in_parallel(&inline_paths, jobs, |path| {
            parse_inline_file(
                &project.absolute_path(path),
                path.to_string_lossy().into_owned(),
            )
        }) {
            project_sources.push(source?);
        }

        let mut errors = Vec::new();
        for source in project_sources {
            if let Err(error) = environment.add_source(source) {
                if !error.is_loading_error() {
                    return Err(error.to_string());
                }
                errors.push(error);
            }
        }
        if let Err(error) = environment.validate_type_params() {
            errors.push(error);
        }
        if !errors.is_empty() {
            return Ok(service(SignatureStatus::AncestorError(diagnostics(
                errors,
            )?)));
        }

        let unresolved_environment = environment;
        let environment = unresolved_environment.resolve_type_names(jobs)?;
        let builder = AncestorBuilder::new(&environment);
        let class_names: Vec<&TypeName> = environment.class_declarations.keys().collect();
        let mut errors: Vec<RbsError> = map_in_parallel(&class_names, jobs, |type_name| {
            [
                builder.one_instance_ancestors(type_name).err(),
                builder.one_singleton_ancestors(type_name).err(),
            ]
        })
        .into_iter()
        .flatten()
        .flatten()
        .collect();
        for type_name in environment.interface_declarations.keys() {
            if let Err(error) = builder.one_interface_ancestors(type_name) {
                errors.push(error);
            }
        }
        drop(class_names);
        let one_ancestors_cache = builder.into_one_ancestors_cache();
        if !errors.is_empty() {
            return Ok(service(SignatureStatus::AncestorError(diagnostics(
                errors,
            )?)));
        }
        Ok(SignatureService {
            one_ancestors_cache: Some(one_ancestors_cache),
            _unresolved_environment: Some(unresolved_environment),
            ..service(SignatureStatus::Loaded(Box::new(environment)))
        })
    }

    pub fn take_one_ancestors_cache(&mut self) -> OneAncestorsCache {
        self.one_ancestors_cache.take().unwrap_or_default()
    }

    pub fn environment(&self) -> Option<&Environment> {
        match &self.status {
            SignatureStatus::Loaded(environment) => Some(environment),
            SignatureStatus::SyntaxError(_) | SignatureStatus::AncestorError(_) => None,
        }
    }

    pub fn library_errors(&self) -> Vec<&Diagnostic> {
        let diagnostics = match &self.status {
            SignatureStatus::SyntaxError(diagnostics)
            | SignatureStatus::AncestorError(diagnostics) => diagnostics,
            SignatureStatus::Loaded(_) => return Vec::new(),
        };
        diagnostics
            .iter()
            .filter(|diagnostic| {
                diagnostic
                    .path()
                    .is_some_and(|path| !self.project_paths.iter().any(|project| project == path))
            })
            .collect()
    }

    pub fn error_diagnostics_for(&self, path: &str) -> Vec<Diagnostic> {
        match &self.status {
            SignatureStatus::SyntaxError(diagnostics) => diagnostics
                .iter()
                .filter(|diagnostic| {
                    diagnostic.path() == Some(path) && diagnostic.is_syntax_error()
                })
                .cloned()
                .collect(),
            SignatureStatus::AncestorError(diagnostics) => diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.path() == Some(path))
                .cloned()
                .collect(),
            SignatureStatus::Loaded(_) => Vec::new(),
        }
    }
}
