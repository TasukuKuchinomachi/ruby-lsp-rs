mod indexer_support;

use ruby_lsp_rs::indexer::entry::MethodAliasTargetKind;
use ruby_lsp_rs::indexer::index::build_non_redundant_full_name;

use indexer_support::{TestCase, kind_name, names_of};
use ruby_lsp_rs::indexer::EntryKind;
use ruby_lsp_rs::indexer::index::actual_nesting;
use ruby_lsp_rs::indexer::indexing::{IndexNotEmptyError, try_index_all};

fn nesting(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|part| part.to_string()).collect()
}

// RBS 4.1.3 declares `class Module` without `< Object`, so singleton linearizations end at "Module" instead of
// continuing with "Object", "Kernel" and "BasicObject" like they did with ruby-lsp's RBS version
fn strings(parts: &[&str]) -> Vec<String> {
    nesting(parts)
}

fn first_names(test: &TestCase, results: &[Vec<usize>]) -> Vec<String> {
    results.iter().map(|ids| test.index.entry(ids[0]).name.clone()).collect()
}

#[test]
fn test_resolving_self_referential_constant_alias() {
    let mut test = TestCase::new();
    test.index(
        "module A\n  module B\n    class C\n    end\n  end\nend\n\nmodule A\n  module D\n    B = B::C\n  end\nend\n",
    );

    let id = test.index.resolve("A::D::B", &[]).unwrap()[0];
    let entry = test.index.entry(id);
    assert_eq!("ConstantAlias", kind_name(entry));
    assert_eq!(10, entry.location.start_line);
    let EntryKind::ConstantAlias { target } = &entry.kind else { unreachable!() };
    assert_eq!("A::B::C", target);
}

#[test]
fn test_resolving_non_existing_self_referential_constant_alias() {
    let mut test = TestCase::new();
    test.index("module Foo\n  SomeClass = ::SomeClass\n  UNRESOLVED = SomeClass::CONSTANT\nend\n");

    let id = test.index.resolve("Foo::UNRESOLVED", &[]).unwrap()[0];
    let entry = test.index.entry(id);
    assert_eq!("UnresolvedConstantAlias", kind_name(entry));
    assert_eq!(3, entry.location.start_line);
    let EntryKind::UnresolvedConstantAlias { target, .. } = &entry.kind else { unreachable!() };
    assert_eq!("SomeClass::CONSTANT", target);

    assert!(test.index.resolve("SomeClass::CONSTANT", &nesting(&["Foo"])).is_none());
}

#[test]
fn test_resolving_qualified_references() {
    let mut test = TestCase::new();
    test.index(
        "module Namespace\n  class Entry\n    CONST = 1\n  end\nend\n\nmodule Namespace\n  class Index\n  end\nend\n",
    );

    let id = test.index.resolve("Entry::CONST", &nesting(&["Namespace", "Index"])).unwrap()[0];
    assert_eq!(3, test.index.entry(id).location.start_line);
}

#[test]
fn test_resolving_unqualified_references() {
    let mut test = TestCase::new();
    test.index(
        "module Foo\n  CONST = 1\nend\n\nmodule Namespace\n  CONST = 2\n\n  class Index\n    include Foo\n  end\nend\n",
    );

    let id = test.index.resolve("CONST", &nesting(&["Namespace", "Index"])).unwrap()[0];
    assert_eq!(6, test.index.entry(id).location.start_line);
}

#[test]
fn test_resolving_references_with_only_top_level_declaration() {
    let mut test = TestCase::new();
    test.index("CONST = 1\n\nmodule Foo; end\n\nmodule Namespace\n  class Index\n    include Foo\n  end\nend\n");

    let id = test.index.resolve("CONST", &nesting(&["Namespace", "Index"])).unwrap()[0];
    assert_eq!(1, test.index.entry(id).location.start_line);
}

#[test]
fn test_instance_variables_completions_from_different_owners_with_conflicting_names() {
    let mut test = TestCase::new();
    test.index(
        "class Foo\n  def initialize\n    @bar = 1\n  end\nend\n\nclass Bar\n  def initialize\n    @bar = 2\n  end\nend\n",
    );

    let id = test.index.instance_variable_completion_candidates("@", "Bar")[0];
    let entry = test.index.entry(id);
    assert_eq!("@bar", entry.name);
    assert_eq!(Some("Bar"), entry.owner());
}

