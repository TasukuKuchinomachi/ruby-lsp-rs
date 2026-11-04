#[path = "services_dump/mod.rs"]
mod services_dump;

use std::{path::Path, process::ExitCode};

use services_dump::{Json, declaration, query_positions, rbs_location, ruby_range};
use steep::{
    ruby::node::Buffer,
    services::hover_provider::{
        CallKind, HoverContent, HoverMethodCall, content_for, declaration_comment,
    },
};

fn method_call(call: &HoverMethodCall) -> Json {
    let kind = match &call.kind {
        CallKind::Typed { .. } => "Typed",
        CallKind::Special { .. } => "Special",
        CallKind::Error => "Error",
    };
    let mut declarations = call.method_decls.clone();
    declarations.sort_by_key(|declaration| declaration.full_name());
    let mut fields = vec![
        ("kind".to_string(), Json::string(kind)),
        ("method_name".to_string(), Json::string(&call.method_name)),
        (
            "receiver_type".to_string(),
            Json::String(call.receiver_type.to_string()),
        ),
        (
            "return_type".to_string(),
            Json::String(call.return_type.to_string()),
        ),
        (
            "method_decls".to_string(),
            Json::Array(
                declarations
                    .iter()
                    .map(|declaration| {
                        Json::object([
                            ("method_name", Json::String(declaration.full_name())),
                            (
                                "method_type",
                                Json::String(declaration.method_type.to_string()),
                            ),
                            ("comment", Json::optional_string(declaration.comment())),
                            ("location", rbs_location(declaration.member_location())),
                        ])
                    })
                    .collect(),
            ),
        ),
    ];
    if let CallKind::Typed { actual_method_type } | CallKind::Special { actual_method_type } =
        &call.kind
    {
        fields.push((
            "actual_method_type".to_string(),
            Json::String(actual_method_type.to_string()),
        ));
    }
    Json::Object(fields)
}

fn hover(path: &Path, buffer: &Buffer, content: &HoverContent) -> Json {
    let location = ruby_range(path, buffer, content.location());
    let (name, fields) = match content {
        HoverContent::Variable { name, type_, .. } => (
            "VariableContent",
            Json::object([
                ("location", location),
                ("name", Json::string(name)),
                ("type", Json::String(type_.to_string())),
            ]),
        ),
        HoverContent::Type { type_, .. } => (
            "TypeContent",
            Json::object([
                ("location", location),
                ("type", Json::String(type_.to_string())),
            ]),
        ),
        HoverContent::TypeAssertion {
            original_type,
            asserted_type,
            ..
        } => (
            "TypeAssertionContent",
            Json::object([
                ("location", location),
                ("original_type", Json::String(original_type.to_string())),
                ("asserted_type", Json::String(asserted_type.to_string())),
            ]),
        ),
        HoverContent::MethodCall {
            method_call: call, ..
        } => (
            "MethodCallContent",
            Json::object([("location", location), ("method_call", method_call(call))]),
        ),
        HoverContent::Definition {
            method_name,
            method_type,
            definition,
            ..
        } => {
            let mut comments: Vec<String> = Vec::new();
            for definition in &definition.defs {
                let comment = match &definition.member {
                    rbs_definition::definition_builder::DefinitionMember::MethodDefinition(
                        member,
                    ) => member.comment.clone(),
                    rbs_definition::definition_builder::DefinitionMember::Attribute(member) => {
                        member.comment.clone()
                    }
                };
                if let Some(comment) = comment
                    && !comments.contains(&comment)
                {
                    comments.push(comment);
                }
            }
            (
                "DefinitionContent",
                Json::object([
                    ("location", location),
                    ("method_name", Json::String(method_name.to_string())),
                    ("method_type", Json::String(method_type.to_string())),
                    (
                        "method_types",
                        Json::Array(
                            definition
                                .method_types()
                                .map(|method_type| Json::String(method_type.to_string()))
                                .collect(),
                        ),
                    ),
                    (
                        "comments",
                        Json::Array(comments.into_iter().map(Json::String).collect()),
                    ),
                ]),
            )
        }
        HoverContent::Constant {
            full_name,
            type_,
            declaration: constant_declaration,
            comments,
            ..
        } => (
            "ConstantContent",
            Json::object([
                ("location", location),
                ("full_name", Json::String(full_name.to_string())),
                ("type", Json::String(type_.to_string())),
                ("decl", declaration(constant_declaration)),
                (
                    "comments",
                    Json::Array(comments.iter().map(Json::string).collect()),
                ),
            ]),
        ),
        HoverContent::TypeAlias {
            declaration: content_declaration,
            ..
        }
        | HoverContent::InterfaceType {
            declaration: content_declaration,
            ..
        }
        | HoverContent::ClassType {
            declaration: content_declaration,
            ..
        } => (
            match content {
                HoverContent::TypeAlias { .. } => "TypeAliasContent",
                HoverContent::InterfaceType { .. } => "InterfaceTypeContent",
                _ => "ClassTypeContent",
            },
            Json::object([
                ("location", location),
                ("decl", declaration(content_declaration)),
                (
                    "comment",
                    Json::optional_string(declaration_comment(content_declaration)),
                ),
            ]),
        ),
    };
    Json::Object(vec![(name.to_string(), fields)])
}

fn main() -> ExitCode {
    services_dump::run(|project, path, text, dump| {
        let buffer = Buffer::new(path.to_string_lossy(), text);
        for query in query_positions(text) {
            if let Some(content) = content_for(&project.service, path, query.position) {
                dump.print(path, &query, hover(path, &buffer, &content));
            }
        }
    })
}
