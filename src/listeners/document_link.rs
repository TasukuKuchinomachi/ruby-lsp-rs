use std::collections::HashMap;
use std::path::Path;

use ruby_environment::{DefaultGemStub, GemStub, RubyEnvironment};
use ruby_prism::Node;
use serde::Serialize;

use crate::document::ParsedDocument;
use crate::position::Range;
use crate::prism::{NodeExt, child_nodes, constant_id_name};
use crate::requests::support::package_url::PackageUrl;
use crate::requests::support::source_uri::{SourceUri, cgi_unescape};

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DocumentLinkItem {
    pub range: Range,
    pub target: String,
    pub tooltip: String,
}

enum GemFiles {
    Directory { version: String, full_gem_path: String },
    Default { version: String, files: HashMap<String, String> },
}

/// Mirrors `GEM_TO_VERSION_MAP` and `DocumentLink.gem_paths`. Instead of globbing every installed gem up front, files
/// of regular gems are checked on disk when a link points to them
pub struct GemPaths {
    versions: HashMap<String, String>,
    lookup: HashMap<String, GemFiles>,
}

impl GemPaths {
    pub fn new(gem_stubs: &[GemStub], default_gem_stubs: &[DefaultGemStub], rubylibdir: &str) -> GemPaths {
        let mut versions = HashMap::new();
        for stub in default_gem_stubs {
            versions.insert(stub.name.clone(), stub.version.clone());
        }
        for stub in gem_stubs {
            versions.insert(stub.name.clone(), stub.version.clone());
        }

        let mut lookup = HashMap::new();
        for stub in gem_stubs {
            lookup.insert(
                stub.name.clone(),
                GemFiles::Directory { version: stub.version.clone(), full_gem_path: stub.full_gem_path.clone() },
            );
        }
        for stub in default_gem_stubs {
            let files = stub
                .files
                .iter()
                .map(|file| {
                    let path = stub
                        .require_paths
                        .iter()
                        .find_map(|require_path| file.strip_prefix(&format!("{require_path}/")))
                        .unwrap_or(file);
                    (path.to_string(), format!("{rubylibdir}/{path}"))
                })
                .collect();
            lookup.insert(stub.name.clone(), GemFiles::Default { version: stub.version.clone(), files });
        }

        GemPaths { versions, lookup }
    }

    pub fn from_environment(environment: &RubyEnvironment) -> GemPaths {
        GemPaths::new(&environment.gem_stubs, &environment.default_gem_stubs, environment.rubylibdir.as_deref().unwrap_or_default())
    }

    fn version_of(&self, gem_name: &str) -> Option<&str> {
        self.versions.get(gem_name).map(String::as_str)
    }

    fn file_path(&self, gem_name: &str, gem_version: &str, path: &str) -> Option<String> {
        match self.lookup.get(gem_name)? {
            GemFiles::Directory { version, full_gem_path } if version == gem_version => {
                let base = full_gem_path.strip_prefix("//?/").unwrap_or(full_gem_path);
                matches_ruby_files_glob(base, path).then(|| format!("{full_gem_path}/{path}"))
            }
            GemFiles::Default { version, files } if version == gem_version => files.get(path).cloned(),
            _ => None,
        }
    }
}

/// Whether `Dir.glob("**/*.rb", base: base)` would list `path`. The glob skips hidden files and directories
fn matches_ruby_files_glob(base: &str, path: &str) -> bool {
    let components: Vec<&str> = path.split('/').collect();
    let well_formed = components.iter().all(|component| !component.is_empty() && !component.starts_with('.'));
    well_formed && path.ends_with(".rb") && Path::new(base).join(path).exists()
}

pub struct DocumentLink<'a, 'b> {
    parsed: &'a ParsedDocument<'b>,
    gem_paths: &'a GemPaths,
    gem_version: Option<String>,
    lines_to_comments: HashMap<usize, (usize, usize)>,
    sig_comments: HashMap<usize, (usize, usize)>,
    response: Vec<DocumentLinkItem>,
}

impl<'a, 'b> DocumentLink<'a, 'b> {
    pub fn new(parsed: &'a ParsedDocument<'b>, uri: &str, gem_paths: &'a GemPaths) -> DocumentLink<'a, 'b> {
        let gem_version = crate::uri::to_path(uri).and_then(|path| version_from_rbi_path(&path));
        let mut document_link = DocumentLink {
            parsed,
            gem_paths,
            gem_version,
            lines_to_comments: HashMap::new(),
            sig_comments: HashMap::new(),
            response: Vec::new(),
        };
        for comment in parsed.parse_result.comments() {
            let location = comment.location();
            let end_line = document_link.line(location.end_offset());
            document_link.lines_to_comments.insert(end_line, (location.start_offset(), location.end_offset()));
        }
        document_link
    }

    pub fn perform(mut self) -> Vec<DocumentLinkItem> {
        let mut stack = vec![self.parsed.ast()];
        while let Some(node) = stack.pop() {
            self.on_node_enter(&node);
            let mut children = child_nodes(&node);
            children.reverse();
            stack.extend(children);
        }
        self.response
    }

    fn on_node_enter(&mut self, node: &Node<'_>) {
        match node {
            Node::CallNode { .. } => self.on_call_node_enter(node),
            Node::DefNode { .. }
            | Node::ClassNode { .. }
            | Node::ModuleNode { .. }
            | Node::ConstantWriteNode { .. }
            | Node::ConstantPathWriteNode { .. } => self.extract_document_link(node),
            _ => {}
        }
    }