#[test]
fn test_resolving_a_qualified_reference() {
    let mut test = TestCase::new();
    test.index(
        "class Base\n  module Third\n    CONST = 1\n  end\nend\n\nclass Foo\n  module Third\n    CONST = 2\n  end\n\n  class Second < Base\n  end\nend\n",
    );

    let id = test.index.resolve("Third::CONST", &nesting(&["Foo"])).unwrap()[0];
    assert_eq!(9, test.index.entry(id).location.start_line);
}

#[test]
fn test_resolving_unindexed_constant_with_no_nesting() {
    let mut test = TestCase::new();
    assert!(test.index.resolve("RSpec", &[]).is_none());
}

#[test]
fn test_object_superclass_indexing_and_resolution_with_reopened_object_class() {
    let mut test = TestCase::new();
    test.index("class Object; end\n");

    let entries = test.entries("Object");
    assert_eq!(2, entries.len());
    let reopened_entry = entries.last().unwrap();
    assert_eq!(Some("::BasicObject"), reopened_entry.namespace().unwrap().parent_class.as_deref());
    assert_eq!(strings(&["Object", "Kernel", "BasicObject"]), test.index.linearized_ancestors_of("Object").unwrap());
}

#[test]
fn test_object_superclass_indexing_and_resolution_with_reopened_basic_object_class() {
    let mut test = TestCase::new();
    test.index("class BasicObject; end\n");

    let entries = test.entries("BasicObject");
    assert_eq!(2, entries.len());
    let reopened_entry = entries.last().unwrap();
    assert_eq!(None, reopened_entry.namespace().unwrap().parent_class.as_deref());
    assert_eq!(strings(&["BasicObject"]), test.index.linearized_ancestors_of("BasicObject").unwrap());
}

#[test]
fn test_object_superclass_resolution() {
    let mut test = TestCase::new();
    test.index("module Foo\n  class Object; end\n\n  class Bar; end\n  class Baz < Object; end\nend\n");

    assert_eq!(
        strings(&["Foo::Bar", "Object", "Kernel", "BasicObject"]),
        test.index.linearized_ancestors_of("Foo::Bar").unwrap()
    );
    assert_eq!(
        strings(&["Foo::Baz", "Foo::Object", "Object", "Kernel", "BasicObject"]),
        test.index.linearized_ancestors_of("Foo::Baz").unwrap()
    );
}

#[test]
fn test_basic_object_superclass_resolution() {
    let mut test = TestCase::new();
    test.index("module Foo\n  class BasicObject; end\n\n  class Bar; end\n  class Baz < BasicObject; end\nend\n");

    assert_eq!(
        strings(&["Foo::Bar", "Object", "Kernel", "BasicObject"]),
        test.index.linearized_ancestors_of("Foo::Bar").unwrap()
    );
    assert_eq!(
        strings(&["Foo::Baz", "Foo::BasicObject", "Object", "Kernel", "BasicObject"]),
        test.index.linearized_ancestors_of("Foo::Baz").unwrap()
    );
}

#[test]
fn test_top_level_object_superclass_resolution() {
    let mut test = TestCase::new();
    test.index("module Foo\n  class Object; end\n\n  class Bar < ::Object; end\nend\n");

    assert_eq!(
        strings(&["Foo::Bar", "Object", "Kernel", "BasicObject"]),
        test.index.linearized_ancestors_of("Foo::Bar").unwrap()
    );
}

#[test]
fn test_top_level_basic_object_superclass_resolution() {
    let mut test = TestCase::new();
    test.index("module Foo\n  class BasicObject; end\n\n  class Bar < ::BasicObject; end\nend\n");

    assert_eq!(strings(&["Foo::Bar", "BasicObject"]), test.index.linearized_ancestors_of("Foo::Bar").unwrap());
}

