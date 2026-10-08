mod code_action_expectations_support;
mod diagnostics_expectations_support;
mod document_highlight_expectations_support;
mod document_symbol_expectations_support;
mod document_link_support;
mod formatter_support;
mod support;
mod test_discovery_support;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use ruby_lsp_rs::indexer::indexing::index_single_with_require_path;
use serde_json::{Value, json};
use support::{DEFAULT_URI, TestOptions, fake_uri, fixture_path, repository_path, with_server};

pub struct ExpectationCase {
    pub fixture_name: String,
    pub source: String,
    pub params: Option<Value>,
}

pub struct RequestKind {
    pub expectation_directory: &'static str,
    pub run: fn(&ExpectationCase) -> Result<Value, String>,
}

fn run_expectations(kind: &RequestKind) {
    let expectations_directory = repository_path().join("tests/expectations").join(kind.expectation_directory);
    assert!(expectations_directory.is_dir(), "Expectations directory {} does not exist", expectations_directory.display());

    let mut fixtures: Vec<PathBuf> = std::fs::read_dir(repository_path().join("tests/fixtures"))
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "rb" || extension == "rake"))
        .collect();
    fixtures.sort();

    let write_expectations = std::env::var_os("WRITE_EXPECTATIONS").is_some();
    let mut failures = Vec::new();
    for fixture in fixtures {
        let fixture_name = fixture.file_stem().unwrap().to_string_lossy().into_owned();
        let source = std::fs::read_to_string(&fixture).unwrap();
        let expectation_path = expectations_directory.join(format!("{fixture_name}.exp.json"));

        if !expectation_path.is_file() {
            let case = ExpectationCase { fixture_name: fixture_name.clone(), source, params: None };
            if let Err(error) = (kind.run)(&case) {
                failures.push(format!("{}__{fixture_name}__does_not_raise: {error}", kind.expectation_directory));
            }
            continue;
        }

        let expected: Value = serde_json::from_str(&std::fs::read_to_string(&expectation_path).unwrap()).unwrap();
        let case = ExpectationCase { fixture_name: fixture_name.clone(), source, params: expected.get("params").cloned() };
        match (kind.run)(&case) {
            Ok(actual) if actual == expected["result"] => {}
            Ok(actual) if write_expectations => write_expectation(&expectation_path, &actual, &case.params),
            Ok(actual) => failures.push(format!(
                "{}__{fixture_name}\n########## Expected ##########\n{}\n##########  Actual  ##########\n{}",
                kind.expectation_directory,
                serde_json::to_string_pretty(&expected["result"]).unwrap(),
                serde_json::to_string_pretty(&actual).unwrap()
            )),
            Err(error) => failures.push(format!("{}__{fixture_name}: {error}", kind.expectation_directory)),
        }
    }

    assert!(failures.is_empty(), "{} expectation(s) failed:\n\n{}", failures.len(), failures.join("\n\n"));
}

fn write_expectation(path: &Path, actual: &Value, params: &Option<Value>) {
    let content = json!({ "result": actual, "params": params.clone().unwrap_or(json!([])) });
    std::fs::write(path, format!("{}\n", serde_json::to_string_pretty(&content).unwrap())).unwrap();
}

fn response_result(response: lsp_server::Response) -> Result<Value, String> {
    response.response_result.map_err(|error| format!("{} ({})", error.message, error.code))
}

fn run_definition(case: &ExpectationCase) -> Result<Value, String> {
    let mut server = with_server(Some(&case.source), DEFAULT_URI, TestOptions { stub_no_typechecker: true, ..TestOptions::default() });
    let position = case.params.as_ref().and_then(|params| params.get(0)).cloned().unwrap_or(json!({ "line": 0, "character": 0 }));

    let lib_path = format!("{}/lib", repository_path().display());
    let class_reference_target = fixture_path("class_reference_target.rb");
    let class_reference_require_path = ruby_lsp_rs::uri::require_path(&class_reference_target.to_string_lossy(), &lib_path);
    server.index_file(&class_reference_target, Some(&class_reference_require_path));
    server.index_file(&fixture_path("constant_reference_target.rb"), None);
    index_single_with_require_path(
        &mut server.index(),
        Arc::from("file:///ruby-lsp/lib/ruby_lsp/server.rb"),
        b"",
        true,
        Some("ruby_lsp/server"),
    );

    let response = server.request(
        "textDocument/definition",
        json!({ "textDocument": { "uri": DEFAULT_URI }, "position": position }),
    );
    let mut result = response_result(response)?;
    if let Some(locations) = result.as_array_mut() {
        for location in locations {
            for key in ["uri", "targetUri"] {
                if let Some(uri) = location.get(key).and_then(Value::as_str).map(fake_uri) {
                    location[key] = Value::String(uri);
                }
            }
        }
    }
    Ok(result)
}

