#[path = "services_dump/mod.rs"]
mod services_dump;

use std::process::ExitCode;

use services_dump::{Json, query_positions};
use steep::services::signature_help_provider::{SignatureHelpError, SignatureHelpProvider};

fn error(error: &SignatureHelpError) -> Option<Json> {
    let name = match error {
        SignatureHelpError::SyntaxError => return None,
        SignatureHelpError::UnknownNode => "Steep::Typing::UnknownNodeError",
        SignatureHelpError::Runtime(name) => name,
    };
    Some(Json::object([("error", Json::string(name))]))
}

fn main() -> ExitCode {
    services_dump::run(|project, path, text, dump| {
        let service = &project.service;
        let project_definition = &service.project;
        if project_definition
            .target_for_inline_source_path(path)
            .is_none()
            && project_definition.target_for_source_path(path).is_none()
        {
            return;
        }
        for query in query_positions(text) {
            let provider = match SignatureHelpProvider::new(service, path, query.position) {
                Ok(Some(provider)) => provider,
                Ok(None) => continue,
                Err(found) => {
                    if let Some(json) = error(&found) {
                        dump.print(path, &query, json);
                    }
                    continue;
                }
            };
            let result = match provider.run(query.position) {
                Ok(Some(help)) => Json::object([
                    (
                        "items",
                        Json::Array(
                            help.items
                                .iter()
                                .map(|item| {
                                    Json::object([
                                        ("method_type", Json::String(item.method_type.to_string())),
                                        (
                                            "parameters",
                                            item.parameters().map_or(Json::Null, |parameters| {
                                                Json::Array(
                                                    parameters
                                                        .into_iter()
                                                        .map(Json::String)
                                                        .collect(),
                                                )
                                            }),
                                        ),
                                        (
                                            "active_parameter",
                                            item.active_parameter.map_or(Json::Null, Json::Integer),
                                        ),
                                        ("comment", Json::optional_string(item.comment.clone())),
                                    ])
                                })
                                .collect(),
                        ),
                    ),
                    (
                        "active_signature",
                        help.active_signature
                            .map_or(Json::Null, |index| Json::Integer(index as i64)),
                    ),
                ]),
                Ok(None) => continue,
                Err(found) => match error(&found) {
                    Some(json) => json,
                    None => continue,
                },
            };
            dump.print(path, &query, result);
        }
    })
}