#[test]
fn test_resolving_method_inside_singleton_context() {
    let mut test = TestCase::new();
    test.index(
        "module Foo\n  class Bar\n    class << self\n      class Baz\n        class << self\n          def found_me!; end\n        end\n      end\n    end\n  end\nend\n",
    );

    let ids = test.index.resolve_method("found_me!", "Foo::Bar::<Class:Bar>::Baz::<Class:Baz>", false).unwrap();
    assert_eq!("found_me!", test.index.entry(ids[0]).name);
}

#[test]
fn test_resolving_constants_in_singleton_contexts() {
    let mut test = TestCase::new();
    test.index(
        "module Foo\n  class Bar\n    CONST = 3\n\n    class << self\n      CONST = 2\n\n      class Baz\n        CONST = 1\n\n        class << self\n        end\n      end\n    end\n  end\nend\n",
    );

    let ids = test.index.resolve("CONST", &nesting(&["Foo", "Bar", "<Class:Bar>", "Baz", "<Class:Baz>"])).unwrap();
    assert_eq!(9, test.index.entry(ids[0]).location.start_line);
}

const SINGLETON_INSTANCE_VARIABLES: &str = "module Foo\n  class Bar\n    @a = 123\n\n    class << self\n      def hello\n        @b = 123\n      end\n\n      @c = 123\n    end\n  end\nend\n";

#[test]
fn test_resolving_instance_variables_in_singleton_contexts() {
    let mut test = TestCase::new();
    test.index(SINGLETON_INSTANCE_VARIABLES);

    let ids = test.index.resolve_instance_variable("@a", "Foo::Bar::<Class:Bar>").unwrap();
    assert_eq!("@a", test.index.entry(ids[0]).name);

    let ids = test.index.resolve_instance_variable("@b", "Foo::Bar::<Class:Bar>").unwrap();
    assert_eq!("@b", test.index.entry(ids[0]).name);

    let ids = test.index.resolve_instance_variable("@c", "Foo::Bar::<Class:Bar>::<Class:<Class:Bar>>").unwrap();
    assert_eq!("@c", test.index.entry(ids[0]).name);
}

#[test]
fn test_instance_variable_completion_in_singleton_contexts() {
    let mut test = TestCase::new();
    test.index(SINGLETON_INSTANCE_VARIABLES);

    let ids = test.index.instance_variable_completion_candidates("@", "Foo::Bar::<Class:Bar>");
    let names = names_of(&test.index, &ids);
    assert!(names.contains(&"@a".to_string()));
    assert!(names.contains(&"@b".to_string()));
}

#[test]
fn test_singletons_are_excluded_from_prefix_search() {
    let mut test = TestCase::new();
    test.index("class Zwq\n  class << self\n  end\nend\n");

    assert!(test.index.prefix_search("Zwq::<C", None).is_empty());
}

#[test]
fn test_singletons_are_excluded_from_fuzzy_search() {
    let mut test = TestCase::new();
    test.index("class Zwq\n  class << self\n  end\nend\n");

    let results = test.index.fuzzy_search(Some("Zwq"), |_| true);
    assert_eq!(1, results.len());
    assert_eq!("Zwq", test.index.entry(results[0]).name);
}

#[test]
fn test_resolving_method_aliases() {
    let mut test = TestCase::new();
    test.index(
        "class Foo\n  def bar(a, b, c)\n  end\n\n  alias double_alias bar\nend\n\nclass Bar < Foo\n  def hello(b); end\n\n  alias baz bar\n  alias_method :qux, :hello\n  alias double double_alias\nend\n",
    );

    let methods = test.index.resolve_method("baz", "Bar", false).unwrap();
    let entry = test.index.entry(methods[0]);
    assert_eq!("MethodAlias", kind_name(entry));
    let EntryKind::MethodAlias { target, .. } = &entry.kind else { unreachable!() };
    assert_eq!("bar", target.name);
    assert_eq!(Some("Foo"), target.owner.as_deref());

    let methods = test.index.resolve_method("qux", "Bar", false).unwrap();
    let entry = test.index.entry(methods[0]);
    assert_eq!("MethodAlias", kind_name(entry));
    let EntryKind::MethodAlias { target, .. } = &entry.kind else { unreachable!() };
    assert_eq!("hello", target.name);
    assert_eq!(Some("Bar"), target.owner.as_deref());

    let methods = test.index.resolve_method("double", "Bar", false).unwrap();
    let entry = test.index.entry(methods[0]);
    assert_eq!("MethodAlias", kind_name(entry));
    let EntryKind::MethodAlias { target, .. } = &entry.kind else { unreachable!() };
    assert_eq!("double_alias", target.name);
    assert_eq!(MethodAliasTargetKind::MethodAlias, target.kind);

    let double_alias = test.entries("double_alias")[0];
    assert_eq!("MethodAlias", kind_name(double_alias));
    assert_eq!(Some("Foo"), double_alias.owner());
    let EntryKind::MethodAlias { target: final_target, .. } = &double_alias.kind else { unreachable!() };
    assert_eq!("bar", final_target.name);
}

