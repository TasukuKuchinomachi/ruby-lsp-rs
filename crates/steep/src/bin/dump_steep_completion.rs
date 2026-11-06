#[path = "services_dump/mod.rs"]
mod services_dump;

use std::{path::Path, process::ExitCode};

use rbs_definition::definition_builder::DefinitionMember;
use services_dump::{Json, declaration, position_range, query_positions, rbs_location, summarize};
use steep::{
    ruby::node::Buffer,
    services::{
        completion_provider::{CompletionError, CompletionItem, ConstantKind, rbs, ruby::Ruby},
        type_check_service::TypeCheckService,
    },
};

fn member_location(member: &DefinitionMember) -> Json {
    rbs_location(match member {
        DefinitionMember::MethodDefinition(definition) => definition.location.as_ref(),
        DefinitionMember::Attribute(attribute) => attribute.location.as_ref(),
    })
}

fn strings<T: ToString>(values: &[T]) -> Json {
    Json::Array(
        values
            .iter()
            .map(|value| Json::String(value.to_string()))
            .collect(),
    )
}

fn item(buffer: &Buffer, item: &CompletionItem) -> Json {
    let range = position_range(buffer, item.range());
    match item {
        CompletionItem::LocalVariable {
            identifier, type_, ..
        } => Json::object([
            ("kind", Json::string("LocalVariableItem")),
            ("identifier", Json::string(identifier)),
            ("range", range),
            ("type", Json::String(type_.to_string())),
        ]),
        CompletionItem::InstanceVariable {
            identifier, type_, ..
        } => Json::object([
            ("kind", Json::string("InstanceVariableItem")),
            ("identifier", Json::string(identifier)),
            ("range", range),
            ("type", Json::String(type_.to_string())),
        ]),
        CompletionItem::KeywordArgument { identifier, .. } => Json::object([
            ("kind", Json::string("KeywordArgumentItem")),
            ("identifier", Json::string(identifier)),
            ("range", range),
        ]),
        CompletionItem::Constant {
            identifier,
            type_,
            full_name,
            kind,
            declaration: constant_declaration,
            deprecated,
            ..
        } => Json::object([
            ("kind", Json::string("ConstantItem")),
            ("identifier", Json::string(identifier)),
            ("range", range),
            ("type", Json::String(type_.to_string())),
            ("full_name", Json::String(full_name.to_string())),
            (
                "constant_kind",
                Json::string(match kind {
                    ConstantKind::Class => "class",
                    ConstantKind::Module => "module",
                    ConstantKind::Constant => "constant",
                }),
            ),
            (
                "decl",
                constant_declaration
                    .as_ref()
                    .map_or(Json::Null, declaration),
            ),
            ("deprecated", Json::Bool(*deprecated)),
        ]),
        CompletionItem::SimpleMethodName {
            identifier,
            receiver_type,
            method_types,
            method_member,
            method_name,
            deprecated,
            ..
        } => Json::object([
            ("kind", Json::string("SimpleMethodNameItem")),
            ("identifier", Json::string(identifier)),
            ("range", range),
            ("receiver_type", Json::String(receiver_type.to_string())),
            ("method_name", Json::String(method_name.to_string())),
            ("method_types", strings(method_types)),
            ("member", member_location(method_member)),
            ("deprecated", Json::Bool(*deprecated)),
        ]),
        CompletionItem::ComplexMethodName {
            identifier,
            receiver_type,
            method_types,
            method_decls,
            ..
        } => Json::object([
            ("kind", Json::string("ComplexMethodNameItem")),
            ("identifier", Json::string(identifier)),
            ("range", range),
            ("receiver_type", Json::String(receiver_type.to_string())),
            ("method_types", strings(method_types)),
            (
                "method_decls",
                Json::Array(
                    method_decls
                        .iter()
                        .map(|declaration| {
                            Json::Array(vec![
                                Json::String(declaration.full_name()),
                                member_location(&declaration.definition.member),
                            ])
                        })
                        .collect(),
                ),
            ),
        ]),
        CompletionItem::GeneratedMethodName {
            identifier,
            receiver_type,
            method_types,
            ..
        } => Json::object([
            ("kind", Json::string("GeneratedMethodNameItem")),
            ("identifier", Json::string(identifier)),
            ("range", range),
            ("receiver_type", Json::String(receiver_type.to_string())),
            ("method_types", strings(method_types)),
        ]),
        CompletionItem::TypeName {
            absolute_type_name,
            relative_type_name,
            ..
        } => Json::object([
            ("kind", Json::string("TypeNameItem")),
            (
                "absolute_type_name",
                Json::String(absolute_type_name.to_string()),
            ),
            (
                "relative_type_name",
                Json::String(relative_type_name.to_string()),
            ),
            ("range", range),
        ]),
        CompletionItem::Text {
            text,
            help_text,
            label,
            ..
        } => Json::object([
            ("kind", Json::string("TextItem")),
            ("label", Json::string(label)),
            ("text", Json::string(text)),
            ("help_text", Json::string(help_text)),
            ("range", range),
        ]),
    }
}

