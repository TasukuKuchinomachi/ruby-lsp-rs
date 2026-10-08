mod indexer_support;

use std::path::PathBuf;

use indexer_support::{TestCase, kind_name};
use ruby_lsp_rs::indexer::Entry;
use ruby_lsp_rs::indexer::configuration::IndexablePath;
use ruby_lsp_rs::indexer::entry::EntryKind;
use ruby_lsp_rs::indexer::index::NonExistingNamespaceError;
use ruby_lsp_rs::indexer::indexing::{index_file, index_single, index_single_with_require_path};

const FOO_URI: &str = "file:///fake/path/foo.rb";
const OTHER_FOO_URI: &str = "file:///fake/path/other_foo.rb";

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| value.to_string()).collect()
}

fn resolve_first<'a>(test: &'a mut TestCase, name: &str, nesting: &[&str]) -> &'a Entry {
    let ids = test.index.resolve(name, &strings(nesting)).unwrap_or_else(|| panic!("Expected {name} to resolve"));
    assert!(!ids.is_empty());
    test.index.entry(ids[0])
}

fn resolve_method_first<'a>(test: &'a mut TestCase, method_name: &str, receiver_name: &str, inherited_only: bool) -> &'a Entry {
    let ids = test
        .index
        .resolve_method(method_name, receiver_name, inherited_only)
        .unwrap_or_else(|| panic!("Expected {method_name} to resolve on {receiver_name}"));
    test.index.entry(ids[0])
}

fn ancestors(test: &mut TestCase, name: &str) -> Vec<String> {
    test.index.linearized_ancestors_of(name).unwrap()
}

