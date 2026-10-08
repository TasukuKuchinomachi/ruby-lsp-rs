use ruby_prism::{ConstantId, Location, Node};

pub(crate) fn constant_text(identifier: &ConstantId<'_>) -> String {
    String::from_utf8_lossy(identifier.as_slice()).into_owned()
}

pub(crate) fn bytes_text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Span {
    pub(crate) start: usize,
    pub(crate) end: usize,
}

impl Span {
    pub(crate) fn of_location(location: &Location<'_>) -> Span {
        let start = location.start_offset();
        let end = location.end_offset().max(start);
        Span { start, end }
    }

    pub(crate) fn of_node(node: &Node<'_>) -> Span {
        Span::of_location(&node.location())
    }
}

pub(crate) struct LineIndex {
    line_starts: Vec<usize>,
}

impl LineIndex {
    pub(crate) fn new(source: &[u8]) -> LineIndex {
        let mut line_starts = vec![0];
        for (offset, byte) in source.iter().enumerate() {
            if *byte == b'\n' {
                line_starts.push(offset + 1);
            }
        }
        LineIndex { line_starts }
    }

    pub(crate) fn line_of(&self, offset: usize) -> usize {
        match self.line_starts.binary_search(&offset) {
            Ok(line) => line,
            Err(next_line) => next_line.saturating_sub(1),
        }
    }
}

pub(crate) struct ConstantPath {
    pub(crate) absolute: bool,
    pub(crate) segments: Vec<String>,
}

impl ConstantPath {
    pub(crate) fn of_node(node: &Node<'_>) -> Option<ConstantPath> {
        let mut segments = Vec::new();
        let mut current = prism_child_nodes::copy_node(node);
        for _ in 0..64 {
            match current {
                Node::ConstantReadNode { .. } => {
                    let read = current.as_constant_read_node()?;
                    segments.push(constant_text(&read.name()));
                    segments.reverse();
                    return Some(ConstantPath {
                        absolute: false,
                        segments,
                    });
                }
                Node::ConstantTargetNode { .. } => {
                    let target = current.as_constant_target_node()?;
                    segments.push(constant_text(&target.name()));
                    segments.reverse();
                    return Some(ConstantPath {
                        absolute: false,
                        segments,
                    });
                }
                Node::ConstantPathNode { .. } => {
                    let path = current.as_constant_path_node()?;
                    segments.push(constant_text(&path.name()?));
                    match path.parent() {
                        Some(parent) => current = parent,
                        None => {
                            segments.reverse();
                            return Some(ConstantPath {
                                absolute: true,
                                segments,
                            });
                        }
                    }
                }
                Node::ConstantPathTargetNode { .. } => {
                    let path = current.as_constant_path_target_node()?;
                    segments.push(constant_text(&path.name()?));
                    match path.parent() {
                        Some(parent) => current = parent,
                        None => {
                            segments.reverse();
                            return Some(ConstantPath {
                                absolute: true,
                                segments,
                            });
                        }
                    }
                }
                _ => return None,
            }
        }
        None
    }

    pub(crate) fn joined(&self) -> String {
        self.segments.join("::")
    }
}

pub(crate) fn qualify(namespace: &str, name: &str) -> String {
    if namespace.is_empty() {
        name.to_string()
    } else {
        format!("{namespace}::{name}")
    }
}
