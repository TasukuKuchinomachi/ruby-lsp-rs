mod support;

use ruby_lsp_rs::document::{ContentChange, Document, LanguageId};
use ruby_lsp_rs::position::{Encoding, Position, Range};
use serde_json::{Value, json};
use support::{TestOptions, TestServer, fixture_path, repository_path, with_server};

fn rename_capable_server(uri: &str) -> TestServer {
    with_server(
        None,
        uri,
        TestOptions {
            initialization_options: Some(json!({
                "capabilities": { "workspace": { "workspaceEdit": { "resourceOperations": ["rename"] } } },
                "initializationOptions": { "experimentalFeaturesEnabled": true },
            })),
            ..TestOptions::default()
        },
    )
}

fn rename_request(server: &mut TestServer, uri: &str, line: u32, character: u32, new_name: &str) -> lsp_server::Response {
    server.request(
        "textDocument/rename",
        json!({
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": character },
            "newName": new_name,
        }),
    )
}

fn apply_edits(source: &str, edits: &[Value]) -> String {
    let mut document = Document::new("file:///applied.rb", source.to_string(), 1, LanguageId::Ruby, Encoding::Utf16);
    let changes: Vec<ContentChange> = edits
        .iter()
        .map(|edit| {
            let range = &edit["range"];
            let position = |key: &str| {
                Position::new(range[key]["line"].as_u64().unwrap() as u32, range[key]["character"].as_u64().unwrap() as u32)
            };
            ContentChange { range: Some(Range::new(position("start"), position("end"))), text: edit["newText"].as_str().unwrap().to_string() }
        })
        .collect();
    document.push_edits(&changes, 2).unwrap();
    document.source().to_string()
}

fn expect_renames(fixture_name: &str, new_fixture_path: &str, expected: &str, line: u32, character: u32, new_name: &str) {
    let path = fixture_path(fixture_name);
    let source = std::fs::read_to_string(&path).unwrap();
    let uri = ruby_lsp_rs::uri::from_path_buf(&path);
    let mut server = rename_capable_server(&uri);
    server.index_file(&path, None);

    let response = rename_request(&mut server, &uri, line, character, new_name);
    let workspace_edit = response.response_result.unwrap();
    let document_changes = workspace_edit["documentChanges"].as_array().unwrap();

    let mut file_renames = Vec::new();
    let mut new_source = source.clone();
    for change in document_changes {
        if change.get("kind").is_some() {
            file_renames.push(change.clone());
            continue;
        }
        if change["textDocument"]["uri"] == json!(uri) {
            new_source = apply_edits(&new_source, change["edits"].as_array().unwrap());
        }
    }

    assert_eq!(expected, new_source);
    let new_uri = file_renames.first().expect("a file rename")["newUri"].as_str().unwrap().to_string();
    assert_eq!(repository_path().join(new_fixture_path), ruby_lsp_rs::uri::to_path_buf(&new_uri).unwrap());
}

#[test]
fn empty_diagnostics_for_ignored_file() {
    let expected = "class Article\nend\n\nArticle\n";
    expect_renames("rename_me.rb", "tests/fixtures/article.rb", expected, 0, 7, "Article");
}

#[test]
fn renaming_conflict() {
    let path = fixture_path("rename_me.rb");
    let uri = ruby_lsp_rs::uri::from_path_buf(&path);
    let mut server = rename_capable_server(&uri);
    server.index_file(&path, None);
    server.index_source(&ruby_lsp_rs::uri::from_path("/fake.rb"), "class Conflicting\nend\n");

    let response = rename_request(&mut server, &uri, 3, 7, "Conflicting");
    let error = response.response_result.expect_err("rename should fail");
    assert_eq!(lsp_server::ErrorCode::RequestFailed as i32, error.code);
    assert_eq!("The new name is already in use by Conflicting", error.message);
}

#[test]
fn renaming_an_unsaved_symbol() {
    let path = fixture_path("rename_me.rb");
    let uri = ruby_lsp_rs::uri::from_path_buf(&path);
    let mut server = rename_capable_server(&uri);
    server.index_file(&path, None);

    let untitled_uri = "untitled:Untitled-1";
    let untitled_source = "class RenameMe\nend\n";
    server.index_source(untitled_uri, untitled_source);
    server.did_open(untitled_uri, untitled_source, "ruby");

    let response = rename_request(&mut server, &uri, 3, 7, "NewMe");
    let workspace_edit = response.response_result.unwrap();
    let document_changes = workspace_edit["documentChanges"].as_array().unwrap();

    // Ruby LSP asserts on `document_changes[1]`, which relies on the insertion order of its store
    let untitled_change = document_changes
        .iter()
        .find(|change| change["textDocument"]["uri"] == json!(untitled_uri))
        .expect("an edit for the untitled document");
    assert_eq!("NewMe", untitled_change["edits"][0]["newText"]);
}

#[test]
fn returns_plain_changes_when_client_cannot_rename_files() {
    let path = fixture_path("rename_me.rb");
    let uri = ruby_lsp_rs::uri::from_path_buf(&path);
    let mut server = with_server(None, &uri, TestOptions::default());
    server.index_file(&path, None);

    let response = rename_request(&mut server, &uri, 0, 7, "Article");
    let workspace_edit = response.response_result.unwrap();
    assert!(workspace_edit.get("documentChanges").is_none());
    let edits = workspace_edit["changes"][uri.as_str()].as_array().unwrap();
    assert_eq!(2, edits.len());
    assert_eq!("Article", edits[0]["newText"]);
}

#[test]
fn file_names_follow_constant_names() {
    use ruby_lsp_rs::requests::rename::file_from_constant_name;

    assert_eq!("rename_me", file_from_constant_name("RenameMe"));
    assert_eq!("html_parser", file_from_constant_name("HTMLParser"));
    assert_eq!("foo", file_from_constant_name("Foo"));
}
