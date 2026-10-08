/// A character trie whose `search` yields values in the same order as Ruby LSP's `PrefixTree`, which pops a stack of
/// children kept in insertion order. Matching that order keeps completion results identical.
#[derive(Clone)]
pub struct PrefixTree<V> {
    nodes: Vec<TrieNode<V>>,
    free_nodes: Vec<usize>,
}

#[derive(Clone)]
struct TrieNode<V> {
    key: char,
    parent: Option<usize>,
    children: Vec<usize>,
    value: Option<V>,
}

impl<V: Clone> Default for PrefixTree<V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<V: Clone> PrefixTree<V> {
    pub fn new() -> Self {
        PrefixTree {
            nodes: vec![TrieNode { key: '\0', parent: None, children: Vec::new(), value: None }],
            free_nodes: Vec::new(),
        }
    }

    fn child(&self, node: usize, character: char) -> Option<usize> {
        self.nodes[node].children.iter().copied().find(|child| self.nodes[*child].key == character)
    }

    fn find_node(&self, key: &str) -> Option<usize> {
        let mut node = 0;
        for character in key.chars() {
            node = self.child(node, character)?;
        }
        Some(node)
    }

    pub fn insert(&mut self, key: &str, value: V) {
        let mut node = 0;
        for character in key.chars() {
            node = match self.child(node, character) {
                Some(child) => child,
                None => {
                    let new_node = TrieNode { key: character, parent: Some(node), children: Vec::new(), value: None };
                    let index = match self.free_nodes.pop() {
                        Some(index) => {
                            self.nodes[index] = new_node;
                            index
                        }
                        None => {
                            self.nodes.push(new_node);
                            self.nodes.len() - 1
                        }
                    };
                    self.nodes[node].children.push(index);
                    index
                }
            };
        }
        self.nodes[node].value = Some(value);
    }

    pub fn delete(&mut self, key: &str) {
        let Some(mut node) = self.find_node(key) else { return };
        self.nodes[node].value = None;

        while let Some(parent) = self.nodes[node].parent {
            if !self.nodes[node].children.is_empty() || self.nodes[node].value.is_some() {
                return;
            }
            self.nodes[parent].children.retain(|child| *child != node);
            self.free_nodes.push(node);
            node = parent;
        }
    }

    pub fn search(&self, prefix: &str) -> Vec<V> {
        let Some(start) = self.find_node(prefix) else { return Vec::new() };
        let mut result = Vec::new();
        let mut stack = vec![start];
        while let Some(node) = stack.pop() {
            if let Some(value) = &self.nodes[node].value {
                result.push(value.clone());
            }
            stack.extend(self.nodes[node].children.iter().copied());
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn searches_by_prefix() {
        let mut tree = PrefixTree::new();
        tree.insert("foo", "foo");
        tree.insert("foobar", "foobar");
        tree.insert("fizz", "fizz");
        assert_eq!(tree.search("f"), vec!["fizz", "foo", "foobar"]);
        tree.delete("foo");
        assert_eq!(tree.search("foo"), vec!["foobar"]);
        tree.delete("foobar");
        assert!(tree.search("foo").is_empty());
        assert_eq!(tree.search(""), vec!["fizz"]);
    }
}
