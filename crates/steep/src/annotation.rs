use std::sync::LazyLock;

use rbs_definition::{
    ast::{self, TypeName},
    collections::IndexMap,
    environment::{TypeNameResolver, map_method_type_names, map_type_names},
    parse::{self, ParsingError},
};
use regex::Regex;
use steep_interface::{factory::Factory, method_type::MethodType, types::Type};

use crate::ruby::node::SourceRange;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DynamicKind {
    Instance,
    Module,
    ModuleInstance,
}

#[derive(Clone, Debug)]
pub enum AnnotationKind {
    VarType {
        name: String,
        type_: ast::Type,
    },
    MethodType {
        name: String,
        type_: ast::MethodType,
    },
    ConstType {
        name: TypeName,
        type_: ast::Type,
    },
    IvarType {
        name: String,
        type_: ast::Type,
    },
    ReturnType(ast::Type),
    BlockType(ast::Type),
    SelfType(ast::Type),
    InstanceType(ast::Type),
    ModuleType(ast::Type),
    BreakType(ast::Type),
    Dynamic(Vec<(String, DynamicKind)>),
    Implements {
        name: TypeName,
        args: Vec<String>,
    },
}

#[derive(Clone, Debug)]
pub struct Annotation {
    pub kind: AnnotationKind,
    pub line: usize,
    pub location: SourceRange,
}

#[derive(Debug)]
pub struct AnnotationSyntaxError {
    pub message: String,
    pub location: SourceRange,
}

#[derive(Debug)]
pub enum AnnotationError {
    Syntax(AnnotationSyntaxError),
    Unexpected {
        message: String,
        location: SourceRange,
    },
}

static VAR_TYPE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"@type\s+var\s+(?<name>[a-z][A-Za-z0-9_]*)\s*:\s*(?<type>.*)").unwrap()
});
static METHOD_TYPE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"@type\s+method\s+(?<name>[^.:\s]+)\s*:\s*(?<type>.*)").unwrap());
static CONST_TYPE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"@type\s+const\s+(?<name>(::)?([A-Z][A-Za-z0-9_]*::)*[A-Z][A-Za-z0-9_]*)\s*:\s*(?<type>.*)",
    )
    .unwrap()
});
static IVAR_TYPE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"@type\s+ivar\s+(?<name>@[^:\s]+)\s*:\s*(?<type>.*)").unwrap());
fn keyword_and_type(keyword: &str) -> Regex {
    Regex::new(&format!(r"@type\s+{keyword}\s*:\s*(?<type>.*)")).unwrap()
}
static RETURN_TYPE: LazyLock<Regex> = LazyLock::new(|| keyword_and_type("return"));
static BLOCK_TYPE: LazyLock<Regex> = LazyLock::new(|| keyword_and_type("block"));
static SELF_TYPE: LazyLock<Regex> = LazyLock::new(|| keyword_and_type("self"));
static INSTANCE_TYPE: LazyLock<Regex> = LazyLock::new(|| keyword_and_type("instance"));
static MODULE_TYPE: LazyLock<Regex> = LazyLock::new(|| keyword_and_type("module"));
static BREAK_TYPE: LazyLock<Regex> = LazyLock::new(|| keyword_and_type("break"));
static DYNAMIC: LazyLock<Regex> = LazyLock::new(|| {
    let name = r"(self\??\.)?[^.:\s]+";
    Regex::new(&format!(r"@dynamic\s+(?<names>({name}\s*,\s*)*{name})")).unwrap()
});
static ANNOTATION_KEYWORD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\A@(type|implements|dynamic)\b").unwrap());

pub fn looks_like_annotation(source: &str) -> bool {
    ANNOTATION_KEYWORD.is_match(source)
}

static IMPLEMENTS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"@implements\s+(?<name>(::)?([A-Z][A-Za-z0-9_]*::)*[A-Z][A-Za-z0-9_]*)(\[(?<params>[A-Z][A-Za-z0-9_]*(,\s*[A-Z][A-Za-z0-9_]*)*)\])?$",
    )
    .unwrap()
});

