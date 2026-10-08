use std::collections::HashMap;

use serde_json::Value;

use crate::document::{ContentChange, Document, LanguageId};
use crate::position::{Encoding, InvalidLocationError};

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct NonExistingDocumentError(pub String);

#[derive(Debug, thiserror::Error)]
pub enum PushEditsError {
    #[error(transparent)]
    NonExistingDocument(#[from] NonExistingDocumentError),
    #[error(transparent)]
    InvalidLocation(#[from] InvalidLocationError),
}

/// The documents managed by the editor. Keys are normalized URIs so that the index, which builds URIs from paths, and
/// the store always agree on the identity of a document
pub struct Store {
    state: HashMap<String, Document>,
    pub client_name: String,
    pub encoding: Encoding,
}

impl Default for Store {
    fn default() -> Self {
        Store::new(Encoding::Utf8)
    }
}

impl Store {
    pub fn new(encoding: Encoding) -> Store {
        Store { state: HashMap::new(), client_name: "Unknown".to_string(), encoding }
    }

    pub fn get(&mut self, uri: &str) -> Result<&mut Document, NonExistingDocumentError> {
        let key = crate::uri::normalize(uri);
        if !self.state.contains_key(&key) {
            let path = crate::uri::to_path(&key).ok_or_else(|| NonExistingDocumentError(uri.to_string()))?;
            let bytes = std::fs::read(&path).map_err(|_| NonExistingDocumentError(uri.to_string()))?;
            let source = String::from_utf8(bytes).unwrap_or_else(|error| String::from_utf8_lossy(error.as_bytes()).into_owned());
            self.set(&key, source, 0, LanguageId::from_path(&path));
        }
        Ok(self.state.get_mut(&key).expect("document was just inserted"))
    }

    pub fn get_open(&self, uri: &str) -> Option<&Document> {
        self.state.get(&crate::uri::normalize(uri))
    }

    pub fn get_open_mut(&mut self, uri: &str) -> Option<&mut Document> {
        self.state.get_mut(&crate::uri::normalize(uri))
    }

    pub fn set(&mut self, uri: &str, source: String, version: i32, language_id: LanguageId) -> &mut Document {
        let key = crate::uri::normalize(uri);
        let document = Document::new(&key, source, version, language_id, self.encoding);
        self.state.insert(key.clone(), document);
        self.state.get_mut(&key).expect("document was just inserted")
    }

    pub fn push_edits(&mut self, uri: &str, edits: &[ContentChange], version: i32) -> Result<(), PushEditsError> {
        let document = self
            .state
            .get_mut(&crate::uri::normalize(uri))
            .ok_or_else(|| NonExistingDocumentError(uri.to_string()))?;
        document.push_edits(edits, version)?;
        Ok(())
    }

    pub fn clear(&mut self) {
        self.state.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.state.is_empty()
    }

    pub fn len(&self) -> usize {
        self.state.len()
    }

    pub fn delete(&mut self, uri: &str) {
        self.state.remove(&crate::uri::normalize(uri));
    }

    pub fn contains(&self, uri: &str) -> bool {
        self.state.contains_key(&crate::uri::normalize(uri))
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &Document)> {
        self.state.iter()
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&String, &mut Document)> {
        self.state.iter_mut()
    }

    pub fn cache_fetch(
        &mut self,
        uri: &str,
        request_name: &str,
        compute: impl FnOnce(&Document) -> Value,
    ) -> Result<Value, NonExistingDocumentError> {
        Ok(self.get(uri)?.cache_fetch(request_name, compute))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_reads_unopened_files_from_disk() {
        let directory = std::env::temp_dir().join(format!("ruby-lsp-rs-store-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("foo.rb");
        std::fs::write(&path, "class Foo\nend\n").unwrap();

        let mut store = Store::default();
        let uri = crate::uri::from_path_buf(&path);
        assert_eq!(store.get(&uri).unwrap().source(), "class Foo\nend\n");
        assert!(store.contains(&uri));
        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn get_fails_for_missing_and_untitled_documents() {
        let mut store = Store::default();
        assert!(store.get("file:///non/existing/file.rb").is_err());
        assert!(store.get("untitled:Untitled-1").is_err());
        store.set("untitled:Untitled-1", "foo".to_string(), 1, LanguageId::Ruby);
        assert_eq!(store.get("untitled:Untitled-1").unwrap().source(), "foo");
    }

    #[test]
    fn keys_are_normalized() {
        let mut store = Store::default();
        store.set("file:///foo/%40bar.rb", "a".to_string(), 1, LanguageId::Ruby);
        assert!(store.contains("file:///foo/@bar.rb"));
        store.delete("file:///foo/@bar.rb");
        assert!(store.is_empty());
    }
}
