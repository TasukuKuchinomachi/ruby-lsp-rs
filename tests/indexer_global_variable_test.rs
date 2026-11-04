mod indexer_support;

use indexer_support::TestCase;

#[test]
fn test_global_variable_and_write() {
    let mut test = TestCase::new();
    test.index("$foo &&= 1\n");
    test.assert_entry("$foo", "GlobalVariable", "/fake/path/foo.rb:0-0:0-4");
}

#[test]
fn test_global_variable_operator_write() {
    let mut test = TestCase::new();
    test.index("$foo += 1\n");
    test.assert_entry("$foo", "GlobalVariable", "/fake/path/foo.rb:0-0:0-4");
}

#[test]
fn test_global_variable_or_write() {
    let mut test = TestCase::new();
    test.index("$foo ||= 1\n");
    test.assert_entry("$foo", "GlobalVariable", "/fake/path/foo.rb:0-0:0-4");
}

#[test]
fn test_global_variable_target_node() {
    let mut test = TestCase::new();
    test.index("$foo, $bar = 1\n");
    test.assert_entry("$foo", "GlobalVariable", "/fake/path/foo.rb:0-0:0-4");
    test.assert_entry("$bar", "GlobalVariable", "/fake/path/foo.rb:0-6:0-10");
}

#[test]
fn test_global_variable_write() {
    let mut test = TestCase::new();
    test.index("$foo = 1\n");
    test.assert_entry("$foo", "GlobalVariable", "/fake/path/foo.rb:0-0:0-4");
}
