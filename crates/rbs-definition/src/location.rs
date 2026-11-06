use std::{
    fmt,
    hash::{Hash, Hasher},
    sync::Arc,
};

#[derive(Debug)]
pub struct Buffer {
    pub name: String,
    pub content: String,
}

impl Buffer {
    pub fn new(name: impl Into<String>, content: impl Into<String>) -> Arc<Buffer> {
        Arc::new(Buffer {
            name: name.into(),
            content: content.into(),
        })
    }

    pub fn position(&self, byte_position: usize) -> (usize, usize) {
        let before = &self.content[..byte_position.min(self.content.len())];
        let line = before.matches('\n').count() + 1;
        let line_start = before.rfind('\n').map_or(0, |index| index + 1);
        (line, before[line_start..].chars().count())
    }
}

#[derive(Clone)]
pub struct Location {
    pub buffer: Arc<Buffer>,
    start: u32,
    end: u32,
}

impl Location {
    pub fn new(buffer: &Arc<Buffer>, start: usize, end: usize) -> Location {
        Location {
            buffer: buffer.clone(),
            start: start as u32,
            end: end as u32,
        }
    }

    pub fn start(&self) -> usize {
        self.start as usize
    }

    pub fn end(&self) -> usize {
        self.end as usize
    }

    pub fn name(&self) -> &str {
        &self.buffer.name
    }

    pub fn start_line(&self) -> usize {
        self.buffer.position(self.start()).0
    }

    pub fn start_column(&self) -> usize {
        self.buffer.position(self.start()).1
    }

    pub fn end_line(&self) -> usize {
        self.buffer.position(self.end()).0
    }

    pub fn end_column(&self) -> usize {
        self.buffer.position(self.end()).1
    }

    pub fn source(&self) -> &str {
        &self.buffer.content[self.start()..self.end()]
    }

    pub fn same_range(&self, other: &Location) -> bool {
        self.buffer.name == other.buffer.name && self.start == other.start && self.end == other.end
    }
}

impl fmt::Display for Location {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (start_line, start_column) = self.buffer.position(self.start());
        let (end_line, end_column) = self.buffer.position(self.end());
        write!(
            formatter,
            "{}:{start_line}:{start_column}...{end_line}:{end_column}",
            self.buffer.name
        )
    }
}

impl fmt::Debug for Location {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}@{}..{}",
            self.buffer.name, self.start, self.end
        )
    }
}

pub fn location_to_string(location: Option<&Location>) -> String {
    match location {
        Some(location) => location.to_string(),
        None => "*:*:*...*:*".to_string(),
    }
}

/// The location of an AST node. Like the rbs gem, two nodes at different locations are equal.
#[derive(Clone, Debug, Default)]
pub struct AstLocation(Option<Arc<Location>>);

impl AstLocation {
    pub fn new(location: Option<Location>) -> AstLocation {
        AstLocation(location.map(Arc::new))
    }

    pub fn location(&self) -> Option<Location> {
        self.0.as_deref().cloned()
    }
}

impl PartialEq for AstLocation {
    fn eq(&self, _: &AstLocation) -> bool {
        true
    }
}

impl Eq for AstLocation {}

impl Hash for AstLocation {
    fn hash<H: Hasher>(&self, _: &mut H) {}
}

#[derive(Debug)]
pub struct NamedTypeLocation {
    pub location: Option<Location>,
    pub name: Option<Location>,
}

/// The location of a named type and of its name. Like the rbs gem, two types at different locations are equal.
#[derive(Clone, Debug, Default)]
pub struct TypeLocation(Option<Arc<NamedTypeLocation>>);

impl TypeLocation {
    pub fn new(location: Option<Location>, name: Option<Location>) -> TypeLocation {
        TypeLocation(Some(Arc::new(NamedTypeLocation { location, name })))
    }

    pub fn location(&self) -> Option<&Location> {
        self.0.as_ref().and_then(|named| named.location.as_ref())
    }

    pub fn name(&self) -> Option<&Location> {
        self.0.as_ref().and_then(|named| named.name.as_ref())
    }
}

impl PartialEq for TypeLocation {
    fn eq(&self, _: &TypeLocation) -> bool {
        true
    }
}

impl Eq for TypeLocation {}

impl Hash for TypeLocation {
    fn hash<H: Hasher>(&self, _: &mut H) {}
}