fn run_semantic_highlighting(case: &ExpectationCase) -> Result<Value, String> {
    let initialization_options = json!({
        "initializationOptions": { "experimentalFeaturesEnabled": true },
        "capabilities": { "general": { "positionEncodings": ["utf-8"] } },
    });
    let mut server = with_server(
        Some(&case.source),
        DEFAULT_URI,
        TestOptions { initialization_options: Some(initialization_options), ..TestOptions::default() },
    );
    let range = case.params.as_ref().and_then(|params| params.get(0)).cloned();
    let response = match range {
        Some(range) => server.request(
            "textDocument/semanticTokens/range",
            json!({ "textDocument": { "uri": DEFAULT_URI }, "range": range }),
        ),
        None => server.request("textDocument/semanticTokens/full", json!({ "textDocument": { "uri": DEFAULT_URI } })),
    };
    let result = response_result(response)?;
    let data: Vec<u64> = result["data"]
        .as_array()
        .ok_or_else(|| format!("Expected semantic tokens, got {result}"))?
        .iter()
        .filter_map(Value::as_u64)
        .collect();
    let tokens = data
        .chunks(5)
        .map(|token| {
            json!({
                "delta_line": token[0],
                "delta_start_char": token[1],
                "length": token[2],
                "token_type": token[3],
                "token_modifiers": token[4],
            })
        })
        .collect();
    Ok(Value::Array(tokens))
}

const REQUEST_KINDS: &[RequestKind] = &[RequestKind { expectation_directory: "definition", run: run_definition },
    RequestKind { expectation_directory: "semantic_highlighting", run: run_semantic_highlighting },
    RequestKind { expectation_directory: "folding_ranges", run: run_folding_ranges },
    RequestKind { expectation_directory: "selection_ranges", run: run_selection_ranges },
    RequestKind { expectation_directory: "document_link", run: run_document_link },
    RequestKind { expectation_directory: "prepare_rename", run: code_action_expectations_support::run_prepare_rename },
    RequestKind { expectation_directory: "code_actions", run: code_action_expectations_support::run_code_actions },
    RequestKind {
        expectation_directory: "code_action_resolve",
        run: code_action_expectations_support::run_code_action_resolve,
    },
    RequestKind { expectation_directory: "diagnostics", run: diagnostics_expectations_support::run_diagnostics },
    RequestKind { expectation_directory: "document_symbol", run: document_symbol_expectations_support::run_document_symbol },
    RequestKind {
        expectation_directory: "document_highlight",
        run: document_highlight_expectations_support::run_document_highlight,
    },
    RequestKind { expectation_directory: "hover", run: run_hover },
    RequestKind { expectation_directory: "code_lens", run: run_code_lens },
    RequestKind { expectation_directory: "inlay_hints", run: run_inlay_hints },
];

fn run_code_lens(case: &ExpectationCase) -> Result<Value, String> {
    let uri = test_discovery_support::fixture_uri(&case.fixture_name);
    let mut server = test_discovery_support::bundled_server(
        Some(&case.source),
        &uri,
        "minitest",
        json!({ "experimentalFeaturesEnabled": true }),
    );
    response_result(server.request("textDocument/codeLens", json!({ "textDocument": { "uri": uri } })))
}

