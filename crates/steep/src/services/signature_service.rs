use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};

use rbs_definition::{
    ancestor_builder::{AncestorBuilder, OneAncestorsCache},
    ast::TypeName,
    collections::IndexMap,
    definition_builder::DefinitionBuilder,
    environment::{Environment, Source},
    errors::RbsError,
    inline_parser,
    loader::RbsGem,
    location::Buffer,
    parallel::map_in_parallel,
    parse::{SignatureError, parse_signature_buffer},
    rbs_version::RbsVersion,
};
use steep_interface::steep_version::SteepVersion;

use crate::{
    environment_loader,
    index::rbs_index::RbsIndex,
    inline_source,
    project::{Project, Target, UniquePaths},
    services::content_change::ContentChange,
    signature::{
        diagnostic::{Diagnostic, from_rbs_error},
        validator::SignatureValidation,
    },
    type_construction::{Checker, CheckerTables},
};

pub enum SignatureStatus {
    SyntaxError(Vec<Diagnostic>),
    AncestorError(Vec<Diagnostic>),
    Loaded,
}

pub struct LoadedSignatures {
    pub environment: Arc<Environment>,
    pub definition_builder: Arc<DefinitionBuilder>,
    pub implicitly_returns_nil: bool,
    pub steep_version: SteepVersion,
    checker_tables: OnceLock<CheckerTables>,
    signature_validation: OnceLock<SignatureValidation>,
    rbs_index: OnceLock<RbsIndex>,
}

impl LoadedSignatures {
    fn new(
        environment: Arc<Environment>,
        one_ancestors_cache: OneAncestorsCache,
        implicitly_returns_nil: bool,
        steep_version: SteepVersion,
    ) -> LoadedSignatures {
        let definition_builder = Arc::new(DefinitionBuilder::with_ancestor_builder(
            AncestorBuilder::with_one_ancestors_cache(environment.clone(), one_ancestors_cache),
        ));
        LoadedSignatures {
            environment,
            definition_builder,
            implicitly_returns_nil,
            steep_version,
            checker_tables: OnceLock::new(),
            signature_validation: OnceLock::new(),
            rbs_index: OnceLock::new(),
        }
    }

    pub fn checker_tables(&self) -> &CheckerTables {
        self.checker_tables
            .get_or_init(|| CheckerTables::new(self.definition_builder.clone()))
    }

    pub fn checker(&self) -> Checker {
        Checker::with_tables(
            self.definition_builder.clone(),
            self.checker_tables(),
            self.implicitly_returns_nil,
            self.steep_version,
        )
    }

    pub fn signature_validation(&self) -> &SignatureValidation {
        self.signature_validation
            .get_or_init(|| SignatureValidation::new(&self.environment))
    }

    pub fn rbs_index(&self) -> &RbsIndex {
        self.rbs_index
            .get_or_init(|| RbsIndex::build(&self.environment))
    }
}

pub enum SignatureFile {
    Rbs {
        content: String,
        source: Result<Source, Diagnostic>,
    },
    Ruby {
        content: String,
        source: Source,
    },
}

impl SignatureFile {
    pub fn content(&self) -> &str {
        match self {
            SignatureFile::Rbs { content, .. } | SignatureFile::Ruby { content, .. } => content,
        }
    }

    fn load(path: &Path, absolute_path: &Path, content: String) -> Result<SignatureFile, String> {
        let name = path.to_string_lossy().into_owned();
        if path.extension().is_some_and(|extension| extension == "rbs") {
            let source = match parse_file_content(absolute_path, name, content.clone())? {
                Parsed::Source(source) => Ok(source),
                Parsed::Error(diagnostic) => Err(diagnostic),
            };
            Ok(SignatureFile::Rbs { content, source })
        } else {
            let buffer = Buffer::new(name, content.clone());
            Ok(SignatureFile::Ruby {
                content,
                source: Source {
                    path: absolute_path.to_path_buf(),
                    directives: Vec::new(),
                    declarations: inline_parser::parse(&buffer).declarations,
                },
            })
        }
    }
}

struct Library {
    rbs_version: RbsVersion,
    sources: Vec<Source>,
    error: Option<Diagnostic>,
}

