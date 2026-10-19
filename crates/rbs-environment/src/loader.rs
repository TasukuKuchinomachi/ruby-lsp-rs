use std::cell::RefCell;
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use ruby_rbs::node::{
    AliasKind, AttrIvarName, AttributeKind, AttributeVisibility, BlockTypeNode, CommentNode,
    MethodDefinitionKind, MethodDefinitionNode, MethodDefinitionVisibility, NamespaceNode, Node,
    NodeList, RBSHash, RBSLocationRange, TypeNameNode, TypeParamVariance,
};

use crate::environment::{
    Ancestor, ClassDeclaration, ClassKind, ConstantDeclaration, Environment, GlobalDeclaration,
    InterfaceDeclaration, LoadError, Location, MethodDefinition, MethodKind, SourcePosition,
    TypeAliasDeclaration, Visibility,
};
use crate::lookup::SingletonAncestor;
use crate::navigation::SignatureTarget;
use crate::signature_validation::{
    AliasOwner, DeclarationRecord, DefinedNameRecord, FileValidation, MethodAliasRecord, MixinKind,
    ReferenceKind, SignatureDiagnostic, TypeReference, ValidationMember, ValidationOwner,
    ValidationScope, duplicated_declaration, duplicated_method_definitions, finish_diagnostics,
    generic_parameter_mismatch, invalid_method_overload, superclass_mismatch,
    syntax_error_diagnostic, validate_files,
};
use crate::steep_compatibility::DuplicatedMethodDefinition;
use crate::types::{
    Block, Function, Literal, MethodType, NameKind, Parameter, ProcType, RecordField, Type,
    Substitution, TypeName, TypeParameter, Variance,
};

enum PendingSource {
    Text { path: PathBuf, source: String },
    File(PathBuf),
}

impl PendingSource {
    fn path(&self) -> &Path {
        match self {
            PendingSource::Text { path, .. } => path,
            PendingSource::File(path) => path,
        }
    }
}

/// Collects RBS files and builds an `Environment`. Files added later reopen classes declared earlier, so add core
/// first, then libraries, then the project signatures
#[derive(Default)]
pub struct Loader {
    sources: Vec<PendingSource>,
    added_files: HashSet<PathBuf>,
    signature_files: HashSet<PathBuf>,
    errors: Vec<LoadError>,
    implicitly_returns_nil: bool,
}

impl Loader {
    pub fn new() -> Loader {
        Loader::default()
    }

    /// `implicitly_returns_nil!` in the `Steepfile`: a method type annotated with `%a{implicitly-returns-nil}`, such as
    /// `Array#[]`, returns `nil` as well, as Steep's `Interface::Builder#add_implicitly_returns_nil` makes it
    pub fn set_implicitly_returns_nil(&mut self, implicitly_returns_nil: bool) {
        self.implicitly_returns_nil = implicitly_returns_nil;
    }

    pub fn add_source(&mut self, path: &Path, source: &str) {
        self.sources.push(PendingSource::Text {
            path: path.to_path_buf(),
            source: source.to_string(),
        });
    }

    /// Like `add_source`, but the file is a project signature validated into `Environment::signature_diagnostics`
    pub fn add_signature_source(&mut self, path: &Path, source: &str) {
        self.signature_files.insert(path.to_path_buf());
        self.add_source(path, source);
    }

    /// Adds every `*.rbs` under `directory`, recursively. A path to a file adds that file alone
    pub fn add_directory(&mut self, directory: &Path) {
        self.add_directory_files(directory, false);
    }

    /// Like `add_directory`, but the files are project signatures validated into `Environment::signature_diagnostics`
    pub fn add_signature_directory(&mut self, directory: &Path) {
        let first_new_source = self.sources.len();
        self.add_directory_files(directory, false);
        for source in &self.sources[first_new_source..] {
            self.signature_files.insert(source.path().to_path_buf());
        }
    }

    /// Like `add_directory`, but skips subdirectories starting with `_` (such as `_test`) the way RBS does for core,
    /// stdlib and collection directories
    pub fn add_library_directory(&mut self, directory: &Path) {
        self.add_directory_files(directory, true);
    }

    /// Records a problem found while locating sources, reported through `Environment::errors`
    pub fn add_error(&mut self, error: LoadError) {
        self.errors.push(error);
    }

    fn add_directory_files(&mut self, directory: &Path, skip_hidden: bool) {
        if directory.is_file() {
            if self.added_files.insert(directory.to_path_buf()) {
                self.sources
                    .push(PendingSource::File(directory.to_path_buf()));
            }
            return;
        }
        if !directory.is_dir() {
            self.errors.push(LoadError {
                path: directory.to_path_buf(),
                message: "directory not found".to_string(),
            });
            return;
        }
        let mut files = Vec::new();
        collect_rbs_files(directory, skip_hidden, &mut files, &mut self.errors);
        files.sort();
        for file in files {
            if self.added_files.insert(file.clone()) {
                self.sources.push(PendingSource::File(file));
            }
        }
    }

    pub fn finish(self) -> Environment {
        let Loader {
            sources,
            signature_files,
            errors: mut load_errors,
            implicitly_returns_nil,
            ..
        } = self;
        let mut files = Vec::with_capacity(sources.len());
        let mut signature_diagnostics = Vec::new();
        let parse_results = parallel_map(&sources, |pending| {
            parse_pending_source(pending, signature_files.contains(pending.path()), implicitly_returns_nil)
        });
        for result in parse_results {
            match result {
                Ok(file) => files.push(file),
                Err(failure) => {
                    load_errors.push(failure.error);
                    signature_diagnostics.extend(failure.diagnostic);
                }
            }
        }

        let known_names = KnownNames::collect(&files);
        parallel_for_each_mut(&mut files, |file| known_names.resolve_file(file));
        let mut validations: Vec<FileValidation> = files
            .iter_mut()
            .filter_map(|file| {
                let mut validation = file.validation.take()?;
                validation.uses = file.uses.clone();
                Some(validation)
            })
            .collect();
        let signature_validation_scopes: Vec<ValidationScope> = validations
            .iter_mut()
            .flat_map(|validation| std::mem::take(&mut validation.scopes))
            .collect();

        let mut environment = Environment::default();
        let mut merge_state = MergeState::new(&signature_files);
        for file in files {
            for fragment in file.fragments {
                merge_fragment(&mut environment, fragment, &mut merge_state);
            }
        }
        fill_default_super_classes(&mut environment);
        synthesize_new_methods(&mut environment);
        let MergeState {
            pending_overloadings,
            diagnostics: merge_diagnostics,
            duplicated_method_definitions,
            ..
        } = merge_state;
        environment.duplicated_method_definitions = duplicated_method_definitions.definitions;
        signature_diagnostics.extend(merge_diagnostics);
        signature_diagnostics.extend(apply_inherited_overloadings(
            &mut environment,
            pending_overloadings,
            &signature_files,
        ));
        signature_diagnostics.extend(validate_files(&environment, &validations));
        finish_diagnostics(&mut signature_diagnostics);
        environment.errors = load_errors;
        environment.signature_diagnostics = signature_diagnostics;
        environment.signature_validation_scopes = signature_validation_scopes;
        environment
    }
}

fn collect_rbs_files(
    directory: &Path,
    skip_hidden: bool,
    files: &mut Vec<PathBuf>,
    errors: &mut Vec<LoadError>,
) {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) => {
            errors.push(LoadError {
                path: directory.to_path_buf(),
                message: error.to_string(),
            });
            return;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let hidden = path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with('_'));
            if !(skip_hidden && hidden) {
                collect_rbs_files(&path, skip_hidden, files, errors);
            }
        } else if path.extension().is_some_and(|extension| extension == "rbs") {
            files.push(path);
        }
    }
}

fn worker_count(item_count: usize) -> usize {
    let available = std::thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(1);
    available.min(item_count).max(1)
}

/// Files differ a lot in size, so workers pull the next index instead of taking fixed chunks
pub(crate) fn parallel_map<T: Sync, R: Send>(
    items: &[T],
    function: impl Fn(&T) -> R + Sync,
) -> Vec<R> {
    let next_index = AtomicUsize::new(0);
    let mut indexed_results: Vec<(usize, R)> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..worker_count(items.len()))
            .map(|_| {
                scope.spawn(|| {
                    let mut results = Vec::new();
                    loop {
                        let index = next_index.fetch_add(1, Ordering::Relaxed);
                        let Some(item) = items.get(index) else { break };
                        results.push((index, function(item)));
                    }
                    results
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|handle| handle.join().expect("loader worker panicked"))
            .collect()
    });
    indexed_results.sort_by_key(|(index, _)| *index);
    indexed_results
        .into_iter()
        .map(|(_, result)| result)
        .collect()
}

fn parallel_for_each_mut<T: Send>(items: &mut [T], function: impl Fn(&mut T) + Sync) {
    if items.is_empty() {
        return;
    }
    let chunk_size = items.len().div_ceil(worker_count(items.len()));
    std::thread::scope(|scope| {
        for chunk in items.chunks_mut(chunk_size) {
            let function = &function;
            scope.spawn(move || chunk.iter_mut().for_each(function));
        }
    });
}

#[derive(Clone)]
pub(crate) enum UseClause {
    Single { name: String, local_name: String },
    Wildcard { namespace: String },
}

struct ParsedFile {
    uses: Vec<UseClause>,
    fragments: Vec<Fragment>,
    /// Present for project signature files only
    validation: Option<FileValidation>,
}

struct ParseFailure {
    error: LoadError,
    diagnostic: Option<SignatureDiagnostic>,
}

struct Fragment {
    /// Fully qualified names of the enclosing declarations, innermost last
    context: Arc<[String]>,
    item: FragmentItem,
}

enum FragmentItem {
    Class(Box<ClassFragment>),
    Interface(InterfaceDeclaration),
    TypeAlias(TypeAliasDeclaration),
    Constant(ConstantDeclaration),
    Global(GlobalDeclaration),
    ClassAlias {
        new_name: TypeName,
        old_name: String,
        location: Location,
    },
}