    fn on_call_node_enter(&mut self, node: &Node<'_>) {
        let call = node.as_call_node().expect("variant was matched");
        if constant_id_name(&call.name()) != "sig" {
            return;
        }
        let start_line = self.line(node.start_offset());
        let Some(comment) = self.lines_to_comments.get(&(start_line - 1)).copied() else { return };
        let end_line = self.line(node.end_offset());
        self.sig_comments.insert(end_line, comment);
    }

    fn extract_document_link(&mut self, node: &Node<'_>) {
        let start_line = self.line(node.start_offset());
        let comment = self.lines_to_comments.get(&(start_line - 1)).or_else(|| self.sig_comments.get(&(start_line - 1)));
        let Some((start_offset, end_offset)) = comment.copied() else { return };

        let source = self.parsed.document.ruby_source();
        let Some(uri_string) = find_link_uri(&source[start_offset..end_offset]) else { return };

        let resolved = if uri_string.starts_with("pkg:gem/") {
            self.parse_package_url(uri_string)
        } else {
            self.parse_source_uri(uri_string)
        };
        let Some((file_path, line_number)) = resolved else { return };

        self.response.push(DocumentLinkItem {
            range: self.parsed.byte_range(start_offset, end_offset),
            target: format!("file://{file_path}#{line_number}"),
            tooltip: format!("Jump to {file_path}#{line_number}"),
        });
    }

    fn parse_package_url(&self, uri_string: &str) -> Option<(String, String)> {
        let package_url = PackageUrl::parse(uri_string).ok()?;
        let gem_version = self.resolve_version(package_url.version.as_deref(), &package_url.name)?;
        let subpath = package_url.subpath.as_deref().filter(|subpath| !subpath.is_empty())?;
        let (path, line_number) = match subpath.split_once(':') {
            Some((path, line_number)) => (path, line_number),
            None => (subpath, ""),
        };
        let file_path = self.gem_paths.file_path(&package_url.name, &gem_version, &cgi_unescape(path))?;
        Some((file_path, line_number.to_string()))
    }

    fn parse_source_uri(&self, uri_string: &str) -> Option<(String, String)> {
        let uri = SourceUri::parse(uri_string).ok()?;
        let gem_version = self.resolve_version(uri.gem_version.as_deref(), &uri.gem_name)?;
        let path = uri.path.as_deref()?;
        let file_path = self.gem_paths.file_path(&uri.gem_name, &gem_version, &cgi_unescape(path))?;
        Some((file_path, uri.line_number.unwrap_or_else(|| "0".to_string())))
    }

    fn resolve_version(&self, version: Option<&str>, gem_name: &str) -> Option<String> {
        if let Some(version) = version.filter(|version| !version.is_empty()) {
            return Some(version.to_string());
        }
        if let Some(gem_version) = self.gem_version.as_ref().filter(|version| !version.is_empty()) {
            return Some(gem_version.clone());
        }
        self.gem_paths.version_of(gem_name).map(str::to_string)
    }

    fn line(&self, offset: usize) -> usize {
        self.parsed.line_index.line_of_offset(offset) + 1
    }
}

/// Mirrors the match of `(?<=%40)[\d.]+(?=\.rbi$)` on the document path
fn version_from_rbi_path(path: &str) -> Option<String> {
    path.match_indices("%40").find_map(|(index, _)| {
        let version = path[index + 3..].strip_suffix(".rbi")?;
        let valid = !version.is_empty() && version.bytes().all(|byte| byte.is_ascii_digit() || byte == b'.');
        valid.then(|| version.to_string())
    })
}

/// Mirrors the match of `(source://.*#\d+|pkg:gem/.*#.*)$` on the comment
fn find_link_uri(comment: &str) -> Option<&str> {
    comment.split('\n').find_map(|line| {
        line.char_indices().map(|(index, _)| &line[index..]).find(|candidate| {
            if let Some(rest) = candidate.strip_prefix("source://") {
                let digits = rest.trim_end_matches(|character: char| character.is_ascii_digit());
                digits.len() < rest.len() && digits.ends_with('#')
            } else if let Some(rest) = candidate.strip_prefix("pkg:gem/") {
                rest.contains('#')
            } else {
                false
            }
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_leftmost_link_in_comments() {
        assert_eq!(find_link_uri("# source://foo/1.0/a.rb#12"), Some("source://foo/1.0/a.rb#12"));
        assert_eq!(find_link_uri("# source://foo/1.0/a.rb#"), None);
        assert_eq!(find_link_uri("# source://#1"), Some("source://#1"));
        assert_eq!(find_link_uri("# pkg:gem/foo#a.rb:1 trailing"), Some("pkg:gem/foo#a.rb:1 trailing"));
        assert_eq!(find_link_uri("# pkg:gem/foo"), None);
        assert_eq!(find_link_uri("=begin\nsource://a/1/b.rb#2\n=end"), Some("source://a/1/b.rb#2"));
    }

    #[test]
    fn extracts_versions_from_rbi_file_names() {
        assert_eq!(version_from_rbi_path("/sorbet/rbi/gems/foo%401.2.3.rbi").as_deref(), Some("1.2.3"));
        assert_eq!(version_from_rbi_path("/sorbet/rbi/gems/foo@1.2.3.rbi"), None);
        assert_eq!(version_from_rbi_path("/sorbet/rbi/gems/foo%401.2.3.rb"), None);
    }
}