pub struct SignatureService {
    pub status: SignatureStatus,
    pub files: IndexMap<PathBuf, SignatureFile>,
    pub project_paths: Vec<String>,
    library: Arc<Library>,
    current: Option<Arc<LoadedSignatures>>,
    latest: Option<Arc<LoadedSignatures>>,
    library_signatures: OnceLock<Option<Arc<LoadedSignatures>>>,
    implicitly_returns_nil: bool,
    steep_version: SteepVersion,
    jobs: usize,
    // Why not drop it once the names are resolved: freeing it takes a noticeable time
    // before the check, so it is freed with the service after the check.
    _unresolved_environment: Option<Environment>,
}

enum Parsed {
    Source(Source),
    Error(Diagnostic),
}

fn parse_file_content(path: &Path, name: String, content: String) -> Result<Parsed, String> {
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

fn parse_file(path: &Path, name: String) -> Result<Parsed, String> {
    let content =
        fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    parse_file_content(path, name, content)
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

pub fn signature_target_names(project: &Project, path: &Path) -> Vec<String> {
    let owner = project
        .target_for_signature_path(path)
        .or_else(|| project.target_for_inline_source_path(path));
    let Some(owner) = owner else {
        return Vec::new();
    };
    project
        .targets
        .iter()
        .filter(|target| target.name == owner.name || !owner.unreferenced)
        .map(|target| target.name.clone())
        .collect()
}

struct Built {
    status: SignatureStatus,
    loaded: Option<Arc<LoadedSignatures>>,
    unresolved_environment: Option<Environment>,
}

impl SignatureService {
    pub fn load(
        rbs_gem: &RbsGem,
        project: &Project,
        target: &Target,
        steep_version: SteepVersion,
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
        let mut files = IndexMap::default();
        let loaded = map_in_parallel(&paths, jobs, |path| {
            let absolute_path = project.absolute_path(path);
            let content = fs::read_to_string(&absolute_path)
                .map_err(|error| format!("{}: {error}", absolute_path.display()))?;
            SignatureFile::load(path, &absolute_path, content)
        });
        for (path, file) in paths.iter().zip(loaded) {
            files.insert(path.clone(), file?);
        }
        let loaded = map_in_parallel(&inline_paths, jobs, |path| {
            let absolute_path = project.absolute_path(path);
            let content = fs::read_to_string(&absolute_path)
                .map_err(|error| format!("{}: {error}", absolute_path.display()))?;
            SignatureFile::load(path, &absolute_path, content)
        });
        for (path, file) in inline_paths.iter().zip(loaded) {
            files.insert(path.clone(), file?);
        }

        let mut service = SignatureService {
            status: SignatureStatus::Loaded,
            files,
            project_paths: Vec::new(),
            library: Arc::new(Library {
                rbs_version: rbs_gem.version,
                sources: library_sources,
                error: library_error,
            }),
            current: None,
            latest: None,
            library_signatures: OnceLock::new(),
            implicitly_returns_nil: target.implicitly_returns_nil,
            steep_version,
            jobs,
            _unresolved_environment: None,
        };
        service.rebuild()?;
        Ok(service)
    }

    pub fn update(
        &mut self,
        project: &Project,
        changes: &[(PathBuf, Vec<ContentChange>)],
    ) -> Result<(), String> {
        for (path, changes) in changes {
            let old_content = self
                .files
                .get(path)
                .map(|file| file.content().to_string())
                .unwrap_or_default();
            let content = changes
                .iter()
                .fold(old_content, |text, change| change.apply_to(&text));
            let file = SignatureFile::load(path, &project.absolute_path(path), content)?;
            self.files.insert(path.clone(), file);
        }
        self.rebuild()
    }

    fn rebuild(&mut self) -> Result<(), String> {
        self.project_paths = self
            .files
            .keys()
            .map(|path| path.to_string_lossy().into_owned())
            .collect();
        let built = self.build()?;
        self.status = built.status;
        self.current = built.loaded.clone();
        self._unresolved_environment = built.unresolved_environment;
        if let Some(loaded) = built.loaded {
            self.latest = Some(loaded);
        }
        Ok(())
    }

    fn build(&self) -> Result<Built, String> {
        let failure = |status| Built {
            status,
            loaded: None,
            unresolved_environment: None,
        };
        let mut environment = Environment::new(self.library.rbs_version);
        for source in &self.library.sources {
            environment.add_source(source.clone())?;
        }

        if self.files.is_empty() {
            return Ok(match &self.library.error {
                Some(diagnostic) => failure(SignatureStatus::SyntaxError(vec![diagnostic.clone()])),
                None => {
                    let environment = Arc::new(environment.resolve_type_names(self.jobs)?);
                    Built {
                        status: SignatureStatus::Loaded,
                        loaded: Some(Arc::new(LoadedSignatures::new(
                            environment,
                            OneAncestorsCache::default(),
                            self.implicitly_returns_nil,
                            self.steep_version,
                        ))),
                        unresolved_environment: None,
                    }
                }
            });
        }

        let syntax_errors: Vec<Diagnostic> = self
            .files
            .values()
            .filter_map(|file| match file {
                SignatureFile::Rbs {
                    source: Err(diagnostic),
                    ..
                } => Some(diagnostic.clone()),
                _ => None,
            })
            .collect();
        if !syntax_errors.is_empty() {
            return Ok(failure(SignatureStatus::SyntaxError(syntax_errors)));
        }

        let mut project_sources: Vec<Source> = Vec::new();
        for file in self.files.values() {
            if let SignatureFile::Rbs {
                source: Ok(source), ..
            } = file
            {
                project_sources.push(source.clone());
            }
        }
        for file in self.files.values() {
            if let SignatureFile::Ruby { source, .. } = file {
                project_sources.push(source.clone());
            }
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
            return Ok(failure(SignatureStatus::AncestorError(diagnostics(
                errors,
            )?)));
        }

        let unresolved_environment = environment;
        let environment = Arc::new(unresolved_environment.resolve_type_names(self.jobs)?);
        let builder = AncestorBuilder::new(environment.clone());
        let class_names: Vec<&TypeName> = environment.class_declarations.keys().collect();
        let mut errors: Vec<RbsError> = map_in_parallel(&class_names, self.jobs, |type_name| {
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
            return Ok(failure(SignatureStatus::AncestorError(diagnostics(
                errors,
            )?)));
        }
        Ok(Built {
            status: SignatureStatus::Loaded,
            loaded: Some(Arc::new(LoadedSignatures::new(
                environment,
                one_ancestors_cache,
                self.implicitly_returns_nil,
                self.steep_version,
            ))),
            unresolved_environment: Some(unresolved_environment),
        })
    }

    pub fn current_signatures(&self) -> Option<&Arc<LoadedSignatures>> {
        self.current.as_ref()
    }

    // Why not build the signatures of the libraries when loading: they are used only
    // when the signatures of the project have never loaded, as Steep keeps the last
    // definition builder in its error statuses.
    pub fn latest_signatures(&self) -> Option<&Arc<LoadedSignatures>> {
        self.latest.as_ref().or_else(|| {
            self.library_signatures
                .get_or_init(|| {
                    let environment = Environment::from_sources(
                        self.library.rbs_version,
                        self.library.sources.clone(),
                    )
                    .and_then(|environment| environment.resolve_type_names(self.jobs))
                    .ok()?;
                    Some(Arc::new(LoadedSignatures::new(
                        Arc::new(environment),
                        OneAncestorsCache::default(),
                        self.implicitly_returns_nil,
                        self.steep_version,
                    )))
                })
                .as_ref()
        })
    }

    pub fn environment(&self) -> Option<&Arc<Environment>> {
        self.current.as_ref().map(|loaded| &loaded.environment)
    }

    pub fn latest_environment(&self) -> Option<&Arc<Environment>> {
        self.latest_signatures().map(|loaded| &loaded.environment)
    }

    pub fn env_rbs_paths(&self) -> Vec<PathBuf> {
        let mut paths = UniquePaths::default();
        for source in self
            .latest_environment()
            .map(|environment| environment.sources.as_slice())
            .unwrap_or_default()
        {
            paths.push(source.path.clone());
        }
        paths.into_paths()
    }

    pub fn library_errors(&self) -> Vec<&Diagnostic> {
        let diagnostics = match &self.status {
            SignatureStatus::SyntaxError(diagnostics)
            | SignatureStatus::AncestorError(diagnostics) => diagnostics,
            SignatureStatus::Loaded => return Vec::new(),
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
            SignatureStatus::Loaded => Vec::new(),
        }
    }
}
