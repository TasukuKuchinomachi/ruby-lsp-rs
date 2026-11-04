#[path = "services_dump/mod.rs"]
mod services_dump;

use std::process::ExitCode;

use services_dump::{Json, query_positions, rbs_location, ruby_range};
use steep::{
    ruby::node::Buffer,
    services::{
        goto_service::{GotoError, GotoLocation, GotoService},
        type_check_service::TypeCheckService,
    },
};

fn locations(service: &TypeCheckService, result: Result<Vec<GotoLocation>, GotoError>) -> Json {
    match result {
        Ok(locations) => {
            let mut strings: Vec<String> = locations
                .iter()
                .map(|location| {
                    let json = match location {
                        GotoLocation::Rbs(location) => rbs_location(Some(location)),
                        GotoLocation::Ruby { path, range } => {
                            let text = service
                                .source_files
                                .get(path)
                                .map(|file| file.content.as_str())
                                .unwrap_or_default();
                            ruby_range(path, &Buffer::new(path.to_string_lossy(), text), *range)
                        }
                    };
                    match json {
                        Json::String(string) => string,
                        _ => String::new(),
                    }
                })
                .collect();
            strings.sort();
            Json::Array(strings.into_iter().map(Json::String).collect())
        }
        Err(error) => Json::object([(
            "error",
            Json::string(match &error {
                GotoError::UnknownNode => "Steep::Typing::UnknownNodeError",
                GotoError::Runtime(name) => name,
            }),
        )]),
    }
}

fn main() -> ExitCode {
    services_dump::run_with_setup(
        |project| {
            for path in project.paths.clone() {
                let _ = project.service.typecheck_source(&path);
            }
        },
        |project, path, text, dump| {
            let goto = GotoService::new(&project.service);
            for query in query_positions(text) {
                let results = [
                    ("definition", goto.definition(path, query.position)),
                    ("implementation", goto.implementation(path, query.position)),
                    (
                        "type_definition",
                        goto.type_definition(path, query.position),
                    ),
                ];
                if results
                    .iter()
                    .all(|(_, result)| result.as_ref().is_ok_and(Vec::is_empty))
                {
                    continue;
                }
                let fields: Vec<(String, Json)> = results
                    .into_iter()
                    .map(|(kind, result)| (kind.to_string(), locations(&project.service, result)))
                    .collect();
                dump.print(path, &query, Json::Object(fields));
            }
        },
    )
}
