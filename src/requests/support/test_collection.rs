use indexmap::IndexMap;
use serde_json::{Value, json};

use crate::position::{Position, Range};
use crate::requests::support::test_item::TestItem;

#[derive(Debug, Default)]
pub struct TestCollection {
    items: IndexMap<String, TestItem>,
    code_lens: Vec<Value>,
}

impl TestCollection {
    pub fn new() -> TestCollection {
        TestCollection::default()
    }

    pub fn add(&mut self, item: TestItem) {
        self.items.insert(item.id.clone(), item);
    }

    pub fn add_code_lens(&mut self, item: &TestItem) {
        let arguments = json!([crate::uri::to_path(&item.uri), item.id]);
        let start = item.range.start;
        let range = Range::new(start, Position::new(start.line, start.character + 1));

        for kind in ["run_test", "run_test_in_terminal", "debug_test"] {
            self.code_lens.push(json!({ "range": range, "data": { "arguments": arguments, "kind": kind } }));
        }
    }

    pub fn get(&self, id: &str) -> Option<&TestItem> {
        self.items.get(id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut TestItem> {
        self.items.get_mut(id)
    }

    /// Test items are addressed by the ids leading to them, since a listener cannot hold a reference into the tree
    /// while other listeners keep adding items to it
    pub fn item_at_mut(&mut self, path: &[String]) -> Option<&mut TestItem> {
        let (first, rest) = path.split_first()?;
        let mut item = self.items.get_mut(first)?;
        for id in rest {
            item = item.get_mut(id)?;
        }
        Some(item)
    }

    pub fn item_at(&self, path: &[String]) -> Option<&TestItem> {
        let (first, rest) = path.split_first()?;
        let mut item = self.items.get(first)?;
        for id in rest {
            item = item.get(id)?;
        }
        Some(item)
    }

    pub fn add_at(&mut self, parent_path: &[String], item: TestItem) -> Option<Vec<String>> {
        let mut item_path = parent_path.to_vec();
        item_path.push(item.id.clone());
        let code_lens_item = item.clone();
        if parent_path.is_empty() {
            self.add(item);
        } else {
            self.item_at_mut(parent_path)?.add(item);
        }
        self.add_code_lens(&code_lens_item);
        Some(item_path)
    }

    pub fn code_lens(&self) -> &[Value] {
        &self.code_lens
    }

    pub fn into_code_lens(self) -> Vec<Value> {
        self.code_lens
    }

    pub fn response(&self) -> Vec<Value> {
        self.items.values().map(TestItem::to_json).collect()
    }
}
