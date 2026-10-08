use indexmap::IndexMap;
use serde_json::{Value, json};

use crate::position::Range;

#[derive(Debug, Clone)]
pub struct TestItem {
    pub id: String,
    pub label: String,
    pub uri: String,
    pub range: Range,
    tags: Vec<String>,
    children: IndexMap<String, TestItem>,
}

impl TestItem {
    pub fn new(id: String, label: String, uri: &str, range: Range, framework: &str) -> TestItem {
        TestItem {
            id,
            label,
            uri: uri.to_string(),
            range,
            tags: vec![format!("framework:{framework}")],
            children: IndexMap::new(),
        }
    }

    pub fn add(&mut self, item: TestItem) {
        self.children.insert(item.id.clone(), item);
    }

    pub fn get(&self, id: &str) -> Option<&TestItem> {
        self.children.get(id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut TestItem> {
        self.children.get_mut(id)
    }

    pub fn children(&self) -> impl Iterator<Item = &TestItem> {
        self.children.values()
    }

    pub fn tags(&self) -> &[String] {
        &self.tags
    }

    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "label": self.label,
            "uri": self.uri,
            "range": self.range,
            "tags": self.tags,
            "children": self.children().map(TestItem::to_json).collect::<Vec<Value>>(),
        })
    }
}
