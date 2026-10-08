mod indexer_support;

use indexer_support::TestCase;

fn class_foo() -> Option<(&'static str, String)> {
    Some(("Class", "Foo".to_string()))
}

#[test]
fn test_class_variable_and_write() {
    let mut test = TestCase::new();
    test.index("class Foo\n  @@bar &&= 1\nend\n");
    test.assert_entry("@@bar", "ClassVariable", "/fake/path/foo.rb:1-2:1-7");
    assert_eq!(class_foo(), test.owner_of("@@bar"));
}

#[test]
fn test_class_variable_operator_write() {
    let mut test = TestCase::new();
    test.index("class Foo\n  @@bar += 1\nend\n");
    test.assert_entry("@@bar", "ClassVariable", "/fake/path/foo.rb:1-2:1-7");
}

#[test]
fn test_class_variable_or_write() {
    let mut test = TestCase::new();
    test.index("class Foo\n  @@bar ||= 1\nend\n");
    test.assert_entry("@@bar", "ClassVariable", "/fake/path/foo.rb:1-2:1-7");
}

#[test]
fn test_class_variable_target_node() {
    let mut test = TestCase::new();
    test.index("class Foo\n  @@foo, @@bar = 1\nend\n");
    test.assert_entry("@@foo", "ClassVariable", "/fake/path/foo.rb:1-2:1-7");
    test.assert_entry("@@bar", "ClassVariable", "/fake/path/foo.rb:1-9:1-14");
    assert_eq!(class_foo(), test.owner_of("@@foo"));
    assert_eq!(class_foo(), test.owner_of("@@bar"));
}

#[test]
fn test_class_variable_write() {
    let mut test = TestCase::new();
    test.index("class Foo\n  @@bar = 1\nend\n");
    test.assert_entry("@@bar", "ClassVariable", "/fake/path/foo.rb:1-2:1-7");
}

#[test]
fn test_empty_name_class_variable() {
    let mut test = TestCase::new();
    test.index("module Foo\n  @@ = 1\nend\n");
    test.refute_entry("@@");
}

#[test]
fn test_top_level_class_variable() {
    let mut test = TestCase::new();
    test.index("@@foo = 123\n");
    assert_eq!(None, test.first("@@foo").owner());
}

#[test]
fn test_class_variable_inside_self_method() {
    let mut test = TestCase::new();
    test.index("class Foo\n  def self.bar\n    @@bar = 123\n  end\nend\n");
    assert_eq!(class_foo(), test.owner_of("@@bar"));
}

#[test]
fn test_class_variable_inside_singleton_class() {
    let mut test = TestCase::new();
    test.index("class Foo\n  class << self\n    @@bar = 123\n  end\nend\n");
    assert_eq!(class_foo(), test.owner_of("@@bar"));
}

#[test]
fn test_class_variable_in_singleton_class_method() {
    let mut test = TestCase::new();
    test.index("class Foo\n  class << self\n    def self.bar\n      @@bar = 123\n    end\n  end\nend\n");
    assert_eq!(class_foo(), test.owner_of("@@bar"));
}