#[test]
fn test_resolving_circular_method_aliases() {
    let mut test = TestCase::new();
    test.index("class Foo\n  alias bar bar\nend\n");

    assert!(test.index.resolve_method("bar", "Foo", false).is_none());
    assert_eq!("UnresolvedMethodAlias", kind_name(test.first("bar")));
}

#[test]
fn test_unresolvable_method_aliases() {
    let mut test = TestCase::new();
    test.index("class Foo\n  alias bar baz\nend\n");

    assert!(test.index.resolve_method("bar", "Foo", false).is_none());
    assert_eq!("UnresolvedMethodAlias", kind_name(test.first("bar")));
}

#[test]
fn test_only_aliases_for_the_right_owner_are_resolved() {
    let mut test = TestCase::new();
    test.index(
        "class Foo\n  attr_reader :name\n  alias_method :decorated_name, :name\nend\n\nclass Bar\n  alias_method :decorated_name, :to_s\nend\n",
    );

    let methods = test.index.resolve_method("decorated_name", "Foo", false).unwrap();
    let entry = test.index.entry(methods[0]);
    assert_eq!("MethodAlias", kind_name(entry));
    let EntryKind::MethodAlias { target, .. } = &entry.kind else { unreachable!() };
    assert_eq!("name", target.name);
    assert_eq!(MethodAliasTargetKind::Accessor, target.kind);
    assert_eq!(Some("Foo"), target.owner.as_deref());

    let other_decorated_name =
        test.entries("decorated_name").into_iter().find(|entry| kind_name(entry) == "UnresolvedMethodAlias");
    assert!(other_decorated_name.is_some());
}

#[test]
fn test_completion_does_not_include_unresolved_aliases() {
    let mut test = TestCase::new();
    test.index("class Foo\n  alias_method :bar, :missing\nend\n");

    assert!(test.index.method_completion_candidates(Some("bar"), "Foo").is_empty());
}

#[test]
fn test_first_unqualified_const() {
    let mut test = TestCase::new();
    test.index("module Foo\n  class Bar; end\nend\n\nmodule Baz\n  class Bar; end\nend\n");

    let ids = test.index.first_unqualified_const("Bar").unwrap();
    assert_eq!("Foo::Bar", test.index.entry(ids[0]).name);
}

#[test]
fn test_first_unqualified_const_prefers_exact_matches() {
    let mut test = TestCase::new();
    test.index("module Foo\n  class ParseResultType\n  end\nend\n\nmodule Namespace\n  class Type\n  end\nend\n");

    let ids = test.index.first_unqualified_const("Type").unwrap();
    assert_eq!("Namespace::Type", test.index.entry(ids[0]).name);
}

#[test]
fn test_completion_does_not_duplicate_overridden_methods() {
    let mut test = TestCase::new();
    test.index("class Foo\n  def bar; end\nend\n\nclass Baz < Foo\n  def bar; end\nend\n");

    let ids = test.index.method_completion_candidates(Some("bar"), "Baz");
    assert_eq!(strings(&["bar"]), names_of(&test.index, &ids));
    assert_eq!(Some("Baz"), test.index.entry(ids[0]).owner());
}