fn run_inlay_hints(case: &ExpectationCase) -> Result<Value, String> {
    let initialization_options = json!({
        "initializationOptions": {
            "featuresConfiguration": { "inlayHint": { "implicitRescue": true, "implicitHashValue": true } },
        },
    });
    let mut server = with_server(
        Some(&case.source),
        DEFAULT_URI,
        TestOptions { initialization_options: Some(initialization_options), ..TestOptions::default() },
    );
    let range = case
        .params
        .as_ref()
        .and_then(|params| params.get(0))
        .cloned()
        .unwrap_or(json!({ "start": { "line": 0, "character": 0 }, "end": { "line": 20, "character": 20 } }));
    response_result(server.request("textDocument/inlayHint", json!({ "textDocument": { "uri": DEFAULT_URI }, "range": range })))
}

fn run_hover(case: &ExpectationCase) -> Result<Value, String> {
    let mut server = with_server(Some(&case.source), DEFAULT_URI, TestOptions { stub_no_typechecker: true, ..TestOptions::default() });
    let position = case.params.as_ref().and_then(|params| params.get(0)).cloned().unwrap_or(json!({ "line": 0, "character": 0 }));
    response_result(server.request("textDocument/hover", json!({ "textDocument": { "uri": DEFAULT_URI }, "position": position })))
}

fn run_folding_ranges(case: &ExpectationCase) -> Result<Value, String> {
    let mut server = with_server(Some(&case.source), DEFAULT_URI, TestOptions { stub_no_typechecker: true, ..TestOptions::default() });
    response_result(server.request("textDocument/foldingRange", json!({ "textDocument": { "uri": DEFAULT_URI } })))
}

fn run_selection_ranges(case: &ExpectationCase) -> Result<Value, String> {
    let mut server = with_server(Some(&case.source), DEFAULT_URI, TestOptions { stub_no_typechecker: true, ..TestOptions::default() });
    let positions = match &case.params {
        Some(Value::Array(positions)) if !positions.is_empty() => Value::Array(positions.clone()),
        _ => json!([{ "line": 0, "character": 0 }]),
    };
    response_result(server.request(
        "textDocument/selectionRange",
        json!({ "textDocument": { "uri": DEFAULT_URI }, "positions": positions }),
    ))
}

fn run_document_link(case: &ExpectationCase) -> Result<Value, String> {
    let uri = format!("file:///fixtures/{}.rb", case.fixture_name);
    let links = document_link_support::document_links(&case.source, &uri);
    let mut result = serde_json::to_value(links).map_err(|error| error.to_string())?;
    for link in result.as_array_mut().into_iter().flatten() {
        for key in ["target", "tooltip"] {
            if let Some(text) = link.get(key).and_then(Value::as_str).map(document_link_support::with_placeholders) {
                link[key] = Value::String(text);
            }
        }
    }
    Ok(result)
}

fn kind(directory: &str) -> &'static RequestKind {
    REQUEST_KINDS.iter().find(|kind| kind.expectation_directory == directory).expect("request kind is registered")
}

#[test]
fn definition_expectations() {
    run_expectations(kind("definition"));
}

#[test]
fn semantic_highlighting_expectations() {
    run_expectations(kind("semantic_highlighting"));
}

#[test]
fn folding_ranges_expectations() {
    run_expectations(kind("folding_ranges"));
}

#[test]
fn selection_ranges_expectations() {
    run_expectations(kind("selection_ranges"));
}

#[test]
fn document_link_expectations() {
    run_expectations(kind("document_link"));
}

#[test]
fn prepare_rename_expectations() {
    run_expectations(kind("prepare_rename"));
}

#[test]
fn code_actions_expectations() {
    run_expectations(kind("code_actions"));
}

#[test]
fn code_action_resolve_expectations() {
    run_expectations(kind("code_action_resolve"));
}

#[test]
fn diagnostics_expectations() {
    if !diagnostics_expectations_support::available() {
        return;
    }
    run_expectations(kind("diagnostics"));
}

#[test]
fn document_symbol_expectations() {
    run_expectations(kind("document_symbol"));
}

#[test]
fn document_highlight_expectations() {
    run_expectations(kind("document_highlight"));
}

#[test]
fn hover_expectations() {
    run_expectations(kind("hover"));
}

#[test]
fn code_lens_expectations() {
    run_expectations(kind("code_lens"));
}

#[test]
fn inlay_hints_expectations() {
    run_expectations(kind("inlay_hints"));
}
