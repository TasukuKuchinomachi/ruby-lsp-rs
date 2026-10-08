mod annotations;
mod checker;
mod file_index;
mod source;
mod subtyping;
mod type_utilities;
mod unification;

use rbs_environment::{Environment, Location, MethodType, Type, TypeName};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TypeHintKind {
    /// `def foo(x) → Integer`, placed after the parameters (or the name when there are none)
    MethodReturnType,
    /// `attr_reader ‹Integer› :count`, placed before each symbol
    AttributeType,
    /// `count‹: Integer› = ...` for local and instance variable writes, placed after the name
    VariableType,
    /// `each { |item‹: String›| }`, placed after each block parameter
    BlockParameterType,
    /// `def greet(target‹: String›, *names‹: Array[String]›)`, placed after the name
    MethodParameterType,
    /// `def greet(name: ‹String›)`, placed after the colon, since a type before it would read as part of the name
    KeywordParameterType,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypeHint {
    pub kind: TypeHintKind,
    /// Byte offset in the source where the hint is shown
    pub offset: usize,
    pub hint_type: Type,
    /// True when the type is evident from the right-hand side, e.g. `x = 1` or `x = Foo.new`
    pub is_obvious: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Information,
    Hint,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypeDiagnostic {
    pub start: usize,
    pub end: usize,
    pub severity: Severity,
    /// Steep compatible code, e.g. `Ruby::NoMethod`
    pub code: &'static str,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum HoverKind {
    /// Type of a variable or an expression
    Type(Type),
    Method {
        receiver_type: Type,
        owner: TypeName,
        method_name: String,
        singleton: bool,
        overloads: Vec<MethodType>,
        return_type: Type,
        comment: Option<String>,
    },
    Constant {
        name: TypeName,
        constant_type: Type,
        comment: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct HoverEntry {
    pub start: usize,
    pub end: usize,
    pub kind: HoverKind,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DefinitionEntry {
    pub start: usize,
    pub end: usize,
    pub targets: Vec<Location>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Analysis {
    pub type_hints: Vec<TypeHint>,
    pub diagnostics: Vec<TypeDiagnostic>,
    pub hovers: Vec<HoverEntry>,
    pub definitions: Vec<DefinitionEntry>,
}

impl Analysis {
    /// The innermost hover entry covering `offset`
    pub fn hover_at(&self, offset: usize) -> Option<&HoverEntry> {
        self.hovers
            .iter()
            .filter(|entry| entry.start <= offset && offset < entry.end)
            .min_by_key(|entry| entry.end - entry.start)
    }

    pub fn definition_at(&self, offset: usize) -> Option<&DefinitionEntry> {
        self.definitions
            .iter()
            .filter(|entry| entry.start <= offset && offset < entry.end)
            .min_by_key(|entry| entry.end - entry.start)
    }
}

/// Infers types for a whole Ruby file. Must not panic on any input, including files with syntax errors
pub fn analyze(environment: &Environment, source: &[u8]) -> Analysis {
    // Deeply nested code recurses deeply; a dedicated thread gives a known stack size regardless of the caller's
    // thread, and a panic there is contained instead of taking down the language server
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .name("type-checker".to_string())
            .stack_size(ANALYSIS_STACK_SIZE)
            .spawn_scoped(scope, || analyze_on_current_thread(environment, source))
            .ok()
            .and_then(|handle| handle.join().ok())
            .unwrap_or_default()
    })
}

const ANALYSIS_STACK_SIZE: usize = 128 * 1024 * 1024;

fn analyze_on_current_thread(environment: &Environment, source: &[u8]) -> Analysis {
    let parse_result = ruby_prism::parse(source);
    let root = parse_result.node();
    let file_index = file_index::FileIndex::build(environment, &root);
    let line_index = source::LineIndex::new(source);
    let annotations = annotations::Annotations::collect(&parse_result, &line_index);
    let mut checker = checker::Checker::new(environment, file_index, line_index, annotations);
    checker.check_program(&root);
    checker.analysis
}