struct ClassFragment {
    name: TypeName,
    kind: ClassKind,
    type_parameters: Vec<TypeParameter>,
    super_class: Option<Ancestor>,
    includes: Vec<Ancestor>,
    prepends: Vec<Ancestor>,
    extends: Vec<Ancestor>,
    module_self_types: Vec<Ancestor>,
    methods: Vec<MethodFragment>,
    instance_variables: Vec<(String, Type)>,
    class_instance_variables: Vec<(String, Type)>,
    class_variables: Vec<(String, Type)>,
    location: Location,
    comment: Option<String>,
    /// Members other than nested declarations, which RBS's `each_member` yields
    has_members: bool,
}

struct MethodFragment {
    definition: MethodDefinition,
    overloading: bool,
}

/// `None` when the source has a syntax error
pub(crate) fn parse_signature_validation(path: &Path, source: &str) -> Option<FileValidation> {
    let pending = PendingSource::Text {
        path: path.to_path_buf(),
        source: source.to_string(),
    };
    let file = parse_pending_source(&pending, true, false).ok()?;
    let mut validation = file.validation?;
    validation.uses = file.uses;
    Some(validation)
}

fn parse_pending_source(
    pending: &PendingSource,
    validated: bool,
    implicitly_returns_nil: bool,
) -> Result<ParsedFile, ParseFailure> {
    let path = pending.path().to_path_buf();
    let read_source;
    let source = match pending {
        PendingSource::Text { source, .. } => source.as_str(),
        PendingSource::File(file) => {
            read_source = std::fs::read_to_string(file).map_err(|error| ParseFailure {
                error: LoadError {
                    path: path.clone(),
                    message: error.to_string(),
                },
                diagnostic: None,
            })?;
            read_source.as_str()
        }
    };
    let signature = ruby_rbs::node::parse(source).map_err(|message| ParseFailure {
        diagnostic: validated.then(|| syntax_error_diagnostic(&path, source, &message)),
        error: LoadError {
            path: path.clone(),
            message,
        },
    })?;
    let mut converter = Converter::new(source, Arc::new(path));
    converter.implicitly_returns_nil = implicitly_returns_nil;
    if validated {
        converter.validation = Some(ValidationCollector::default());
    }
    let mut file = ParsedFile {
        uses: Vec::new(),
        fragments: Vec::new(),
        validation: None,
    };
    for directive in signature.directives().iter() {
        let Node::Use(use_directive) = directive else {
            continue;
        };
        for clause in use_directive.clauses().iter() {
            match clause {
                Node::UseSingleClause(single) => {
                    let local_name = match single.new_name() {
                        Some(new_name) => new_name.as_str().to_string(),
                        None => single.type_name().name().as_str().to_string(),
                    };
                    let name = type_name_string(&single.type_name())
                        .trim_start_matches("::")
                        .to_string();
                    file.uses.push(UseClause::Single { name, local_name });
                }
                Node::UseWildcardClause(wildcard) => {
                    let namespace = namespace_string(&wildcard.namespace());
                    let namespace = namespace
                        .trim_start_matches("::")
                        .trim_end_matches("::")
                        .to_string();
                    file.uses.push(UseClause::Wildcard { namespace });
                }
                _ => {}
            }
        }
    }
    let top_level_context: Arc<[String]> = Arc::from(Vec::new());
    for declaration in signature.declarations().iter() {
        converter.convert_declaration(declaration, &top_level_context, &mut file.fragments);
    }
    file.validation = converter
        .validation
        .map(|collector| collector.file.into_inner());
    Ok(file)
}

fn attribute_method_kind(kind: AttributeKind) -> MethodKind {
    match kind {
        AttributeKind::Instance => MethodKind::Instance,
        AttributeKind::Singleton => MethodKind::Singleton,
    }
}

fn attribute_validation_member(kind: AttributeKind) -> ValidationMember {
    match kind {
        AttributeKind::Instance => ValidationMember::InstanceMethod,
        AttributeKind::Singleton => ValidationMember::SingletonMember,
    }
}

fn namespace_string(namespace: &NamespaceNode) -> String {
    let mut result = String::new();
    if namespace.absolute() {
        result.push_str("::");
    }
    for segment in namespace.path().iter() {
        if let Node::Symbol(symbol) = segment {
            result.push_str(symbol.as_str());
            result.push_str("::");
        }
    }
    result
}

/// The name as written, with a leading `::` when it is absolute
fn type_name_string(type_name: &TypeNameNode) -> String {
    let mut result = namespace_string(&type_name.namespace());
    result.push_str(type_name.name().as_str());
    result
}

fn qualify(context: &[String], written_name: &str) -> TypeName {
    if let Some(absolute) = written_name.strip_prefix("::") {
        return absolute.to_string();
    }
    match context.last() {
        Some(outer) => format!("{outer}::{written_name}"),
        None => written_name.to_string(),
    }
}

fn inner_context(outer_context: &[String], name: &TypeName) -> Arc<[String]> {
    outer_context
        .iter()
        .cloned()
        .chain(std::iter::once(name.clone()))
        .collect()
}

const IMPLICITLY_RETURNS_NIL: &str = "implicitly-returns-nil";

const ALWAYS_PRIVATE_INSTANCE_METHODS: [&str; 5] = [
    "initialize",
    "initialize_copy",
    "initialize_clone",
    "initialize_dup",
    "respond_to_missing?",
];

#[derive(Default)]
struct ValidationCollector {
    /// The context names written in the declaration being converted are resolved in
    context: RefCell<Arc<[String]>>,
    /// Names of the type parameter list being converted. The parser reads a parameter used in the bound of an earlier
    /// parameter, as `U` in `[A < Array[U], U]`, as a class name
    type_parameter_names: RefCell<Vec<String>>,
    file: RefCell<FileValidation>,
}

struct Converter<'source> {
    source: &'source str,
    path: Arc<PathBuf>,
    line_starts: Vec<usize>,
    validation: Option<ValidationCollector>,
    implicitly_returns_nil: bool,
}

impl<'source> Converter<'source> {
    fn new(source: &'source str, path: Arc<PathBuf>) -> Converter<'source> {
        let mut line_starts = vec![0];
        line_starts.extend(source.match_indices('\n').map(|(index, _)| index + 1));
        Converter {
            source,
            path,
            line_starts,
            validation: None,
            implicitly_returns_nil: false,
        }
    }

    fn with_context<T>(&self, context: &Arc<[String]>, body: impl FnOnce() -> T) -> T {
        let Some(collector) = &self.validation else {
            return body();
        };
        let previous = collector.context.replace(context.clone());
        let result = body();
        collector.context.replace(previous);
        result
    }

    fn record_reference(
        &self,
        kind: impl FnOnce() -> ReferenceKind,
        name: &TypeNameNode,
        argument_count: usize,
        range: RBSLocationRange,
    ) {
        let Some(collector) = &self.validation else {
            return;
        };
        let kind = kind();
        if matches!(kind, ReferenceKind::Type(NameKind::Class))
            && argument_count == 0
            && name.namespace().path().iter().next().is_none()
            && !name.namespace().absolute()
            && collector
                .type_parameter_names
                .borrow()
                .iter()
                .any(|type_parameter_name| type_parameter_name == name.name().as_str())
        {
            return;
        }
        let context = collector.context.borrow().clone();
        collector.file.borrow_mut().references.push(TypeReference {
            context,
            kind,
            written_name: type_name_string(name),
            argument_count,
            location: self.location(range),
        });
    }

    fn record_scope(
        &self,
        owner: impl FnOnce() -> ValidationOwner,
        member: ValidationMember,
        range: RBSLocationRange,
    ) {
        if let Some(collector) = &self.validation {
            collector.file.borrow_mut().scopes.push(ValidationScope {
                location: self.location(range),
                owner: owner(),
                member,
            });
        }
    }

    fn record_declaration(&self, name: &TypeName, range: RBSLocationRange) {
        if self.validation.is_some() {
            self.record_declaration_at(name, &self.location(range));
        }
    }

    fn record_declaration_at(&self, name: &TypeName, location: &Location) {
        if let Some(collector) = &self.validation {
            collector
                .file
                .borrow_mut()
                .declarations
                .push(DeclarationRecord {
                    name: name.clone(),
                    location: location.clone(),
                });
        }
    }

    fn record_defined_name(
        &self,
        target: impl FnOnce() -> SignatureTarget,
        range: RBSLocationRange,
    ) {
        if let Some(collector) = &self.validation {
            collector
                .file
                .borrow_mut()
                .defined_names
                .push(DefinedNameRecord {
                    target: target(),
                    location: self.location(range),
                });
        }
    }

    fn record_method_name(
        &self,
        owner: &TypeName,
        kind: MethodKind,
        method_name: &str,
        range: RBSLocationRange,
    ) {
        self.record_defined_name(
            || SignatureTarget::Method {
                owner: owner.clone(),
                kind,
                name: method_name.to_string(),
            },
            range,
        );
    }

    fn record_method_alias(
        &self,
        owner_name: &TypeName,
        owner: AliasOwner,
        old_name: &str,
        range: RBSLocationRange,
    ) {
        if let Some(collector) = &self.validation {
            collector
                .file
                .borrow_mut()
                .method_aliases
                .push(MethodAliasRecord {
                    owner_name: owner_name.clone(),
                    owner,
                    old_name: old_name.to_string(),
                    location: self.location(range),
                });
        }
    }

    fn record_duplicated_interface_method(
        &self,
        interface_name: &str,
        previous: &MethodDefinition,
        current: &MethodDefinition,
    ) {
        if let Some(collector) = &self.validation {
            let signature_files = HashSet::from([self.path.as_ref().clone()]);
            collector
                .file
                .borrow_mut()
                .diagnostics
                .extend(duplicated_method_definitions(
                    &signature_files,
                    interface_name,
                    &current.name,
                    [previous.location.as_ref(), current.location.as_ref()],
                ));
        }
    }

    fn convert_recorded_ancestor(
        &self,
        name: &TypeNameNode,
        arguments: NodeList,
        kind: impl FnOnce() -> ReferenceKind,
        range: RBSLocationRange,
    ) -> Ancestor {
        let ancestor = self.convert_ancestor(name, arguments);
        self.record_reference(kind, name, ancestor.arguments.len(), range);
        ancestor
    }

    fn position(&self, byte_offset: i32) -> SourcePosition {
        let byte_offset = (byte_offset.max(0) as usize).min(self.source.len());
        let line = self
            .line_starts
            .partition_point(|start| *start <= byte_offset)
            - 1;
        let line_start = self.line_starts[line];
        let byte_column = byte_offset - line_start;
        let utf16_column = self
            .source
            .get(line_start..byte_offset)
            .map(|prefix| prefix.chars().map(char::len_utf16).sum::<usize>())
            .unwrap_or(byte_column);
        SourcePosition {
            line: line as u32,
            byte_column: byte_column as u32,
            utf16_column: utf16_column as u32,
        }
    }

    fn location(&self, range: RBSLocationRange) -> Location {
        Location {
            path: self.path.clone(),
            start: self.position(range.start()),
            end: self.position(range.end()),
        }
    }

    fn slice(&self, range: RBSLocationRange) -> &'source str {
        self.source
            .get(range.start().max(0) as usize..range.end().max(0) as usize)
            .unwrap_or("")
    }

