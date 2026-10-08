use std::sync::Arc;

use ruby_prism::Node;

use crate::position::Location;
use crate::prism::bytes_to_string;

pub type EntryId = usize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visibility {
    Public,
    Protected,
    Private,
}

impl Visibility {
    pub fn as_str(self) -> &'static str {
        match self {
            Visibility::Public => "public",
            Visibility::Protected => "protected",
            Visibility::Private => "private",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parameter {
    Required(String),
    Optional(String),
    Keyword(String),
    OptionalKeyword(String),
    Rest(String),
    KeywordRest(String),
    Block(String),
    Forwarding,
}

pub const REST_PARAMETER_DEFAULT_NAME: &str = "<anonymous splat>";
pub const KEYWORD_REST_PARAMETER_DEFAULT_NAME: &str = "<anonymous keyword splat>";
pub const BLOCK_PARAMETER_DEFAULT_NAME: &str = "<anonymous block>";

impl Parameter {
    pub fn name(&self) -> &str {
        match self {
            Parameter::Required(name)
            | Parameter::Optional(name)
            | Parameter::Keyword(name)
            | Parameter::OptionalKeyword(name)
            | Parameter::Rest(name)
            | Parameter::KeywordRest(name)
            | Parameter::Block(name) => name,
            Parameter::Forwarding => "...",
        }
    }

    pub fn decorated_name(&self) -> String {
        match self {
            Parameter::Required(name) => name.clone(),
            Parameter::Optional(name) => format!("{name} = <default>"),
            Parameter::Keyword(name) => format!("{name}:"),
            Parameter::OptionalKeyword(name) => format!("{name}: <default>"),
            Parameter::Rest(name) => format!("*{name}"),
            Parameter::KeywordRest(name) => format!("**{name}"),
            Parameter::Block(name) => format!("&{name}"),
            Parameter::Forwarding => "...".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Signature {
    pub parameters: Vec<Parameter>,
}

impl Signature {
    pub fn new(parameters: Vec<Parameter>) -> Self {
        Signature { parameters }
    }

    pub fn format(&self) -> String {
        self.parameters.iter().map(Parameter::decorated_name).collect::<Vec<_>>().join(", ")
    }

    /// Mirrors `Entry::Signature#matches?`, preferring `true` for situations that cannot be analyzed statically.
    pub fn matches(&self, arguments: &[Node<'_>]) -> bool {
        let mut min_pos: usize = 0;
        let mut max_pos: Option<usize> = Some(0);
        let mut names: Vec<&str> = Vec::new();
        let mut has_forward = false;
        let mut has_keyword_rest = false;

        for parameter in &self.parameters {
            match parameter {
                Parameter::Required(_) => {
                    min_pos += 1;
                    max_pos = max_pos.map(|max| max + 1);
                }
                Parameter::Optional(_) => max_pos = max_pos.map(|max| max + 1),
                Parameter::Rest(_) => max_pos = None,
                Parameter::Forwarding => {
                    max_pos = None;
                    has_forward = true;
                }
                Parameter::Keyword(name) | Parameter::OptionalKeyword(name) => names.push(name),
                Parameter::KeywordRest(_) => has_keyword_rest = true,
                Parameter::Block(_) => {}
            }
        }

        let keyword_hash = arguments.iter().find_map(|argument| argument.as_keyword_hash_node());
        let keyword_args: Option<Vec<Node<'_>>> = keyword_hash.map(|hash| hash.elements().iter().collect());
        let positional_args: Vec<&Node<'_>> =
            arguments.iter().filter(|argument| argument.as_keyword_hash_node().is_none()).collect();
        let forwarding_arguments: Vec<&&Node<'_>> =
            positional_args.iter().filter(|argument| argument.as_forwarding_arguments_node().is_some()).collect();
        let positionals: Vec<&&Node<'_>> =
            positional_args.iter().filter(|argument| argument.as_forwarding_arguments_node().is_none()).collect();

        if has_forward && min_pos == 0 {
            return true;
        }

        (positionals.is_empty() && !forwarding_arguments.is_empty())
            || (positional_arguments_match(
                &positionals,
                !forwarding_arguments.is_empty(),
                keyword_args.as_deref(),
                min_pos,
                max_pos,
            ) && (has_forward || has_keyword_rest || keyword_arguments_match(keyword_args.as_deref(), &names)))
    }
}

fn positional_arguments_match(
    positional_args: &[&&Node<'_>],
    has_forwarding_arguments: bool,
    keyword_args: Option<&[Node<'_>]>,
    min_pos: usize,
    max_pos: Option<usize>,
) -> bool {
    let unaccounted = min_pos as isize - positional_args.len() as isize > 0;
    (min_pos > 0 && positional_args.iter().any(|argument| argument.as_splat_node().is_some()))
        || (unaccounted
            && keyword_args.is_some_and(|args| args.iter().any(|argument| argument.as_assoc_splat_node().is_some())))
        || (unaccounted && has_forwarding_arguments)
        || (min_pos > 0 && max_pos.is_none_or(|max| positional_args.len() <= max))
        || (min_pos == 0 && positional_args.is_empty())
}

fn keyword_arguments_match(arguments: Option<&[Node<'_>]>, names: &[&str]) -> bool {
    let Some(arguments) = arguments else { return true };
    if arguments.iter().any(|argument| argument.as_assoc_splat_node().is_some()) {
        return true;
    }

    arguments
        .iter()
        .filter_map(|argument| argument.as_assoc_node())
        .filter_map(|assoc| {
            let key = assoc.key();
            let symbol = key.as_symbol_node()?;
            symbol.value_loc().map(|location| bytes_to_string(location.as_slice()))
        })
        .all(|name| names.contains(&name.as_str()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModuleOperation {
    Include(String),
    Prepend(String),
}

impl ModuleOperation {
    pub fn module_name(&self) -> &str {
        match self {
            ModuleOperation::Include(name) | ModuleOperation::Prepend(name) => name,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NamespaceKind {
    Module,
    Class,
    SingletonClass,
}

#[derive(Debug, Clone)]
pub struct Namespace {
    pub kind: NamespaceKind,
    pub nesting: Vec<String>,
    pub name_location: Location,
    pub mixin_operations: Vec<ModuleOperation>,
    pub parent_class: Option<String>,
}

impl Namespace {
    pub fn ancestor_hash(&self) -> (Vec<String>, Option<String>) {
        let names = self.mixin_operations.iter().map(|operation| operation.module_name().to_string()).collect();
        match self.kind {
            NamespaceKind::Module => (names, None),
            _ => (names, self.parent_class.clone()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct MethodAliasTarget {
    pub name: String,
    pub signatures: Vec<Signature>,
    pub owner: Option<String>,
    pub kind: MethodAliasTargetKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MethodAliasTargetKind {
    Method,
    Accessor,
    MethodAlias,
}

#[derive(Debug, Clone)]
pub enum EntryKind {
    Namespace(Namespace),
    Constant,
    UnresolvedConstantAlias { target: String, nesting: Vec<String> },
    ConstantAlias { target: String },
    GlobalVariable,
    ClassVariable { owner: Option<String> },
    InstanceVariable { owner: Option<String> },
    Accessor { owner: Option<String> },
    Method { owner: Option<String>, signatures: Vec<Signature>, name_location: Location },
    UnresolvedMethodAlias { new_name: String, old_name: String, owner: Option<String> },
    MethodAlias { target: MethodAliasTarget, owner: Option<String> },
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    pub uri: Arc<str>,
    pub location: Location,
    pub comments: Option<String>,
    pub visibility: Visibility,
    pub kind: EntryKind,
}

impl Entry {
    pub fn new(name: String, uri: Arc<str>, location: Location, comments: Option<String>, kind: EntryKind) -> Self {
        Entry { name, uri, location, comments, visibility: Visibility::Public, kind }
    }

    pub fn name_location(&self) -> Location {
        match &self.kind {
            EntryKind::Namespace(namespace) => namespace.name_location,
            EntryKind::Method { name_location, .. } => *name_location,
            _ => self.location,
        }
    }

    pub fn is_private(&self) -> bool {
        self.visibility == Visibility::Private
    }

    pub fn is_public(&self) -> bool {
        self.visibility == Visibility::Public
    }

    pub fn is_protected(&self) -> bool {
        self.visibility == Visibility::Protected
    }

    pub fn namespace(&self) -> Option<&Namespace> {
        match &self.kind {
            EntryKind::Namespace(namespace) => Some(namespace),
            _ => None,
        }
    }

    pub fn namespace_mut(&mut self) -> Option<&mut Namespace> {
        match &mut self.kind {
            EntryKind::Namespace(namespace) => Some(namespace),
            _ => None,
        }
    }

    pub fn is_namespace(&self) -> bool {
        matches!(self.kind, EntryKind::Namespace(_))
    }

    pub fn is_namespace_kind(&self, kind: NamespaceKind) -> bool {
        matches!(&self.kind, EntryKind::Namespace(namespace) if namespace.kind == kind)
    }

    pub fn is_singleton_class(&self) -> bool {
        self.is_namespace_kind(NamespaceKind::SingletonClass)
    }

    /// Mirrors `Entry::Class`, which `Entry::SingletonClass` inherits from.
    pub fn is_class(&self) -> bool {
        self.is_namespace_kind(NamespaceKind::Class) || self.is_singleton_class()
    }

    pub fn is_module(&self) -> bool {
        self.is_namespace_kind(NamespaceKind::Module)
    }

    pub fn is_constant_like(&self) -> bool {
        matches!(
            self.kind,
            EntryKind::Constant
                | EntryKind::ConstantAlias { .. }
                | EntryKind::Namespace(_)
                | EntryKind::UnresolvedConstantAlias { .. }
        )
    }

    pub fn is_member(&self) -> bool {
        matches!(self.kind, EntryKind::Accessor { .. } | EntryKind::Method { .. })
    }

    pub fn is_method_like(&self) -> bool {
        matches!(self.kind, EntryKind::Accessor { .. } | EntryKind::Method { .. } | EntryKind::MethodAlias { .. })
    }

    pub fn owner(&self) -> Option<&str> {
        match &self.kind {
            EntryKind::ClassVariable { owner }
            | EntryKind::InstanceVariable { owner }
            | EntryKind::Accessor { owner }
            | EntryKind::Method { owner, .. }
            | EntryKind::UnresolvedMethodAlias { owner, .. }
            | EntryKind::MethodAlias { owner, .. } => owner.as_deref(),
            _ => None,
        }
    }

    pub fn signatures(&self) -> Vec<Signature> {
        match &self.kind {
            EntryKind::Accessor { .. } => {
                let mut parameters = Vec::new();
                if let Some(stripped) = self.name.strip_suffix('=') {
                    parameters.push(Parameter::Required(stripped.to_string()));
                }
                vec![Signature::new(parameters)]
            }
            EntryKind::Method { signatures, .. } => signatures.clone(),
            EntryKind::MethodAlias { target, .. } => target.signatures.clone(),
            _ => Vec::new(),
        }
    }

    pub fn decorated_parameters(&self) -> String {
        match self.signatures().first() {
            Some(signature) => format!("({})", signature.format()),
            None => "()".to_string(),
        }
    }

    pub fn formatted_signatures(&self) -> String {
        match self.signatures().len() {
            1 => String::new(),
            2 => "\n(+1 overload)".to_string(),
            count => format!("\n(+{} overloads)", count.saturating_sub(1)),
        }
    }

    pub fn file_path(&self) -> Option<String> {
        crate::uri::to_path(&self.uri)
    }

    pub fn file_name(&self) -> String {
        if crate::uri::scheme(&self.uri) == "untitled" {
            return self.uri.trim_start_matches("untitled:").to_string();
        }
        self.file_path()
            .map(|path| {
                std::path::Path::new(&path).file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or(path)
            })
            .unwrap_or_default()
    }
}
