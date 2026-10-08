mod indexer_support;

use indexer_support::{TestCase, kind_name};
use ruby_lsp_rs::indexer::EntryKind;

fn assert_unresolved_alias(test: &TestCase, name: &str, expected_nesting: &[&str], expected_target: &str) {
    let entry = test.first(name);
    let EntryKind::UnresolvedConstantAlias { target, nesting } = &entry.kind else {
        panic!("Expected {name} to be an UnresolvedConstantAlias, got {}", kind_name(entry));
    };
    assert_eq!(expected_nesting, nesting.as_slice());
    assert_eq!(expected_target, target);
}

fn assert_resolves_to_alias(test: &mut TestCase, name: &str, nesting: &[&str], expected_target: &str) {
    let nesting: Vec<String> = nesting.iter().map(|part| part.to_string()).collect();
    let resolved = test.index.resolve(name, &nesting).unwrap_or_else(|| panic!("Expected {name} to resolve"));
    let entry = test.index.entry(resolved[0]);
    let EntryKind::ConstantAlias { target } = &entry.kind else {
        panic!("Expected {name} to resolve to a ConstantAlias, got {}", kind_name(entry));
    };
    assert_eq!(expected_target, target);
}

fn assert_constant_alias(test: &TestCase, name: &str, expected_target: &str) {
    let entry = test.first(name);
    let EntryKind::ConstantAlias { target } = &entry.kind else {
        panic!("Expected {name} to be a ConstantAlias, got {}", kind_name(entry));
    };
    assert_eq!(expected_target, target);
}

#[test]
fn test_constant_writes() {
    let mut test = TestCase::new();
    test.index("FOO = 1\n\nclass ::Bar\n  FOO = 2\nend\n\nBAR = 3 if condition\n");
    test.assert_entry("FOO", "Constant", "/fake/path/foo.rb:0-0:0-7");
    test.assert_entry("Bar::FOO", "Constant", "/fake/path/foo.rb:3-2:3-9");
    test.assert_entry("BAR", "Constant", "/fake/path/foo.rb:6-0:6-7");
}

#[test]
fn test_constant_with_multibyte_characters() {
    let mut test = TestCase::new();
    test.index("CONST_💎 = \"Ruby\"\n");
    test.assert_entry("CONST_💎", "Constant", "/fake/path/foo.rb:0-0:0-16");
}

#[test]
fn test_constant_or_writes() {
    let mut test = TestCase::new();
    test.index("FOO ||= 1\n\nclass ::Bar\n  FOO ||= 2\nend\n");
    test.assert_entry("FOO", "Constant", "/fake/path/foo.rb:0-0:0-9");
    test.assert_entry("Bar::FOO", "Constant", "/fake/path/foo.rb:3-2:3-11");
}

#[test]
fn test_constant_path_writes() {
    let mut test = TestCase::new();
    test.index(
        "class A\n  FOO = 1\n  ::BAR = 1\n\n  module B\n    FOO = 1\n  end\nend\n\nA::BAZ = 1\n",
    );
    test.assert_entry("A::FOO", "Constant", "/fake/path/foo.rb:1-2:1-9");
    test.assert_entry("BAR", "Constant", "/fake/path/foo.rb:2-2:2-11");
    test.assert_entry("A::B::FOO", "Constant", "/fake/path/foo.rb:5-4:5-11");
    test.assert_entry("A::BAZ", "Constant", "/fake/path/foo.rb:9-0:9-10");
}

#[test]
fn test_constant_path_or_writes() {
    let mut test = TestCase::new();
    test.index("class A\n  FOO ||= 1\n  ::BAR ||= 1\nend\n\nA::BAZ ||= 1\n");
    test.assert_entry("A::FOO", "Constant", "/fake/path/foo.rb:1-2:1-11");
    test.assert_entry("BAR", "Constant", "/fake/path/foo.rb:2-2:2-13");
    test.assert_entry("A::BAZ", "Constant", "/fake/path/foo.rb:5-0:5-12");
}

#[test]
fn test_comments_for_constants() {
    let mut test = TestCase::new();
    test.index(
        "# FOO comment\nFOO = 1\n\nclass A\n  # A::FOO comment\n  FOO = 1\n\n  # ::BAR comment\n  ::BAR = 1\nend\n\n# A::BAZ comment\nA::BAZ = 1\n",
    );
    assert_eq!(Some("FOO comment"), test.first("FOO").comments.as_deref());
    assert_eq!(Some("A::FOO comment"), test.first("A::FOO").comments.as_deref());
    assert_eq!(Some("::BAR comment"), test.first("BAR").comments.as_deref());
    assert_eq!(Some("A::BAZ comment"), test.first("A::BAZ").comments.as_deref());
}