    fn convert_declaration(
        &self,
        declaration: Node,
        context: &Arc<[String]>,
        fragments: &mut Vec<Fragment>,
    ) {
        self.with_context(context, || {
            self.convert_declaration_in_context(declaration, context, fragments)
        });
    }

    fn convert_declaration_in_context(
        &self,
        declaration: Node,
        context: &Arc<[String]>,
        fragments: &mut Vec<Fragment>,
    ) {
        match declaration {
            Node::Class(class) => {
                let name = qualify(context, &type_name_string(&class.name()));
                self.record_declaration(&name, class.location());
                self.record_defined_name(
                    || SignatureTarget::ClassDeclaration(name.clone()),
                    class.name_location(),
                );
                self.record_scope(
                    || ValidationOwner::Class(name.clone()),
                    ValidationMember::Declaration,
                    class.location(),
                );
                let inner_context = inner_context(context, &name);
                let mut fragment = self.with_context(&inner_context, || {
                    self.empty_class_fragment(
                        name,
                        ClassKind::Class,
                        class.type_params(),
                        class.location(),
                        class.comment(),
                    )
                });
                fragment.super_class = class.super_class().map(|super_class| {
                    self.convert_recorded_ancestor(
                        &super_class.name(),
                        super_class.args(),
                        || ReferenceKind::SuperClass {
                            declaration_location: self.location(class.location()),
                        },
                        super_class.location(),
                    )
                });
                self.convert_class_body(fragment, class.members(), inner_context, fragments);
            }
            Node::Module(module) => {
                let name = qualify(context, &type_name_string(&module.name()));
                self.record_declaration(&name, module.location());
                self.record_defined_name(
                    || SignatureTarget::ClassDeclaration(name.clone()),
                    module.name_location(),
                );
                self.record_scope(
                    || ValidationOwner::Class(name.clone()),
                    ValidationMember::Declaration,
                    module.location(),
                );
                let inner_context = inner_context(context, &name);
                let fragment = self.with_context(&inner_context, || {
                    let mut fragment = self.empty_class_fragment(
                        name,
                        ClassKind::Module,
                        module.type_params(),
                        module.location(),
                        module.comment(),
                    );
                    fragment.module_self_types = module
                        .self_types()
                        .iter()
                        .filter_map(|self_type| match self_type {
                            Node::ModuleSelf(module_self) => Some(self.convert_recorded_ancestor(
                                &module_self.name(),
                                module_self.args(),
                                || ReferenceKind::ModuleSelfType,
                                module_self.location(),
                            )),
                            _ => None,
                        })
                        .collect();
                    fragment
                });
                self.convert_class_body(fragment, module.members(), inner_context, fragments);
            }
            Node::Interface(interface) => {
                let name = qualify(context, &type_name_string(&interface.name()));
                self.record_declaration(&name, interface.location());
                self.record_scope(
                    || ValidationOwner::Interface(name.clone()),
                    ValidationMember::Declaration,
                    interface.location(),
                );
                let mut declaration = InterfaceDeclaration {
                    name: name.clone(),
                    type_parameters: self.convert_type_parameters(interface.type_params()),
                    includes: Vec::new(),
                    methods: HashMap::new(),
                    location: Some(self.location(interface.location())),
                    comment: comment_text(interface.comment()),
                };
                for member in interface.members().iter() {
                    match member {
                        Node::Include(include) => {
                            declaration.includes.push(self.convert_recorded_ancestor(
                                &include.name(),
                                include.args(),
                                || ReferenceKind::InterfaceInclude,
                                include.location(),
                            ))
                        }
                        Node::MethodDefinition(method) => {
                            let method_name = method.name().as_str().to_string();
                            let mut overloads = self.convert_method_overloads(&method);
                            if method.overloading()
                                && let Some(previous) = declaration.methods.remove(&method_name)
                            {
                                overloads.extend(previous.overloads);
                            }
                            let definition = MethodDefinition {
                                name: method_name.clone(),
                                owner: name.clone(),
                                kind: MethodKind::Instance,
                                overloads,
                                visibility: Visibility::Public,
                                alias_of: None,
                                location: Some(self.location(method.location())),
                                comment: comment_text(method.comment()),
                                annotations: method_annotations(&method),
                                is_attribute_reader: false,
                            };
                            self.insert_interface_method(&mut declaration, definition);
                        }
                        Node::Alias(alias) => {
                            let new_name = alias.new_name().as_str().to_string();
                            self.record_method_alias(
                                &name,
                                AliasOwner::Interface,
                                alias.old_name().as_str(),
                                alias.location(),
                            );
                            let definition = MethodDefinition {
                                name: new_name.clone(),
                                owner: name.clone(),
                                kind: MethodKind::Instance,
                                overloads: Vec::new(),
                                visibility: Visibility::Public,
                                alias_of: Some(alias.old_name().as_str().to_string()),
                                location: Some(self.location(alias.location())),
                                comment: comment_text(alias.comment()),
                                annotations: annotation_texts(alias.annotations()),
                                is_attribute_reader: false,
                            };
                            self.insert_interface_method(&mut declaration, definition);
                        }
                        _ => {}
                    }
                }
                fragments.push(Fragment {
                    context: context.clone(),
                    item: FragmentItem::Interface(declaration),
                });
            }
            Node::TypeAlias(type_alias) => {
                let name = qualify(context, &type_name_string(&type_alias.name()));
                self.record_declaration(&name, type_alias.location());
                self.record_scope(
                    || ValidationOwner::TypeAlias(name.clone()),
                    ValidationMember::Declaration,
                    type_alias.location(),
                );
                let declaration = TypeAliasDeclaration {
                    name,
                    type_parameters: self.convert_type_parameters(type_alias.type_params()),
                    aliased_type: self.convert_type(type_alias.type_()),
                    location: Some(self.location(type_alias.location())),
                };
                fragments.push(Fragment {
                    context: context.clone(),
                    item: FragmentItem::TypeAlias(declaration),
                });
            }
            Node::Constant(constant) => {
                let name = qualify(context, &type_name_string(&constant.name()));
                self.record_declaration(&name, constant.location());
                self.record_defined_name(
                    || SignatureTarget::ConstantDeclaration(name.clone()),
                    constant.name_location(),
                );
                self.record_scope(
                    || ValidationOwner::Constant(name.clone()),
                    ValidationMember::Declaration,
                    constant.location(),
                );
                let declaration = ConstantDeclaration {
                    name,
                    constant_type: self.convert_type(constant.type_()),
                    location: Some(self.location(constant.location())),
                    comment: comment_text(constant.comment()),
                };
                fragments.push(Fragment {
                    context: context.clone(),
                    item: FragmentItem::Constant(declaration),
                });
            }
            Node::Global(global) => {
                self.record_scope(
                    || ValidationOwner::Global(global.name().as_str().to_string()),
                    ValidationMember::Declaration,
                    global.location(),
                );
                let declaration = GlobalDeclaration {
                    name: global.name().as_str().to_string(),
                    global_type: self.convert_type(global.type_()),
                    location: Some(self.location(global.location())),
                };
                fragments.push(Fragment {
                    context: context.clone(),
                    item: FragmentItem::Global(declaration),
                });
            }
            Node::ClassAlias(class_alias) => {
                self.record_defined_name(
                    || {
                        SignatureTarget::ClassDeclaration(qualify(
                            context,
                            &type_name_string(&class_alias.new_name()),
                        ))
                    },
                    class_alias.new_name_location(),
                );
                self.convert_class_alias(
                    &class_alias.new_name(),
                    &class_alias.old_name(),
                    class_alias.location(),
                    class_alias.old_name_location(),
                    context,
                    fragments,
                )
            }
            Node::ModuleAlias(module_alias) => {
                self.record_defined_name(
                    || {
                        SignatureTarget::ClassDeclaration(qualify(
                            context,
                            &type_name_string(&module_alias.new_name()),
                        ))
                    },
                    module_alias.new_name_location(),
                );
                self.convert_class_alias(
                    &module_alias.new_name(),
                    &module_alias.old_name(),
                    module_alias.location(),
                    module_alias.old_name_location(),
                    context,
                    fragments,
                )
            }
            _ => {}
        }
    }

