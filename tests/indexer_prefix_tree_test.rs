use ruby_lsp_rs::indexer::prefix_tree::PrefixTree;

fn tree_of(items: &[&'static str]) -> PrefixTree<&'static str> {
    let mut tree = PrefixTree::new();
    for item in items {
        tree.insert(item, *item);
    }
    tree
}

#[test]
fn test_empty() {
    let tree: PrefixTree<&str> = PrefixTree::new();
    assert!(tree.search("").is_empty());
    assert!(tree.search("foo").is_empty());
}

#[test]
fn test_single_item() {
    let tree = tree_of(&["foo"]);
    assert_eq!(vec!["foo"], tree.search(""));
    assert_eq!(vec!["foo"], tree.search("foo"));
    assert!(tree.search("bar").is_empty());
}

#[test]
fn test_multiple_items() {
    let tree = tree_of(&["foo", "bar", "baz"]);
    assert_eq!(vec!["baz", "bar", "foo"], tree.search(""));
    assert_eq!(vec!["baz", "bar"], tree.search("b"));
    assert_eq!(vec!["foo"], tree.search("fo"));
    assert_eq!(vec!["baz", "bar"], tree.search("ba"));
    assert_eq!(vec!["baz"], tree.search("baz"));
    assert!(tree.search("qux").is_empty());
}

#[test]
fn test_multiple_prefixes() {
    let tree = tree_of(&["fo", "foo"]);
    assert_eq!(vec!["fo", "foo"], tree.search(""));
    assert_eq!(vec!["fo", "foo"], tree.search("f"));
    assert_eq!(vec!["fo", "foo"], tree.search("fo"));
    assert_eq!(vec!["foo"], tree.search("foo"));
    assert!(tree.search("fooo").is_empty());
}

#[test]
fn test_multiple_prefixes_with_shuffled_order() {
    let tree = tree_of(&[
        "foo/bar/base",
        "foo/bar/on",
        "foo/bar/support/selection",
        "foo/bar/support/runner",
        "foo/internal",
        "foo/bar/document",
        "foo/bar/code",
        "foo/bar/support/rails",
        "foo/bar/diagnostics",
        "foo/bar/document2",
        "foo/bar/support/runner2",
        "foo/bar/support/diagnostic",
        "foo/document",
        "foo/bar/formatting",
        "foo/bar/support/highlight",
        "foo/bar/semantic",
        "foo/bar/support/prefix",
        "foo/bar/folding",
        "foo/bar/selection",
        "foo/bar/support/syntax",
        "foo/bar/document3",
        "foo/bar/hover",
        "foo/bar/support/semantic",
        "foo/bar/support/source",
        "foo/bar/inlay",
        "foo/requests",
        "foo/bar/support/formatting",
        "foo/bar/path",
        "foo/executor",
    ]);

    assert_eq!(
        vec![
            "foo/bar/support/formatting",
            "foo/bar/support/prefix",
            "foo/bar/support/highlight",
            "foo/bar/support/diagnostic",
            "foo/bar/support/rails",
            "foo/bar/support/runner",
            "foo/bar/support/runner2",
            "foo/bar/support/source",
            "foo/bar/support/syntax",
            "foo/bar/support/semantic",
            "foo/bar/support/selection",
        ],
        tree.search("foo/bar/support"),
    );
}

#[test]
fn test_deletion() {
    let mut tree = tree_of(&["foo/bar", "foo/baz"]);
    assert_eq!(vec!["foo/baz", "foo/bar"], tree.search("foo"));

    tree.delete("foo/bar");
    assert!(tree.search("foo/bar").is_empty());
    assert_eq!(vec!["foo/baz"], tree.search("foo"));
}

#[test]
fn test_delete_does_not_impact_other_keys_with_the_same_value() {
    let mut tree = PrefixTree::new();
    tree.insert("key1", "value");
    tree.insert("key2", "value");
    assert_eq!(vec!["value", "value"], tree.search("key"));

    tree.delete("key2");
    assert!(tree.search("key2").is_empty());
    assert_eq!(vec!["value"], tree.search("key1"));
}

#[test]
fn test_deleted_node_is_removed_from_the_tree() {
    let mut tree = tree_of(&["foo/bar"]);
    assert_eq!(vec!["foo/bar"], tree.search("foo"));

    tree.delete("foo/bar");
    // The root node is private, so the removal is observed through searches instead of inspecting the root's children
    assert!(tree.search("").is_empty());
    assert!(tree.search("f").is_empty());
}

#[test]
fn test_deleting_non_terminal_nodes() {
    let mut tree = PrefixTree::new();
    tree.insert("abc", "value1");
    tree.insert("abcdef", "value2");

    tree.delete("abcdef");
    assert!(tree.search("abcdef").is_empty());
    assert_eq!(vec!["value1"], tree.search("abc"));
}

#[test]
fn test_overriding_values() {
    let mut tree = PrefixTree::new();
    tree.insert("foo/bar", 123);
    assert_eq!(vec![123], tree.search("foo/bar"));

    tree.insert("foo/bar", 456);
    assert_eq!(vec![456], tree.search("foo/bar"));
}
