use std::{env, fs, process::ExitCode};

use steep::ruby::{
    node::{Buffer, Child, Location, MapKind, Node, SourceRange},
    parse,
    translation::inspect_float,
};
use steep::steep_version::SteepVersion;

const MAP_FIELDS: [&str; 18] = [
    "expression",
    "keyword",
    "name",
    "selector",
    "operator",
    "begin",
    "end",
    "dot",
    "else",
    "assignment",
    "question",
    "colon",
    "double_colon",
    "in",
    "assoc",
    "argument",
    "heredoc_body",
    "heredoc_end",
];

fn escape(text: &str) -> String {
    let mut escaped = String::new();
    for character in text.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\u{0}'..='\u{1f}' | '\u{7f}' => {
                escaped.push_str(&format!("\\x{:02X}", character as u32))
            }
            _ => escaped.push(character),
        }
    }
    escaped
}

fn range(buffer: &Buffer, range: Option<SourceRange>) -> String {
    match range {
        None => "nil".to_string(),
        Some(range) => format!(
            "{}:{}-{}:{}",
            buffer.line(range.begin_pos),
            buffer.column(range.begin_pos),
            buffer.line(range.end_pos),
            buffer.column(range.end_pos),
        ),
    }
}

fn field(location: &Location, name: &str) -> Option<SourceRange> {
    match name {
        "expression" => location.expression,
        "keyword" => location.keyword,
        "name" => location.name,
        "selector" => location.selector,
        "operator" => location.operator,
        "begin" => location.begin,
        "end" => location.end,
        "dot" => location.dot,
        "else" => location.else_,
        "assignment" => location.assignment,
        "question" => location.question,
        "colon" => location.colon,
        "double_colon" => location.double_colon,
        "in" => location.in_,
        "assoc" => location.assoc,
        "argument" => location.argument,
        "heredoc_body" => location.heredoc_body,
        "heredoc_end" => location.heredoc_end,
        _ => None,
    }
}

fn map(buffer: &Buffer, location: &Location) -> String {
    if location.kind == MapKind::Map && location.expression.is_none() {
        return "nil".to_string();
    }
    let mut parts = vec![format!("{:?}", location.kind)];
    for name in MAP_FIELDS {
        if location.responds_to(name) {
            parts.push(format!("{name}={}", range(buffer, field(location, name))));
        }
    }
    parts.join(" ")
}

fn child(buffer: &Buffer, value: &Child, indent: &str, output: &mut Vec<String>) {
    match value {
        Child::Node(value) => node(buffer, value, indent, output),
        Child::Nil => output.push(format!("{indent}nil")),
        Child::Symbol(value) => output.push(format!("{indent}:\"{}\"", escape(value))),
        Child::String(value) => output.push(format!("{indent}\"{}\"", escape(value))),
        Child::Integer(value) => output.push(format!("{indent}integer {value}")),
        Child::Float(value) => output.push(format!("{indent}float {}", inspect_float(*value))),
        Child::Rational(value) => output.push(format!("{indent}rational {value}")),
        Child::Complex(value) => output.push(format!("{indent}complex {value}")),
    }
}

fn node(buffer: &Buffer, node: &Node, indent: &str, output: &mut Vec<String>) {
    output.push(format!(
        "{indent}({} {}",
        node.type_,
        map(buffer, &node.location)
    ));
    let indent = format!("{indent}  ");
    for value in &node.children {
        child(buffer, value, &indent, output);
    }
}

fn main() -> ExitCode {
    let mut steep_version = SteepVersion::default();
    let mut paths = Vec::new();
    for argument in env::args().skip(1) {
        match argument.strip_prefix("--steep-version=") {
            Some(name) => match SteepVersion::parse(name) {
                Some(version) => steep_version = version,
                None => {
                    eprintln!(
                        "unsupported Steep version {name}: choose one of {}",
                        SteepVersion::supported_names()
                    );
                    return ExitCode::FAILURE;
                }
            },
            None => paths.push(argument),
        }
    }
    for path in paths {
        let source = match fs::read_to_string(&path) {
            Ok(source) => source,
            Err(error) => {
                eprintln!("{path}: {error}");
                return ExitCode::FAILURE;
            }
        };
        let buffer = Buffer::new(path.clone(), source);
        let mut output = vec![format!("file {path}")];
        match parse(&buffer, steep_version) {
            Ok(result) => {
                match &result.node {
                    Some(root) => node(&buffer, root, "", &mut output),
                    None => output.push("nil".to_string()),
                }
                for comment in &result.comments {
                    output.push(format!(
                        "comment {} \"{}\"",
                        range(&buffer, Some(comment.location)),
                        escape(&comment.text)
                    ));
                }
            }
            Err(error) => output.push(format!(
                "error {} \"{}\"",
                range(&buffer, Some(error.location)),
                escape(&error.message)
            )),
        }
        println!("{}", output.join("\n"));
    }
    ExitCode::SUCCESS
}
