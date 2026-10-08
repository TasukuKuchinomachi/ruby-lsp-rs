mod indexer_support;

use indexer_support::{TestCase, kind_name, parameter_kinds, parameter_names};
use ruby_lsp_rs::indexer::Entry;
use ruby_lsp_rs::indexer::entry::{
    BLOCK_PARAMETER_DEFAULT_NAME, KEYWORD_REST_PARAMETER_DEFAULT_NAME, Parameter, REST_PARAMETER_DEFAULT_NAME,
};

fn signature_matches(entry: &Entry, call_string: &str) -> bool {
    let signature = entry.signatures().into_iter().next().expect("entry must have a signature");
    let parse_result = ruby_prism::parse(call_string.as_bytes());
    let program = parse_result.node();
    let program = program.as_program_node().expect("program node");
    let first_statement = program.statements().body().iter().next().expect("one statement");
    let arguments: Vec<ruby_prism::Node<'_>> = match first_statement.as_call_node().and_then(|call| call.arguments()) {
        Some(arguments) => arguments.arguments().iter().collect(),
        None => Vec::new(),
    };
    signature.matches(&arguments)
}

fn assert_signature_matches(entry: &Entry, call_string: &str) {
    assert!(
        signature_matches(entry, call_string),
        "Expected {call_string} to match {}{}",
        entry.name,
        entry.decorated_parameters()
    );
}

fn refute_signature_matches(entry: &Entry, call_string: &str) {
    assert!(
        !signature_matches(entry, call_string),
        "Expected {call_string} to not match {}{}",
        entry.name,
        entry.decorated_parameters()
    );
}

fn first_parameters(test: &TestCase, name: &str) -> Vec<Parameter> {
    test.first(name).signatures().into_iter().next().map(|signature| signature.parameters).unwrap_or_default()
}

#[test]
fn test_method_with_no_parameters() {
    let mut test = TestCase::new();
    test.index("class Foo\n  def bar\n  end\nend\n");
    test.assert_entry("bar", "Method", "/fake/path/foo.rb:1-2:2-5");
}

#[test]
fn test_conditional_method() {
    let mut test = TestCase::new();
    test.index("class Foo\n  def bar\n  end if condition\nend\n");
    test.assert_entry("bar", "Method", "/fake/path/foo.rb:1-2:2-5");
}

#[test]
fn test_method_with_multibyte_characters() {
    let mut test = TestCase::new();
    test.index("class Foo\n  def こんにちは; end\nend\n");
    test.assert_entry("こんにちは", "Method", "/fake/path/foo.rb:1-2:1-16");
}

#[test]
fn test_singleton_method_using_self_receiver() {
    let mut test = TestCase::new();
    test.index("class Foo\n  def self.bar\n  end\nend\n");
    test.assert_entry("bar", "Method", "/fake/path/foo.rb:1-2:2-5");
    assert_eq!(Some(("SingletonClass", "Foo::<Class:Foo>".to_string())), test.owner_of("bar"));
}

#[test]
fn test_singleton_method_using_other_receiver_is_not_indexed() {
    let mut test = TestCase::new();
    test.index("class Foo\n  def String.bar\n  end\nend\n");
    test.assert_no_entry("bar");
}

#[test]
fn test_method_under_dynamic_class_or_module() {
    let mut test = TestCase::new();
    test.index("module Foo\n  class self::Bar\n    def bar\n    end\n  end\nend\n\nmodule Bar\n  def bar\n  end\nend\n");
    let entries = test.entries("bar");
    assert_eq!(2, entries.len());
    assert_eq!(Some("Foo::self::Bar"), entries[0].owner());
    assert_eq!(Some("Bar"), entries[1].owner());
}

#[test]
fn test_visibility_tracking() {
    let mut test = TestCase::new();
    test.index("class Foo\n  private def foo\n  end\n\n  def bar; end\n\n  protected\n\n  def baz; end\nend\n");
    test.assert_entry_with_visibility("foo", "Method", "/fake/path/foo.rb:1-10:2-5", "private");
    test.assert_entry_with_visibility("bar", "Method", "/fake/path/foo.rb:4-2:4-14", "public");
    test.assert_entry_with_visibility("baz", "Method", "/fake/path/foo.rb:8-2:8-14", "protected");
}