#[test]
fn test_variable_path_constants_are_ignored() {
    let mut test = TestCase::new();
    test.index("var::FOO = 1\nself.class::FOO = 1\n");
    test.assert_no_indexed_entries();
}

#[test]
fn test_private_constant_indexing() {
    let mut test = TestCase::new();
    test.index(
        "class A\n  B = 1\n  private_constant(:B)\n\n  C = 2\n  private_constant(\"C\")\n\n  D = 1\nend\n",
    );
    assert!(test.first("A::B").is_private());
    assert!(test.first("A::C").is_private());
    assert!(test.first("A::D").is_public());
}

#[test]
fn test_marking_constants_as_private_reopening_namespaces() {
    let mut test = TestCase::new();
    test.index(
        "module A\n  module B\n    CONST_A = 1\n    private_constant(:CONST_A)\n\n    CONST_B = 2\n    CONST_C = 3\n  end\n\n  module B\n    private_constant(:CONST_B)\n  end\nend\n\nmodule A\n  module B\n    private_constant(:CONST_C)\n  end\nend\n",
    );
    assert!(test.first("A::B::CONST_A").is_private());
    assert!(test.first("A::B::CONST_B").is_private());
    assert!(test.first("A::B::CONST_C").is_private());
}

#[test]
fn test_marking_constants_as_private_with_receiver() {
    let mut test = TestCase::new();
    test.index(
        "module A\n  module B\n    CONST_A = 1\n    CONST_B = 2\n  end\n\n  B.private_constant(:CONST_A)\nend\n\nA::B.private_constant(:CONST_B)\n",
    );
    assert!(test.first("A::B::CONST_A").is_private());
    assert!(test.first("A::B::CONST_B").is_private());
}

#[test]
fn test_indexing_constant_aliases() {
    let mut test = TestCase::new();
    test.index("module A\n  module B\n    module C\n    end\n  end\n\n  FIRST = B::C\nend\n\nSECOND = A::FIRST\n");
    assert_unresolved_alias(&test, "A::FIRST", &["A"], "B::C");
    assert_resolves_to_alias(&mut test, "A::FIRST", &[], "A::B::C");
}

#[test]
fn test_aliasing_namespaces() {
    let mut test = TestCase::new();
    test.index(
        "module A\n  module B\n    module C\n    end\n  end\n\n  ALIAS = B\nend\n\nmodule Other\n  ONE_MORE = A::ALIAS\nend\n",
    );
    assert_unresolved_alias(&test, "A::ALIAS", &["A"], "B");
    assert_resolves_to_alias(&mut test, "ALIAS", &["A"], "A::B");

    let resolved = test.index.resolve("ALIAS::C", &["A".to_string()]).unwrap();
    let entry = test.index.entry(resolved[0]);
    assert_eq!("Module", kind_name(entry));
    assert_eq!("A::B::C", entry.name);

    assert_unresolved_alias(&test, "Other::ONE_MORE", &["Other"], "A::ALIAS");

    let resolved = test.index.resolve("Other::ONE_MORE::C", &[]).unwrap();
    assert_eq!("Module", kind_name(test.index.entry(resolved[0])));
}

#[test]
fn test_indexing_same_line_constant_aliases() {
    let mut test = TestCase::new();
    test.index(
        "module A\n  B = C = 1\n  D = E ||= 1\n  F = G::H &&= 1\n  I::J = K::L = M = 1\nend\n",
    );

    assert_unresolved_alias(&test, "A::B", &["A"], "C");
    assert_resolves_to_alias(&mut test, "A::B", &[], "A::C");
    assert_eq!("Constant", kind_name(test.first("A::C")));

    assert_unresolved_alias(&test, "A::D", &["A"], "E");
    assert_resolves_to_alias(&mut test, "A::D", &[], "A::E");

    assert_unresolved_alias(&test, "A::F", &["A"], "G::H");
    assert_resolves_to_alias(&mut test, "A::F", &[], "A::G::H");

    assert_unresolved_alias(&test, "A::I::J", &["A"], "K::L");
    assert_resolves_to_alias(&mut test, "A::I::J", &[], "A::K::L");

    assert_constant_alias(&test, "A::K::L", "A::M");
    assert_eq!("Constant", kind_name(test.first("A::M")));
}

