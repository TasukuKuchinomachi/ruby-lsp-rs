use serde::Serialize;

use crate::position::Range;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentSymbol {
    pub name: String,
    pub kind: u32,
    pub range: Range,
    pub selection_range: Range,
    pub children: Vec<DocumentSymbol>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SymbolId(usize);

struct SymbolData {
    name: String,
    kind: u32,
    range: Range,
    selection_range: Range,
    children: Vec<usize>,
}

/// Ruby LSP keeps references to the same symbol in the stack and in its parent's children, so symbols are stored in an
/// arena and linked by index to preserve that sharing
#[derive(Default)]
pub struct DocumentSymbolResponseBuilder {
    symbols: Vec<SymbolData>,
    root_children: Vec<usize>,
    stack: Vec<usize>,
}

impl DocumentSymbolResponseBuilder {
    pub fn new() -> DocumentSymbolResponseBuilder {
        DocumentSymbolResponseBuilder::default()
    }

    pub fn add_to_last(&mut self, name: String, kind: u32, range: Range, selection_range: Range) -> SymbolId {
        let id = self.symbols.len();
        self.symbols.push(SymbolData { name, kind, range, selection_range, children: Vec::new() });
        match self.stack.last() {
            Some(parent) => self.symbols[*parent].children.push(id),
            None => self.root_children.push(id),
        }
        SymbolId(id)
    }

    pub fn push(&mut self, symbol: SymbolId) {
        self.stack.push(symbol.0);
    }

    pub fn pop(&mut self) {
        self.stack.pop();
    }

    pub fn last_name(&self) -> Option<&str> {
        self.stack.last().map(|id| self.symbols[*id].name.as_str())
    }

    pub fn response(&self) -> Vec<DocumentSymbol> {
        self.root_children.iter().map(|id| self.build(*id)).collect()
    }

    fn build(&self, id: usize) -> DocumentSymbol {
        let symbol = &self.symbols[id];
        DocumentSymbol {
            name: symbol.name.clone(),
            kind: symbol.kind,
            range: symbol.range,
            selection_range: symbol.selection_range,
            children: symbol.children.iter().map(|child| self.build(*child)).collect(),
        }
    }
}