fn parser_syntax_error_message(error: &ParsingError) -> String {
    let source = error.location.source();
    let source = if source.is_empty() {
        String::new()
    } else {
        format!(" ({source})")
    };
    format!(
        "Syntax error caused by token `{}`{source}",
        error.token_type
    )
}

const FAILED_TO_PARSE_TYPE: &str = "Failed to parse a type in annotation";

fn strip(source: &str) -> &str {
    source.trim_matches(|character| {
        matches!(character, ' ' | '\t' | '\n' | '\x0b' | '\x0c' | '\r' | '\0')
    })
}

fn parse_type_at(
    captures: &regex::Captures,
    location: SourceRange,
) -> Result<ast::Type, AnnotationError> {
    let matched = captures.name("type").expect("the pattern has a type group");
    let source = matched.as_str();
    let syntax_error = |message| {
        AnnotationError::Syntax(AnnotationSyntaxError {
            message,
            location: SourceRange::new(
                location.begin_pos + matched.start(),
                location.begin_pos + matched.end(),
            ),
        })
    };
    match parse::parse_type(source) {
        Err(error) => Err(syntax_error(parser_syntax_error_message(&error))),
        Ok(Some(parsed)) if parsed.location.source() == strip(source) => Ok(parsed.type_),
        Ok(_) => Err(syntax_error(FAILED_TO_PARSE_TYPE.to_string())),
    }
}

fn parse_method_type_at(
    captures: &regex::Captures,
    location: SourceRange,
) -> Result<ast::MethodType, AnnotationError> {
    let source = captures
        .name("type")
        .expect("the pattern has a type group")
        .as_str();
    match parse::parse_method_type(source) {
        Ok(Some(method_type)) => Ok(method_type),
        Err(error) => Err(AnnotationError::Syntax(AnnotationSyntaxError {
            message: parser_syntax_error_message(&error),
            location,
        })),
        Ok(None) => Err(AnnotationError::Unexpected {
            message: format!(
                "`RBS::Parser.parse_method_type` returned nil for the method type annotation `{source}`"
            ),
            location,
        }),
    }
}

