use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use rbs_definition::loader::RbsGem;
use steep::{
    method_name::MethodName,
    project::dsl,
    services::{
        completion_provider::{CompletionItem, ruby::Ruby},
        content_change::{ContentChange, Position},
        goto_service::{GotoLocation, GotoService, QueryName, parse_name},
        hover_provider::{HoverContent, content_for},
        signature_help_provider::SignatureHelpProvider,
        type_check_service::TypeCheckService,
    },
    steep_version::SteepVersion,
    strict::EnabledChecks,
};

fn tests_directory() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/services")
}

fn load_service() -> TypeCheckService {
    let base_directory = tests_directory().join("project");
    let steepfile =
        fs::read_to_string(base_directory.join("Steepfile")).expect("the Steepfile is readable");
    let project = dsl::parse(&base_directory, &steepfile).expect("the Steepfile is read");
    let rbs_gem =
        RbsGem::new(&tests_directory().join("rbs_gem"), &[]).expect("the rbs gem is found");
    let mut service = TypeCheckService::load(
        project,
        &rbs_gem,
        SteepVersion::default(),
        EnabledChecks::default(),
        1,
    )
    .expect("the signatures load");
    service.load_files().expect("the files are read");
    service
}

fn position_of(service: &TypeCheckService, path: &str, text: &str) -> usize {
    service.source_files[Path::new(path)]
        .content
        .find(text)
        .expect("the text is in the file")
}

fn replace_line(path: &str, line: usize, text: &str) -> (PathBuf, Vec<ContentChange>) {
    (
        PathBuf::from(path),
        vec![ContentChange::new(
            Position { line, column: 0 },
            Position {
                line: line + 1,
                column: 0,
            },
            text,
        )],
    )
}

#[test]
fn checks_an_edited_file_again_without_reloading_the_signatures() {
    let mut service = load_service();
    let main = Path::new("lib/main.rb");
    let diagnostics = service
        .typecheck_source(main)
        .expect("the file is checked")
        .expect("the file is a source file");
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    let signatures = service.signature_services["app"]
        .current_signatures()
        .expect("the signatures are loaded")
        .clone();

    service
        .update(&[replace_line(
            "lib/main.rb",
            2,
            "message = greeter.greet(1)\n",
        )])
        .expect("the change is applied");
    let diagnostics = service
        .typecheck_source(main)
        .expect("the file is checked")
        .expect("the file is a source file");

    assert_eq!(
        diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code.as_str())
            .collect::<Vec<_>>(),
        ["Ruby::ArgumentTypeMismatch"]
    );
    assert!(Arc::ptr_eq(
        &signatures,
        service.signature_services["app"]
            .current_signatures()
            .expect("the signatures are loaded")
    ));
}

#[test]
fn reloads_the_signatures_when_a_signature_file_changes() {
    let mut service = load_service();
    let signatures = service.signature_services["app"]
        .current_signatures()
        .expect("the signatures are loaded")
        .clone();

    service
        .update(&[replace_line(
            "sig/greeter.rbs",
            6,
            "  def greet: (Integer greeting, ?times: Integer) -> String\n",
        )])
        .expect("the change is applied");
    let diagnostics = service
        .typecheck_source(Path::new("lib/main.rb"))
        .expect("the file is checked")
        .expect("the file is a source file");

    assert!(!Arc::ptr_eq(
        &signatures,
        service.signature_services["app"]
            .current_signatures()
            .expect("the signatures are loaded")
    ));
    assert_eq!(
        diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code.as_str())
            .collect::<Vec<_>>(),
        ["Ruby::ArgumentTypeMismatch"]
    );
}

#[test]
fn keeps_the_last_signatures_while_a_signature_file_has_a_syntax_error() {
    let mut service = load_service();
    service
        .update(&[replace_line("sig/greeter.rbs", 6, "  def greet: (\n")])
        .expect("the change is applied");

    let signature = &service.signature_services["app"];
    assert!(signature.current_signatures().is_none());
    assert!(
        signature
            .latest_environment()
            .expect("the last environment is kept")
            .class_declarations
            .contains_key(&rbs_definition::ast::TypeName::parse("::Greeter"))
    );
}

