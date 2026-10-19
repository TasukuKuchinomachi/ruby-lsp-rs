use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ruby_rbs_sys::bindings::{
    rbs_encoding_t, rbs_encoding_type_t, rbs_encodings, rbs_parse_signature, rbs_parser_free,
    rbs_parser_new, rbs_string_new,
};

use crate::environment::{ClassKind, Environment, Location, MethodKind, SourcePosition};
use crate::loader::{UseClause, is_interface_name, parallel_map};
use crate::navigation::SignatureTarget;
use crate::types::{NameKind, TypeName, TypeParameter};

pub const SYNTAX_ERROR: &str = "RBS::SyntaxError";
pub const DUPLICATED_DECLARATION: &str = "RBS::DuplicatedDeclaration";
pub const UNKNOWN_TYPE_NAME: &str = "RBS::UnknownTypeName";
pub const INVALID_TYPE_APPLICATION: &str = "RBS::InvalidTypeApplication";
pub const INVALID_METHOD_OVERLOAD: &str = "RBS::InvalidMethodOverload";
pub const UNKNOWN_METHOD_ALIAS: &str = "RBS::UnknownMethodAlias";
pub const DUPLICATED_METHOD_DEFINITION: &str = "RBS::DuplicatedMethodDefinition";
pub const SUPERCLASS_MISMATCH: &str = "RBS::SuperclassMismatch";
pub const GENERIC_PARAMETER_MISMATCH: &str = "RBS::GenericParameterMismatch";
pub const MIXIN_CLASS_ERROR: &str = "RBS::MixinClassError";
pub const INHERIT_MODULE_ERROR: &str = "RBS::InheritModuleError";

/// A problem in a signature file of the project, reported as an error the way Steep reports `RBS::*` diagnostics.
/// Only files added with `Loader::add_signature_directory` or `Loader::add_signature_source` are validated
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureDiagnostic {
    pub path: PathBuf,
    pub start: SourcePosition,
    pub end: SourcePosition,
    pub code: &'static str,
    pub message: String,
}

impl SignatureDiagnostic {
    pub(crate) fn new(
        location: &Location,
        code: &'static str,
        message: String,
    ) -> SignatureDiagnostic {
        SignatureDiagnostic {
            path: location.path.as_ref().clone(),
            start: location.start,
            end: location.end,
            code,
            message,
        }
    }
}

impl Environment {
    pub fn signature_diagnostics_for<'a>(
        &'a self,
        path: &'a Path,
    ) -> impl Iterator<Item = &'a SignatureDiagnostic> + 'a {
        self.signature_diagnostics
            .iter()
            .filter(move |diagnostic| diagnostic.path == path)
    }
}

/// The syntax error of `source`, if any, located at the token the parser stopped at
pub fn signature_syntax_error(path: &Path, source: &str) -> Option<SignatureDiagnostic> {
    let message = ruby_rbs::node::parse(source).err()?;
    Some(syntax_error_diagnostic(path, source, &message))
}

pub(crate) fn syntax_error_diagnostic(
    path: &Path,
    source: &str,
    message: &str,
) -> SignatureDiagnostic {
    let (start_offset, end_offset) = syntax_error_token_range(source).unwrap_or((0, 0));
    let token = source.get(start_offset..end_offset).unwrap_or_default();
    SignatureDiagnostic {
        path: path.to_path_buf(),
        start: source_position(source, start_offset),
        end: source_position(source, end_offset),
        code: SYNTAX_ERROR,
        message: format!("Syntax error: {message}, token=`{token}`"),
    }
}

/// `ruby_rbs::node::parse` reports only the message of a syntax error, so the parser is run again here to read the
/// token it stopped at
fn syntax_error_token_range(source: &str) -> Option<(usize, usize)> {
    let length = i32::try_from(source.len()).ok()?;
    unsafe {
        let start_pointer = source.as_ptr().cast::<std::os::raw::c_char>();
        let string = rbs_string_new(start_pointer, start_pointer.add(source.len()));
        let encoding = &rbs_encodings[rbs_encoding_type_t::RBS_ENCODING_UTF_8 as usize]
            as *const rbs_encoding_t;
        let parser = rbs_parser_new(string, encoding, 0, length);
        let mut signature = std::ptr::null_mut();
        let succeeded = rbs_parse_signature(parser, &mut signature);
        let range = if succeeded {
            None
        } else {
            (*parser).error.as_ref().map(|error| {
                let start = (error.token.range.start.byte_pos.max(0) as usize).min(source.len());
                let end =
                    (error.token.range.end.byte_pos.max(0) as usize).clamp(start, source.len());
                (start, end)
            })
        };
        rbs_parser_free(parser);
        range
    }
}