fn items(buffer: &Buffer, items: &[CompletionItem]) -> Json {
    summarize(items.iter().map(|value| item(buffer, value)).collect())
}

fn error(error: &CompletionError) -> Json {
    Json::object([(
        "error",
        Json::string(match error {
            CompletionError::SyntaxError => "Parser::SyntaxError",
            CompletionError::UnknownNode => "Steep::Typing::UnknownNodeError",
            CompletionError::Runtime(name) => name,
        }),
    )])
}

fn ruby_completion(
    service: &TypeCheckService,
    path: &Path,
    buffer: &Buffer,
    position: usize,
) -> Option<Json> {
    let provider = Ruby::new(service, path)?;
    match provider.run_at_comment(position) {
        Ok(Some((prefix_size, found))) => {
            return Some(Json::object([
                ("prefix_size", Json::Integer(prefix_size as i64)),
                ("items", items(buffer, &found)),
            ]));
        }
        Ok(None) => {}
        Err(found) => return Some(error(&found)),
    }
    match provider.run(position) {
        Ok(found) if found.is_empty() => None,
        Ok(found) => Some(Json::object([("items", items(buffer, &found))])),
        Err(CompletionError::SyntaxError) => None,
        Err(found) => Some(error(&found)),
    }
}

fn rbs_completion(
    service: &TypeCheckService,
    target_name: &str,
    path: &Path,
    position: usize,
) -> Option<Json> {
    let completion = match rbs::run(service, target_name, path, position)? {
        Ok(completion) => completion,
        Err(name) => return Some(error(&CompletionError::Runtime(name))),
    };
    if completion.type_names.is_empty() {
        return None;
    }
    let mut type_names: Vec<(String, String)> = completion
        .type_names
        .iter()
        .map(|(absolute, relative)| (absolute.to_string(), relative.to_string()))
        .collect();
    type_names.sort();
    Some(Json::object([
        ("prefix_size", Json::Integer(completion.prefix_size as i64)),
        (
            "type_names",
            Json::Array(
                type_names
                    .into_iter()
                    .map(|(absolute, relative)| {
                        Json::Array(vec![Json::String(absolute), Json::String(relative)])
                    })
                    .collect(),
            ),
        ),
    ]))
}

fn main() -> ExitCode {
    services_dump::run(|project, path, text, dump| {
        let buffer = Buffer::new(path.to_string_lossy(), text);
        let service = &project.service;
        for query in query_positions(text) {
            let project_definition = &service.project;
            let result = if project_definition
                .target_for_inline_source_path(path)
                .is_some()
                || project_definition.target_for_source_path(path).is_some()
            {
                ruby_completion(service, path, &buffer, query.position)
            } else if let Some(target) = project_definition.target_for_signature_path(path) {
                rbs_completion(service, &target.name, path, query.position)
            } else {
                None
            };
            if let Some(result) = result {
                dump.print(path, &query, result);
            }
        }
    })
}
