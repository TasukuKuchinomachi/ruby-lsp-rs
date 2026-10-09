mod indexer_support;

use std::path::PathBuf;
use std::sync::Arc;

use indexer_support::{DEFAULT_URI, TestCase};
use ruby_lsp_rs::indexer::indexing::{index_single, index_single_with_require_path};

fn mixin_operation_module_names(test: &TestCase, name: &str) -> Vec<String> {
    test.first(name)
        .namespace()
        .expect("expected a namespace")
        .mixin_operations
        .iter()
        .map(|operation| operation.module_name().to_string())
        .collect()
}

fn parent_class(test: &TestCase, name: &str) -> Option<String> {
    test.first(name).namespace().expect("expected a namespace").parent_class.clone()
}

fn comments(test: &TestCase, name: &str) -> Option<String> {
    test.first(name).comments.clone()
}

fn lazy_comments(test: &mut TestCase, name: &str) -> String {
    let id = test.ids(name)[0];
    test.index.comments_for(id)
}

fn temporary_ruby_file(test_name: &str, source: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(format!("ruby_lsp_rs_classes_and_modules_{}_{test_name}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("foo.rb");
    std::fs::write(&path, source).unwrap();
    path
}

fn index_lazily_from_file(test: &mut TestCase, test_name: &str, source: &str) -> PathBuf {
    let path = temporary_ruby_file(test_name, source);
    let uri = ruby_lsp_rs::uri::from_path_buf(&path);
    index_single(&mut test.index, &uri, source, false);
    path
}

fn reference_repository() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ruby-lsp")
}

#[test]
fn test_empty_statements_class() {
    let mut test = TestCase::new();
    test.index("class Foo\nend\n");
    test.assert_entry("Foo", "Class", "/fake/path/foo.rb:0-0:1-3");
}

#[test]
fn test_conditional_class() {
    let mut test = TestCase::new();
    test.index("class Foo\nend if condition\n");
    test.assert_entry("Foo", "Class", "/fake/path/foo.rb:0-0:1-3");
}

#[test]
fn test_class_with_statements() {
    let mut test = TestCase::new();
    test.index("class Foo\n  def something; end\nend\n");
    test.assert_entry("Foo", "Class", "/fake/path/foo.rb:0-0:2-3");
}

#[test]
fn test_colon_colon_class() {
    let mut test = TestCase::new();
    test.index("class ::Foo\nend\n");
    test.assert_entry("Foo", "Class", "/fake/path/foo.rb:0-0:1-3");
}

#[test]
fn test_colon_colon_class_inside_class() {
    let mut test = TestCase::new();
    test.index("class Bar\n  class ::Foo\n  end\nend\n");
    test.assert_entry("Bar", "Class", "/fake/path/foo.rb:0-0:3-3");
    test.assert_entry("Foo", "Class", "/fake/path/foo.rb:1-2:2-5");
}

#[test]
fn test_namespaced_class() {
    let mut test = TestCase::new();
    test.index("class Foo::Bar\nend\n");
    test.assert_entry("Foo::Bar", "Class", "/fake/path/foo.rb:0-0:1-3");
}

#[test]
fn test_dynamically_namespaced_class() {
    let mut test = TestCase::new();
    test.index("class self::Bar\nend\n");
    test.assert_entry("self::Bar", "Class", "/fake/path/foo.rb:0-0:1-3");
}

#[test]
fn test_dynamically_namespaced_class_does_not_affect_other_classes() {
    let mut test = TestCase::new();
    test.index("class Foo\n  class self::Bar\n  end\n\n  class Bar\n  end\nend\n");
    test.refute_entry("self::Bar");
    test.assert_entry("Foo", "Class", "/fake/path/foo.rb:0-0:6-3");
    test.assert_entry("Foo::Bar", "Class", "/fake/path/foo.rb:4-2:5-5");
}

#[test]
fn test_empty_statements_module() {
    let mut test = TestCase::new();
    test.index("module Foo\nend\n");
    test.assert_entry("Foo", "Module", "/fake/path/foo.rb:0-0:1-3");
}

#[test]
fn test_conditional_module() {
    let mut test = TestCase::new();
    test.index("module Foo\nend if condition\n");
    test.assert_entry("Foo", "Module", "/fake/path/foo.rb:0-0:1-3");
}

#[test]
fn test_module_with_statements() {
    let mut test = TestCase::new();
    test.index("module Foo\n  def something; end\nend\n");
    test.assert_entry("Foo", "Module", "/fake/path/foo.rb:0-0:2-3");
}

#[test]
fn test_colon_colon_module() {
    let mut test = TestCase::new();
    test.index("module ::Foo\nend\n");
    test.assert_entry("Foo", "Module", "/fake/path/foo.rb:0-0:1-3");
}

#[test]
fn test_namespaced_module() {
    let mut test = TestCase::new();
    test.index("module Foo::Bar\nend\n");
    test.assert_entry("Foo::Bar", "Module", "/fake/path/foo.rb:0-0:1-3");
}

#[test]
fn test_dynamically_namespaced_module() {
    let mut test = TestCase::new();
    test.index("module self::Bar\nend\n");
    test.assert_entry("self::Bar", "Module", "/fake/path/foo.rb:0-0:1-3");
}

#[test]
fn test_dynamically_namespaced_module_does_not_affect_other_modules() {
    let mut test = TestCase::new();
    test.index("module Foo\n  class self::Bar\n  end\n\n  module Bar\n  end\nend\n");
    test.assert_entry("Foo::self::Bar", "Class", "/fake/path/foo.rb:1-2:2-5");
    test.assert_entry("Foo", "Module", "/fake/path/foo.rb:0-0:6-3");
    test.assert_entry("Foo::Bar", "Module", "/fake/path/foo.rb:4-2:5-5");
}

#[test]
fn test_nested_modules_and_classes_with_multibyte_characters() {
    let mut test = TestCase::new();
    test.index("module A動物\n  class Bねこ; end\nend\n");
    test.assert_entry("A動物", "Module", "/fake/path/foo.rb:0-0:2-3");
    test.assert_entry("A動物::Bねこ", "Class", "/fake/path/foo.rb:1-2:1-16");
}

#[test]
fn test_nested_modules_and_classes() {
    let mut test = TestCase::new();
    test.index(
        "module Foo\n  class Bar\n  end\n\n  module Baz\n    class Qux\n      class Something\n      end\n    end\n  end\nend\n",
    );
    test.assert_entry("Foo", "Module", "/fake/path/foo.rb:0-0:10-3");
    test.assert_entry("Foo::Bar", "Class", "/fake/path/foo.rb:1-2:2-5");
    test.assert_entry("Foo::Baz", "Module", "/fake/path/foo.rb:4-2:9-5");
    test.assert_entry("Foo::Baz::Qux", "Class", "/fake/path/foo.rb:5-4:8-7");
    test.assert_entry("Foo::Baz::Qux::Something", "Class", "/fake/path/foo.rb:6-6:7-9");
}

#[test]
fn test_deleting_from_index_based_on_file_path() {
    let mut test = TestCase::new();
    test.index("class Foo\nend\n");
    test.assert_entry("Foo", "Class", "/fake/path/foo.rb:0-0:1-3");

    test.index.delete(DEFAULT_URI, false);
    test.refute_entry("Foo");
    test.assert_no_indexed_entries();
}

#[test]
fn test_comments_can_be_attached_to_a_class() {
    let mut test = TestCase::new();
    test.index(
        "# This is method comment\ndef foo; end\n# This is a Foo comment\n# This is another Foo comment\nclass Foo\n  # This should not be attached\nend\n\n# Ignore me\n\n# This Bar comment has 1 line padding\n\nclass Bar; end\n",
    );
    assert_eq!(Some("This is a Foo comment\nThis is another Foo comment".to_string()), comments(&test, "Foo"));
    assert_eq!(Some("This Bar comment has 1 line padding".to_string()), comments(&test, "Bar"));
}

#[test]
fn test_skips_comments_containing_invalid_encodings() {
    let mut test = TestCase::new();
    let source: &[u8] = b"# comment \xBA\nclass Foo\nend\n";
    index_single_with_require_path(&mut test.index, Arc::from(DEFAULT_URI), source, true, None);
    assert!(!test.entries("Foo").is_empty());
}

#[test]
fn test_comments_can_be_attached_to_a_namespaced_class() {
    let mut test = TestCase::new();
    test.index(
        "# This is a Foo comment\n# This is another Foo comment\nclass Foo\n  # This is a Bar comment\n  class Bar; end\nend\n",
    );
    assert_eq!(Some("This is a Foo comment\nThis is another Foo comment".to_string()), comments(&test, "Foo"));
    assert_eq!(Some("This is a Bar comment".to_string()), comments(&test, "Foo::Bar"));
}

#[test]
fn test_comments_can_be_attached_to_a_reopened_class() {
    let mut test = TestCase::new();
    test.index("# This is a Foo comment\nclass Foo; end\n\n# This is another Foo comment\nclass Foo; end\n");
    let entries = test.entries("Foo");
    assert_eq!(Some("This is a Foo comment"), entries[0].comments.as_deref());
    assert_eq!(Some("This is another Foo comment"), entries[1].comments.as_deref());
}

#[test]
fn test_comments_removes_the_leading_pound_and_space() {
    let mut test = TestCase::new();
    test.index("# This is a Foo comment\nclass Foo; end\n\n#This is a Bar comment\nclass Bar; end\n");
    assert_eq!(Some("This is a Foo comment".to_string()), comments(&test, "Foo"));
    assert_eq!(Some("This is a Bar comment".to_string()), comments(&test, "Bar"));
}

#[test]
fn test_private_class_and_module_indexing() {
    let mut test = TestCase::new();
    test.index(
        "class A\n  class B; end\n  private_constant(:B)\n\n  module C; end\n  private_constant(\"C\")\n\n  class D; end\nend\n",
    );
    assert!(test.first("A::B").is_private());
    assert!(test.first("A::C").is_private());
    assert!(test.first("A::D").is_public());
}

#[test]
fn test_keeping_track_of_super_classes() {
    let mut test = TestCase::new();
    test.index(
        "class Foo < Bar\nend\n\nclass Baz\nend\n\nmodule Something\n  class Baz\n  end\n\n  class Qux < ::Baz\n  end\nend\n\nclass FinalThing < Something::Baz\nend\n",
    );
    assert_eq!(Some("Bar".to_string()), parent_class(&test, "Foo"));
    assert_eq!(Some("::Object".to_string()), parent_class(&test, "Baz"));
    assert_eq!(Some("::Baz".to_string()), parent_class(&test, "Something::Qux"));
    assert_eq!(Some("Something::Baz".to_string()), parent_class(&test, "FinalThing"));
}

fn mixin_source(operation: &str) -> String {
    format!(
        "class Foo
  # valid syntaxes that we can index
  {operation} A1
  self.{operation} A2
  {operation} A3, A4
  self.{operation} A5, A6

  # valid syntaxes that we cannot index because of their dynamic nature
  {operation} some_variable_or_method_call
  self.{operation} some_variable_or_method_call

  def something
    {operation} A7 # We should not index this because of this dynamic nature
  end

  # Valid inner class syntax definition with its own modules included
  class Qux
    {operation} Corge
    self.{operation} Corge
    {operation} Baz

    {operation} some_variable_or_method_call
  end
end

class ConstantPathReferences
  {operation} Foo::Bar
  self.{operation} Foo::Bar2

  {operation} dynamic::Bar
  {operation} Foo::
end
"
    )
}

#[test]
fn test_keeping_track_of_included_modules() {
    let mut test = TestCase::new();
    test.index(&mixin_source("include"));
    assert_eq!(vec!["A1", "A2", "A3", "A4", "A5", "A6"], mixin_operation_module_names(&test, "Foo"));
    assert_eq!(vec!["Corge", "Corge", "Baz"], mixin_operation_module_names(&test, "Foo::Qux"));
    assert_eq!(vec!["Foo::Bar", "Foo::Bar2"], mixin_operation_module_names(&test, "ConstantPathReferences"));
}

#[test]
fn test_keeping_track_of_prepended_modules() {
    let mut test = TestCase::new();
    test.index(&mixin_source("prepend"));
    assert_eq!(vec!["A1", "A2", "A3", "A4", "A5", "A6"], mixin_operation_module_names(&test, "Foo"));
    assert_eq!(vec!["Corge", "Corge", "Baz"], mixin_operation_module_names(&test, "Foo::Qux"));
    assert_eq!(vec!["Foo::Bar", "Foo::Bar2"], mixin_operation_module_names(&test, "ConstantPathReferences"));
}

#[test]
fn test_keeping_track_of_extended_modules() {
    let mut test = TestCase::new();
    test.index(&mixin_source("extend"));
    assert_eq!(vec!["A1", "A2", "A3", "A4", "A5", "A6"], mixin_operation_module_names(&test, "Foo::<Class:Foo>"));
    assert_eq!(vec!["Corge", "Corge", "Baz"], mixin_operation_module_names(&test, "Foo::Qux::<Class:Qux>"));
    assert_eq!(
        vec!["Foo::Bar", "Foo::Bar2"],
        mixin_operation_module_names(&test, "ConstantPathReferences::<Class:ConstantPathReferences>")
    );
}

#[test]
fn test_tracking_singleton_classes() {
    let mut test = TestCase::new();
    test.index("class Foo; end\nclass Foo\n  # Some extra comments\n  class << self\n  end\nend\n");
    let foo = test.first("Foo::<Class:Foo>");
    assert_eq!(4, foo.location.start_line);
    assert_eq!(Some("Some extra comments"), foo.comments.as_deref());
}

#[test]
fn test_dynamic_singleton_class_blocks() {
    let mut test = TestCase::new();
    test.index("class Foo\n  # Some extra comments\n  class << bar\n  end\nend\n");
    let singleton = test.first("Foo::<Class:bar>");
    assert_eq!(3, singleton.location.start_line);
    assert_eq!(Some("Some extra comments"), singleton.comments.as_deref());
}

#[test]
fn test_namespaces_inside_singleton_blocks() {
    let mut test = TestCase::new();
    test.index("class Foo\n  class << self\n    class Bar\n    end\n  end\nend\n");
    test.assert_entry("Foo::<Class:Foo>::Bar", "Class", "/fake/path/foo.rb:2-4:3-7");
}

#[test]
fn test_name_location_points_to_constant_path_location() {
    let mut test = TestCase::new();
    test.index("class Foo\n  def foo; end\nend\n\nmodule Bar\n  def bar; end\nend\n");

    let foo = test.first("Foo");
    assert_ne!(foo.location, foo.name_location());
    let name_location = foo.name_location();
    assert_eq!(1, name_location.start_line);
    assert_eq!(1, name_location.end_line);
    assert_eq!(6, name_location.start_column);
    assert_eq!(9, name_location.end_column);

    let bar = test.first("Bar");
    assert_ne!(bar.location, bar.name_location());
    let name_location = bar.name_location();
    assert_eq!(5, name_location.start_line);
    assert_eq!(5, name_location.end_line);
    assert_eq!(7, name_location.start_column);
    assert_eq!(10, name_location.end_column);
}

#[test]
fn test_indexing_namespaces_inside_top_level_references() {
    let mut test = TestCase::new();
    test.index("module ::Foo\n  class Bar\n  end\nend\n");
    assert!(!test.index.is_indexed("::Foo"));
    assert!(!test.index.is_indexed("::Foo::Bar"));
    test.assert_entry("Foo", "Module", "/fake/path/foo.rb:0-0:3-3");
    test.assert_entry("Foo::Bar", "Class", "/fake/path/foo.rb:1-2:2-5");
}

#[test]
fn test_indexing_singletons_inside_top_level_references() {
    let mut test = TestCase::new();
    test.index("module ::Foo\n  class Bar\n    class << self\n    end\n  end\nend\n");
    assert!(!test.index.is_indexed("::Foo"));
    assert!(!test.index.is_indexed("::Foo::Bar"));
    assert!(!test.index.is_indexed("::Foo::Bar::<Class:Bar>"));
    test.assert_entry("Foo", "Module", "/fake/path/foo.rb:0-0:5-3");
    test.assert_entry("Foo::Bar", "Class", "/fake/path/foo.rb:1-2:4-5");
    test.assert_entry("Foo::Bar::<Class:Bar>", "SingletonClass", "/fake/path/foo.rb:2-4:3-7");
}

#[test]
fn test_indexing_namespaces_inside_nested_top_level_references() {
    let mut test = TestCase::new();
    test.index("class Baz\n  module ::Foo\n    class Bar\n    end\n\n    class ::Qux\n    end\n  end\nend\n");
    test.refute_entry("Baz::Foo");
    test.refute_entry("Baz::Foo::Bar");
    test.assert_entry("Baz", "Class", "/fake/path/foo.rb:0-0:8-3");
    test.assert_entry("Foo", "Module", "/fake/path/foo.rb:1-2:7-5");
    test.assert_entry("Foo::Bar", "Class", "/fake/path/foo.rb:2-4:3-7");
    test.assert_entry("Qux", "Class", "/fake/path/foo.rb:5-4:6-7");
}

#[test]
fn test_lazy_comment_fetching_uses_correct_line_breaks_for_rendering() {
    let path = reference_repository().join("lib/ruby_lsp/node_context.rb");
    let mut test = TestCase::new();
    let source = std::fs::read_to_string(&path).unwrap();
    index_single(&mut test.index, &ruby_lsp_rs::uri::from_path_buf(&path), &source, false);

    assert_eq!(
        "This class allows listeners to access contextual information about a node in the AST, such as its parent,\nits namespace nesting, and the surrounding CallNode (e.g. a method call).",
        lazy_comments(&mut test, "RubyLsp::NodeContext")
    );
}

#[test]
fn test_lazy_comment_fetching_does_not_fail_if_file_gets_deleted() {
    let mut test = TestCase::new();
    let path = std::env::temp_dir().join("ruby_lsp_rs_does_not_exist/does_not_exist.rb");
    index_single(&mut test.index, &ruby_lsp_rs::uri::from_path_buf(&path), "class Foo\nend\n", false);
    assert!(lazy_comments(&mut test, "Foo").is_empty());
}

#[test]
fn test_singleton_inside_compact_namespace() {
    let mut test = TestCase::new();
    test.index("module Foo::Bar\n  class << self\n    def baz; end\n  end\nend\n");
    assert!(test.index.get("Foo::Bar::<Class:Foo::Bar>").is_none());
    test.assert_entry("Foo::Bar::<Class:Bar>", "SingletonClass", "/fake/path/foo.rb:1-2:3-5");
    assert_eq!(Some("Foo::Bar::<Class:Bar>"), test.first("baz").owner());
}

#[test]
fn test_lazy_comments_with_spaces_are_properly_attributed() {
    let mut test = TestCase::new();
    let source = "require \"whatever\"\n\n# These comments belong to the declaration below\n# They have to be associated with it\n\nclass Foo\nend\n";
    let path = index_lazily_from_file(&mut test, "with_spaces", source);
    let comments = lazy_comments(&mut test, "Foo");
    std::fs::remove_file(&path).unwrap();
    assert_eq!("These comments belong to the declaration below\nThey have to be associated with it", comments);
}

#[test]
fn test_lazy_comments_with_no_spaces_are_properly_attributed() {
    let mut test = TestCase::new();
    let source = "require \"whatever\"\n\n# These comments belong to the declaration below\n# They have to be associated with it\nclass Foo\nend\n";
    let path = index_lazily_from_file(&mut test, "no_spaces", source);
    let comments = lazy_comments(&mut test, "Foo");
    std::fs::remove_file(&path).unwrap();
    assert_eq!("These comments belong to the declaration below\nThey have to be associated with it", comments);
}

#[test]
fn test_lazy_comments_with_two_extra_spaces_are_properly_ignored() {
    let mut test = TestCase::new();
    let source = "require \"whatever\"\n\n# These comments don't belong to the declaration below\n# They will not be associated with it\n\n\nclass Foo\nend\n";
    let path = index_lazily_from_file(&mut test, "two_extra_spaces", source);
    let comments = lazy_comments(&mut test, "Foo");
    std::fs::remove_file(&path).unwrap();
    assert!(comments.is_empty());
}

#[test]
fn test_lazy_comments_ignores_magic_comments() {
    let mut test = TestCase::new();
    let source = "# frozen_string_literal: true\n\nclass Foo\nend\n";
    let path = index_lazily_from_file(&mut test, "magic_comments", source);
    let comments = lazy_comments(&mut test, "Foo");
    std::fs::remove_file(&path).unwrap();
    assert!(comments.is_empty());
}