pub fn parse(
    source: &str,
    location: SourceRange,
    line: usize,
) -> Result<Option<Annotation>, AnnotationError> {
    let kind = if let Some(captures) = VAR_TYPE.captures(source) {
        AnnotationKind::VarType {
            name: captures["name"].to_string(),
            type_: parse_type_at(&captures, location)?,
        }
    } else if let Some(captures) = METHOD_TYPE.captures(source) {
        AnnotationKind::MethodType {
            name: captures["name"].to_string(),
            type_: parse_method_type_at(&captures, location)?,
        }
    } else if let Some(captures) = CONST_TYPE.captures(source) {
        AnnotationKind::ConstType {
            name: TypeName::parse(&captures["name"]),
            type_: parse_type_at(&captures, location)?,
        }
    } else if let Some(captures) = IVAR_TYPE.captures(source) {
        AnnotationKind::IvarType {
            name: captures["name"].to_string(),
            type_: parse_type_at(&captures, location)?,
        }
    } else if let Some(captures) = RETURN_TYPE.captures(source) {
        AnnotationKind::ReturnType(parse_type_at(&captures, location)?)
    } else if let Some(captures) = BLOCK_TYPE.captures(source) {
        AnnotationKind::BlockType(parse_type_at(&captures, location)?)
    } else if let Some(captures) = SELF_TYPE.captures(source) {
        AnnotationKind::SelfType(parse_type_at(&captures, location)?)
    } else if let Some(captures) = INSTANCE_TYPE.captures(source) {
        AnnotationKind::InstanceType(parse_type_at(&captures, location)?)
    } else if let Some(captures) = MODULE_TYPE.captures(source) {
        AnnotationKind::ModuleType(parse_type_at(&captures, location)?)
    } else if let Some(captures) = BREAK_TYPE.captures(source) {
        AnnotationKind::BreakType(parse_type_at(&captures, location)?)
    } else if let Some(captures) = DYNAMIC.captures(source) {
        let names = Regex::new(r"\s*,\s*")
            .unwrap()
            .split(&captures["names"])
            .map(|name| {
                if let Some(name) = name.strip_prefix("self.") {
                    (name.to_string(), DynamicKind::Module)
                } else if let Some(name) = name.strip_prefix("self?.") {
                    (name.to_string(), DynamicKind::ModuleInstance)
                } else {
                    (name.to_string(), DynamicKind::Instance)
                }
            })
            .collect();
        AnnotationKind::Dynamic(names)
    } else if let Some(captures) = IMPLEMENTS.captures(source) {
        AnnotationKind::Implements {
            name: TypeName::parse(&captures["name"]),
            args: captures
                .name("params")
                .map(|params| {
                    Regex::new(r"\s*,\s*")
                        .unwrap()
                        .split(params.as_str())
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
        }
    } else {
        return Ok(None);
    };
    Ok(Some(Annotation {
        kind,
        line,
        location,
    }))
}

#[derive(Clone, Default)]
pub struct Collection {
    pub annotations: Vec<Annotation>,
    pub var_types: IndexMap<String, Type>,
    pub method_types: IndexMap<String, MethodType>,
    pub const_types: IndexMap<TypeName, Type>,
    pub ivar_types: IndexMap<String, Type>,
    pub block_type: Option<Type>,
    pub return_type: Option<Type>,
    pub self_type: Option<Type>,
    pub instance_type: Option<Type>,
    pub module_type: Option<Type>,
    pub break_type: Option<Type>,
    pub implement_module: Option<(TypeName, Vec<String>)>,
    pub dynamics: Vec<(String, DynamicKind)>,
}

impl Collection {
    pub fn new(
        annotations: Vec<Annotation>,
        factory: &Factory,
        resolver: &TypeNameResolver,
        context: &[TypeName],
    ) -> Collection {
        let absolute = |type_: &ast::Type| {
            factory.type_(&map_type_names(type_, &mut |type_name| {
                resolver
                    .resolve(type_name, context)
                    .unwrap_or_else(|| type_name.absolute())
            }))
        };
        let mut collection = Collection::default();
        for annotation in &annotations {
            match &annotation.kind {
                AnnotationKind::VarType { name, type_ } => {
                    collection.var_types.insert(name.clone(), absolute(type_));
                }
                AnnotationKind::MethodType { name, type_ } => {
                    let resolved = map_method_type_names(type_, &mut |type_name| {
                        resolver
                            .resolve(type_name, context)
                            .unwrap_or_else(|| type_name.absolute())
                    });
                    collection
                        .method_types
                        .insert(name.clone(), factory.method_type(&resolved));
                }
                AnnotationKind::ConstType { name, type_ } => {
                    collection.const_types.insert(name.clone(), absolute(type_));
                }
                AnnotationKind::IvarType { name, type_ } => {
                    collection.ivar_types.insert(name.clone(), absolute(type_));
                }
                AnnotationKind::ReturnType(type_) => collection.return_type = Some(absolute(type_)),
                AnnotationKind::BlockType(type_) => collection.block_type = Some(absolute(type_)),
                AnnotationKind::SelfType(type_) => collection.self_type = Some(absolute(type_)),
                AnnotationKind::InstanceType(type_) => {
                    collection.instance_type = Some(absolute(type_))
                }
                AnnotationKind::ModuleType(type_) => collection.module_type = Some(absolute(type_)),
                AnnotationKind::BreakType(type_) => collection.break_type = Some(absolute(type_)),
                AnnotationKind::Dynamic(names) => collection.dynamics.extend(names.iter().cloned()),
                AnnotationKind::Implements { name, args } => {
                    collection.implement_module = Some((name.clone(), args.clone()))
                }
            }
        }
        collection.annotations = annotations;
        collection
    }

    pub fn instance_dynamics(&self) -> Vec<String> {
        self.dynamics
            .iter()
            .filter(|(_, kind)| *kind != DynamicKind::Module)
            .map(|(name, _)| name.clone())
            .collect()
    }

    pub fn module_dynamics(&self) -> Vec<String> {
        self.dynamics
            .iter()
            .filter(|(_, kind)| *kind != DynamicKind::Instance)
            .map(|(name, _)| name.clone())
            .collect()
    }
}