fn write_temporary_file(name: &str, source: &str) -> (PathBuf, IndexablePath) {
    let directory = std::env::temp_dir().join(format!("ruby_lsp_rs_index_test_{name}_{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("foo.rb");
    std::fs::write(&path, source).unwrap();
    (directory, IndexablePath { path, require_path: None })
}

#[test]
fn test_deleting_one_entry_for_a_class() {
    let mut test = TestCase::new();
    index_single(&mut test.index, FOO_URI, "class Foo\nend\n", true);
    index_single(&mut test.index, OTHER_FOO_URI, "class Foo\nend\n", true);
    assert_eq!(2, test.entries("Foo").len());

    test.index.delete(OTHER_FOO_URI, false);
    assert_eq!(1, test.entries("Foo").len());
}

#[test]
fn test_deleting_all_entries_for_a_class() {
    let mut test = TestCase::new();
    index_single(&mut test.index, FOO_URI, "class Foo\nend\n", true);
    assert_eq!(1, test.entries("Foo").len());

    test.index.delete(FOO_URI, false);
    assert!(test.index.get("Foo").is_none());
}

const RESOLVE_SOURCE: &str = "class Bar; end

module Foo
  class Bar
  end

  class Baz
    class Something
    end
  end
end
";

#[test]
fn test_index_resolve() {
    let mut test = TestCase::new();
    index_single(&mut test.index, FOO_URI, RESOLVE_SOURCE, true);

    assert_eq!("Foo::Baz::Something", resolve_first(&mut test, "Something", &["Foo", "Baz"]).name);
    assert_eq!("Foo::Bar", resolve_first(&mut test, "Bar", &["Foo"]).name);
    assert_eq!("Foo::Bar", resolve_first(&mut test, "Bar", &["Foo", "Baz"]).name);
    assert_eq!("Foo::Bar", resolve_first(&mut test, "Foo::Bar", &["Foo", "Baz"]).name);
    assert!(test.index.resolve("DoesNotExist", &strings(&["Foo"])).is_none());
}

#[test]
fn test_accessing_with_colon_colon_prefix() {
    let mut test = TestCase::new();
    index_single(&mut test.index, FOO_URI, RESOLVE_SOURCE, true);
    let entries = test.entries("::Foo::Baz::Something");
    assert!(!entries.is_empty());
    assert_eq!("Foo::Baz::Something", entries[0].name);
}

#[test]
fn test_fuzzy_search() {
    let mut test = TestCase::new();
    index_single(
        &mut test.index,
        FOO_URI,
        "class Zws; end

module Qtl
  class Zws
  end

  class Zwo
    class Something
    end
  end
end
",
        true,
    );

    let names = |test: &TestCase, query: &str| -> Vec<String> {
        test.index.fuzzy_search(Some(query), |_| true).into_iter().map(|id| test.index.entry(id).name.clone()).collect()
    };

    assert_eq!(strings(&["Zws", "Qtl::Zwo::Something"]), names(&test, "Zws"));
    assert_eq!(
        strings(&["Qtl::Zwo::Something", "Qtl::Zws", "Qtl::Zwo", "Qtl", "Zws", "blocking"]),
        names(&test, "qtlzwssomeking")
    );
    assert_eq!(strings(&["Qtl::Zwo", "Qtl::Zws", "Qtl::Zwo::Something", "Qtl"]), names(&test, "QltZwo"));
}

#[test]
fn test_index_single_ignores_directories() {
    let mut test = TestCase::new();
    let directory = std::env::temp_dir().join(format!("ruby_lsp_rs_this_is_a_dir_{}.rb", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    index_file(&mut test.index, &IndexablePath { path: directory.clone(), require_path: None }, true);
    std::fs::remove_dir_all(&directory).unwrap();
    test.assert_no_indexed_entries();
}

#[test]
fn test_searching_for_require_paths() {
    let mut test = TestCase::new();
    index_single_with_require_path(&mut test.index, FOO_URI.into(), b"class Foo\nend\n", true, Some("path/foo"));
    index_single_with_require_path(&mut test.index, OTHER_FOO_URI.into(), b"class Foo\nend\n", true, Some("path/other_foo"));

    let require_paths: Vec<String> =
        test.index.search_require_paths("path").into_iter().map(|(require_path, _)| require_path.to_string()).collect();
    assert_eq!(strings(&["path/other_foo", "path/foo"]), require_paths);
}

#[test]
fn test_searching_for_entries_based_on_prefix() {
    let mut test = TestCase::new();
    index_single_with_require_path(&mut test.index, FOO_URI.into(), b"class Foo::Bizw\nend\n", true, Some("path/foo"));
    index_single_with_require_path(
        &mut test.index,
        OTHER_FOO_URI.into(),
        b"class Foo::Bizw\nend\n\nclass Foo::Bizt\nend\n",
        true,
        Some("path/other_foo"),
    );

    let names = |test: &TestCase, query: &str, nesting: &[&str]| -> Vec<Vec<String>> {
        test.index
            .prefix_search(query, Some(&strings(nesting)))
            .into_iter()
            .map(|ids| ids.into_iter().map(|id| test.index.entry(id).name.clone()).collect())
            .collect()
    };

    let expected = vec![strings(&["Foo::Bizt"]), strings(&["Foo::Bizw", "Foo::Bizw"])];
    assert_eq!(expected, names(&test, "Foo", &[]));
    assert_eq!(expected, names(&test, "Biz", &["Foo"]));
}

#[test]
fn test_resolve_normalizes_top_level_names() {
    let mut test = TestCase::new();
    index_single(&mut test.index, FOO_URI, "class Bar; end\n\nmodule Foo\n  class Bar; end\nend\n", true);

    assert_eq!("Foo::Bar", resolve_first(&mut test, "::Foo::Bar", &[]).name);
    assert_eq!("Bar", resolve_first(&mut test, "::Bar", &["Foo"]).name);
}

#[test]
fn test_resolving_aliases_to_non_existing_constants_with_conflicting_names() {
    let mut test = TestCase::new();
    index_single(
        &mut test.index,
        FOO_URI,
        "class Bar
end

module Foo
  class Bar < self
    BAZ = ::Bar::BAZ
  end
end
",
        true,
    );

    let entry = resolve_first(&mut test, "BAZ", &["Foo", "Bar"]);
    assert_eq!("UnresolvedConstantAlias", kind_name(entry));
}

#[test]
fn test_visitor_does_not_visit_unnecessary_nodes() {
    let concats: String = (0..10_000).map(|index| format!("\"string{index}\" \\\n")).collect();
    let source = format!(
        "module Foo
  local_var = {concats}    \"final\"
  @class_instance_var = {concats}    \"final\"
  @@class_var = {concats}    \"final\"
  $global_var = {concats}    \"final\"
  CONST = {concats}    \"final\"
end
"
    );

    let mut test = TestCase::new();
    test.index(&source);
}

#[test]
fn test_resolve_method_with_known_receiver() {
    let mut test = TestCase::new();
    test.index("module Foo\n  module Bar\n    def baz; end\n  end\nend\n");
    let entry = resolve_method_first(&mut test, "baz", "Foo::Bar", false);
    assert_eq!("baz", entry.name);
    assert_eq!(Some("Foo::Bar"), entry.owner());
}

#[test]
fn test_resolve_method_with_class_name_conflict() {
    let mut test = TestCase::new();
    test.index("class Array\nend\n\nclass Foo\n  def Array(*args); end\nend\n");
    let entry = resolve_method_first(&mut test, "Array", "Foo", false);
    assert_eq!("Array", entry.name);
    assert_eq!(Some("Foo"), entry.owner());
}

#[test]
fn test_resolve_method_attribute() {
    let mut test = TestCase::new();
    test.index("class Foo\n  attr_reader :bar\nend\n");
    let entry = resolve_method_first(&mut test, "bar", "Foo", false);
    assert_eq!("bar", entry.name);
    assert_eq!(Some("Foo"), entry.owner());
}

#[test]
fn test_resolve_method_with_two_definitions() {
    let mut test = TestCase::new();
    test.index(
        "class Foo
  # Hello from first `bar`
  def bar; end
end

class Foo
  # Hello from second `bar`
  def bar; end
end
",
    );

    let ids = test.index.resolve_method("bar", "Foo", false).unwrap();
    let first_entry = test.index.entry(ids[0]);
    assert_eq!("bar", first_entry.name);
    assert_eq!(Some("Foo"), first_entry.owner());
    assert!(first_entry.comments.as_deref().unwrap().contains("Hello from first `bar`"));

    let second_entry = test.index.entry(ids[1]);
    assert_eq!("bar", second_entry.name);
    assert_eq!(Some("Foo"), second_entry.owner());
    assert!(second_entry.comments.as_deref().unwrap().contains("Hello from second `bar`"));
}

#[test]
fn test_resolve_method_inherited_only() {
    let mut test = TestCase::new();
    test.index("class Bar\n  def baz; end\nend\n\nclass Foo < Bar\n  def baz; end\nend\n");
    let entry = resolve_method_first(&mut test, "baz", "Foo", true);
    assert_eq!(Some("Bar"), entry.owner());
}

#[test]
fn test_resolve_method_inherited_only_for_prepended_module() {
    let mut test = TestCase::new();
    test.index(
        "module Bar
  def baz
    super
  end
end

class Foo
  prepend Bar

  def baz; end
end
",
    );
    assert!(test.index.resolve_method("baz", "Bar", true).is_none());
}

#[test]
fn test_prefix_search_for_methods() {
    let mut test = TestCase::new();
    test.index("module Foo\n  module Bar\n    def qzx; end\n  end\nend\n");
    let entries = test.index.prefix_search("qz", None);
    assert!(!entries.is_empty());
    assert_eq!("qzx", test.index.entry(entries[0][0]).name);
}

#[test]
fn test_index_single_does_not_fail_for_non_existing_file() {
    let mut test = TestCase::new();
    index_file(&mut test.index, &IndexablePath { path: PathBuf::from("/fake/path/foo.rb"), require_path: None }, true);
    test.assert_no_indexed_entries();
}

#[test]
fn test_linearized_ancestors_basic_ordering() {
    let mut test = TestCase::new();
    test.index(
        "module A; end
module B; end

class Foo
  prepend A
  prepend B
end

class Bar
  include A
  include B
end
",
    );

    assert_eq!(strings(&["B", "A", "Foo", "Object", "Kernel", "BasicObject"]), ancestors(&mut test, "Foo"));
    assert_eq!(strings(&["Bar", "B", "A", "Object", "Kernel", "BasicObject"]), ancestors(&mut test, "Bar"));
}

#[test]
fn test_linearized_ancestors() {
    let mut test = TestCase::new();
    test.index(
        "module A; end
module B; end
module C; end

module D
  include A
end

module E
  prepend B
end

module F
  include C
  include A
end

class Bar
  prepend F
end

class Foo < Bar
  include E
  prepend D
end
",
    );

    assert_eq!(
        strings(&["D", "A", "Foo", "B", "E", "F", "A", "C", "Bar", "Object", "Kernel", "BasicObject"]),
        ancestors(&mut test, "Foo")
    );
}

#[test]
fn test_linearized_ancestors_duplicates() {
    let mut test = TestCase::new();
    test.index(
        "module A; end
module B
  include A
end

class Foo
  include B
  include A
end

class Bar
  prepend B
  prepend A
end
",
    );

    assert_eq!(strings(&["Foo", "B", "A", "Object", "Kernel", "BasicObject"]), ancestors(&mut test, "Foo"));
    assert_eq!(strings(&["B", "A", "Bar", "Object", "Kernel", "BasicObject"]), ancestors(&mut test, "Bar"));
}

#[test]
fn test_linearizing_ancestors_is_cached() {
    let mut test = TestCase::new();
    test.index("module C; end\nmodule A; end\nmodule B\n  include A\nend\n\nclass Foo\n  include B\n  include A\nend\n");
    test.index.linearized_ancestors_of("Foo").unwrap();
    assert!(test.index.has_cached_ancestors("Foo"));
    assert!(test.index.has_cached_ancestors("A"));
    assert!(test.index.has_cached_ancestors("B"));
    assert!(!test.index.has_cached_ancestors("C"));
}

#[test]
fn test_duplicate_prepend_include() {
    let mut test = TestCase::new();
    test.index(
        "module A; end

class Foo
  prepend A
  include A
end

class Bar
  include A
  prepend A
end
",
    );

    assert_eq!(strings(&["A", "Foo", "Object", "Kernel", "BasicObject"]), ancestors(&mut test, "Foo"));
    assert_eq!(strings(&["A", "Bar", "A", "Object", "Kernel", "BasicObject"]), ancestors(&mut test, "Bar"));
}

#[test]
fn test_linearizing_ancestors_handles_circular_parent_class() {
    let mut test = TestCase::new();
    test.index("class Foo < Foo\nend\n");
    assert_eq!(strings(&["Foo"]), ancestors(&mut test, "Foo"));
}

#[test]
fn test_ancestors_linearization_complex_prepend_duplication() {
    let mut test = TestCase::new();
    test.index(
        "module A; end
module B
  prepend A
end
module C
  prepend B
end

class Foo
  prepend A
  prepend C
end
",
    );

    assert_eq!(strings(&["A", "B", "C", "Foo", "Object", "Kernel", "BasicObject"]), ancestors(&mut test, "Foo"));
}

#[test]
fn test_ancestors_linearization_complex_include_duplication() {
    let mut test = TestCase::new();
    test.index(
        "module A; end
module B
  include A
end
module C
  include B
end

class Foo
  include A
  include C
end
",
    );

    assert_eq!(strings(&["Foo", "C", "B", "A", "Object", "Kernel", "BasicObject"]), ancestors(&mut test, "Foo"));
}

#[test]
fn test_linearizing_ancestors_that_need_to_be_resolved() {
    let mut test = TestCase::new();
    test.index(
        "module Foo
  module Baz
  end
  module Qux
  end

  class Something; end

  class Bar < Something
    include Baz
    prepend Qux
  end
end
",
    );

    assert_eq!(
        strings(&["Foo::Qux", "Foo::Bar", "Foo::Baz", "Foo::Something", "Object", "Kernel", "BasicObject"]),
        ancestors(&mut test, "Foo::Bar")
    );
}

#[test]
fn test_linearizing_ancestors_for_non_existing_namespaces() {
    let mut test = TestCase::new();
    test.index("def Bar(a); end\n");
    assert_eq!(Err(NonExistingNamespaceError), test.index.linearized_ancestors_of("Foo"));
    assert_eq!(Err(NonExistingNamespaceError), test.index.linearized_ancestors_of("Bar"));
}

#[test]
fn test_linearizing_circular_ancestors() {
    let mut test = TestCase::new();
    test.index(
        "module M1
  include M2
end

module M2
  include M1
end

module A1
  include A2
end

module A2
  include A3
end

module A3
  include A1
end

class Foo < Foo
  include Foo
end

module Bar
  include Bar
end
",
    );

    assert_eq!(strings(&["M2", "M1"]), ancestors(&mut test, "M2"));
    assert_eq!(strings(&["A3", "A1", "A2"]), ancestors(&mut test, "A3"));
    assert_eq!(strings(&["Foo"]), ancestors(&mut test, "Foo"));
    assert_eq!(strings(&["Bar"]), ancestors(&mut test, "Bar"));
}

#[test]
fn test_linearizing_circular_aliased_dependency() {
    let mut test = TestCase::new();
    test.index("module A\nend\n\nALIAS = A\n\nmodule A\n  include ALIAS\nend\n");
    assert_eq!(strings(&["A", "ALIAS"]), ancestors(&mut test, "A"));
}

#[test]
fn test_linearizing_ancestors_for_classes_with_overridden_parents() {
    let mut test = TestCase::new();
    test.index(
        "# Find the re-open of a class first, without specifying a parent
class Child
end

# Now, find the actual definition of the class, which includes a parent
class Parent; end
class Child < Parent
end
",
    );

    assert_eq!(strings(&["Child", "Parent", "Object", "Kernel", "BasicObject"]), ancestors(&mut test, "Child"));
}

#[test]
fn test_resolving_an_inherited_method() {
    let mut test = TestCase::new();
    test.index(
        "module Foo
  def baz; end
end

class Bar
  def qux; end
end

class Wow < Bar
  include Foo
end
",
    );

    let entry = resolve_method_first(&mut test, "baz", "Wow", false);
    assert_eq!("baz", entry.name);
    assert_eq!(Some("Foo"), entry.owner());

    let entry = resolve_method_first(&mut test, "qux", "Wow", false);
    assert_eq!("qux", entry.name);
    assert_eq!(Some("Bar"), entry.owner());
}

#[test]
fn test_resolving_an_inherited_method_lands_on_first_match() {
    let mut test = TestCase::new();
    test.index(
        "module Foo
  def qux; end
end

class Bar
  def qux; end
end

class Wow < Bar
  prepend Foo

  def qux; end
end
",
    );

    let ids = test.index.resolve_method("qux", "Wow", false).unwrap();
    assert_eq!(1, ids.len());
    let entry = test.index.entry(ids[0]);
    assert_eq!("qux", entry.name);
    assert_eq!(Some("Foo"), entry.owner());
}

fn assert_handle_change(
    name: &str,
    original: &str,
    updated: &str,
    original_ancestors: &[&str],
    updated_ancestors: &[&str],
    expect_cache_cleared: bool,
) {
    let mut test = TestCase::new();
    let (directory, indexable_path) = write_temporary_file(name, original);
    index_file(&mut test.index, &indexable_path, true);
    assert_eq!(strings(original_ancestors), ancestors(&mut test, "Bar"));

    std::fs::write(&indexable_path.path, updated).unwrap();
    let uri = ruby_lsp_rs::uri::from_path_buf(&indexable_path.path);
    let source = std::fs::read_to_string(&indexable_path.path).unwrap();
    test.index.handle_change_with_source(&uri, &source);
    assert_eq!(expect_cache_cleared, test.index.is_ancestors_cache_empty());
    assert_eq!(strings(updated_ancestors), ancestors(&mut test, "Bar"));
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn test_handle_change_clears_ancestor_cache_if_tree_changed() {
    assert_handle_change(
        "tree_changed",
        "module Foo\nend\n\nclass Bar\n  include Foo\nend\n",
        "module Foo\nend\n\nclass Bar\nend\n",
        &["Bar", "Foo", "Object", "Kernel", "BasicObject"],
        &["Bar", "Object", "Kernel", "BasicObject"],
        true,
    );
}

#[test]
fn test_handle_change_does_not_clear_ancestor_cache_if_tree_not_changed() {
    assert_handle_change(
        "tree_not_changed",
        "module Foo\nend\n\nclass Bar\n  include Foo\nend\n",
        "module Foo\nend\n\nclass Bar\n  include Foo\n\n  def baz; end\nend\n",
        &["Bar", "Foo", "Object", "Kernel", "BasicObject"],
        &["Bar", "Foo", "Object", "Kernel", "BasicObject"],
        false,
    );
}

#[test]
fn test_handle_change_clears_ancestor_cache_if_parent_class_changed() {
    assert_handle_change(
        "parent_class_changed",
        "class Foo\nend\n\nclass Bar < Foo\nend\n",
        "class Foo\nend\n\nclass Bar\nend\n",
        &["Bar", "Foo", "Object", "Kernel", "BasicObject"],
        &["Bar", "Object", "Kernel", "BasicObject"],
        true,
    );
}

#[test]
fn test_resolving_inherited_constants() {
    let mut test = TestCase::new();
    test.index(
        "module Foo
  CONST = 1
end

module Baz
  CONST = 2
end

module Qux
  include Foo
end

module Namespace
  CONST = 3

  include Baz

  class Bar
    include Qux
  end
end

CONST = 4
",
    );

    assert_eq!(14, resolve_first(&mut test, "CONST", &["Namespace", "Bar"]).location.start_line);
}

#[test]
fn test_resolving_inherited_aliased_namespace() {
    let mut test = TestCase::new();
    test.index(
        "module Bar
  TARGET = 123
end

module Foo
  CONST = Bar
end

module Namespace
  class Bar
    include Foo
  end
end
",
    );

    assert_eq!(2, resolve_first(&mut test, "Foo::CONST::TARGET", &[]).location.start_line);
    assert_eq!(2, resolve_first(&mut test, "Namespace::Bar::CONST::TARGET", &[]).location.start_line);
}

#[test]
fn test_resolving_same_constant_from_different_scopes() {
    let mut test = TestCase::new();
    test.index(
        "module Namespace
  CONST = 123

  class Parent
    CONST = 321
  end

  class Child < Parent
  end
end
",
    );

    assert_eq!(2, resolve_first(&mut test, "CONST", &["Namespace", "Child"]).location.start_line);
    assert_eq!(5, resolve_first(&mut test, "Namespace::Child::CONST", &[]).location.start_line);
}

#[test]
fn test_resolving_prepended_constants() {
    let mut test = TestCase::new();
    test.index(
        "module Included
  CONST = 123
end

module Prepended
  CONST = 321
end

class Foo
  include Included
  prepend Prepended
end

class Bar
  CONST = 456
  include Included
  prepend Prepended
end
",
    );

    assert_eq!(6, resolve_first(&mut test, "CONST", &["Foo"]).location.start_line);
    assert_eq!(6, resolve_first(&mut test, "Foo::CONST", &[]).location.start_line);
    assert_eq!(15, resolve_first(&mut test, "Bar::CONST", &[]).location.start_line);
}

#[test]
fn test_resolving_constants_favors_ancestors_over_top_level() {
    let mut test = TestCase::new();
    test.index(
        "module Value1
  CONST = 1
end

module Value2
  CONST = 2
end

CONST = 3
module First
  include Value1

  module Second
    include Value2
  end
end
",
    );

    assert_eq!(6, resolve_first(&mut test, "CONST", &["First", "Second"]).location.start_line);
}

#[test]
fn test_resolving_circular_alias() {
    let mut test = TestCase::new();
    test.index("module Namespace\n  FOO = BAR\n  BAR = FOO\nend\n");

    let foo_entry = resolve_first(&mut test, "FOO", &["Namespace"]);
    assert_eq!(2, foo_entry.location.start_line);
    assert_eq!("ConstantAlias", kind_name(foo_entry));

    let bar_entry = resolve_first(&mut test, "BAR", &["Namespace"]);
    assert_eq!(3, bar_entry.location.start_line);
    assert_eq!("ConstantAlias", kind_name(bar_entry));
}

#[test]
fn test_resolving_circular_alias_three_levels() {
    let mut test = TestCase::new();
    test.index("module Namespace\n  FOO = BAR\n  BAR = BAZ\n  BAZ = FOO\nend\n");

    let foo_entry = resolve_first(&mut test, "FOO", &["Namespace"]);
    assert_eq!(2, foo_entry.location.start_line);
    assert_eq!("ConstantAlias", kind_name(foo_entry));

    let bar_entry = resolve_first(&mut test, "BAR", &["Namespace"]);
    assert_eq!(3, bar_entry.location.start_line);
    assert_eq!("ConstantAlias", kind_name(bar_entry));

    let baz_entry = resolve_first(&mut test, "BAZ", &["Namespace"]);
    assert_eq!(4, baz_entry.location.start_line);
    assert_eq!("ConstantAlias", kind_name(baz_entry));
}

#[test]
fn test_resolving_constants_in_aliased_namespace() {
    let mut test = TestCase::new();
    test.index(
        "module Original
  module Something
    CONST = 123
  end
end

module Other
  ALIAS = Original::Something
end

module Third
  Other::ALIAS::CONST
end
",
    );

    let entry = resolve_first(&mut test, "Other::ALIAS::CONST", &["Third"]);
    assert!(matches!(entry.kind, EntryKind::Constant));
    assert_eq!("Original::Something::CONST", entry.name);
}

#[test]
fn test_resolving_top_level_aliases() {
    let mut test = TestCase::new();
    test.index("class Foo\n  CONST = 123\nend\n\nFOO = Foo\nFOO::CONST\n");

    let entry = resolve_first(&mut test, "FOO::CONST", &[]);
    assert!(matches!(entry.kind, EntryKind::Constant));
    assert_eq!("Foo::CONST", entry.name);
}

#[test]
fn test_resolving_top_level_compact_reference() {
    let mut test = TestCase::new();
    test.index("class Foo::Bar\nend\n");

    let foo_entry = resolve_first(&mut test, "Foo::Bar", &[]);
    assert_eq!(1, foo_entry.location.start_line);
    assert_eq!("Class", kind_name(foo_entry));
}

#[test]
fn test_resolving_references_with_redundant_namespaces() {
    let mut test = TestCase::new();
    test.index(
        "module Bar
  CONST = 1
end

module A
  CONST = 2

  module B
    CONST = 3

    class Foo
      include Bar
    end

    A::B::Foo::CONST
  end
end
",
    );

    assert_eq!(2, resolve_first(&mut test, "A::B::Foo::CONST", &["A", "B"]).location.start_line);
}