#[test]
fn test_completion_does_not_duplicate_methods_overridden_by_aliases() {
    let mut test = TestCase::new();
    test.index("class Foo\n  def bar; end\nend\n\nclass Baz < Foo\n  alias bar to_s\nend\n");

    let ids = test.index.method_completion_candidates(Some("bar"), "Baz");
    assert_eq!(strings(&["bar"]), names_of(&test.index, &ids));
    assert_eq!(Some("Baz"), test.index.entry(ids[0]).owner());
}

#[test]
fn test_decorated_parameters() {
    let mut test = TestCase::new();
    test.index("class Foo\n  def bar(a, b = 1, c: 2)\n  end\nend\n");

    let methods = test.index.resolve_method("bar", "Foo", false).unwrap();
    assert_eq!("(a, b = <default>, c: <default>)", test.index.entry(methods[0]).decorated_parameters());
}

#[test]
fn test_decorated_parameters_when_method_has_no_parameters() {
    let mut test = TestCase::new();
    test.index("class Foo\n  def bar\n  end\nend\n");

    let methods = test.index.resolve_method("bar", "Foo", false).unwrap();
    assert_eq!("()", test.index.entry(methods[0]).decorated_parameters());
}

#[test]
fn test_linearizing_singleton_ancestors_of_singleton_when_class_has_parent() {
    let mut test = TestCase::new();
    test.index(
        "class Foo; end\n\nclass Bar < Foo\nend\n\nclass Baz < Bar\n  class << self\n    class << self\n    end\n  end\nend\n",
    );

    assert_eq!(
        strings(&[
            "Baz::<Class:Baz>::<Class:<Class:Baz>>",
            "Bar::<Class:Bar>::<Class:<Class:Bar>>",
            "Foo::<Class:Foo>::<Class:<Class:Foo>>",
            "Object::<Class:Object>::<Class:<Class:Object>>",
            "BasicObject::<Class:BasicObject>::<Class:<Class:BasicObject>>",
            "Class::<Class:Class>",
            "Module::<Class:Module>",
        ]),
        test.index.linearized_ancestors_of("Baz::<Class:Baz>::<Class:<Class:Baz>>").unwrap()
    );
}

#[test]
fn test_linearizing_singleton_object() {
    let mut test = TestCase::new();
    assert_eq!(
        strings(&[
            "Object::<Class:Object>",
            "BasicObject::<Class:BasicObject>",
            "Class",
            "Module",
        ]),
        test.index.linearized_ancestors_of("Object::<Class:Object>").unwrap()
    );
}

#[test]
fn test_extend_self() {
    let mut test = TestCase::new();
    test.index("module Foo\n  def bar\n  end\n\n  extend self\n\n  def baz\n  end\nend\n");

    for method in ["bar", "baz"] {
        for receiver in ["Foo", "Foo::<Class:Foo>"] {
            let ids = test.index.resolve_method(method, receiver, false).unwrap();
            assert_eq!(method, test.index.entry(ids[0]).name);
        }
    }

    assert_eq!(
        strings(&["Foo::<Class:Foo>", "Foo", "Module"]),
        test.index.linearized_ancestors_of("Foo::<Class:Foo>").unwrap()
    );
}

#[test]
fn test_linearizing_singleton_ancestors() {
    let mut test = TestCase::new();
    test.index(
        "module First\nend\n\nmodule Second\n  include First\nend\n\nmodule Foo\n  class Bar\n    class << self\n      class Baz\n        extend Second\n\n        class << self\n          include First\n        end\n      end\n    end\n  end\nend\n",
    );

    assert_eq!(
        strings(&[
            "Foo::Bar::<Class:Bar>::Baz::<Class:Baz>",
            "Second",
            "First",
            "Object::<Class:Object>",
            "BasicObject::<Class:BasicObject>",
            "Class",
            "Module",
        ]),
        test.index.linearized_ancestors_of("Foo::Bar::<Class:Bar>::Baz::<Class:Baz>").unwrap()
    );
}

