mod indexer_support;

use std::sync::Arc;

use indexer_support::{TestCase, kind_name, parameter_kinds, parameter_names};
use ruby_lsp_rs::indexer::entry::{Parameter, Signature};
use ruby_lsp_rs::indexer::rbs_indexer::RbsIndexer;
use ruby_lsp_rs::indexer::{Configuration, Entry, EntryKind, Index};

fn find<'a>(test: &'a TestCase, name: &str, owner: &str) -> &'a Entry {
    test.entries(name).into_iter().find(|entry| entry.owner() == Some(owner)).unwrap_or_else(|| panic!("{name} owned by {owner}"))
}

fn parse_rbs_methods(rbs: &str, method_name: &str) -> Vec<Signature> {
    let mut index = Index::new(Arc::new(Configuration::default()));
    RbsIndexer::new(&mut index).process_signature("/file.rbs", rbs);
    index.entries_named(method_name)[0].signatures()
}

fn assert_file_path(entry: &Entry, file_name: &str) {
    let path = entry.file_path().unwrap();
    assert!(path.contains("/gems/rbs-") && path.ends_with(&format!("/core/{file_name}")), "{path}");
    assert_eq!(file_name, entry.file_name());
}

#[test]
fn test_index_core_classes() {
    let test = TestCase::new();
    let entries = test.entries("Array");
    assert_eq!(2, entries.len());
    let entry = entries.iter().find(|entry| kind_name(entry) == "Class").unwrap();
    assert_file_path(entry, "array.rbs");
    let namespace = entry.namespace().unwrap();
    // RBS 4.1.3 declares `class Array[unchecked out E]` without an explicit superclass, ruby-lsp's RBS declared `< Object`
    assert_eq!(None, namespace.parent_class.as_deref());
    assert_eq!(1, namespace.mixin_operations.len());
    assert_eq!("Enumerable", namespace.mixin_operations[0].module_name());

    assert!(entry.location.start_line > 0);
    assert!(entry.location.end_line > entry.location.start_line);
    assert_eq!(0, entry.location.start_column);
    assert!(entry.location.end_column > 0);
}

#[test]
fn test_index_core_modules() {
    let test = TestCase::new();
    let entries = test.entries("Kernel");
    assert_eq!(2, entries.len());
    let entry = entries[0];
    assert_file_path(entry, "kernel.rbs");
    assert!(entry.location.start_line > 0);
    assert!(entry.location.end_line > entry.location.start_line);
    assert_eq!(0, entry.location.start_column);
    assert!(entry.location.end_column > 0);
}

#[test]
fn test_index_core_constants() {
    let test = TestCase::new();
    assert_eq!(1, test.entries("RUBY_VERSION").len());
    assert_eq!(1, test.entries("Complex::I").len());
    assert_eq!(1, test.entries("Encoding::US_ASCII").len());
}

#[test]
fn test_index_methods() {
    let test = TestCase::new();
    let entry = find(&test, "initialize", "Array");
    assert_file_path(entry, "array.rbs");
    assert_eq!("public", entry.visibility.as_str());
    assert!(entry.location.start_line > 0);
    assert!(entry.location.end_line > entry.location.start_line);
    assert_eq!(2, entry.location.start_column);
    assert!(entry.location.end_column > 0);
}

#[test]
fn test_index_global_declaration() {
    let test = TestCase::new();
    let entries = test.entries("$DEBUG");
    assert_eq!(1, entries.len());
    let entry = entries[0];
    assert_eq!("GlobalVariable", kind_name(entry));
    assert_eq!("$DEBUG", entry.name);
    assert!(entry.file_path().unwrap().ends_with("/core/global_variables.rbs"));
    assert!(entry.location.start_column < entry.location.end_column);
    assert_eq!(entry.location.start_line, entry.location.end_line);
}

#[test]
fn test_attaches_correct_owner_to_singleton_methods() {
    let test = TestCase::new();
    let entry = test.first("basename");
    let owner = entry.owner().unwrap();
    assert_eq!("File::<Class:File>", owner);
    assert_eq!("SingletonClass", kind_name(test.first(owner)));
}

#[test]
fn test_location_and_name_location_are_the_same() {
    let test = TestCase::new();
    let entry = test.entries("Array").into_iter().find(|entry| kind_name(entry) == "Class").unwrap();
    assert_eq!(entry.location, entry.name_location());
}

#[test]
fn test_rbs_method_with_required_positionals() {
    let test = TestCase::new();
    let entries = test.entries("crypt");
    assert_eq!(1, entries.len());
    let signatures = entries[0].signatures();
    assert_eq!(1, signatures.len());
    assert_eq!(vec![Parameter::Required("salt_str".into())], signatures[0].parameters);
}

