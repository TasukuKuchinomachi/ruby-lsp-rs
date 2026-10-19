use std::path::Path;

use crate::environment::{Environment, Location, MethodKind};
use crate::signature_validation::FileValidator;
use crate::types::{NameKind, TypeName};

/// What a name written in a signature file stands for
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignatureTarget {
    TypeReference {
        kind: NameKind,
        name: TypeName,
    },
    ClassDeclaration(TypeName),
    ConstantDeclaration(TypeName),
    Method {
        owner: TypeName,
        kind: MethodKind,
        name: String,
    },
}

impl Environment {
    /// The source is parsed again rather than read from the environment, since the editor may hold unsaved changes.
    /// `def self?.name` gives both the instance and the singleton method
    pub fn signature_targets_at(
        &self,
        path: &Path,
        source: &str,
        covers: impl Fn(&Location) -> bool,
    ) -> Vec<SignatureTarget> {
        let Some(validation) = crate::loader::parse_signature_validation(path, source) else {
            return Vec::new();
        };
        let innermost_reference = validation
            .references
            .iter()
            .filter(|reference| covers(&reference.location))
            .max_by_key(|reference| {
                (
                    reference.location.start.line,
                    reference.location.start.byte_column,
                )
            });
        if let Some(reference) = innermost_reference {
            let validator = FileValidator {
                environment: self,
                uses: &validation.uses,
            };
            return validator
                .resolve_reference(reference)
                .map(|(kind, name)| SignatureTarget::TypeReference { kind, name })
                .into_iter()
                .collect();
        }
        validation
            .defined_names
            .into_iter()
            .filter(|defined_name| covers(&defined_name.location))
            .map(|defined_name| defined_name.target)
            .collect()
    }
}