    fn convert_class_alias(
        &self,
        new_name: &TypeNameNode,
        old_name: &TypeNameNode,
        range: RBSLocationRange,
        old_name_range: RBSLocationRange,
        context: &Arc<[String]>,
        fragments: &mut Vec<Fragment>,
    ) {
        let new_name = qualify(context, &type_name_string(new_name));
        let location = self.location(range);
        self.record_declaration_at(&new_name, &location);
        self.record_reference(
            || ReferenceKind::ClassAliasTarget,
            old_name,
            0,
            old_name_range,
        );
        fragments.push(Fragment {
            context: context.clone(),
            item: FragmentItem::ClassAlias {
                new_name,
                old_name: type_name_string(old_name),
                location,
            },
        });
    }

    fn insert_interface_method(
        &self,
        declaration: &mut InterfaceDeclaration,
        definition: MethodDefinition,
    ) {
        match declaration.methods.entry(definition.name.clone()) {
            Entry::Occupied(mut occupied) => {
                let previous = occupied.insert(definition);
                self.record_duplicated_interface_method(
                    &declaration.name,
                    &previous,
                    occupied.get(),
                );
            }
            Entry::Vacant(vacant) => {
                vacant.insert(definition);
            }
        }
    }

    fn empty_class_fragment(
        &self,
        name: TypeName,
        kind: ClassKind,
        type_parameters: NodeList,
        location: RBSLocationRange,
        comment: Option<CommentNode>,
    ) -> ClassFragment {
        ClassFragment {
            name,
            kind,
            type_parameters: self.convert_type_parameters(type_parameters),
            super_class: None,
            includes: Vec::new(),
            prepends: Vec::new(),
            extends: Vec::new(),
            module_self_types: Vec::new(),
            methods: Vec::new(),
            instance_variables: Vec::new(),
            class_instance_variables: Vec::new(),
            class_variables: Vec::new(),
            location: self.location(location),
            comment: comment_text(comment),
            has_members: false,
        }
    }

    fn convert_class_body(
        &self,
        fragment: ClassFragment,
        members: NodeList,
        inner_context: Arc<[String]>,
        fragments: &mut Vec<Fragment>,
    ) {
        let mut nested_fragments = Vec::new();
        let fragment = self.with_context(&inner_context, || {
            self.convert_class_members(fragment, members, &inner_context, &mut nested_fragments)
        });
        fragments.push(Fragment {
            context: inner_context,
            item: FragmentItem::Class(Box::new(fragment)),
        });
        fragments.extend(nested_fragments);
    }

    fn convert_class_members(
        &self,
        mut fragment: ClassFragment,
        members: NodeList,
        inner_context: &Arc<[String]>,
        nested_fragments: &mut Vec<Fragment>,
    ) -> ClassFragment {
        let mut default_visibility = Visibility::Public;
        fragment.has_members = members.iter().any(|member| {
            matches!(
                member,
                Node::MethodDefinition(_)
                    | Node::Alias(_)
                    | Node::AttrAccessor(_)
                    | Node::AttrReader(_)
                    | Node::AttrWriter(_)
                    | Node::InstanceVariable(_)
                    | Node::ClassInstanceVariable(_)
                    | Node::ClassVariable(_)
                    | Node::Include(_)
                    | Node::Extend(_)
                    | Node::Prepend(_)
                    | Node::Private(_)
                    | Node::Public(_)
            )
        });
        for member in members.iter() {
            match member {
                Node::MethodDefinition(method) => {
                    let method_name = method.name().as_str().to_string();
                    let member = match method.kind() {
                        MethodDefinitionKind::Singleton => ValidationMember::SingletonMember,
                        _ if method_name == "initialize" => ValidationMember::Initializer,
                        _ => ValidationMember::InstanceMethod,
                    };
                    self.record_scope(
                        || ValidationOwner::Class(fragment.name.clone()),
                        member,
                        method.location(),
                    );
                    let overloads = self.convert_method_overloads(&method);
                    let explicit_visibility = match method.visibility() {
                        MethodDefinitionVisibility::Public => Some(Visibility::Public),
                        MethodDefinitionVisibility::Private => Some(Visibility::Private),
                        MethodDefinitionVisibility::Unspecified => None,
                    };
                    let is_module_function =
                        method.kind() == MethodDefinitionKind::SingletonInstance;
                    let kinds: &[MethodKind] = match method.kind() {
                        MethodDefinitionKind::Instance => &[MethodKind::Instance],
                        MethodDefinitionKind::Singleton => &[MethodKind::Singleton],
                        MethodDefinitionKind::SingletonInstance => {
                            &[MethodKind::Instance, MethodKind::Singleton]
                        }
                    };
                    for kind in kinds {
                        self.record_method_name(
                            &fragment.name,
                            *kind,
                            &method_name,
                            method.name_location(),
                        );
                        let visibility = match kind {
                            MethodKind::Instance if is_module_function => Visibility::Private,
                            MethodKind::Instance
                                if ALWAYS_PRIVATE_INSTANCE_METHODS
                                    .contains(&method_name.as_str()) =>
                            {
                                Visibility::Private
                            }
                            MethodKind::Instance => {
                                explicit_visibility.unwrap_or(default_visibility)
                            }
                            MethodKind::Singleton => {
                                explicit_visibility.unwrap_or(Visibility::Public)
                            }
                        };
                        fragment.methods.push(MethodFragment {
                            definition: MethodDefinition {
                                name: method_name.clone(),
                                owner: fragment.name.clone(),
                                kind: *kind,
                                overloads: overloads.clone(),
                                visibility,
                                alias_of: None,
                                location: Some(self.location(method.location())),
                                comment: comment_text(method.comment()),
                                annotations: method_annotations(&method),
                                is_attribute_reader: false,
                            },
                            overloading: method.overloading(),
                        });
                    }
                }
                Node::Alias(alias) => {
                    let (kind, visibility, member) = match alias.kind() {
                        AliasKind::Instance => (
                            MethodKind::Instance,
                            default_visibility,
                            ValidationMember::InstanceAlias,
                        ),
                        AliasKind::Singleton => (
                            MethodKind::Singleton,
                            Visibility::Public,
                            ValidationMember::SingletonMember,
                        ),
                    };
                    self.record_scope(
                        || ValidationOwner::Class(fragment.name.clone()),
                        member,
                        alias.location(),
                    );
                    self.record_method_alias(
                        &fragment.name,
                        AliasOwner::Class(kind),
                        alias.old_name().as_str(),
                        alias.location(),
                    );
                    self.record_method_name(
                        &fragment.name,
                        kind,
                        alias.new_name().as_str(),
                        alias.new_name_location(),
                    );
                    self.record_method_name(
                        &fragment.name,
                        kind,
                        alias.old_name().as_str(),
                        alias.old_name_location(),
                    );
                    fragment.methods.push(MethodFragment {
                        definition: MethodDefinition {
                            name: alias.new_name().as_str().to_string(),
                            owner: fragment.name.clone(),
                            kind,
                            overloads: Vec::new(),
                            visibility,
                            alias_of: Some(alias.old_name().as_str().to_string()),
                            location: Some(self.location(alias.location())),
                            comment: comment_text(alias.comment()),
                            annotations: annotation_texts(alias.annotations()),
                            is_attribute_reader: false,
                        },
                        overloading: false,
                    });
                }
                Node::AttrReader(attribute) => {
                    self.record_method_name(
                        &fragment.name,
                        attribute_method_kind(attribute.kind()),
                        attribute.name().as_str(),
                        attribute.name_location(),
                    );
                    self.record_scope(
                        || ValidationOwner::Class(fragment.name.clone()),
                        attribute_validation_member(attribute.kind()),
                        attribute.location(),
                    );
                    Attribute {
                        name: attribute.name().as_str().to_string(),
                        attribute_type: self.convert_type(attribute.type_()),
                        kind: attribute.kind(),
                        visibility: attribute.visibility(),
                        instance_variable: self.attribute_instance_variable(
                            attribute.name().as_str(),
                            attribute.ivar_name(),
                            attribute.ivar_name_location(),
                        ),
                        location: self.location(attribute.location()),
                        comment: comment_text(attribute.comment()),
                        annotations: annotation_texts(attribute.annotations()),
                    }
                    .register(
                        &mut fragment,
                        default_visibility,
                        AttributeAccess::Reader,
                    )
                }
                Node::AttrWriter(attribute) => {
                    self.record_method_name(
                        &fragment.name,
                        attribute_method_kind(attribute.kind()),
                        &format!("{}=", attribute.name().as_str()),
                        attribute.name_location(),
                    );
                    self.record_scope(
                        || ValidationOwner::Class(fragment.name.clone()),
                        attribute_validation_member(attribute.kind()),
                        attribute.location(),
                    );
                    Attribute {
                        name: attribute.name().as_str().to_string(),
                        attribute_type: self.convert_type(attribute.type_()),
                        kind: attribute.kind(),
                        visibility: attribute.visibility(),
                        instance_variable: self.attribute_instance_variable(
                            attribute.name().as_str(),
                            attribute.ivar_name(),
                            attribute.ivar_name_location(),
                        ),
                        location: self.location(attribute.location()),
                        comment: comment_text(attribute.comment()),
                        annotations: annotation_texts(attribute.annotations()),
                    }
                    .register(
                        &mut fragment,
                        default_visibility,
                        AttributeAccess::Writer,
                    )
                }
                Node::AttrAccessor(attribute) => {
                    self.record_method_name(
                        &fragment.name,
                        attribute_method_kind(attribute.kind()),
                        attribute.name().as_str(),
                        attribute.name_location(),
                    );
                    self.record_scope(
                        || ValidationOwner::Class(fragment.name.clone()),
                        attribute_validation_member(attribute.kind()),
                        attribute.location(),
                    );
                    Attribute {
                        name: attribute.name().as_str().to_string(),
                        attribute_type: self.convert_type(attribute.type_()),
                        kind: attribute.kind(),
                        visibility: attribute.visibility(),
                        instance_variable: self.attribute_instance_variable(
                            attribute.name().as_str(),
                            attribute.ivar_name(),
                            attribute.ivar_name_location(),
                        ),
                        location: self.location(attribute.location()),
                        comment: comment_text(attribute.comment()),
                        annotations: annotation_texts(attribute.annotations()),
                    }
                    .register(
                        &mut fragment,
                        default_visibility,
                        AttributeAccess::Accessor,
                    )
                }
                Node::InstanceVariable(variable) => {
                    self.record_scope(
                        || ValidationOwner::Class(fragment.name.clone()),
                        ValidationMember::InstanceVariable,
                        variable.location(),
                    );
                    fragment.instance_variables.push((
                        variable.name().as_str().to_string(),
                        self.convert_type(variable.type_()),
                    ))
                }
                Node::ClassInstanceVariable(variable) => {
                    self.record_scope(
                        || ValidationOwner::Class(fragment.name.clone()),
                        ValidationMember::SingletonMember,
                        variable.location(),
                    );
                    fragment.class_instance_variables.push((
                        variable.name().as_str().to_string(),
                        self.convert_type(variable.type_()),
                    ))
                }
                Node::ClassVariable(variable) => {
                    self.record_scope(
                        || ValidationOwner::Class(fragment.name.clone()),
                        ValidationMember::SingletonMember,
                        variable.location(),
                    );
                    fragment.class_variables.push((
                        variable.name().as_str().to_string(),
                        self.convert_type(variable.type_()),
                    ))
                }
                Node::Include(include) => {
                    let ancestor = self.convert_recorded_ancestor(
                        &include.name(),
                        include.args(),
                        || ReferenceKind::Mixin {
                            kind: MixinKind::Include,
                            owner: fragment.name.clone(),
                        },
                        include.location(),
                    );
                    fragment.includes.push(ancestor);
                }
                Node::Extend(extend) => {
                    let ancestor = self.convert_recorded_ancestor(
                        &extend.name(),
                        extend.args(),
                        || ReferenceKind::Mixin {
                            kind: MixinKind::Extend,
                            owner: fragment.name.clone(),
                        },
                        extend.location(),
                    );
                    fragment.extends.push(ancestor);
                }
                Node::Prepend(prepend) => {
                    let ancestor = self.convert_recorded_ancestor(
                        &prepend.name(),
                        prepend.args(),
                        || ReferenceKind::Mixin {
                            kind: MixinKind::Prepend,
                            owner: fragment.name.clone(),
                        },
                        prepend.location(),
                    );
                    fragment.prepends.push(ancestor);
                }
                Node::Private(_) => default_visibility = Visibility::Private,
                Node::Public(_) => default_visibility = Visibility::Public,
                other => self.convert_declaration(other, inner_context, nested_fragments),
            }
        }
        fragment
    }