#[test]
fn test_rbs_method_with_unnamed_required_positionals() {
    let test = TestCase::new();
    let entry = find(&test, "try_convert", "Array::<Class:Array>");
    let parameters = &entry.signatures()[0].parameters;
    // RBS 4.1.3 names the parameter (`(A array) -> A`) where older RBS left it unnamed
    assert_eq!(vec!["array"], parameter_names(parameters));
    assert_eq!(vec!["RequiredParameter"], parameter_kinds(parameters));

    let signatures = parse_rbs_methods("class Array
  def self.try_convert: (untyped) -> Array[untyped]?
end
", "try_convert");
    assert_eq!(vec!["arg0"], parameter_names(&signatures[0].parameters));
}

#[test]
fn test_rbs_method_with_optional_positionals() {
    let test = TestCase::new();
    let entry = find(&test, "polar", "Complex::<Class:Complex>");
    let parameters = &entry.signatures()[0].parameters;
    assert_eq!(vec!["arg0", "arg1"], parameter_names(parameters));
    assert_eq!(vec!["RequiredParameter", "OptionalParameter"], parameter_kinds(parameters));
}

#[test]
fn test_rbs_method_with_optional_parameter() {
    let test = TestCase::new();
    let entries = test.entries("chomp");
    assert_eq!(1, entries.len());
    let signatures = entries[0].signatures();
    assert_eq!(1, signatures.len());
    assert_eq!(vec![Parameter::Optional("separator".into())], signatures[0].parameters);
}

#[test]
fn test_rbs_method_with_required_and_optional_parameters() {
    let test = TestCase::new();
    let entries = test.entries("gsub");
    assert_eq!(1, entries.len());
    let signatures = entries[0].signatures();
    assert_eq!(3, signatures.len());

    assert_eq!(vec!["pattern", "replacement"], parameter_names(&signatures[0].parameters));
    assert_eq!(vec!["RequiredParameter", "RequiredParameter"], parameter_kinds(&signatures[0].parameters));
    assert_eq!(vec!["pattern"], parameter_names(&signatures[1].parameters));
    assert_eq!(vec!["RequiredParameter"], parameter_kinds(&signatures[1].parameters));
    assert_eq!(vec!["pattern", "<anonymous block>"], parameter_names(&signatures[2].parameters));
    assert_eq!(vec!["RequiredParameter", "BlockParameter"], parameter_kinds(&signatures[2].parameters));
}

#[test]
fn test_rbs_anonymous_block_parameter() {
    let test = TestCase::new();
    let entry = find(&test, "open", "File::<Class:File>");
    let signatures = entry.signatures();
    assert_eq!(2, signatures.len());

    // RBS 4.1.3 adds the open options as optional keywords after the positionals that ruby-lsp's RBS declared
    let positionals = |signature: &Signature| -> Vec<String> {
        signature
            .parameters
            .iter()
            .filter(|parameter| matches!(parameter, Parameter::Required(_) | Parameter::Optional(_)))
            .map(|parameter| parameter.name().to_string())
            .collect()
    };
    assert_eq!(vec!["file_name", "mode", "perm"], positionals(&signatures[0]));
    assert_eq!(
        vec!["RequiredParameter", "OptionalParameter", "OptionalParameter"],
        parameter_kinds(&signatures[0].parameters[..3])
    );
    assert!(!signatures[0].parameters.iter().any(|parameter| matches!(parameter, Parameter::Block(_))));

    assert_eq!(vec!["file_name", "mode", "perm"], positionals(&signatures[1]));
    assert_eq!(Some(&Parameter::Block("<anonymous block>".into())), signatures[1].parameters.last());
}

#[test]
fn test_rbs_method_with_rest_positionals() {
    let test = TestCase::new();
    let entry = find(&test, "count", "String");
    let signatures = entry.signatures();
    assert_eq!(1, signatures.len());
    assert_eq!(vec!["selector_0", "more_selectors"], parameter_names(&signatures[0].parameters));
    assert_eq!(vec!["RequiredParameter", "RestParameter"], parameter_kinds(&signatures[0].parameters));
}

#[test]
fn test_rbs_method_with_trailing_positionals() {
    let test = TestCase::new();
    let entry = find(&test, "select", "IO::<Class:IO>");
    let signatures = entry.signatures();
    assert_eq!(2, signatures.len());

    assert_eq!(vec!["read_array", "write_array", "error_array"], parameter_names(&signatures[0].parameters));
    assert_eq!(
        vec!["RequiredParameter", "OptionalParameter", "OptionalParameter"],
        parameter_kinds(&signatures[0].parameters)
    );
    assert_eq!(vec!["read_array", "write_array", "error_array", "timeout"], parameter_names(&signatures[1].parameters));
    assert_eq!(
        vec!["RequiredParameter", "OptionalParameter", "OptionalParameter", "OptionalParameter"],
        parameter_kinds(&signatures[1].parameters)
    );
}

#[test]
fn test_rbs_method_with_optional_keywords() {
    let test = TestCase::new();
    let entry = find(&test, "step", "Numeric");
    let signatures = entry.signatures();
    assert_eq!(4, signatures.len());

    assert_eq!(vec!["limit", "step", "<anonymous block>"], parameter_names(&signatures[0].parameters));
    assert_eq!(
        vec!["OptionalParameter", "OptionalParameter", "BlockParameter"],
        parameter_kinds(&signatures[0].parameters)
    );
    assert_eq!(vec!["limit", "step"], parameter_names(&signatures[1].parameters));
    assert_eq!(vec!["OptionalParameter", "OptionalParameter"], parameter_kinds(&signatures[1].parameters));
    assert_eq!(vec!["by", "to", "<anonymous block>"], parameter_names(&signatures[2].parameters));
    assert_eq!(
        vec!["OptionalKeywordParameter", "OptionalKeywordParameter", "BlockParameter"],
        parameter_kinds(&signatures[2].parameters)
    );
    assert_eq!(vec!["by", "to"], parameter_names(&signatures[3].parameters));
    assert_eq!(vec!["OptionalKeywordParameter", "OptionalKeywordParameter"], parameter_kinds(&signatures[3].parameters));
}

#[test]
fn test_rbs_method_with_required_keywords() {
    let signatures = parse_rbs_methods(
        "class File\n  def foo: (a: ::Numeric sz, b: ::Numeric) -> void\nend\n",
        "foo",
    );
    assert_eq!(vec!["a", "b"], parameter_names(&signatures[0].parameters));
    assert_eq!(vec!["KeywordParameter", "KeywordParameter"], parameter_kinds(&signatures[0].parameters));
}

#[test]
fn test_rbs_method_with_rest_keywords() {
    let test = TestCase::new();
    let entry = find(&test, "method_missing", "BasicObject");
    let signatures = entry.signatures();
    assert_eq!(1, signatures.len());
    assert_eq!(
        vec!["arg0", "<anonymous splat>", "<anonymous keyword splat>"],
        parameter_names(&signatures[0].parameters)
    );
    assert_eq!(
        vec!["RequiredParameter", "RestParameter", "KeywordRestParameter"],
        parameter_kinds(&signatures[0].parameters)
    );
}

#[test]
fn test_parse_simple_rbs() {
    let rbs = "class File\n  def self?.open: (String name, ?String mode, ?Integer perm) -> IO?\n      | [T] (String name, ?String mode, ?Integer perm) { (IO) -> T } -> T\nend\n";
    let signatures = parse_rbs_methods(rbs, "open");
    assert_eq!(2, signatures.len());
    assert_eq!(vec!["name", "mode", "perm"], parameter_names(&signatures[0].parameters));
    assert_eq!(
        vec!["RequiredParameter", "OptionalParameter", "OptionalParameter"],
        parameter_kinds(&signatures[0].parameters)
    );
    assert_eq!(vec!["name", "mode", "perm", "<anonymous block>"], parameter_names(&signatures[1].parameters));
    assert_eq!(
        vec!["RequiredParameter", "OptionalParameter", "OptionalParameter", "BlockParameter"],
        parameter_kinds(&signatures[1].parameters)
    );
}

#[test]
fn test_signature_alias() {
    let test = TestCase::new();
    // RBS 4.1.3 no longer declares `alias any? all?`, so `alias size length` from array.rbs is used instead
    let entry = test
        .entries("size")
        .into_iter()
        .find(|entry| entry.owner() == Some("Array") && kind_name(entry) == "UnresolvedMethodAlias")
        .unwrap();
    let EntryKind::UnresolvedMethodAlias { new_name, old_name, .. } = &entry.kind else { panic!("expected alias") };
    assert_eq!("size", new_name);
    assert_eq!("length", old_name);
    assert!(entry.file_path().unwrap().ends_with("core/array.rbs"));
    assert!(!entry.comments.as_deref().unwrap_or("").is_empty());
}

#[test]
fn test_indexing_untyped_functions() {
    let mut test = TestCase::new();
    let entries = test.index.resolve_method("call", "Method", false).unwrap();
    let parameters = test.index.entry(entries[0]).signatures()[0].parameters.clone();
    assert_eq!(vec![Parameter::Forwarding], parameters);
}