fn source_position(source: &str, byte_offset: usize) -> SourcePosition {
    let mut byte_offset = byte_offset.min(source.len());
    while !source.is_char_boundary(byte_offset) {
        byte_offset -= 1;
    }
    let prefix = &source[..byte_offset];
    let line_start = prefix.rfind('\n').map(|index| index + 1).unwrap_or(0);
    let line_prefix = &source[line_start..byte_offset];
    SourcePosition {
        line: prefix.matches('\n').count() as u32,
        byte_column: line_prefix.len() as u32,
        utf16_column: line_prefix.chars().map(char::len_utf16).sum::<usize>() as u32,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MixinKind {
    Include,
    Extend,
    Prepend,
}

impl MixinKind {
    fn keyword(self) -> &'static str {
        match self {
            MixinKind::Include => "include",
            MixinKind::Extend => "extend",
            MixinKind::Prepend => "prepend",
        }
    }
}

pub(crate) enum ReferenceKind {
    Type(NameKind),
    Singleton,
    /// Steep reports a missing superclass at the whole class declaration
    SuperClass {
        declaration_location: Location,
    },
    Mixin {
        kind: MixinKind,
        owner: TypeName,
    },
    ModuleSelfType,
    InterfaceInclude,
    ClassAliasTarget,
}

/// A type name written in a signature file, before name resolution
pub(crate) struct TypeReference {
    /// Fully qualified names of the enclosing declarations, innermost last
    pub(crate) context: Arc<[String]>,
    pub(crate) kind: ReferenceKind,
    pub(crate) written_name: String,
    pub(crate) argument_count: usize,
    pub(crate) location: Location,
}

pub(crate) struct DeclarationRecord {
    pub(crate) name: TypeName,
    pub(crate) location: Location,
}

pub(crate) enum AliasOwner {
    Class(MethodKind),
    Interface,
}

pub(crate) struct MethodAliasRecord {
    pub(crate) owner_name: TypeName,
    pub(crate) owner: AliasOwner,
    pub(crate) old_name: String,
    pub(crate) location: Location,
}

/// The declaration `Steep::Signature::Validator` validates under one `rescue_validation_errors`
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum ValidationOwner {
    Class(TypeName),
    Interface(TypeName),
    TypeAlias(TypeName),
    Constant(TypeName),
    Global(String),
}

/// A name a signature file defines, kept for jumping from the signature to its implementation
pub(crate) struct DefinedNameRecord {
    pub(crate) target: SignatureTarget,
    pub(crate) location: Location,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ValidationMember {
    Declaration,
    /// An instance method other than `initialize`, or an instance attribute, whose types
    /// `RBS::DefinitionBuilder#validate_type_params` checks before anything else in the class body
    InstanceMethod,
    Initializer,
    InstanceAlias,
    InstanceVariable,
    SingletonMember,
}

#[derive(Debug, Clone)]
pub(crate) struct ValidationScope {
    pub(crate) location: Location,
    pub(crate) owner: ValidationOwner,
    pub(crate) member: ValidationMember,
}

/// What a signature file needs checked once every file is merged into the environment
#[derive(Default)]
pub(crate) struct FileValidation {
    pub(crate) scopes: Vec<ValidationScope>,
    pub(crate) uses: Vec<UseClause>,
    pub(crate) references: Vec<TypeReference>,
    pub(crate) declarations: Vec<DeclarationRecord>,
    pub(crate) method_aliases: Vec<MethodAliasRecord>,
    pub(crate) defined_names: Vec<DefinedNameRecord>,
    pub(crate) diagnostics: Vec<SignatureDiagnostic>,
}

pub(crate) fn absolute_name(name: &str) -> String {
    format!("::{}", name.trim_start_matches("::"))
}

pub(crate) fn duplicated_declaration(location: &Location, name: &str) -> SignatureDiagnostic {
    let name = if name.starts_with('$') {
        name.to_string()
    } else {
        absolute_name(name)
    };
    SignatureDiagnostic::new(
        location,
        DUPLICATED_DECLARATION,
        format!("Declaration of `{name}` is duplicated"),
    )
}

/// One diagnostic for each definition in a signature file, because RBS reports the earliest definition only and that
/// one is often in a library, where Steep drops it
pub(crate) fn duplicated_method_definitions(
    signature_files: &HashSet<PathBuf>,
    owner: &str,
    method_name: &str,
    locations: [Option<&Location>; 2],
) -> Vec<SignatureDiagnostic> {
    locations
        .into_iter()
        .flatten()
        .filter(|location| signature_files.contains(location.path.as_ref()))
        .map(|location| {
            SignatureDiagnostic::new(
                location,
                DUPLICATED_METHOD_DEFINITION,
                format!(
                    "Non-overloading method definition of `{method_name}` in `{}` cannot be duplicated",
                    absolute_name(owner)
                ),
            )
        })
        .collect()
}

pub(crate) fn invalid_method_overload(
    location: &Location,
    owner: &str,
    method_name: &str,
) -> SignatureDiagnostic {
    SignatureDiagnostic::new(
        location,
        INVALID_METHOD_OVERLOAD,
        format!(
            "Cannot find a non-overloading definition of `{method_name}` in `{}`",
            absolute_name(owner)
        ),
    )
}

pub(crate) fn superclass_mismatch(location: &Location, name: &str) -> SignatureDiagnostic {
    SignatureDiagnostic::new(
        location,
        SUPERCLASS_MISMATCH,
        format!(
            "Different superclasses are specified for `{}`",
            absolute_name(name)
        ),
    )
}

pub(crate) fn generic_parameter_mismatch(location: &Location, name: &str) -> SignatureDiagnostic {
    SignatureDiagnostic::new(
        location,
        GENERIC_PARAMETER_MISMATCH,
        format!(
            "Different generic parameters are specified across definitions of `{}`",
            absolute_name(name)
        ),
    )
}

pub(crate) fn validate_files(
    environment: &Environment,
    validations: &[FileValidation],
) -> Vec<SignatureDiagnostic> {
    parallel_map(validations, |validation| {
        FileValidator {
            environment,
            uses: &validation.uses,
        }
        .validate(validation)
    })
    .into_iter()
    .flatten()
    .collect()
}

/// Sorted by file and position, without the same problem reported twice at one place
pub(crate) fn finish_diagnostics(diagnostics: &mut Vec<SignatureDiagnostic>) {
    diagnostics.sort_by(|left, right| {
        (
            &left.path,
            left.start.line,
            left.start.byte_column,
            left.code,
            &left.message,
        )
            .cmp(&(
                &right.path,
                right.start.line,
                right.start.byte_column,
                right.code,
                &right.message,
            ))
    });
    diagnostics.dedup_by(|left, right| {
        left.path == right.path
            && left.start == right.start
            && left.end == right.end
            && left.code == right.code
    });
}

fn interface_or_class(written_name: &str) -> NameKind {
    if is_interface_name(written_name) {
        NameKind::Interface
    } else {
        NameKind::Class
    }
}

pub(crate) struct FileValidator<'a> {
    pub(crate) environment: &'a Environment,
    pub(crate) uses: &'a [UseClause],
}

impl FileValidator<'_> {
    fn validate(&self, validation: &FileValidation) -> Vec<SignatureDiagnostic> {
        let mut diagnostics = validation.diagnostics.clone();
        for reference in &validation.references {
            self.validate_reference(reference, &mut diagnostics);
        }
        for declaration in &validation.declarations {
            self.validate_namespace(declaration, &mut diagnostics);
        }
        for method_alias in &validation.method_aliases {
            self.validate_method_alias(method_alias, &mut diagnostics);
        }
        diagnostics
    }

    fn validate_reference(
        &self,
        reference: &TypeReference,
        diagnostics: &mut Vec<SignatureDiagnostic>,
    ) {
        let unknown = |location: &Location| {
            SignatureDiagnostic::new(
                location,
                UNKNOWN_TYPE_NAME,
                format!("Cannot find type `{}`", reference.written_name),
            )
        };
        let interface_or_class = interface_or_class(&reference.written_name);
        match &reference.kind {
            ReferenceKind::Type(kind) => match self.resolve(*kind, reference) {
                Some(name) => self.check_arity(*kind, &name, reference, diagnostics),
                None => diagnostics.push(unknown(&reference.location)),
            },
            ReferenceKind::Singleton | ReferenceKind::ClassAliasTarget => {
                if self.resolve(NameKind::Class, reference).is_none() {
                    diagnostics.push(unknown(&reference.location));
                }
            }
            ReferenceKind::InterfaceInclude => {
                if self.resolve(NameKind::Interface, reference).is_none() {
                    diagnostics.push(unknown(&reference.location));
                }
            }
            ReferenceKind::ModuleSelfType => match self.resolve(interface_or_class, reference) {
                Some(name) => self.check_arity(interface_or_class, &name, reference, diagnostics),
                None => diagnostics.push(unknown(&reference.location)),
            },
            ReferenceKind::SuperClass {
                declaration_location,
            } => match self.resolve(NameKind::Class, reference) {
                None => diagnostics.push(unknown(declaration_location)),
                Some(name) if self.class_kind(&name) == Some(ClassKind::Module) => diagnostics
                    .push(SignatureDiagnostic::new(
                        &reference.location,
                        INHERIT_MODULE_ERROR,
                        format!("Cannot inherit from a module `{}`", absolute_name(&name)),
                    )),
                Some(name) => self.check_arity(NameKind::Class, &name, reference, diagnostics),
            },
            ReferenceKind::Mixin { kind, owner } => {
                match self.resolve(interface_or_class, reference) {
                    None => diagnostics.push(unknown(&reference.location)),
                    Some(name) if self.class_kind(&name) == Some(ClassKind::Class) => diagnostics
                        .push(SignatureDiagnostic::new(
                            &reference.location,
                            MIXIN_CLASS_ERROR,
                            format!(
                                "Cannot {} a class `{}` in the definition of `{}`",
                                kind.keyword(),
                                absolute_name(&name),
                                absolute_name(owner)
                            ),
                        )),
                    Some(_) => {}
                }
            }
        }
    }

    pub(crate) fn resolve_reference(
        &self,
        reference: &TypeReference,
    ) -> Option<(NameKind, TypeName)> {
        let kind = match &reference.kind {
            ReferenceKind::Type(kind) => *kind,
            ReferenceKind::Singleton
            | ReferenceKind::ClassAliasTarget
            | ReferenceKind::SuperClass { .. } => NameKind::Class,
            ReferenceKind::InterfaceInclude => NameKind::Interface,
            ReferenceKind::Mixin { .. } | ReferenceKind::ModuleSelfType => {
                interface_or_class(&reference.written_name)
            }
        };
        self.resolve(kind, reference).map(|name| (kind, name))
    }

    fn class_kind(&self, name: &str) -> Option<ClassKind> {
        let declared_name = self.environment.resolve_class_name(name)?;
        self.environment
            .classes
            .get(declared_name)
            .map(|declaration| declaration.kind)
    }

    fn type_parameters(&self, kind: NameKind, name: &str) -> Option<&[TypeParameter]> {
        match kind {
            NameKind::Class => {
                let declared_name = self.environment.resolve_class_name(name)?;
                self.environment
                    .classes
                    .get(declared_name)
                    .map(|declaration| declaration.type_parameters.as_slice())
            }
            NameKind::Interface => self
                .environment
                .interfaces
                .get(name)
                .map(|declaration| declaration.type_parameters.as_slice()),
            NameKind::Alias => self
                .environment
                .type_aliases
                .get(name)
                .map(|declaration| declaration.type_parameters.as_slice()),
        }
    }

    /// `RBS::InvalidTypeApplicationError.check!`: type parameters with a default may be left out
    fn check_arity(
        &self,
        kind: NameKind,
        name: &str,
        reference: &TypeReference,
        diagnostics: &mut Vec<SignatureDiagnostic>,
    ) {
        let Some(type_parameters) = self.type_parameters(kind, name) else {
            return;
        };
        let maximum = type_parameters.len();
        let minimum = type_parameters
            .iter()
            .filter(|type_parameter| type_parameter.default_type.is_none())
            .count();
        let given = reference.argument_count;
        if minimum <= given && given <= maximum {
            return;
        }
        let name = absolute_name(name);
        let message = if maximum == 0 {
            format!(
                "Type `{name}` is not generic but used as a generic type with {given} arguments"
            )
        } else if given == 0 {
            format!("Type `{name}` is generic but used as a non generic type")
        } else {
            format!("Type `{name}` expects {maximum} arguments, but {given} arguments are given")
        };
        diagnostics.push(SignatureDiagnostic::new(
            &reference.location,
            INVALID_TYPE_APPLICATION,
            message,
        ));
    }

    fn resolve(&self, kind: NameKind, reference: &TypeReference) -> Option<TypeName> {
        let written_name = reference.written_name.as_str();
        if let Some(absolute) = written_name.strip_prefix("::") {
            return self.normalize(kind, absolute);
        }
        for prefix in reference.context.iter().rev() {
            if let Some(name) = self.normalize(kind, &format!("{prefix}::{written_name}")) {
                return Some(name);
            }
        }
        if let Some(name) = self.normalize(kind, written_name) {
            return Some(name);
        }
        let (head, rest) = match written_name.split_once("::") {
            Some((head, rest)) => (head, Some(rest)),
            None => (written_name, None),
        };
        self.uses.iter().find_map(|clause| {
            let candidate = match clause {
                UseClause::Single { name, local_name } if local_name == head => match rest {
                    Some(rest) => format!("{name}::{rest}"),
                    None => name.clone(),
                },
                UseClause::Single { .. } => return None,
                UseClause::Wildcard { namespace } => format!("{namespace}::{written_name}"),
            };
            self.normalize(kind, &candidate)
        })
    }

    fn exists(&self, kind: NameKind, name: &str) -> bool {
        match kind {
            NameKind::Class => {
                self.environment.classes.contains_key(name)
                    || self.environment.class_aliases.contains_key(name)
            }
            NameKind::Interface => self.environment.interfaces.contains_key(name),
            NameKind::Alias => self.environment.type_aliases.contains_key(name),
        }
    }

    /// The declared name of `name`, following class aliases in its namespace as `RBS::Environment#normalize_type_name`
    fn normalize(&self, kind: NameKind, name: &str) -> Option<TypeName> {
        if self.exists(kind, name) {
            return Some(name.to_string());
        }
        let (namespace, last_segment) = name.rsplit_once("::")?;
        let namespace = self.normalize(NameKind::Class, namespace)?;
        let declared_namespace = self.environment.resolve_class_name(&namespace)?;
        if declared_namespace == namespace {
            return None;
        }
        let candidate = format!("{declared_namespace}::{last_segment}");
        self.exists(kind, &candidate).then_some(candidate)
    }

    /// `RBS::DefinitionBuilder#ensure_namespace!`
    fn validate_namespace(
        &self,
        declaration: &DeclarationRecord,
        diagnostics: &mut Vec<SignatureDiagnostic>,
    ) {
        let mut namespace = declaration.name.as_str();
        while let Some((outer, _)) = namespace.rsplit_once("::") {
            if self.normalize(NameKind::Class, outer).is_none() {
                diagnostics.push(SignatureDiagnostic::new(
                    &declaration.location,
                    UNKNOWN_TYPE_NAME,
                    format!("Cannot find type `{}`", absolute_name(outer)),
                ));
                return;
            }
            namespace = outer;
        }
    }

    fn validate_method_alias(
        &self,
        method_alias: &MethodAliasRecord,
        diagnostics: &mut Vec<SignatureDiagnostic>,
    ) {
        let environment = self.environment;
        let found = match method_alias.owner {
            AliasOwner::Class(MethodKind::Instance) => environment
                .find_instance_method(&method_alias.owner_name, &method_alias.old_name)
                .is_some(),
            AliasOwner::Class(MethodKind::Singleton) => environment
                .find_singleton_method(&method_alias.owner_name, &method_alias.old_name)
                .is_some(),
            AliasOwner::Interface => environment
                .find_interface_method(&method_alias.owner_name, &method_alias.old_name)
                .is_some(),
        };
        if !found {
            diagnostics.push(SignatureDiagnostic::new(
                &method_alias.location,
                UNKNOWN_METHOD_ALIAS,
                format!(
                    "Cannot find the original method `{}` in `{}`",
                    method_alias.old_name,
                    absolute_name(&method_alias.owner_name)
                ),
            ));
        }
    }
}
