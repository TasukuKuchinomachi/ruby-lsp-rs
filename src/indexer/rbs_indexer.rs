use std::path::{Path, PathBuf};
use std::sync::Arc;

use ruby_rbs::node::{
    AliasNode, ClassNode, ConstantNode, FunctionTypeNode, GlobalNode, MethodDefinitionKind, MethodDefinitionNode,
    MethodDefinitionVisibility, ModuleNode, Node, RBSLocationRange, TypeNameNode,
};

use super::entry::{
    BLOCK_PARAMETER_DEFAULT_NAME, Entry, EntryId, EntryKind, KEYWORD_REST_PARAMETER_DEFAULT_NAME, ModuleOperation,
    Namespace, NamespaceKind, Parameter, REST_PARAMETER_DEFAULT_NAME, Signature, Visibility,
};
use super::index::Index;
use crate::position::{LineIndex, Location};

/// Mirrors `RubyIndexer::RBSIndexer`, indexing declarations from RBS signature files
pub struct RbsIndexer<'index> {
    index: &'index mut Index,
}

struct SignatureFile<'source> {
    uri: Arc<str>,
    line_index: LineIndex<'source>,
}

impl SignatureFile<'_> {
    fn location(&self, range: &RBSLocationRange) -> Location {
        self.line_index.location_of(range.start().max(0) as usize, range.end().max(0) as usize)
    }
}

/// Mirrors `RBS::FileFinder.each_file` with `skip_hidden: true`, which is how the core signatures are loaded
pub fn core_signature_paths(directory: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = walkdir::WalkDir::new(directory)
        .into_iter()
        .filter_entry(|entry| entry.depth() == 0 || !entry.file_type().is_dir() || !entry.file_name().to_string_lossy().starts_with('_'))
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file() && entry.path().extension().is_some_and(|extension| extension == "rbs"))
        .map(|entry| entry.into_path())
        .collect();
    paths.sort_by(|left, right| left.to_string_lossy().cmp(&right.to_string_lossy()));
    paths
}

fn symbol_name(type_name: &TypeNameNode<'_>) -> String {
    type_name.name().as_str().to_string()
}

fn comment_string(comment: Option<ruby_rbs::node::CommentNode<'_>>) -> Option<String> {
    comment.map(|comment| comment.string().as_str().to_string())
}

impl<'index> RbsIndexer<'index> {
    pub fn new(index: &'index mut Index) -> Self {
        RbsIndexer { index }
    }

    /// Mirrors `RBSIndexer#index_ruby_core`, given the `core` directory of the RBS gem
    pub fn index_core_directory(&mut self, directory: &Path) {
        for path in core_signature_paths(directory) {
            let Ok(source) = std::fs::read_to_string(&path) else { continue };
            self.process_signature(&path.to_string_lossy(), &source);
        }
    }

    /// Mirrors `RBSIndexer#process_signature`. Files that RBS fails to parse are skipped
    pub fn process_signature(&mut self, path: &str, source: &str) {
        let Ok(signature) = ruby_rbs::node::parse(source) else { return };
        let file = SignatureFile {
            uri: Arc::from(crate::uri::from_path(path)),
            line_index: LineIndex::new(source.as_bytes(), super::index::indexer_column_encoding(self.index.configuration.encoding)),
        };

        for declaration in signature.declarations().iter() {
            self.process_declaration(&file, declaration);
        }
    }

    fn process_declaration(&mut self, file: &SignatureFile<'_>, declaration: Node<'_>) {
        match declaration {
            Node::Class(class) => self.handle_class(file, &class),
            Node::Module(module) => self.handle_module(file, &module),
            Node::Constant(constant) => {
                let nesting: Vec<String> = constant
                    .name()
                    .namespace()
                    .path()
                    .iter()
                    .filter_map(|part| match part {
                        Node::Symbol(symbol) => Some(symbol.as_str().to_string()),
                        _ => None,
                    })
                    .collect();
                self.handle_constant(file, &constant, &nesting);
            }
            Node::Global(global) => self.handle_global_variable(file, &global),
            _ => {}
        }
    }