#[test]
fn test_visibility_tracking_with_nested_class_or_modules() {
    let mut test = TestCase::new();
    test.index(
        "class Foo\n  private\n\n  def foo; end\n\n  class Bar\n    def bar; end\n  end\n\n  def baz; end\nend\n",
    );
    test.assert_entry_with_visibility("foo", "Method", "/fake/path/foo.rb:3-2:3-14", "private");
    test.assert_entry_with_visibility("bar", "Method", "/fake/path/foo.rb:6-4:6-16", "public");
    test.assert_entry_with_visibility("baz", "Method", "/fake/path/foo.rb:9-2:9-14", "private");
}

#[test]
fn test_visibility_tracking_with_module_function() {
    let mut test = TestCase::new();
    test.index("module Test\n  def foo; end\n  def bar; end\n  module_function :foo, \"bar\"\nend\n");

    for keyword in ["foo", "bar"] {
        let entries = test.entries(keyword);
        assert_eq!(2, entries.len());
        let (first_entry, second_entry) = (entries[0], entries[1]);
        assert_eq!(Some("Test"), first_entry.owner());
        assert_eq!("Module", kind_name(test.first("Test")));
        assert!(first_entry.is_private());
        assert_eq!(Some("Test::<Class:Test>"), second_entry.owner());
        assert_eq!("SingletonClass", kind_name(test.first("Test::<Class:Test>")));
        assert_eq!("public", second_entry.visibility.as_str());
    }
}

#[test]
fn test_private_class_method_visibility_tracking_string_symbol_arguments() {
    let mut test = TestCase::new();
    test.index(
        "class Test\n  def self.foo\n  end\n\n  def self.bar\n  end\n\n  private_class_method(\"foo\", :bar)\n\n  def self.baz\n  end\nend\n",
    );
    for keyword in ["foo", "bar"] {
        let entries = test.entries(keyword);
        assert_eq!(1, entries.len());
        assert!(entries[0].is_private());
    }
    let entries = test.entries("baz");
    assert_eq!(1, entries.len());
    assert!(entries[0].is_public());
}

#[test]
fn test_private_class_method_visibility_tracking_array_argument() {
    let mut test = TestCase::new();
    test.index(
        "class Test\n  def self.foo\n  end\n\n  def self.bar\n  end\n\n  private_class_method([\"foo\", :bar])\n\n  def self.baz\n  end\nend\n",
    );
    for keyword in ["foo", "bar"] {
        let entries = test.entries(keyword);
        assert_eq!(1, entries.len());
        assert!(entries[0].is_private());
    }
    let entries = test.entries("baz");
    assert_eq!(1, entries.len());
    assert!(entries[0].is_public());
}

#[test]
fn test_private_class_method_visibility_tracking_method_argument() {
    let mut test = TestCase::new();
    test.index("class Test\n  private_class_method def self.foo\n  end\n\n  def self.bar\n  end\nend\n");
    let entries = test.entries("foo");
    assert_eq!(1, entries.len());
    assert!(entries[0].is_private());
    let entries = test.entries("bar");
    assert_eq!(1, entries.len());
    assert!(entries[0].is_public());
}

#[test]
fn test_comments_documentation() {
    let mut test = TestCase::new();
    test.index(
        "# Documentation for Foo\n\nclass Foo\n  # ####################\n  # Documentation for bar\n  # ####################\n  #\n  def bar\n  end\n\n  # test\n\n  # Documentation for baz\n  def baz; end\n  def ban; end\nend\n",
    );
    let foo = test.index.get_owned("Foo").unwrap()[0];
    assert_eq!("Documentation for Foo", test.index.comments_for(foo));
    let bar = test.index.get_owned("bar").unwrap()[0];
    assert_eq!("####################\nDocumentation for bar\n####################\n", test.index.comments_for(bar));
    let baz = test.index.get_owned("baz").unwrap()[0];
    assert_eq!("Documentation for baz", test.index.comments_for(baz));
    let ban = test.index.get_owned("ban").unwrap()[0];
    assert!(test.index.comments_for(ban).is_empty());
}

#[test]
fn test_method_with_parameters() {
    let mut test = TestCase::new();
    test.index("class Foo\n  def bar(a)\n  end\nend\n");
    test.assert_entry("bar", "Method", "/fake/path/foo.rb:1-2:2-5");
    assert_eq!(vec![Parameter::Required("a".into())], first_parameters(&test, "bar"));
}