#[test]
fn answers_hover_on_the_edited_text() {
    let mut service = load_service();
    service
        .update(&[replace_line(
            "lib/main.rb",
            2,
            "message = greeter.greet(\"Hello\").upcase\n",
        )])
        .expect("the change is applied");
    let path = Path::new("lib/main.rb");
    let position = position_of(&service, "lib/main.rb", "upcase");

    let content = content_for(&service, path, position).expect("a hover content");

    match content {
        HoverContent::MethodCall { method_call, .. } => {
            assert_eq!(method_call.method_name, "upcase");
            assert_eq!(method_call.return_type.to_string(), "::String");
        }
        content => panic!("unexpected content: {content:?}"),
    }
}

#[test]
fn completes_methods_after_a_trailing_dot() {
    let mut service = load_service();
    service
        .update(&[replace_line("lib/main.rb", 2, "greeter.\n")])
        .expect("the change is applied");
    let path = Path::new("lib/main.rb");
    let position = position_of(&service, "lib/main.rb", "greeter.") + "greeter.".len();

    let items = Ruby::new(&service, path)
        .expect("the file can be completed")
        .run(position)
        .expect("the completion succeeds");

    let names: Vec<&str> = items
        .iter()
        .filter_map(|item| match item {
            CompletionItem::SimpleMethodName { identifier, .. } => Some(identifier.as_str()),
            _ => None,
        })
        .collect();
    assert!(names.contains(&"greet"), "{names:?}");
    assert!(!names.contains(&"initialize"), "{names:?}");
}

#[test]
fn helps_with_the_signature_of_a_call() {
    let service = load_service();
    let path = Path::new("lib/main.rb");
    let position = position_of(&service, "lib/main.rb", "\"Hello\"");

    let help = SignatureHelpProvider::new(&service, path, position)
        .expect("the file parses")
        .expect("the file is a source file")
        .run(position)
        .expect("the signature help succeeds")
        .expect("the position is in a call");

    assert_eq!(help.items.len(), 1);
    assert_eq!(
        help.items[0].method_type.to_string(),
        "(::String, ?times: ::Integer) -> ::String"
    );
    assert_eq!(help.items[0].active_parameter, Some(0));
    assert_eq!(help.active_signature, Some(0));
}

#[test]
fn goes_to_the_definitions_of_a_method_in_rbs_and_ruby() {
    let mut service = load_service();
    for path in service.project_paths() {
        service
            .typecheck_source(&path)
            .expect("the file is checked");
    }
    let path = Path::new("lib/main.rb");
    let position = position_of(&service, "lib/main.rb", "greet(");
    let goto = GotoService::new(&service);

    let definitions = goto
        .definition(path, position)
        .expect("the definition is found");
    let implementations = goto
        .implementation(path, position)
        .expect("the implementation is found");

    assert!(matches!(
        definitions.as_slice(),
        [GotoLocation::Rbs(location)] if location.name() == "sig/greeter.rbs" && location.source() == "greet"
    ));
    assert!(matches!(
        implementations.as_slice(),
        [GotoLocation::Ruby { path, .. }] if path == Path::new("lib/greeter.rb")
    ));
}

#[test]
fn goes_to_the_definitions_of_a_queried_name() {
    let mut service = load_service();
    for path in service.project_paths() {
        service
            .typecheck_source(&path)
            .expect("the file is checked");
    }
    let goto = GotoService::new(&service);

    let name = parse_name("Greeter#greet").expect("the name parses");
    let locations = goto.query_definition(&name).expect("the query succeeds");

    assert_eq!(
        name,
        QueryName::Method(MethodName::instance(
            rbs_definition::ast::TypeName::parse("::Greeter"),
            "greet"
        ))
    );
    assert_eq!(locations.len(), 2, "{locations:?}");
    assert_eq!(
        parse_name("Greeter.new"),
        Some(QueryName::Method(MethodName::singleton(
            rbs_definition::ast::TypeName::parse("::Greeter"),
            "new"
        )))
    );
    assert_eq!(parse_name(""), None);
}
