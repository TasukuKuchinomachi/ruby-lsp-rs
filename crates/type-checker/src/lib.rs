mod annotations;
mod checker;
mod file_index;
mod source;
mod steep_relation;
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

/// Type of the receiver of a method call, e.g. `user` in `user.name` or in the incomplete `user.`
#[derive(Debug, Clone, PartialEq)]
pub struct ReceiverEntry {
    pub start: usize,
    pub end: usize,
    pub receiver_type: Type,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Analysis {
    pub type_hints: Vec<TypeHint>,
    pub diagnostics: Vec<TypeDiagnostic>,
    pub hovers: Vec<HoverEntry>,
    pub definitions: Vec<DefinitionEntry>,
    pub receivers: Vec<ReceiverEntry>,
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

    /// Type of the call receiver spanning exactly `start..end`
    pub fn receiver_type(&self, start: usize, end: usize) -> Option<&Type> {
        self.receivers
            .iter()
            .rev()
            .find(|entry| entry.start == start && entry.end == end)
            .map(|entry| &entry.receiver_type)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AnalysisOptions {
    /// Reports what `steep check` (Steep 2.0.0) reports instead of inferring more than it does: `it` is a method call,
    /// an instance variable without a signature is `untyped`, `proc { }` is a plain `Proc`, `$1` is `String?`, and a
    /// node that needs the definition of a class RBS cannot build is `Ruby::UnexpectedError` and left untyped
    pub steep_compatible: bool,
}

/// Infers types for a whole Ruby file. Must not panic on any input, including files with syntax errors
pub fn analyze(environment: &Environment, source: &[u8]) -> Analysis {
    analyze_with_options(environment, source, AnalysisOptions::default())
}

pub fn analyze_with_options(
    environment: &Environment,
    source: &[u8],
    options: AnalysisOptions,
) -> Analysis {
    // Deeply nested code recurses deeply; a dedicated thread gives a known stack size regardless of the caller's
    // thread, and a panic there is contained instead of taking down the language server
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .name("type-checker".to_string())
            .stack_size(ANALYSIS_STACK_SIZE)
            .spawn_scoped(scope, || {
                analyze_on_current_thread(environment, source, options)
            })
            .ok()
            .and_then(|handle| handle.join().ok())
            .unwrap_or_default()
    })
}

const ANALYSIS_STACK_SIZE: usize = 128 * 1024 * 1024;

fn analyze_on_current_thread(environment: &Environment, source: &[u8], options: AnalysisOptions) -> Analysis {
    let parse_result = ruby_prism::parse(source);
    let root = parse_result.node();
    let line_index = source::LineIndex::new(source);
    let file_index = file_index::FileIndex::build(environment, &root, &line_index);
    let annotations = annotations::Annotations::collect(&parse_result, &line_index);
    // Steep's `TypeCheckService` gives up on a file whose annotation does not parse and reports nothing else
    if let Some(error) = &annotations.syntax_error {
        return Analysis {
            diagnostics: vec![TypeDiagnostic {
                start: error.start,
                end: error.end,
                severity: Severity::Error,
                code: "Ruby::AnnotationSyntaxError",
                message: error.message.clone(),
            }],
            ..Analysis::default()
        };
    }
    let mut checker =
        checker::Checker::new(environment, file_index, line_index, annotations, options);
    checker.check_program(&root);
    checker.analysis
}