    fn attribute_instance_variable(
        &self,
        attribute_name: &str,
        instance_variable_name: AttrIvarName,
        instance_variable_name_location: Option<RBSLocationRange>,
    ) -> Option<String> {
        match instance_variable_name {
            AttrIvarName::Unspecified => Some(format!("@{attribute_name}")),
            AttrIvarName::Empty => None,
            AttrIvarName::Name(_) => {
                instance_variable_name_location.map(|range| self.slice(range).to_string())
            }
        }
    }

    fn convert_ancestor(&self, name: &TypeNameNode, arguments: NodeList) -> Ancestor {
        Ancestor {
            name: type_name_string(name),
            arguments: self.convert_types(arguments),
        }
    }

    fn convert_type_parameters(&self, type_parameters: NodeList) -> Vec<TypeParameter> {
        let Some(collector) = &self.validation else {
            return self.convert_type_parameter_list(type_parameters);
        };
        let names = type_parameters
            .iter()
            .filter_map(|type_parameter| match type_parameter {
                Node::TypeParam(type_parameter) => Some(type_parameter.name().as_str().to_string()),
                _ => None,
            });
        let previous_length = collector.type_parameter_names.borrow().len();
        collector.type_parameter_names.borrow_mut().extend(names);
        let converted = self.convert_type_parameter_list(type_parameters);
        collector
            .type_parameter_names
            .borrow_mut()
            .truncate(previous_length);
        converted
    }

    fn convert_type_parameter_list(&self, type_parameters: NodeList) -> Vec<TypeParameter> {
        type_parameters
            .iter()
            .filter_map(|type_parameter| match type_parameter {
                Node::TypeParam(type_parameter) => Some(TypeParameter {
                    name: type_parameter.name().as_str().to_string(),
                    upper_bound: type_parameter
                        .upper_bound()
                        .map(|bound| self.convert_type(bound)),
                    default_type: type_parameter
                        .default_type()
                        .map(|default_type| self.convert_type(default_type)),
                    variance: match type_parameter.variance() {
                        TypeParamVariance::Invariant => Variance::Invariant,
                        TypeParamVariance::Covariant => Variance::Covariant,
                        TypeParamVariance::Contravariant => Variance::Contravariant,
                    },
                }),
                _ => None,
            })
            .collect()
    }

    fn convert_overloads(&self, overloads: NodeList) -> Vec<MethodType> {
        overloads
            .iter()
            .filter_map(|overload| match overload {
                Node::MethodDefinitionOverload(overload) => {
                    self.convert_method_type(overload.method_type())
                }
                _ => None,
            })
            .collect()
    }

    /// The overloads of a method member, each returning `nil` as well when the member or the overload is annotated
    /// with `%a{implicitly-returns-nil}` and `implicitly_returns_nil` is set
    fn convert_method_overloads(&self, method: &MethodDefinitionNode) -> Vec<MethodType> {
        let member_annotated = annotation_texts(method.annotations()).iter().any(|text| text == IMPLICITLY_RETURNS_NIL);
        method
            .overloads()
            .iter()
            .filter_map(|overload| match overload {
                Node::MethodDefinitionOverload(overload) => {
                    let mut method_type = self.convert_method_type(overload.method_type())?;
                    let annotated = member_annotated
                        || annotation_texts(overload.annotations()).iter().any(|text| text == IMPLICITLY_RETURNS_NIL);
                    if self.implicitly_returns_nil && annotated {
                        let return_type = std::mem::replace(&mut method_type.function.return_type, Type::Nil);
                        method_type.function.return_type = Type::optional(return_type);
                    }
                    Some(method_type)
                }
                _ => None,
            })
            .collect()
    }

    fn convert_method_type(&self, node: Node) -> Option<MethodType> {
        let Node::MethodType(method_type) = node else {
            return None;
        };
        Some(MethodType {
            type_parameters: self.convert_type_parameters(method_type.type_params()),
            function: self.convert_function(method_type.type_()),
            block: method_type.block().map(|block| self.convert_block(&block)),
        })
    }

    fn convert_block(&self, block: &BlockTypeNode) -> Block {
        Block {
            function: self.convert_function(block.type_()),
            required: block.required(),
            self_type: block
                .self_type()
                .map(|self_type| self.convert_type(self_type)),
        }
    }

    fn convert_parameter(&self, node: Node) -> Parameter {
        match node {
            Node::FunctionParam(parameter) => Parameter {
                parameter_type: self.convert_type(parameter.type_()),
                name: parameter.name().map(|name| name.as_str().to_string()),
            },
            other => Parameter::new(self.convert_type(other)),
        }
    }

    fn convert_parameters(&self, parameters: NodeList) -> Vec<Parameter> {
        parameters
            .iter()
            .map(|parameter| self.convert_parameter(parameter))
            .collect()
    }

    fn convert_keywords(&self, keywords: RBSHash) -> Vec<(String, Parameter)> {
        keywords
            .iter()
            .map(|(key, value)| (literal_key(&key), self.convert_parameter(value)))
            .collect()
    }

    fn convert_function(&self, node: Node) -> Function {
        match node {
            Node::FunctionType(function) => Function {
                required_positionals: self.convert_parameters(function.required_positionals()),
                optional_positionals: self.convert_parameters(function.optional_positionals()),
                rest_positionals: function
                    .rest_positionals()
                    .map(|parameter| self.convert_parameter(parameter)),
                trailing_positionals: self.convert_parameters(function.trailing_positionals()),
                required_keywords: self.convert_keywords(function.required_keywords()),
                optional_keywords: self.convert_keywords(function.optional_keywords()),
                rest_keywords: function
                    .rest_keywords()
                    .map(|parameter| self.convert_parameter(parameter)),
                return_type: self.convert_type(function.return_type()),
                untyped_parameters: false,
            },
            Node::UntypedFunctionType(function) => Function {
                untyped_parameters: true,
                ..Function::returning(self.convert_type(function.return_type()))
            },
            _ => Function {
                untyped_parameters: true,
                ..Function::returning(Type::Untyped)
            },
        }
    }

    fn convert_types(&self, types: NodeList) -> Vec<Type> {
        types
            .iter()
            .map(|member| self.convert_type(member))
            .collect()
    }