#[test]
fn test_linearizing_singleton_ancestors_when_class_has_parent() {
    let mut test = TestCase::new();
    test.index("class Foo; end\n\nclass Bar < Foo\nend\n\nclass Baz < Bar\n  class << self\n  end\nend\n");

    assert_eq!(
        strings(&[
            "Baz::<Class:Baz>",
            "Bar::<Class:Bar>",
            "Foo::<Class:Foo>",
            "Object::<Class:Object>",
            "BasicObject::<Class:BasicObject>",
            "Class",
            "Module",
        ]),
        test.index.linearized_ancestors_of("Baz::<Class:Baz>").unwrap()
    );
}

#[test]
fn test_linearizing_a_module_singleton_class() {
    let mut test = TestCase::new();
    test.index("module A; end\n");

    assert_eq!(
        strings(&["A::<Class:A>", "Module"]),
        test.index.linearized_ancestors_of("A::<Class:A>").unwrap()
    );
}

#[test]
fn test_linearizing_a_singleton_class_with_no_attached() {
    let mut test = TestCase::new();
    assert!(test.index.linearized_ancestors_of("A::<Class:A>").is_err());
}

#[test]
fn test_linearizing_singleton_parent_class_with_namespace() {
    let mut test = TestCase::new();
    test.index("class ActiveRecord::Base; end\n\nclass User < ActiveRecord::Base\nend\n");

    assert_eq!(
        strings(&[
            "User::<Class:User>",
            "ActiveRecord::Base::<Class:Base>",
            "Object::<Class:Object>",
            "BasicObject::<Class:BasicObject>",
            "Class",
            "Module",
        ]),
        test.index.linearized_ancestors_of("User::<Class:User>").unwrap()
    );
}

#[test]
fn test_singleton_nesting_is_correctly_split_during_linearization() {
    let mut test = TestCase::new();
    test.index(
        "module Bar; end\n\nmodule Foo\n  class Namespace::Parent\n    extend Bar\n  end\nend\n\nmodule Foo\n  class Child < Namespace::Parent\n  end\nend\n",
    );

    assert_eq!(
        strings(&[
            "Foo::Child::<Class:Child>",
            "Foo::Namespace::Parent::<Class:Parent>",
            "Bar",
            "Object::<Class:Object>",
            "BasicObject::<Class:BasicObject>",
            "Class",
            "Module",
        ]),
        test.index.linearized_ancestors_of("Foo::Child::<Class:Child>").unwrap()
    );
}

#[test]
fn test_resolving_circular_method_aliases_on_class_reopen() {
    let mut test = TestCase::new();
    test.index("class Foo\n  alias bar ==\n  def ==(other) = true\nend\n\nclass Foo\n  alias == bar\nend\n");

    let ids = test.index.resolve_method("==", "Foo", false).unwrap();
    let method = test.index.entry(ids[0]);
    assert_eq!("Method", kind_name(method));
    assert_eq!("==", method.name);

    let candidates = test.index.method_completion_candidates(Some("="), "Foo");
    assert_eq!(strings(&["==", "==="]), names_of(&test.index, &candidates));
}

#[test]
fn test_entries_for() {
    let mut test = TestCase::new();
    test.index("class Foo; end\n\nmodule Bar\n  def my_def; end\n  def self.my_singleton_def; end\nend\n");

    let ids = test.index.entries_for("file:///fake/path/foo.rb");
    assert_eq!(
        strings(&["Foo", "Bar", "my_def", "Bar::<Class:Bar>", "my_singleton_def"]),
        names_of(&test.index, &ids)
    );

    let namespace_ids: Vec<usize> = ids.iter().copied().filter(|id| test.index.entry(*id).is_namespace()).collect();
    assert_eq!(strings(&["Foo", "Bar", "Bar::<Class:Bar>"]), names_of(&test.index, &namespace_ids));
}

#[test]
fn test_entries_for_returns_nil_if_no_matches() {
    let test = TestCase::new();
    assert!(test.index.entries_for("non_existing_file.rb").is_empty());
}