#[test]
fn test_method_with_destructed_parameters() {
    let mut test = TestCase::new();
    test.index("class Foo\n  def bar((a, (b, )))\n  end\nend\n");
    test.assert_entry("bar", "Method", "/fake/path/foo.rb:1-2:2-5");
    assert_eq!(vec![Parameter::Required("(a, (b, ))".into())], first_parameters(&test, "bar"));
}

#[test]
fn test_method_with_optional_parameters() {
    let mut test = TestCase::new();
    test.index("class Foo\n  def bar(a = 123)\n  end\nend\n");
    test.assert_entry("bar", "Method", "/fake/path/foo.rb:1-2:2-5");
    assert_eq!(vec![Parameter::Optional("a".into())], first_parameters(&test, "bar"));
}

#[test]
fn test_method_with_keyword_parameters() {
    let mut test = TestCase::new();
    test.index("class Foo\n  def bar(a:, b: 123)\n  end\nend\n");
    test.assert_entry("bar", "Method", "/fake/path/foo.rb:1-2:2-5");
    let parameters = first_parameters(&test, "bar");
    assert_eq!(vec!["a", "b"], parameter_names(&parameters));
    assert_eq!(vec!["KeywordParameter", "OptionalKeywordParameter"], parameter_kinds(&parameters));
}

#[test]
fn test_method_with_rest_and_keyword_rest_parameters() {
    let mut test = TestCase::new();
    test.index("class Foo\n  def bar(*a, **b)\n  end\nend\n");
    test.assert_entry("bar", "Method", "/fake/path/foo.rb:1-2:2-5");
    let parameters = first_parameters(&test, "bar");
    assert_eq!(vec!["a", "b"], parameter_names(&parameters));
    assert_eq!(vec!["RestParameter", "KeywordRestParameter"], parameter_kinds(&parameters));
}

#[test]
fn test_method_with_post_parameters() {
    let mut test = TestCase::new();
    test.index(
        "class Foo\n  def bar(*a, b)\n  end\n\n  def baz(**a, b)\n  end\n\n  def qux(*a, (b, c))\nend\n",
    );
    test.assert_entry("bar", "Method", "/fake/path/foo.rb:1-2:2-5");
    let parameters = first_parameters(&test, "bar");
    assert_eq!(vec!["a", "b"], parameter_names(&parameters));
    assert_eq!(vec!["RestParameter", "RequiredParameter"], parameter_kinds(&parameters));

    let parameters = first_parameters(&test, "baz");
    assert_eq!(vec!["a", "b"], parameter_names(&parameters));
    assert_eq!(vec!["KeywordRestParameter", "RequiredParameter"], parameter_kinds(&parameters));

    let parameters = first_parameters(&test, "qux");
    assert_eq!(2, parameters.len());
    assert_eq!(Parameter::Required("(b, c)".into()), parameters[1]);
}

#[test]
fn test_method_with_destructured_rest_parameters() {
    let mut test = TestCase::new();
    test.index("class Foo\n  def bar((a, *b))\n  end\nend\n");
    test.assert_entry("bar", "Method", "/fake/path/foo.rb:1-2:2-5");
    assert_eq!(vec![Parameter::Required("(a, *b)".into())], first_parameters(&test, "bar"));
}

#[test]
fn test_method_with_block_parameters() {
    let mut test = TestCase::new();
    test.index("class Foo\n  def bar(&block)\n  end\n\n  def baz(&)\n  end\nend\n");
    assert_eq!(Parameter::Block("block".into()), first_parameters(&test, "bar")[0]);
    assert_eq!(vec![Parameter::Block(BLOCK_PARAMETER_DEFAULT_NAME.into())], first_parameters(&test, "baz"));
}

#[test]
fn test_method_with_anonymous_rest_parameters() {
    let mut test = TestCase::new();
    test.index("class Foo\n  def bar(*, **)\n  end\nend\n");
    test.assert_entry("bar", "Method", "/fake/path/foo.rb:1-2:2-5");
    assert_eq!(
        vec![
            Parameter::Rest(REST_PARAMETER_DEFAULT_NAME.into()),
            Parameter::KeywordRest(KEYWORD_REST_PARAMETER_DEFAULT_NAME.into())
        ],
        first_parameters(&test, "bar")
    );
}

#[test]
fn test_method_with_forbidden_keyword_splat_parameter() {
    let mut test = TestCase::new();
    test.index("class Foo\n  def bar(**nil)\n  end\nend\n");
    test.assert_entry("bar", "Method", "/fake/path/foo.rb:1-2:2-5");
    assert!(first_parameters(&test, "bar").is_empty());
}