    fn convert_type(&self, node: Node) -> Type {
        match node {
            Node::AnyType(_) => Type::Untyped,
            Node::BoolType(_) => Type::Bool,
            Node::BottomType(_) => Type::Bottom,
            Node::ClassType(_) => Type::Class,
            Node::InstanceType(_) => Type::Instance,
            Node::NilType(_) => Type::Nil,
            Node::SelfType(_) => Type::SelfType,
            Node::TopType(_) => Type::Top,
            Node::VoidType(_) => Type::Void,
            Node::AliasType(alias) => {
                let arguments = self.convert_types(alias.args());
                self.record_reference(
                    || ReferenceKind::Type(NameKind::Alias),
                    &alias.name(),
                    arguments.len(),
                    alias.location(),
                );
                Type::Alias {
                    name: type_name_string(&alias.name()),
                    arguments,
                }
            }
            Node::ClassInstanceType(class_instance) => {
                let arguments = self.convert_types(class_instance.args());
                self.record_reference(
                    || ReferenceKind::Type(NameKind::Class),
                    &class_instance.name(),
                    arguments.len(),
                    class_instance.location(),
                );
                Type::ClassInstance {
                    name: type_name_string(&class_instance.name()),
                    arguments,
                }
            }
            Node::ClassSingletonType(class_singleton) => {
                self.record_reference(
                    || ReferenceKind::Singleton,
                    &class_singleton.name(),
                    0,
                    class_singleton.location(),
                );
                Type::ClassSingleton {
                    name: type_name_string(&class_singleton.name()),
                }
            }
            Node::InterfaceType(interface) => {
                let arguments = self.convert_types(interface.args());
                self.record_reference(
                    || ReferenceKind::Type(NameKind::Interface),
                    &interface.name(),
                    arguments.len(),
                    interface.location(),
                );
                Type::Interface {
                    name: type_name_string(&interface.name()),
                    arguments,
                }
            }
            Node::IntersectionType(intersection) => {
                Type::Intersection(self.convert_types(intersection.types()))
            }
            Node::UnionType(union) => Type::union(self.convert_types(union.types())),
            Node::OptionalType(optional) => {
                Type::optional(self.convert_type(optional.type_()))
            }
            Node::TupleType(tuple) => Type::Tuple(self.convert_types(tuple.types())),
            Node::RecordType(record) => Type::Record(
                record
                    .all_fields()
                    .iter()
                    .map(|(key, value)| match value {
                        Node::RecordFieldType(field) => RecordField {
                            name: literal_key(&key),
                            field_type: self.convert_type(field.type_()),
                            required: field.required(),
                        },
                        other => RecordField {
                            name: literal_key(&key),
                            field_type: self.convert_type(other),
                            required: true,
                        },
                    })
                    .collect(),
            ),
            Node::ProcType(proc_type) => Type::Proc(Box::new(ProcType {
                function: self.convert_function(proc_type.type_()),
                block: proc_type.block().map(|block| self.convert_block(&block)),
                self_type: proc_type
                    .self_type()
                    .map(|self_type| self.convert_type(self_type)),
            })),
            Node::LiteralType(literal) => Type::Literal(match literal.literal() {
                Node::Integer(integer) => {
                    Literal::Integer(integer.string_representation().as_str().to_string())
                }
                Node::String(string) => Literal::String(string.string().as_str().to_string()),
                Node::Symbol(symbol) => Literal::Symbol(symbol.as_str().to_string()),
                Node::Bool(boolean) if boolean.value() => Literal::True,
                Node::Bool(_) => Literal::False,
                _ => return Type::Untyped,
            }),
            Node::VariableType(variable) => Type::Variable(variable.name().as_str().to_string()),
            _ => Type::Untyped,
        }
    }
}

fn literal_key(node: &Node) -> String {
    match node {
        Node::Symbol(symbol) => symbol.as_str().to_string(),
        Node::String(string) => string.string().as_str().to_string(),
        Node::Integer(integer) => integer.string_representation().as_str().to_string(),
        Node::Bool(boolean) => boolean.value().to_string(),
        _ => String::new(),
    }
}

fn comment_text(comment: Option<CommentNode>) -> Option<String> {
    comment.map(|comment| comment.string().as_str().trim_end().to_string())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AttributeAccess {
    Reader,
    Writer,
    Accessor,
}

struct Attribute {
    name: String,
    attribute_type: Type,
    kind: AttributeKind,
    visibility: AttributeVisibility,
    instance_variable: Option<String>,
    location: Location,
    comment: Option<String>,
    annotations: Vec<String>,
}

impl Attribute {
    fn register(
        self,
        fragment: &mut ClassFragment,
        default_visibility: Visibility,
        access: AttributeAccess,
    ) {
        let kind = attribute_method_kind(self.kind);
        let visibility = match (self.visibility, kind) {
            (AttributeVisibility::Public, _) => Visibility::Public,
            (AttributeVisibility::Private, _) => Visibility::Private,
            (AttributeVisibility::Unspecified, MethodKind::Instance) => default_visibility,
            (AttributeVisibility::Unspecified, MethodKind::Singleton) => Visibility::Public,
        };
        let owner = fragment.name.clone();
        let mut push_method = |name: String, function: Function, is_attribute_reader: bool| {
            fragment.methods.push(MethodFragment {
                definition: MethodDefinition {
                    name,
                    owner: owner.clone(),
                    kind,
                    overloads: vec![MethodType {
                        type_parameters: Vec::new(),
                        function,
                        block: None,
                    }],
                    visibility,
                    alias_of: None,
                    location: Some(self.location.clone()),
                    comment: self.comment.clone(),
                    annotations: self.annotations.clone(),
                    is_attribute_reader,
                },
                overloading: false,
            })
        };
        if matches!(access, AttributeAccess::Reader | AttributeAccess::Accessor) {
            push_method(
                self.name.clone(),
                Function::returning(self.attribute_type.clone()),
                true,
            );
        }
        if matches!(access, AttributeAccess::Writer | AttributeAccess::Accessor) {
            let mut function = Function::returning(self.attribute_type.clone());
            function.required_positionals.push(Parameter {
                parameter_type: self.attribute_type.clone(),
                name: Some(self.name.clone()),
            });
            push_method(format!("{}=", self.name), function, false);
        }
        if let Some(instance_variable) = self.instance_variable {
            let variables = match kind {
                MethodKind::Instance => &mut fragment.instance_variables,
                MethodKind::Singleton => &mut fragment.class_instance_variables,
            };
            variables.push((instance_variable, self.attribute_type));
        }
    }
}

#[derive(Default)]
struct KnownNames {
    classes: HashSet<String>,
    interfaces: HashSet<String>,
    aliases: HashSet<String>,
}

impl KnownNames {
    fn collect(files: &[ParsedFile]) -> KnownNames {
        let mut known_names = KnownNames::default();
        for fragment in files.iter().flat_map(|file| file.fragments.iter()) {
            match &fragment.item {
                FragmentItem::Class(class) => {
                    known_names.classes.insert(class.name.clone());
                }
                FragmentItem::ClassAlias { new_name, .. } => {
                    known_names.classes.insert(new_name.clone());
                }
                FragmentItem::Interface(interface) => {
                    known_names.interfaces.insert(interface.name.clone());
                }
                FragmentItem::TypeAlias(type_alias) => {
                    known_names.aliases.insert(type_alias.name.clone());
                }
                FragmentItem::Constant(_) | FragmentItem::Global(_) => {}
            }
        }
        known_names
    }

    fn resolve(
        &self,
        kind: NameKind,
        written_name: &str,
        context: &[String],
        uses: &[UseClause],
    ) -> String {
        if let Some(absolute) = written_name.strip_prefix("::") {
            return absolute.to_string();
        }
        let known = match kind {
            NameKind::Class => &self.classes,
            NameKind::Interface => &self.interfaces,
            NameKind::Alias => &self.aliases,
        };
        for prefix in context.iter().rev() {
            let candidate = format!("{prefix}::{written_name}");
            if known.contains(&candidate) {
                return candidate;
            }
        }
        if known.contains(written_name) {
            return written_name.to_string();
        }
        let (head, rest) = match written_name.split_once("::") {
            Some((head, rest)) => (head, Some(rest)),
            None => (written_name, None),
        };
        for clause in uses {
            let candidate = match clause {
                UseClause::Single { name, local_name } if local_name == head => match rest {
                    Some(rest) => format!("{name}::{rest}"),
                    None => name.clone(),
                },
                UseClause::Single { .. } => continue,
                UseClause::Wildcard { namespace } => format!("{namespace}::{written_name}"),
            };
            if known.contains(&candidate) {
                return candidate;
            }
        }
        written_name.to_string()
    }

    fn resolve_type(&self, target: &mut Type, context: &[String], uses: &[UseClause]) {
        target.map_names(&mut |kind, name| self.resolve(kind, name, context, uses));
    }

    fn resolve_ancestor(&self, ancestor: &mut Ancestor, context: &[String], uses: &[UseClause]) {
        let kind = if is_interface_name(&ancestor.name) {
            NameKind::Interface
        } else {
            NameKind::Class
        };
        ancestor.name = self.resolve(kind, &ancestor.name, context, uses);
        for argument in &mut ancestor.arguments {
            self.resolve_type(argument, context, uses);
        }
    }

    fn resolve_type_parameters(
        &self,
        type_parameters: &mut [TypeParameter],
        context: &[String],
        uses: &[UseClause],
    ) {
        for type_parameter in type_parameters {
            if let Some(upper_bound) = &mut type_parameter.upper_bound {
                self.resolve_type(upper_bound, context, uses);
            }
            if let Some(default_type) = &mut type_parameter.default_type {
                self.resolve_type(default_type, context, uses);
            }
        }
    }

    fn resolve_method(
        &self,
        method: &mut MethodDefinition,
        context: &[String],
        uses: &[UseClause],
    ) {
        for overload in &mut method.overloads {
            overload.map_names(&mut |kind, name| self.resolve(kind, name, context, uses));
        }
    }

    fn resolve_file(&self, file: &mut ParsedFile) {
        let uses = &file.uses;
        for fragment in &mut file.fragments {
            let context = &fragment.context[..];
            match &mut fragment.item {
                FragmentItem::Class(class) => {
                    let outer_context = &context[..context.len().saturating_sub(1)];
                    if let Some(super_class) = &mut class.super_class {
                        self.resolve_ancestor(super_class, outer_context, uses);
                    }
                    self.resolve_type_parameters(&mut class.type_parameters, context, uses);
                    for ancestor in class
                        .includes
                        .iter_mut()
                        .chain(class.prepends.iter_mut())
                        .chain(class.extends.iter_mut())
                        .chain(class.module_self_types.iter_mut())
                    {
                        self.resolve_ancestor(ancestor, context, uses);
                    }
                    for method in &mut class.methods {
                        self.resolve_method(&mut method.definition, context, uses);
                    }
                    for (_, variable_type) in class
                        .instance_variables
                        .iter_mut()
                        .chain(class.class_instance_variables.iter_mut())
                        .chain(class.class_variables.iter_mut())
                    {
                        self.resolve_type(variable_type, context, uses);
                    }
                }
                FragmentItem::Interface(interface) => {
                    self.resolve_type_parameters(&mut interface.type_parameters, context, uses);
                    for ancestor in &mut interface.includes {
                        self.resolve_ancestor(ancestor, context, uses);
                    }
                    for method in interface.methods.values_mut() {
                        self.resolve_method(method, context, uses);
                    }
                }
                FragmentItem::TypeAlias(type_alias) => {
                    self.resolve_type_parameters(&mut type_alias.type_parameters, context, uses);
                    self.resolve_type(&mut type_alias.aliased_type, context, uses);
                }
                FragmentItem::Constant(constant) => {
                    self.resolve_type(&mut constant.constant_type, context, uses)
                }
                FragmentItem::Global(global) => {
                    self.resolve_type(&mut global.global_type, context, uses)
                }
                FragmentItem::ClassAlias { old_name, .. } => {
                    *old_name = self.resolve(NameKind::Class, old_name, context, uses);
                }
            }
        }
    }
}

pub(crate) fn is_interface_name(name: &str) -> bool {
    name.rsplit("::")
        .next()
        .is_some_and(|last_segment| last_segment.starts_with('_'))
}

struct PendingOverloading {
    class_name: TypeName,
    kind: MethodKind,
    method_name: String,
}

/// Declarations seen so far while merging, to find what RBS rejects but the merge silently combines
struct MergeState<'a> {
    signature_files: &'a HashSet<PathBuf>,
    pending_overloadings: Vec<PendingOverloading>,
    /// The number of type parameters and the location of the first declaration of each class
    first_declarations: HashMap<TypeName, (usize, Location)>,
    /// The first superclass given to each class, which RBS calls the primary declaration
    first_super_classes: HashMap<TypeName, (Ancestor, Location)>,
    diagnostics: Vec<SignatureDiagnostic>,
    duplicated_method_definitions: DuplicatedMethodDefinitionRecorder,
}