#[test]
fn test_constant_completion_candidates_all_possible_constants() {
    let mut test = TestCase::new();
    test.index(
        "XQRK = 3\n\nmodule Bar\n  XQRK = 2\nend\n\nmodule Foo\n  XQRK = 1\nend\n\nmodule Namespace\n  XQRK = 0\n\n  class Baz\n    include Foo\n    include Bar\n  end\nend\n",
    );

    let result = test.index.constant_completion_candidates("X", &nesting(&["Namespace", "Baz"]));
    for ids in &result {
        let name = &test.index.entry(ids[0]).name;
        assert!(ids.iter().all(|id| &test.index.entry(*id).name == name));
    }
    assert_eq!(strings(&["Namespace::XQRK", "Bar::XQRK", "XQRK"]), first_names(&test, &result));

    let result = test.index.constant_completion_candidates("::X", &nesting(&["Namespace", "Baz"]));
    assert_eq!(strings(&["XQRK"]), first_names(&test, &result));
}

#[test]
fn test_constant_completion_does_not_confuse_uppercase_methods() {
    let mut test = TestCase::new();
    test.index("class Foo\n  def Qux\n  end\nend\n");

    let candidates = test.index.constant_completion_candidates("Q", &[]);
    let names: Vec<String> = candidates.iter().flat_map(|ids| names_of(&test.index, ids)).collect();
    assert!(!names.contains(&"Qux".to_string()));

    assert_eq!(0, test.index.constant_completion_candidates("Qux", &[]).len());
}

#[test]
fn test_constant_completion_candidates_for_empty_name() {
    let mut test = TestCase::new();
    test.index("module Foo\n  Bar = 1\nend\n\nclass Baz\n  include Foo\nend\n");

    let result = test.index.constant_completion_candidates("Baz::", &[]);
    assert!(first_names(&test, &result).contains(&"Foo::Bar".to_string()));
}

#[test]
fn test_follow_alias_namespace() {
    let mut test = TestCase::new();
    test.index(
        "module First\n  module Second\n    class Foo\n    end\n  end\nend\n\nmodule Namespace\n  Second = First::Second\nend\n",
    );

    assert_eq!(Some("First::Second".to_string()), test.index.follow_aliased_namespace("Namespace::Second"));
}

#[test]
fn test_resolving_alias_to_non_existing_namespace() {
    let mut test = TestCase::new();
    test.index(
        "module Namespace\n  class Foo\n    module InnerNamespace\n      Constants = Namespace::Foo::Constants\n    end\n  end\nend\n",
    );

    let inner = nesting(&["Namespace", "Foo", "InnerNamespace"]);
    let ids = test.index.resolve("Constants", &inner).unwrap();
    assert_eq!("UnresolvedConstantAlias", kind_name(test.index.entry(ids[0])));

    assert!(test.index.resolve("Namespace::Foo::Constants", &inner).is_none());
}

#[test]
fn test_resolving_alias_to_existing_constant_from_inner_namespace() {
    let mut test = TestCase::new();
    test.index(
        "module Parent\n  CONST = 123\nend\n\nmodule First\n  module Namespace\n    class Foo\n      include Parent\n\n      module InnerNamespace\n        Constants = Namespace::Foo::CONST\n      end\n    end\n  end\nend\n",
    );

    let ids =
        test.index.resolve("Namespace::Foo::CONST", &nesting(&["First", "Namespace", "Foo", "InnerNamespace"])).unwrap();
    let entry = test.index.entry(ids[0]);
    assert_eq!("Parent::CONST", entry.name);
    assert_eq!("Constant", kind_name(entry));
}

#[test]
fn test_prevents_multiple_calls_to_index_all() {
    let mut test = TestCase::new();
    let progress = |_: u32| {};
    assert_eq!(Ok(()), try_index_all(&mut test.index, &[], None, &progress));
    assert_eq!(Err(IndexNotEmptyError), try_index_all(&mut test.index, &[], None, &progress));
}

#[test]
fn test_index_can_handle_entries_from_untitled_scheme() {
    let mut test = TestCase::new();
    let uri = "untitled:Untitled-1";
    test.index_with_uri("class Foo\nend\n", uri);

    let entry = test.first("Foo");
    assert_eq!(uri, entry.uri.as_ref());
    assert_eq!("Untitled-1", entry.file_name());
    assert_eq!(None, entry.file_path());

    test.index.handle_change_with_source(uri, "# I added this comment!\nclass Foo\nend\n");

    let entry = test.first("Foo");
    assert_eq!(Some("I added this comment!"), entry.comments.as_deref());
}