#[test]
fn test_methods_with_argument_forwarding() {
    let mut test = TestCase::new();
    test.index("class Foo\n  def bar(...)\n  end\n\n  def baz(a, ...)\n  end\nend\n");
    assert_eq!("Method", kind_name(test.first("bar")), "Expected `bar` to be indexed");
    assert_eq!(vec![Parameter::Forwarding], first_parameters(&test, "bar"));
    assert_eq!("Method", kind_name(test.first("baz")), "Expected `baz` to be indexed");
    assert_eq!(vec!["RequiredParameter", "ForwardingParameter"], parameter_kinds(&first_parameters(&test, "baz")));
}

#[test]
fn test_keeps_track_of_method_owner() {
    let mut test = TestCase::new();
    test.index("class Foo\n  def bar\n  end\nend\n");
    assert_eq!(Some("Foo"), test.first("bar").owner());
}

#[test]
fn test_keeps_track_of_attributes() {
    let mut test = TestCase::new();
    test.index(
        "class Foo\n  # Hello there\n  attr_reader :bar, :other\n  attr_writer :baz\n  attr_accessor :qux\nend\n",
    );
    test.assert_entry("bar", "Accessor", "/fake/path/foo.rb:2-15:2-18");
    assert_eq!(Some("Hello there"), test.first("bar").comments.as_deref());
    test.assert_entry("other", "Accessor", "/fake/path/foo.rb:2-21:2-26");
    assert_eq!(Some("Hello there"), test.first("other").comments.as_deref());
    test.assert_entry("baz=", "Accessor", "/fake/path/foo.rb:3-15:3-18");
    test.assert_entry("qux", "Accessor", "/fake/path/foo.rb:4-17:4-20");
    test.assert_entry("qux=", "Accessor", "/fake/path/foo.rb:4-17:4-20");
}

#[test]
fn test_ignores_attributes_invoked_on_constant() {
    let mut test = TestCase::new();
    test.index("class Foo\nend\n\nFoo.attr_reader :bar\n");
    test.assert_no_entry("bar");
}

#[test]
fn test_properly_tracks_multiple_levels_of_nesting() {
    let mut test = TestCase::new();
    test.index(
        "module Foo\n  def first_method; end\n\n  module Bar\n    def second_method; end\n  end\n\n  def third_method; end\nend\n",
    );
    assert_eq!(Some("Foo"), test.first("first_method").owner());
    assert_eq!(Some("Foo::Bar"), test.first("second_method").owner());
    assert_eq!(Some("Foo"), test.first("third_method").owner());
}

#[test]
fn test_keeps_track_of_aliases() {
    let mut test = TestCase::new();
    let default_length = test.index.len();
    test.index(
        "class Foo\n  alias whatever to_s\n  alias_method :foo, :to_a\n  alias_method \"bar\", \"to_a\"\n\n  # These two are not indexed because they are dynamic or incomplete\n  alias_method baz, :to_a\n  alias_method :baz\nend\n",
    );
    test.assert_entry("whatever", "UnresolvedMethodAlias", "/fake/path/foo.rb:1-8:1-16");
    test.assert_entry("foo", "UnresolvedMethodAlias", "/fake/path/foo.rb:2-15:2-19");
    test.assert_entry("bar", "UnresolvedMethodAlias", "/fake/path/foo.rb:3-15:3-20");
    assert_eq!(4, test.index.len() - default_length);
}

#[test]
fn test_singleton_methods() {
    let mut test = TestCase::new();
    test.index("class Foo\n  def self.bar; end\n\n  class << self\n    def baz; end\n  end\nend\n");
    test.assert_entry("bar", "Method", "/fake/path/foo.rb:1-2:1-19");
    test.assert_entry("baz", "Method", "/fake/path/foo.rb:4-4:4-16");

    let bar_owner = test.owner_of("bar").unwrap();
    let baz_owner = test.owner_of("baz").unwrap();
    assert_eq!("SingletonClass", bar_owner.0);
    assert_eq!("SingletonClass", baz_owner.0);
    assert_eq!(bar_owner, baz_owner);
    assert_eq!(1, test.entries(&bar_owner.1).len());
}