/// Every duplicated method, in libraries too, because Steep fails to build the definition of its class wherever it is
#[derive(Default)]
struct DuplicatedMethodDefinitionRecorder {
    definitions: Vec<DuplicatedMethodDefinition>,
    indexes: HashMap<(TypeName, MethodKind, String), usize>,
}

impl DuplicatedMethodDefinitionRecorder {
    fn record(
        &mut self,
        type_name: &TypeName,
        previous: &MethodDefinition,
        current: &MethodDefinition,
    ) {
        let key = (type_name.clone(), current.kind, current.name.clone());
        let definitions = &mut self.definitions;
        let index = *self.indexes.entry(key).or_insert_with(|| {
            definitions.push(DuplicatedMethodDefinition {
                type_name: type_name.clone(),
                kind: current.kind,
                method_name: current.name.clone(),
                locations: previous.location.iter().cloned().collect(),
            });
            definitions.len() - 1
        });
        definitions[index]
            .locations
            .extend(current.location.iter().cloned());
    }
}

impl MergeState<'_> {
    fn new(signature_files: &HashSet<PathBuf>) -> MergeState<'_> {
        MergeState {
            signature_files,
            pending_overloadings: Vec::new(),
            first_declarations: HashMap::new(),
            first_super_classes: HashMap::new(),
            diagnostics: Vec::new(),
            duplicated_method_definitions: DuplicatedMethodDefinitionRecorder::default(),
        }
    }

    fn in_signature_file(&self, location: &Location) -> bool {
        self.signature_files.contains(location.path.as_ref())
    }

    fn report(
        &mut self,
        location: Option<&Location>,
        diagnostic: impl FnOnce(&Location) -> SignatureDiagnostic,
    ) {
        if let Some(location) = location
            && self.in_signature_file(location)
        {
            self.diagnostics.push(diagnostic(location));
        }
    }
}

fn constant_like_name_exists(environment: &Environment, name: &str) -> bool {
    environment.classes.contains_key(name)
        || environment.class_aliases.contains_key(name)
        || environment.constants.contains_key(name)
}

fn merge_fragment(environment: &mut Environment, fragment: Fragment, state: &mut MergeState) {
    match fragment.item {
        FragmentItem::Class(class) => merge_class(environment, *class, state),
        FragmentItem::Interface(interface) => match environment.interfaces.get_mut(&interface.name)
        {
            Some(existing) => {
                state.report(interface.location.as_ref(), |location| {
                    duplicated_declaration(location, &interface.name)
                });
                existing.includes.extend(interface.includes);
                existing.methods.extend(interface.methods);
            }
            None => {
                environment
                    .interfaces
                    .insert(interface.name.clone(), interface);
            }
        },
        FragmentItem::TypeAlias(type_alias) => {
            if environment.type_aliases.contains_key(&type_alias.name) {
                state.report(type_alias.location.as_ref(), |location| {
                    duplicated_declaration(location, &type_alias.name)
                });
            }
            environment
                .type_aliases
                .insert(type_alias.name.clone(), type_alias);
        }
        FragmentItem::Constant(constant) => {
            if constant_like_name_exists(environment, &constant.name) {
                state.report(constant.location.as_ref(), |location| {
                    duplicated_declaration(location, &constant.name)
                });
            }
            environment
                .constants
                .insert(constant.name.clone(), constant);
        }
        FragmentItem::Global(global) => {
            if environment.globals.contains_key(&global.name) {
                state.report(global.location.as_ref(), |location| {
                    duplicated_declaration(location, &global.name)
                });
            }
            environment.globals.insert(global.name.clone(), global);
        }
        FragmentItem::ClassAlias {
            new_name,
            old_name,
            location,
        } => {
            if constant_like_name_exists(environment, &new_name) {
                state.report(Some(&location), |location| {
                    duplicated_declaration(location, &new_name)
                });
            }
            environment.class_aliases.insert(new_name, old_name);
        }
    }
}

/// Reports what `RBS::Environment#insert_rbs_decl`, `RBS::Environment::ClassEntry#validate_type_params` and
/// `RBS::DefinitionBuilder::AncestorBuilder#validate_super_class!` reject
fn validate_class_fragment(
    environment: &Environment,
    class: &ClassFragment,
    state: &mut MergeState,
) {
    if environment.constants.contains_key(&class.name)
        || environment.class_aliases.contains_key(&class.name)
    {
        state.report(Some(&class.location), |location| {
            duplicated_declaration(location, &class.name)
        });
        return;
    }
    let Some(existing) = environment.classes.get(&class.name) else {
        state.first_declarations.insert(
            class.name.clone(),
            (class.type_parameters.len(), class.location.clone()),
        );
        if let Some(super_class) = &class.super_class {
            state.first_super_classes.insert(
                class.name.clone(),
                (super_class.clone(), class.location.clone()),
            );
        }
        return;
    };
    if existing.kind != class.kind {
        state.report(Some(&class.location), |location| {
            duplicated_declaration(location, &class.name)
        });
        return;
    }
    if let Some((first_count, _)) = state.first_declarations.get(&class.name)
        && *first_count != class.type_parameters.len()
    {
        state.report(Some(&class.location), |location| {
            generic_parameter_mismatch(location, &class.name)
        });
    }
    let Some(super_class) = &class.super_class else {
        return;
    };
    match state.first_super_classes.get(&class.name) {
        None => {
            state.first_super_classes.insert(
                class.name.clone(),
                (super_class.clone(), class.location.clone()),
            );
        }
        Some((first_super_class, _)) if first_super_class == super_class => {}
        Some((_, first_location)) => {
            let first_location = first_location.clone();
            state.report(Some(&first_location), |location| {
                superclass_mismatch(location, &class.name)
            });
            state.report(Some(&class.location), |location| {
                superclass_mismatch(location, &class.name)
            });
        }
    }
}

/// A class reopened as `class Array[X]` after `class Array[Elem]` refers to `Elem` by `X`, which RBS's definition
/// builder maps back to the type parameters of the first declaration
fn rename_type_parameters(class: &mut ClassFragment, declared_parameters: &[TypeParameter]) {
    if class.type_parameters.len() != declared_parameters.len() {
        return;
    }
    let variables: HashMap<String, Type> = class
        .type_parameters
        .iter()
        .zip(declared_parameters)
        .filter(|(written, declared)| written.name != declared.name)
        .map(|(written, declared)| (written.name.clone(), Type::Variable(declared.name.clone())))
        .collect();
    if variables.is_empty() {
        return;
    }
    let substitution = Substitution { variables, ..Substitution::default() };
    let rename_ancestor = |ancestor: &mut Ancestor| {
        ancestor.arguments = ancestor.arguments.iter().map(|argument| argument.substitute(&substitution)).collect();
    };
    class.super_class.iter_mut().for_each(rename_ancestor);
    class
        .includes
        .iter_mut()
        .chain(class.prepends.iter_mut())
        .chain(class.extends.iter_mut())
        .chain(class.module_self_types.iter_mut())
        .for_each(rename_ancestor);
    for (_, variable_type) in &mut class.instance_variables {
        *variable_type = variable_type.substitute(&substitution);
    }
    for method in &mut class.methods {
        for overload in &mut method.definition.overloads {
            let mut narrowed = substitution.clone();
            for type_parameter in &overload.type_parameters {
                narrowed.variables.remove(&type_parameter.name);
            }
            *overload = overload.substitute(&narrowed);
        }
    }
    class.type_parameters = declared_parameters.to_vec();
}