#[test]
fn test_instance_variable_completion_returns_class_variables_too() {
    let mut test = TestCase::new();
    test.index("class Parent\n  @@abc = 123\nend\n\nclass Child < Parent\n  @@adf = 123\n\n  def self.do\n  end\nend\n");

    let ids = test.index.instance_variable_completion_candidates("@", "Child::<Class:Child>");
    assert_eq!(strings(&["@@adf", "@@abc"]), names_of(&test.index, &ids[..2.min(ids.len())]));
}

#[test]
fn test_class_variable_completion_from_singleton_context() {
    let mut test = TestCase::new();
    test.index("class Foo\n  @@hello = 123\n\n  def self.do\n  end\nend\n");

    let candidates = test.index.class_variable_completion_candidates("@@", "Foo::<Class:Foo>");
    assert!(!candidates.is_empty());
    assert_eq!("@@hello", test.index.entry(candidates[0]).name);
}

#[test]
fn test_resolve_class_variable_in_singleton_context() {
    let mut test = TestCase::new();
    test.index("class Foo\n  @@hello = 123\nend\n");

    let candidates = test.index.resolve_class_variable("@@hello", "Foo::<Class:Foo>").unwrap();
    assert!(!candidates.is_empty());
    assert_eq!("@@hello", test.index.entry(candidates[0]).name);
}

#[test]
fn test_actual_nesting() {
    assert_eq!(strings(&["Foo"]), actual_nesting(&[], Some("Foo")));
    assert_eq!(strings(&["TopLevel", "Foo"]), actual_nesting(&strings(&["First", "::TopLevel"]), Some("Foo")));
    assert_eq!(
        strings(&["TopLevel", "Another", "Foo"]),
        actual_nesting(&strings(&["::TopLevel", "Another"]), Some("Foo"))
    );
    assert_eq!(strings(&["TopLevel"]), actual_nesting(&strings(&["First", "::TopLevel"]), None));
}

fn constant_name_of_first_class(source: &str) -> Option<String> {
    let parse_result = ruby_prism::parse(source.as_bytes());
    let program = parse_result.node();
    let program = program.as_program_node().unwrap();
    let first = program.statements().body().iter().next().unwrap();
    let constant_path = first.as_class_node().unwrap().constant_path();
    ruby_lsp_rs::prism::constant_name(&constant_path)
}

#[test]
fn test_constant_name() {
    assert_eq!(None, constant_name_of_first_class("class var::Foo; end"));
    assert_eq!(None, constant_name_of_first_class("class ; end"));
    assert_eq!(None, constant_name_of_first_class("class method_call; end"));
    assert_eq!(Some("Foo".to_string()), constant_name_of_first_class("class Foo; end"));
    assert_eq!(None, constant_name_of_first_class("class class Foo\nend\nend\n"));
}

#[test]
fn test_build_non_redundant_name() {
    let nesting = |parts: &[&str]| -> Vec<String> { parts.iter().map(|part| part.to_string()).collect() };
    assert_eq!(
        "Namespace::Foo::Constants",
        build_non_redundant_full_name("Namespace::Foo::Constants", &nesting(&["Namespace", "Foo", "InnerNamespace"]))
    );
    assert_eq!("Namespace::Foo::Constants", build_non_redundant_full_name("Namespace::Foo::Constants", &nesting(&["Namespace", "Foo"])));
    assert_eq!("Namespace::Foo::Constants", build_non_redundant_full_name("Foo::Constants", &nesting(&["Namespace", "Foo"])));
    assert_eq!("Bar::Namespace::Foo::Constants", build_non_redundant_full_name("Namespace::Foo::Constants", &nesting(&["Bar"])));
    assert_eq!(
        "First::Namespace::Foo::Constants",
        build_non_redundant_full_name("Namespace::Foo::Constants", &nesting(&["First", "Namespace", "Foo", "InnerNamespace"]))
    );
}
