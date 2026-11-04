pub use prism_child_nodes::{child_nodes, node_type_name};
use ruby_prism::{ConstantId, Location, Node};

pub fn bytes_to_string(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

pub fn location_slice(location: &Location<'_>) -> String {
    bytes_to_string(location.as_slice())
}

pub fn node_slice(node: &Node<'_>) -> String {
    location_slice(&node.location())
}

pub fn constant_id_name(id: &ConstantId<'_>) -> String {
    bytes_to_string(id.as_slice())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstantNameError {
    DynamicParts,
    MissingNodes,
}

/// Mirrors `Prism::ConstantPathNode#full_name`, which raises for dynamic or incomplete constant paths.
pub fn constant_full_name(node: &Node<'_>) -> Result<String, ConstantNameError> {
    if let Some(read) = node.as_constant_read_node() {
        return Ok(constant_id_name(&read.name()));
    }
    if let Some(target) = node.as_constant_target_node() {
        return Ok(constant_id_name(&target.name()));
    }

    let mut parts = Vec::new();
    let mut current = Some(node.clone_node());
    while let Some(node) = current {
        if let Some(path) = node.as_constant_path_node() {
            parts.push(constant_id_name(&path.name().ok_or(ConstantNameError::MissingNodes)?));
            current = path.parent();
        } else if let Some(path) = node.as_constant_path_target_node() {
            parts.push(constant_id_name(&path.name().ok_or(ConstantNameError::MissingNodes)?));
            current = path.parent();
        } else if let Some(read) = node.as_constant_read_node() {
            parts.push(constant_id_name(&read.name()));
            parts.reverse();
            return Ok(parts.join("::"));
        } else {
            return Err(ConstantNameError::DynamicParts);
        }
    }
    parts.push(String::new());
    parts.reverse();
    Ok(parts.join("::"))
}

/// Mirrors `RubyIndexer::Index.constant_name`.
pub fn constant_name(node: &Node<'_>) -> Option<String> {
    let supported = node.as_constant_path_node().is_some()
        || node.as_constant_read_node().is_some()
        || node.as_constant_path_target_node().is_some();
    if !supported {
        return None;
    }
    constant_full_name(node).ok()
}

pub trait NodeExt<'pr> {
    fn clone_node(&self) -> Node<'pr>;
    fn start_offset(&self) -> usize;
    fn end_offset(&self) -> usize;
    fn type_name(&self) -> &'static str;
    fn same_node(&self, other: &Node<'_>) -> bool;
}

impl<'pr> NodeExt<'pr> for Node<'pr> {
    fn clone_node(&self) -> Node<'pr> {
        prism_child_nodes::copy_node(self)
    }

    fn start_offset(&self) -> usize {
        self.location().start_offset()
    }

    fn end_offset(&self) -> usize {
        self.location().end_offset()
    }

    fn type_name(&self) -> &'static str {
        node_type_name(self)
    }

    fn same_node(&self, other: &Node<'_>) -> bool {
        self.type_name() == other.type_name()
            && self.start_offset() == other.start_offset()
            && self.end_offset() == other.end_offset()
    }
}
