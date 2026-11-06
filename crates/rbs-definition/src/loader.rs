use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::{
    environment::{Environment, Source},
    environment_loader::EnvironmentLoader,
    parse::parse_signature,
    rbs_version::{RbsVersion, release_of_gem_directory},
    repository::Repository,
    rubygems::{Specification, Specifications, Version},
};

pub struct RbsGem {
    pub directory: PathBuf,
    pub release: Option<String>,
    pub version: RbsVersion,
    pub specifications: Arc<Specifications>,
    pub default_repository: Arc<Repository>,
}

impl RbsGem {
    pub fn new(
        directory: &Path,
        other_gem_paths: &[PathBuf],
        requested_release: Option<&str>,
    ) -> Result<RbsGem, String> {
        let mut gem_paths: Vec<PathBuf> = directory
            .parent()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .into_iter()
            .collect();
        for gem_path in other_gem_paths {
            if !gem_paths.contains(gem_path) {
                gem_paths.push(gem_path.clone());
            }
        }
        let release =
            release_of_gem_directory(directory).or_else(|| requested_release.map(str::to_string));
        let version = match &release {
            Some(release) => RbsVersion::of_release(release).ok_or_else(|| {
                format!(
                    "{} is rbs {release}, but only rbs {} are supported",
                    directory.display(),
                    RbsVersion::supported_names()
                )
            })?,
            None => RbsVersion::default(),
        };
        let activated = directory
            .file_name()
            .and_then(|name| name.to_str()?.strip_prefix("rbs-"))
            .and_then(Version::new)
            .map(|version| Specification {
                name: "rbs".to_string(),
                version,
                gem_directory: directory.to_path_buf(),
            });
        Ok(RbsGem {
            directory: directory.to_path_buf(),
            release,
            version,
            specifications: Arc::new(Specifications::new(
                &gem_paths,
                activated.into_iter().collect(),
            )),
            default_repository: Arc::new(Repository::new(Some(&directory.join("stdlib")))?),
        })
    }

    pub fn core_root(&self) -> PathBuf {
        self.directory.join("core")
    }

    pub fn environment_loader(
        &self,
        core_root: Option<PathBuf>,
        repository: Repository,
    ) -> EnvironmentLoader {
        EnvironmentLoader::new(
            core_root,
            repository,
            Arc::clone(&self.specifications),
            Arc::clone(&self.default_repository),
        )
    }

    pub fn core_signature_files(&self) -> Result<Vec<PathBuf>, String> {
        self.environment_loader(Some(self.core_root()), (*self.default_repository).clone())
            .signature_files()
            .map_err(|error| error.to_string())
    }
}

pub fn load(rbs_gem_directory: &Path, directories: &[PathBuf]) -> Result<Environment, String> {
    let rbs_gem = RbsGem::new(rbs_gem_directory, &[], None)?;
    let mut loader = rbs_gem.environment_loader(
        Some(rbs_gem.core_root()),
        (*rbs_gem.default_repository).clone(),
    );
    for directory in directories {
        loader.add_path(directory.clone());
    }
    let files = loader
        .signature_files()
        .map_err(|error| error.to_string())?;
    let sources = files
        .into_iter()
        .map(|path| {
            let content = fs::read_to_string(&path)
                .map_err(|error| format!("{}: {error}", path.display()))?;
            let signature = parse_signature(&content)
                .map_err(|error| format!("{}: {error}", path.display()))?;
            Ok(Source {
                path,
                directives: signature.directives,
                declarations: signature.declarations,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Environment::from_sources(rbs_gem.version, sources)?.resolve_type_names(1)
}
