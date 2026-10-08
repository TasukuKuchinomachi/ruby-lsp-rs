mod indexer_support;

use indexer_support::TestCase;

fn owned_by(kind: &'static str, name: &str) -> Option<(&'static str, String)> {
    Some((kind, name.to_string()))
}

#[test]
fn test_instance_variable_write() {
    let mut test = TestCase::new();
    test.index("module Foo\n  class Bar\n    def initialize\n      # Hello\n      @a = 1\n    end\n  end\nend\n");
    test.assert_entry("@a", "InstanceVariable", "/fake/path/foo.rb:4-6:4-8");
    assert_eq!(owned_by("Class", "Foo::Bar"), test.owner_of("@a"));
}

#[test]
fn test_instance_variable_with_multibyte_characters() {
    let mut test = TestCase::new();
    test.index("class Foo\n  def initialize\n    @あ = 1\n  end\nend\n");
    test.assert_entry("@あ", "InstanceVariable", "/fake/path/foo.rb:2-4:2-6");
}

#[test]
fn test_instance_variable_and_write() {
    let mut test = TestCase::new();
    test.index("module Foo\n  class Bar\n    def initialize\n      # Hello\n      @a &&= value\n    end\n  end\nend\n");
    test.assert_entry("@a", "InstanceVariable", "/fake/path/foo.rb:4-6:4-8");
    assert_eq!(owned_by("Class", "Foo::Bar"), test.owner_of("@a"));
}

#[test]
fn test_instance_variable_operator_write() {
    let mut test = TestCase::new();
    test.index("module Foo\n  class Bar\n    def initialize\n      # Hello\n      @a += value\n    end\n  end\nend\n");
    test.assert_entry("@a", "InstanceVariable", "/fake/path/foo.rb:4-6:4-8");
    assert_eq!(owned_by("Class", "Foo::Bar"), test.owner_of("@a"));
}

#[test]
fn test_instance_variable_or_write() {
    let mut test = TestCase::new();
    test.index("module Foo\n  class Bar\n    def initialize\n      # Hello\n      @a ||= value\n    end\n  end\nend\n");
    test.assert_entry("@a", "InstanceVariable", "/fake/path/foo.rb:4-6:4-8");
    assert_eq!(owned_by("Class", "Foo::Bar"), test.owner_of("@a"));
}

#[test]
fn test_instance_variable_target() {
    let mut test = TestCase::new();
    test.index("module Foo\n  class Bar\n    def initialize\n      # Hello\n      @a, @b = [1, 2]\n    end\n  end\nend\n");
    test.assert_entry("@a", "InstanceVariable", "/fake/path/foo.rb:4-6:4-8");
    test.assert_entry("@b", "InstanceVariable", "/fake/path/foo.rb:4-10:4-12");
    assert_eq!(owned_by("Class", "Foo::Bar"), test.owner_of("@a"));
    assert_eq!(owned_by("Class", "Foo::Bar"), test.owner_of("@b"));
}

#[test]
fn test_empty_name_instance_variables() {
    let mut test = TestCase::new();
    test.index("module Foo\n  class Bar\n    def initialize\n      @ = 123\n    end\n  end\nend\n");
    test.refute_entry("@");
}

#[test]
fn test_class_instance_variables() {
    let mut test = TestCase::new();
    test.index(
        "module Foo\n  class Bar\n    @a = 123\n\n    class << self\n      def hello\n        @b = 123\n      end\n\n      @c = 123\n    end\n  end\nend\n",
    );
    test.assert_entry("@a", "InstanceVariable", "/fake/path/foo.rb:2-4:2-6");
    assert_eq!(owned_by("SingletonClass", "Foo::Bar::<Class:Bar>"), test.owner_of("@a"));

    test.assert_entry("@b", "InstanceVariable", "/fake/path/foo.rb:6-8:6-10");
    assert_eq!(owned_by("SingletonClass", "Foo::Bar::<Class:Bar>"), test.owner_of("@b"));

    test.assert_entry("@c", "InstanceVariable", "/fake/path/foo.rb:9-6:9-8");
    assert_eq!(owned_by("SingletonClass", "Foo::Bar::<Class:Bar>::<Class:<Class:Bar>>"), test.owner_of("@c"));
}

#[test]
fn test_top_level_instance_variables() {
    let mut test = TestCase::new();
    test.index("@a = 123\n");
    assert_eq!(None, test.first("@a").owner());
}

#[test]
fn test_class_instance_variables_inside_self_method() {
    let mut test = TestCase::new();
    test.index("class Foo\n  def self.bar\n    @a = 123\n  end\nend\n");
    assert_eq!(owned_by("SingletonClass", "Foo::<Class:Foo>"), test.owner_of("@a"));
}

#[test]
fn test_instance_variable_inside_dynamic_method_declaration() {
    let mut test = TestCase::new();
    test.index("class Foo\n  def something.bar\n    @a = 123\n  end\nend\n");
    assert_eq!(owned_by("Class", "Foo"), test.owner_of("@a"));
}

#[test]
fn test_module_function_does_not_impact_instance_variables() {
    let mut test = TestCase::new();
    test.index("module Foo\n  module_function\n\n  def something; end\n\n  @a = 123\nend\n");
    assert_eq!(owned_by("SingletonClass", "Foo::<Class:Foo>"), test.owner_of("@a"));
}

#[test]
fn test_class_instance_variable_comments() {
    let mut test = TestCase::new();
    test.index(
        "  class Foo\n    # Documentation for @a\n    @a = \"Hello\" #: String\n    @b = \"World\" # trailing comment\n    @c = \"!\"\n  end\nend\n",
    );
    test.assert_entry("@a", "InstanceVariable", "/fake/path/foo.rb:2-4:2-6");
    assert_eq!(Some("Documentation for @a"), test.first("@a").comments.as_deref());

    test.assert_entry("@b", "InstanceVariable", "/fake/path/foo.rb:3-4:3-6");
    assert_eq!(Some(""), test.first("@b").comments.as_deref());

    test.assert_entry("@c", "InstanceVariable", "/fake/path/foo.rb:4-4:4-6");
    assert_eq!(Some(""), test.first("@c").comments.as_deref());
}
