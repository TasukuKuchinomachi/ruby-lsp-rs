use std::sync::Arc;

use ruby_lsp_rs::indexer::indexing::index_single;
use ruby_lsp_rs::indexer::reference_finder::{Reference, Target, find_references};
use ruby_lsp_rs::indexer::{Configuration, Index};

fn find(target: Target, source: &str) -> Vec<Reference> {
    let uri = ruby_lsp_rs::uri::from_path("/fake.rb");
    let mut index = Index::new(Arc::new(Configuration::default()));
    index_single(&mut index, &uri, source, true);
    find_references(&target, &mut index, source.as_bytes(), &uri, true)
}

fn find_const_references(const_name: &str, source: &str) -> Vec<Reference> {
    find(Target::Constant { fully_qualified_name: const_name.to_string() }, source)
}

fn find_method_references(method_name: &str, source: &str) -> Vec<Reference> {
    find(Target::Method { method_name: method_name.to_string() }, source)
}

fn find_instance_variable_references(name: &str, owner_ancestors: &[&str], source: &str) -> Vec<Reference> {
    let owner_ancestors = owner_ancestors.iter().map(|ancestor| ancestor.to_string()).collect();
    find(Target::InstanceVariable { name: name.to_string(), owner_ancestors }, source)
}

fn names_and_lines(references: &[Reference]) -> Vec<(&str, u32)> {
    references.iter().map(|reference| (reference.name.as_str(), reference.location.start_line)).collect()
}

#[test]
fn test_finds_constant_references() {
    let refs = find_const_references(
        "Foo::Bar",
        "module Foo\n  class Bar\n  end\n\n  Bar\nend\n\nFoo::Bar\n",
    );
    assert_eq!(vec![("Bar", 2), ("Bar", 5), ("Foo::Bar", 8)], names_and_lines(&refs[..3]));
}

#[test]
fn test_finds_constant_references_inside_singleton_contexts() {
    let refs = find_const_references(
        "Foo::<Class:Foo>::Bar",
        "class Foo\n  class << self\n    class Bar\n    end\n\n    Bar\n  end\nend\n",
    );
    assert_eq!(vec![("Bar", 3), ("Bar", 6)], names_and_lines(&refs[..2]));
}

#[test]
fn test_finds_top_level_constant_references() {
    let refs = find_const_references(
        "Bar",
        "class Bar\nend\n\nclass Foo\n  ::Bar\n\n  class << self\n    ::Bar\n  end\nend\n",
    );
    assert_eq!(vec![("Bar", 1), ("::Bar", 5), ("::Bar", 8)], names_and_lines(&refs[..3]));
}

#[test]
fn test_finds_method_references() {
    let refs = find_method_references("foo", "class Bar\n  def foo\n  end\n\n  def baz\n    foo\n  end\nend\n");
    assert_eq!(vec![("foo", 2), ("foo", 6)], names_and_lines(&refs));
}

#[test]
fn test_does_not_mismatch_on_readers_and_writers() {
    let refs = find_method_references(
        "foo",
        "class Bar\n  def foo\n  end\n\n  def foo=(value)\n  end\n\n  def baz\n    self.foo = 1\n    self.foo\n  end\nend\n",
    );
    assert_eq!(vec![("foo", 2), ("foo", 10)], names_and_lines(&refs));
}

#[test]
fn test_matches_writers() {
    let refs = find_method_references(
        "foo=",
        "class Bar\n  def foo\n  end\n\n  def foo=(value)\n  end\n\n  def baz\n    self.foo = 1\n    self.foo\n  end\nend\n",
    );
    assert_eq!(vec![("foo=", 5), ("foo=", 9)], names_and_lines(&refs));
}

#[test]
fn test_find_inherited_methods() {
    let refs = find_method_references("foo", "class Bar\n  def foo\n  end\nend\n\nclass Baz < Bar\n  super.foo\nend\n");
    assert_eq!(vec![("foo", 2), ("foo", 7)], names_and_lines(&refs));
}

#[test]
fn test_finds_methods_created_in_mixins() {
    let refs = find_method_references(
        "foo",
        "module Mixin\n  def foo\n  end\nend\n\nclass Bar\n  include Mixin\nend\n\nBar.foo\n",
    );
    assert_eq!(vec![("foo", 2), ("foo", 10)], names_and_lines(&refs));
}

#[test]
fn test_finds_singleton_methods() {
    let refs = find_method_references(
        "foo",
        "class Bar\n  class << self\n    def foo\n    end\n  end\n\n  def foo\n  end\nend\n\nBar.foo\n",
    );
    assert_eq!(vec![("foo", 3), ("foo", 7), ("foo", 11)], names_and_lines(&refs));
}

#[test]
fn test_finds_instance_variable_references() {
    let refs = find_instance_variable_references(
        "@name",
        &["Foo"],
        "class Foo\n  def initialize\n    @name = \"foo\"\n  end\n  def name\n    @name\n  end\n  def name_capital\n    @name[0]\n  end\nend\n\nclass Bar\n  def initialize\n    @name = \"foo\"\n  end\n  def name\n    @name\n  end\nend\n",
    );
    assert_eq!(vec![("@name", 3), ("@name", 6), ("@name", 9)], names_and_lines(&refs));
}

#[test]
fn test_finds_instance_variable_write_references() {
    let refs = find_instance_variable_references(
        "@foo",
        &["Foo"],
        "class Foo\n  def write\n    @foo = 1\n    @foo &&= 2\n    @foo ||= 3\n    @foo += 4\n    @foo, @bar = []\n  end\nend\n",
    );
    assert_eq!(vec![("@foo", 3), ("@foo", 4), ("@foo", 5), ("@foo", 6), ("@foo", 7)], names_and_lines(&refs));
}

#[test]
fn test_finds_instance_variable_references_in_owner_ancestors() {
    let source = "module Base\n  def change_name(name)\n    @name = name\n  end\n  def name\n    @name\n  end\n\n  module ::Top\n    def name\n      @name\n    end\n  end\nend\n\nclass Parent\n  def initialize\n    @name = \"parent\"\n  end\n  def name_capital\n    @name[0]\n  end\nend\n\nclass Foo < Parent\n  include Base\n  def initialize\n    @name = \"foo\"\n  end\n  def name\n    @name\n  end\nend\n\nclass Bar\n  def name\n    @name = \"bar\"\n  end\nend\n";
    let refs = find_instance_variable_references("@name", &["Foo", "Base", "Top", "Parent"], source);
    assert_eq!(
        vec![("@name", 3), ("@name", 6), ("@name", 11), ("@name", 18), ("@name", 21), ("@name", 28), ("@name", 31)],
        names_and_lines(&refs)
    );
}

#[test]
fn test_accounts_for_reopened_classes() {
    let refs = find_const_references("Foo", "class Foo\nend\nclass Foo\n  class Bar\n  end\nend\n\nFoo.new\n");
    assert_eq!(vec![("Foo", 1), ("Foo", 3), ("Foo", 8)], names_and_lines(&refs));
}