#[test]
fn test_name_location_points_to_method_identifier_location() {
    let mut test = TestCase::new();
    test.index("class Foo\n  def bar\n    a = 123\n    a + 456\n  end\nend\n");
    let entry = test.first("bar");
    assert_ne!(entry.location, entry.name_location());
    let name_location = entry.name_location();
    assert_eq!(2, name_location.start_line);
    assert_eq!(2, name_location.end_line);
    assert_eq!(6, name_location.start_column);
    assert_eq!(9, name_location.end_column);
}

#[test]
fn test_signature_matches_for_a_method_with_positional_params() {
    let mut test = TestCase::new();
    test.index("class Foo\n  def bar(a, b = 123)\n  end\nend\n");
    let entry = test.first("bar");

    for call in [
        "bar()",
        "bar(1)",
        "bar(1, 2)",
        "bar(...)",
        "bar(1, ...)",
        "bar(*a)",
        "bar(1, *a)",
        "bar(1, *a, 2)",
        "bar(*a, 2)",
        "bar(1, **a)",
        "bar(1) {}",
        "bar(**a)",
    ] {
        assert_signature_matches(entry, call);
    }

    for call in ["bar(1, 2, 3)", "bar(1, b: 2)", "bar(1, 2, c: 3)"] {
        refute_signature_matches(entry, call);
    }
}

#[test]
fn test_signature_matches_for_a_method_with_argument_forwarding() {
    let mut test = TestCase::new();
    test.index("class Foo\n  def bar(...)\n  end\nend\n");
    let entry = test.first("bar");

    for call in [
        "bar(1)",
        "bar(1, 2)",
        "bar(...)",
        "bar(1, ...)",
        "bar(*a)",
        "bar(1, *a)",
        "bar(1, *a, 2)",
        "bar(*a, 2)",
        "bar(1, **a)",
        "bar(1) {}",
        "bar()",
        "bar(1, 2, 3)",
        "bar(1, 2, a: 1, b: 5) {}",
    ] {
        assert_signature_matches(entry, call);
    }
}

#[test]
fn test_signature_matches_for_post_forwarding_parameter() {
    let mut test = TestCase::new();
    test.index("class Foo\n  def bar(a, ...)\n  end\nend\n");
    let entry = test.first("bar");

    for call in [
        "bar(1)",
        "bar(1, 2)",
        "bar(...)",
        "bar(1, ...)",
        "bar(*a)",
        "bar(1, *a)",
        "bar(1, *a, 2)",
        "bar(*a, 2)",
        "bar(1, **a)",
        "bar(1) {}",
        "bar(1, 2, 3)",
        "bar(1, 2, a: 1, b: 5) {}",
        "bar()",
    ] {
        assert_signature_matches(entry, call);
    }
}

#[test]
fn test_signature_matches_for_destructured_parameters() {
    let mut test = TestCase::new();
    test.index("class Foo\n  def bar(a, (b, c))\n  end\nend\n");
    let entry = test.first("bar");

    for call in [
        "bar()",
        "bar(1)",
        "bar(1, 2)",
        "bar(...)",
        "bar(1, ...)",
        "bar(*a)",
        "bar(1, *a)",
        "bar(*a, 2)",
        "bar(1, *a, 2)",
        "bar(1, **a)",
        "bar(1) {}",
    ] {
        assert_signature_matches(entry, call);
    }

    for call in ["bar(1, 2, 3)", "bar(1, 2, a: 1, b: 5) {}"] {
        refute_signature_matches(entry, call);
    }
}

#[test]
fn test_signature_matches_for_post_parameters() {
    let mut test = TestCase::new();
    test.index("class Foo\n  def bar(*splat, a)\n  end\nend\n");
    let entry = test.first("bar");

    for call in [
        "bar(1)",
        "bar(1, 2)",
        "bar(...)",
        "bar(1, ...)",
        "bar(*a)",
        "bar(1, *a)",
        "bar(*a, 2)",
        "bar(1, *a, 2)",
        "bar(1, **a)",
        "bar(1, 2, 3)",
        "bar(1) {}",
        "bar()",
    ] {
        assert_signature_matches(entry, call);
    }

    refute_signature_matches(entry, "bar(1, 2, a: 1, b: 5) {}");
}