    fn handle_class(&mut self, file: &SignatureFile<'_>, declaration: &ClassNode<'_>) {
        let parent_class = declaration.super_class().map(|super_class| symbol_name(&super_class.name()));
        self.handle_class_or_module(
            file,
            symbol_name(&declaration.name()),
            file.location(&declaration.location()),
            comment_string(declaration.comment()),
            NamespaceKind::Class,
            parent_class,
            declaration.members().iter().collect(),
        );
    }

    fn handle_module(&mut self, file: &SignatureFile<'_>, declaration: &ModuleNode<'_>) {
        self.handle_class_or_module(
            file,
            symbol_name(&declaration.name()),
            file.location(&declaration.location()),
            comment_string(declaration.comment()),
            NamespaceKind::Module,
            None,
            declaration.members().iter().collect(),
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn handle_class_or_module(
        &mut self,
        file: &SignatureFile<'_>,
        name: String,
        location: Location,
        comments: Option<String>,
        kind: NamespaceKind,
        parent_class: Option<String>,
        members: Vec<Node<'_>>,
    ) {
        let nesting = vec![name.clone()];
        let mut mixin_operations = Vec::new();
        let mut extended_modules = Vec::new();
        for member in &members {
            match member {
                Node::Include(include) => mixin_operations.push(ModuleOperation::Include(symbol_name(&include.name()))),
                Node::Prepend(prepend) => mixin_operations.push(ModuleOperation::Prepend(symbol_name(&prepend.name()))),
                Node::Extend(extend) => extended_modules.push(symbol_name(&extend.name())),
                _ => {}
            }
        }

        let entry = Entry::new(
            name.clone(),
            file.uri.clone(),
            location,
            comments,
            EntryKind::Namespace(Namespace {
                kind,
                nesting: nesting.clone(),
                name_location: location,
                mixin_operations,
                parent_class,
            }),
        );
        let owner = self.index.add(entry, false);

        if !extended_modules.is_empty() {
            let singleton = self.index.existing_or_new_singleton_class(&name);
            if let Some(namespace) = self.index.entry_mut(singleton).namespace_mut() {
                namespace.mixin_operations.extend(extended_modules.into_iter().map(ModuleOperation::Include));
            }
        }

        for member in members {
            match member {
                Node::MethodDefinition(method) => self.handle_method(file, &method, owner),
                Node::Constant(constant) => self.handle_constant(file, &constant, &nesting),
                Node::Alias(alias) => self.handle_signature_alias(file, &alias, owner),
                _ => {}
            }
        }
    }

    fn handle_method(&mut self, file: &SignatureFile<'_>, member: &MethodDefinitionNode<'_>, owner: EntryId) {
        let name = member.name().as_str().to_string();
        let location = file.location(&member.location());
        let comments = comment_string(member.comment());
        let owner_name = self.index.entry(owner).name.clone();
        let real_owner = match member.kind() {
            MethodDefinitionKind::Singleton | MethodDefinitionKind::SingletonInstance => {
                let singleton = self.index.existing_or_new_singleton_class(&owner_name);
                self.index.entry(singleton).name.clone()
            }
            MethodDefinitionKind::Instance => owner_name,
        };
        let visibility = match member.visibility() {
            MethodDefinitionVisibility::Private => Visibility::Private,
            MethodDefinitionVisibility::Public | MethodDefinitionVisibility::Unspecified => Visibility::Public,
        };
        let signatures = member.overloads().iter().map(|overload| Signature::new(process_overload(overload))).collect();

        let mut entry = Entry::new(
            name,
            file.uri.clone(),
            location,
            comments,
            EntryKind::Method { owner: Some(real_owner), signatures, name_location: location },
        );
        entry.visibility = visibility;
        self.index.add(entry, false);
    }

    fn handle_constant(&mut self, file: &SignatureFile<'_>, declaration: &ConstantNode<'_>, nesting: &[String]) {
        let mut parts = nesting.to_vec();
        parts.push(symbol_name(&declaration.name()));
        let entry = Entry::new(
            parts.join("::"),
            file.uri.clone(),
            file.location(&declaration.location()),
            comment_string(declaration.comment()),
            EntryKind::Constant,
        );
        self.index.add(entry, false);
    }

    fn handle_global_variable(&mut self, file: &SignatureFile<'_>, declaration: &GlobalNode<'_>) {
        let entry = Entry::new(
            declaration.name().as_str().to_string(),
            file.uri.clone(),
            file.location(&declaration.location()),
            comment_string(declaration.comment()),
            EntryKind::GlobalVariable,
        );
        self.index.add(entry, false);
    }

    fn handle_signature_alias(&mut self, file: &SignatureFile<'_>, member: &AliasNode<'_>, owner: EntryId) {
        let new_name = member.new_name().as_str().to_string();
        let owner_name = self.index.entry(owner).name.clone();
        let entry = Entry::new(
            new_name.clone(),
            file.uri.clone(),
            file.location(&member.location()),
            comment_string(member.comment()),
            EntryKind::UnresolvedMethodAlias {
                new_name,
                old_name: member.old_name().as_str().to_string(),
                owner: Some(owner_name),
            },
        );
        self.index.add(entry, false);
    }
}

fn process_overload(overload: Node<'_>) -> Vec<Parameter> {
    let Node::MethodDefinitionOverload(overload) = overload else { return Vec::new() };
    let Node::MethodType(method_type) = overload.method_type() else { return Vec::new() };

    match method_type.type_() {
        Node::FunctionType(function) => {
            let mut parameters = parse_arguments(&function);
            if method_type.block().is_some_and(|block| block.required()) {
                parameters.push(Parameter::Block(BLOCK_PARAMETER_DEFAULT_NAME.to_string()));
            }
            parameters
        }
        Node::UntypedFunctionType(_) => vec![Parameter::Forwarding],
        _ => Vec::new(),
    }
}

fn function_parameter_name(parameter: &Node<'_>) -> Option<String> {
    match parameter {
        Node::FunctionParam(parameter) => parameter.name().map(|name| name.as_str().to_string()),
        _ => None,
    }
}

fn keyword_name(key: &Node<'_>) -> String {
    match key {
        Node::Symbol(symbol) => symbol.as_str().to_string(),
        _ => String::new(),
    }
}

fn parse_arguments(function: &FunctionTypeNode<'_>) -> Vec<Parameter> {
    let mut parameters = Vec::new();
    let mut argument_offset = 0;

    for (index, parameter) in function.required_positionals().iter().enumerate() {
        let name = function_parameter_name(&parameter).unwrap_or_else(|| format!("arg{index}"));
        argument_offset += 1;
        parameters.push(Parameter::Required(name));
    }

    for (index, parameter) in function.optional_positionals().iter().enumerate() {
        let name = function_parameter_name(&parameter).unwrap_or_else(|| format!("arg{}", index + argument_offset));
        parameters.push(Parameter::Optional(name));
    }

    for parameter in function.trailing_positionals().iter() {
        parameters.push(Parameter::Optional(function_parameter_name(&parameter).unwrap_or_default()));
    }

    if let Some(rest) = function.rest_positionals() {
        let name = function_parameter_name(&rest).unwrap_or_else(|| REST_PARAMETER_DEFAULT_NAME.to_string());
        parameters.push(Parameter::Rest(name));
    }

    for (key, _) in function.required_keywords().iter() {
        parameters.push(Parameter::Keyword(keyword_name(&key)));
    }

    for (key, _) in function.optional_keywords().iter() {
        parameters.push(Parameter::OptionalKeyword(keyword_name(&key)));
    }

    if let Some(rest) = function.rest_keywords() {
        let name = function_parameter_name(&rest).unwrap_or_else(|| KEYWORD_REST_PARAMETER_DEFAULT_NAME.to_string());
        parameters.push(Parameter::KeywordRest(name));
    }

    parameters
}