#[test]
fn test_indexing_or_and_operator_nodes() {
    let mut test = TestCase::new();
    test.index("A ||= 1\nB &&= 2\nC &= 3\nD::E ||= 4\nF::G &&= 5\nH::I &= 6\n");
    test.assert_entry("A", "Constant", "/fake/path/foo.rb:0-0:0-7");
    test.assert_entry("B", "Constant", "/fake/path/foo.rb:1-0:1-7");
    test.assert_entry("C", "Constant", "/fake/path/foo.rb:2-0:2-6");
    test.assert_entry("D::E", "Constant", "/fake/path/foo.rb:3-0:3-10");
    test.assert_entry("F::G", "Constant", "/fake/path/foo.rb:4-0:4-10");
    test.assert_entry("H::I", "Constant", "/fake/path/foo.rb:5-0:5-9");
}

#[test]
fn test_indexing_constant_targets() {
    let mut test = TestCase::new();
    test.index(
        "module A\n  B, C = [1, Y]\n  D::E, F::G = [Z, 4]\n  H, I::J = [5, B]\n  K, L = C\nend\n\nmodule Real\n  Z = 1\n  Y = 2\nend\n",
    );
    test.assert_entry("A::B", "Constant", "/fake/path/foo.rb:1-2:1-3");
    test.assert_entry("A::C", "UnresolvedConstantAlias", "/fake/path/foo.rb:1-5:1-6");
    test.assert_entry("A::D::E", "UnresolvedConstantAlias", "/fake/path/foo.rb:2-2:2-6");
    test.assert_entry("A::F::G", "Constant", "/fake/path/foo.rb:2-8:2-12");
    test.assert_entry("A::H", "Constant", "/fake/path/foo.rb:3-2:3-3");
    test.assert_entry("A::I::J", "UnresolvedConstantAlias", "/fake/path/foo.rb:3-5:3-9");
    test.assert_entry("A::K", "Constant", "/fake/path/foo.rb:4-2:4-3");
    test.assert_entry("A::L", "Constant", "/fake/path/foo.rb:4-5:4-6");
}

#[test]
fn test_indexing_constant_targets_with_splats() {
    let mut test = TestCase::new();
    test.index(
        "A, *, B = baz\nC, = bar\n(D, E) = baz\nF, G = *baz, qux\nH, I = [baz, *qux]\nJ, L = [*something, String]\nM = [String]\n",
    );
    test.assert_entry("A", "Constant", "/fake/path/foo.rb:0-0:0-1");
    test.assert_entry("B", "Constant", "/fake/path/foo.rb:0-6:0-7");
    test.assert_entry("D", "Constant", "/fake/path/foo.rb:2-1:2-2");
    test.assert_entry("E", "Constant", "/fake/path/foo.rb:2-4:2-5");
    test.assert_entry("F", "Constant", "/fake/path/foo.rb:3-0:3-1");
    test.assert_entry("G", "Constant", "/fake/path/foo.rb:3-3:3-4");
    test.assert_entry("H", "Constant", "/fake/path/foo.rb:4-0:4-1");
    test.assert_entry("I", "Constant", "/fake/path/foo.rb:4-3:4-4");
    test.assert_entry("J", "Constant", "/fake/path/foo.rb:5-0:5-1");
    test.assert_entry("L", "Constant", "/fake/path/foo.rb:5-3:5-4");
    test.assert_entry("M", "Constant", "/fake/path/foo.rb:6-0:6-12");
}

#[test]
fn test_indexing_destructuring_an_array() {
    let mut test = TestCase::new();
    test.index("Baz = [1, 2]\nFoo, Bar = Baz\nThis, That = foo, bar\n");
    test.assert_entry("Baz", "Constant", "/fake/path/foo.rb:0-0:0-12");
    test.assert_entry("Foo", "Constant", "/fake/path/foo.rb:1-0:1-3");
    test.assert_entry("Bar", "Constant", "/fake/path/foo.rb:1-5:1-8");
    test.assert_entry("This", "Constant", "/fake/path/foo.rb:2-0:2-4");
    test.assert_entry("That", "Constant", "/fake/path/foo.rb:2-6:2-10");
}