#[test]
fn test_signature_matches_for_keyword_parameters() {
    let mut test = TestCase::new();
    test.index("class Foo\n  def bar(a:, b: 123)\n  end\nend\n");
    let entry = test.first("bar");

    for call in ["bar(...)", "bar()", "bar(a: 1)", "bar(a: 1, b: 32)"] {
        assert_signature_matches(entry, call);
    }

    for call in [
        "bar(a: 1, c: 2)",
        "bar(1, ...)",
        "bar(1) {}",
        "bar(1, *a)",
        "bar(*a, 2)",
        "bar(1, *a, 2)",
        "bar(1, **a)",
        "bar(*a)",
        "bar(1)",
        "bar(1, 2)",
        "bar(1, 2, a: 1, b: 5) {}",
    ] {
        refute_signature_matches(entry, call);
    }
}

#[test]
fn test_signature_matches_for_keyword_splats() {
    let mut test = TestCase::new();
    test.index("class Foo\n  def bar(a, b:, **kwargs)\n  end\nend\n");
    let entry = test.first("bar");

    for call in ["bar(...)", "bar()", "bar(1)", "bar(1, b: 2)", "bar(1, b: 2, c: 3, d: 4)"] {
        assert_signature_matches(entry, call);
    }

    refute_signature_matches(entry, "bar(1, 2, b: 2)");
}

#[test]
fn test_partial_signature_matches() {
    let mut test = TestCase::new();
    test.index("class Foo\n  def bar(a:, b:)\n  end\n\n  def baz(a, b)\n  end\nend\n");
    assert_signature_matches(test.first("bar"), "bar(a: 1)");
    assert_signature_matches(test.first("baz"), "baz(1)");
}

#[test]
fn test_module_function_with_no_arguments() {
    let mut test = TestCase::new();
    test.index(
        "module Foo\n  def bar; end\n\n  module_function\n\n  def baz; end\n  attr_reader :attribute\n\n  public\n\n  def qux; end\nend\n",
    );

    let entry = test.first("bar");
    assert!(entry.is_public());
    assert_eq!(Some("Foo"), entry.owner());

    let baz_entries = test.entries("baz");
    let (instance_baz, singleton_baz) = (baz_entries[0], baz_entries[1]);
    assert!(instance_baz.is_private());
    assert_eq!(Some("Foo"), instance_baz.owner());
    assert!(singleton_baz.is_public());
    assert_eq!(Some("Foo::<Class:Foo>"), singleton_baz.owner());

    let qux_entries = test.entries("qux");
    assert_eq!(1, qux_entries.len());
    assert!(qux_entries[0].is_public());
    assert_eq!(Some("Foo"), instance_baz.owner());

    let attribute_entries = test.entries("attribute");
    assert_eq!(1, attribute_entries.len());
    assert_eq!(Some("Foo"), attribute_entries[0].owner());
    assert!(attribute_entries[0].is_private());
}

#[test]
fn test_module_function_does_nothing_in_classes() {
    let mut test = TestCase::new();
    test.index("class Foo\n  def bar; end\n\n  module_function\n\n  def baz; end\nend\n");

    let entry = test.first("bar");
    assert!(entry.is_public());
    assert_eq!(Some("Foo"), entry.owner());

    let entry = test.first("baz");
    assert!(entry.is_public());
    assert_eq!(Some("Foo"), entry.owner());
}

#[test]
fn test_making_several_class_methods_private() {
    let mut test = TestCase::new();
    test.index(
        "class Foo\n  def self.bar; end\n  def self.baz; end\n  def self.qux; end\n\n  private_class_method :bar, :baz, :qux\n\n  def initialize\n  end\nend\n",
    );
}

#[test]
fn test_changing_visibility_post_definition() {
    let mut test = TestCase::new();
    test.index(
        "class Foo\n  def bar; end\n  private :bar\n\n  def baz; end\n  protected :baz\n\n  private\n  def qux; end\n\n  public :qux\nend\n",
    );
    assert!(test.first("bar").is_private());
    assert!(test.first("baz").is_protected());
    assert!(test.first("qux").is_public());
}

#[test]
fn test_handling_attr() {
    let mut test = TestCase::new();
    test.index("class Foo\n  attr :bar\n  attr :baz, true\n  attr :qux, false\nend\n");
    test.assert_entry("bar", "Accessor", "/fake/path/foo.rb:1-8:1-11");
    test.assert_no_entry("bar=");
    test.assert_entry("baz", "Accessor", "/fake/path/foo.rb:2-8:2-11");
    test.assert_entry("baz=", "Accessor", "/fake/path/foo.rb:2-8:2-11");
    test.assert_entry("qux", "Accessor", "/fake/path/foo.rb:3-8:3-11");
    test.assert_no_entry("qux=");
}