fn merge_class(environment: &mut Environment, class: ClassFragment, state: &mut MergeState) {
    validate_class_fragment(environment, &class, state);
    let declaration = environment
        .classes
        .entry(class.name.clone())
        .or_insert_with(|| ClassDeclaration {
            name: class.name.clone(),
            kind: class.kind,
            type_parameters: Vec::new(),
            super_class: None,
            includes: Vec::new(),
            prepends: Vec::new(),
            extends: Vec::new(),
            module_self_types: Vec::new(),
            instance_methods: HashMap::new(),
            singleton_methods: HashMap::new(),
            instance_variables: HashMap::new(),
            class_instance_variables: HashMap::new(),
            class_variables: HashMap::new(),
            locations: Vec::new(),
            comment: None,
            member_declaration_count: 0,
        });
    if class.has_members {
        declaration.member_declaration_count += 1;
    }
    if declaration.type_parameters.is_empty() {
        declaration.type_parameters = class.type_parameters.clone();
    }
    let mut class = class;
    rename_type_parameters(&mut class, &declaration.type_parameters);
    if declaration.super_class.is_none() {
        declaration.super_class = class.super_class;
    }
    declaration.includes.extend(class.includes);
    declaration.prepends.extend(class.prepends);
    declaration.extends.extend(class.extends);
    declaration
        .module_self_types
        .extend(class.module_self_types);
    declaration
        .instance_variables
        .extend(class.instance_variables);
    declaration
        .class_instance_variables
        .extend(class.class_instance_variables);
    declaration.class_variables.extend(class.class_variables);
    declaration.locations.push(class.location);
    if declaration.comment.is_none() {
        declaration.comment = class.comment;
    }
    let pending_overloadings = &mut state.pending_overloadings;
    for MethodFragment {
        definition,
        overloading,
    } in class.methods
    {
        let methods = match definition.kind {
            MethodKind::Instance => &mut declaration.instance_methods,
            MethodKind::Singleton => &mut declaration.singleton_methods,
        };
        if overloading {
            if let Some(existing) = methods.get_mut(&definition.name)
                && existing.alias_of.is_none()
            {
                let previous_overloads =
                    std::mem::replace(&mut existing.overloads, definition.overloads);
                existing.overloads.extend(previous_overloads);
                continue;
            }
            pending_overloadings.push(PendingOverloading {
                class_name: declaration.name.clone(),
                kind: definition.kind,
                method_name: definition.name.clone(),
            });
        } else if let Some(position) = pending_overloadings.iter().position(|pending| {
            pending.class_name == declaration.name
                && pending.kind == definition.kind
                && pending.method_name == definition.name
        }) && let Some(overloading_only) = methods.remove(&definition.name)
        {
            pending_overloadings.remove(position);
            let mut definition = definition;
            let original_overloads =
                std::mem::replace(&mut definition.overloads, overloading_only.overloads);
            definition.overloads.extend(original_overloads);
            methods.insert(definition.name.clone(), definition);
            continue;
        }
        match methods.entry(definition.name.clone()) {
            Entry::Occupied(mut occupied) => {
                let previous = occupied.insert(definition);
                if !overloading {
                    let current = occupied.get();
                    state.duplicated_method_definitions.record(
                        &declaration.name,
                        &previous,
                        current,
                    );
                    state.diagnostics.extend(duplicated_method_definitions(
                        state.signature_files,
                        &declaration.name,
                        &current.name,
                        [previous.location.as_ref(), current.location.as_ref()],
                    ));
                }
            }
            Entry::Vacant(vacant) => {
                vacant.insert(definition);
            }
        }
    }
}

fn fill_default_super_classes(environment: &mut Environment) {
    for declaration in environment.classes.values_mut() {
        if declaration.kind == ClassKind::Class
            && declaration.super_class.is_none()
            && declaration.name != "BasicObject"
        {
            declaration.super_class = Some(Ancestor {
                name: "Object".to_string(),
                arguments: Vec::new(),
            });
        }
    }
}

/// RBS derives `Foo.new` from `Foo#initialize` when the class does not declare `self.new` itself
fn synthesize_new_methods(environment: &mut Environment) {
    for declaration in environment.classes.values_mut() {
        if declaration.kind != ClassKind::Class || declaration.singleton_methods.contains_key("new")
        {
            continue;
        }
        let Some(initialize) = declaration.instance_methods.get("initialize") else {
            continue;
        };
        if initialize.alias_of.is_some() {
            continue;
        }
        let return_type = if declaration.type_parameters.is_empty() {
            Type::Instance
        } else {
            let arguments = declaration
                .type_parameters
                .iter()
                .map(|type_parameter| Type::Variable(type_parameter.name.clone()))
                .collect();
            Type::ClassInstance {
                name: declaration.name.clone(),
                arguments,
            }
        };
        let overloads = initialize
            .overloads
            .iter()
            .map(|overload| MethodType {
                type_parameters: declaration
                    .type_parameters
                    .iter()
                    .chain(&overload.type_parameters)
                    .cloned()
                    .collect(),
                function: Function {
                    return_type: return_type.clone(),
                    ..overload.function.clone()
                },
                block: overload.block.clone(),
            })
            .collect();
        let new_method = MethodDefinition {
            name: "new".to_string(),
            owner: declaration.name.clone(),
            kind: MethodKind::Singleton,
            overloads,
            visibility: Visibility::Public,
            alias_of: None,
            location: initialize.location.clone(),
            comment: initialize.comment.clone(),
            annotations: initialize.annotations.clone(),
            is_attribute_reader: false,
        };
        declaration
            .singleton_methods
            .insert("new".to_string(), new_method);
    }
}

/// `def foo: (A) -> B | ...` in a class that does not define `foo` itself extends the method inherited from an
/// ancestor, which is only known once every declaration is merged
fn apply_inherited_overloadings(
    environment: &mut Environment,
    pending_overloadings: Vec<PendingOverloading>,
    signature_files: &HashSet<PathBuf>,
) -> Vec<SignatureDiagnostic> {
    let mut diagnostics = Vec::new();
    let mut inherited_overloads: Vec<(PendingOverloading, Vec<MethodType>)> = Vec::new();
    for pending in pending_overloadings {
        let chain = match pending.kind {
            MethodKind::Instance => environment
                .instance_lookup_chain(&pending.class_name)
                .into_iter()
                .map(SingletonAncestor::Instance)
                .collect::<Vec<_>>(),
            MethodKind::Singleton => environment.singleton_ancestors(&pending.class_name),
        };
        match environment.find_in_chain(chain.get(1..).unwrap_or_default(), &pending.method_name, 0)
        {
            Some(resolved) => {
                let overloads = resolved.overloads;
                inherited_overloads.push((pending, overloads));
            }
            None => {
                let location = environment
                    .classes
                    .get(&pending.class_name)
                    .and_then(|declaration| match pending.kind {
                        MethodKind::Instance => {
                            declaration.instance_methods.get(&pending.method_name)
                        }
                        MethodKind::Singleton => {
                            declaration.singleton_methods.get(&pending.method_name)
                        }
                    })
                    .and_then(|method| method.location.as_ref())
                    .filter(|location| signature_files.contains(location.path.as_ref()));
                if let Some(location) = location {
                    diagnostics.push(invalid_method_overload(
                        location,
                        &pending.class_name,
                        &pending.method_name,
                    ));
                }
            }
        }
    }
    for (pending, overloads) in inherited_overloads {
        let Some(declaration) = environment.classes.get_mut(&pending.class_name) else {
            continue;
        };
        let methods = match pending.kind {
            MethodKind::Instance => &mut declaration.instance_methods,
            MethodKind::Singleton => &mut declaration.singleton_methods,
        };
        if let Some(method) = methods.get_mut(&pending.method_name) {
            method.overloads.extend(overloads);
        }
    }
    diagnostics
}

/// Parses a single RBS type such as `Array[String]?`. Names are left as written; resolve them with
/// `Environment::resolve_type_names`
pub fn parse_type(source: &str) -> Result<Type, String> {
    // `type t = ...` rejects `void`, `self` and `instance`, while a method return type accepts every type
    let wrapped_source =
        format!("class TypeWrapper\n  def type_wrapper: () -> (\n{source}\n)\nend\n");
    let signature = ruby_rbs::node::parse(&wrapped_source)?;
    let converter = Converter::new(&wrapped_source, Arc::new(PathBuf::new()));
    let declarations: Vec<Node> = signature.declarations().iter().collect();
    let [Node::Class(class)] = declarations.as_slice() else {
        return Err(format!("not a type: {source}"));
    };
    let Some(Node::MethodDefinition(method)) = class.members().iter().next() else {
        return Err(format!("not a type: {source}"));
    };
    let mut overloads = converter.convert_overloads(method.overloads());
    if overloads.len() != 1 || class.members().iter().count() != 1 {
        return Err(format!("not a type: {source}"));
    }
    Ok(overloads.remove(0).function.return_type)
}

/// Parses a single RBS method type such as `[T] (T) { (T) -> void } -> T`. Names are left as written; resolve them
/// with `Environment::resolve_type_names`
pub fn parse_method_type(source: &str) -> Result<MethodType, String> {
    let wrapped_source = format!("class TypeWrapper\n  def type_wrapper: {source}\nend\n");
    let signature = ruby_rbs::node::parse(&wrapped_source)?;
    let converter = Converter::new(&wrapped_source, Arc::new(PathBuf::new()));
    let declarations: Vec<Node> = signature.declarations().iter().collect();
    let [Node::Class(class)] = declarations.as_slice() else {
        return Err(format!("not a method type: {source}"));
    };
    let Some(Node::MethodDefinition(method)) = class.members().iter().next() else {
        return Err(format!("not a method type: {source}"));
    };
    let mut overloads = converter.convert_overloads(method.overloads());
    if overloads.len() != 1 || class.members().iter().count() != 1 {
        return Err(format!("not a method type: {source}"));
    }
    Ok(overloads.remove(0))
}

fn annotation_texts(annotations: NodeList) -> Vec<String> {
    annotations
        .iter()
        .filter_map(|annotation| match annotation {
            Node::Annotation(annotation) => Some(annotation.string().as_str().to_string()),
            _ => None,
        })
        .collect()
}

fn method_annotations(method: &MethodDefinitionNode) -> Vec<String> {
    let mut annotations = annotation_texts(method.annotations());
    for overload in method.overloads().iter() {
        if let Node::MethodDefinitionOverload(overload) = overload {
            annotations.extend(annotation_texts(overload.annotations()));
        }
    }
    annotations
}
