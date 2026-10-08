use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use rayon::prelude::*;
use ruby_prism::Visit;

use super::configuration::IndexablePath;
use super::declaration_listener::DeclarationListener;
use super::index::Index;
use super::rbs_indexer::RbsIndexer;

const INDEXING_THREAD_STACK_SIZE: usize = 64 * 1024 * 1024;

/// Indexes a single document, mirroring `RubyIndexer::Index#index_single`
pub fn index_single(index: &mut Index, uri: &str, source: &str, collect_comments: bool) {
    index_single_with_require_path(index, Arc::from(uri), source.as_bytes(), collect_comments, None);
}

/// Same as `index_single`, also registering the document's require path for require completion when it is known
pub fn index_single_with_require_path(
    index: &mut Index,
    uri: Arc<str>,
    source: &[u8],
    collect_comments: bool,
    require_path: Option<&str>,
) {
    let parse_result = ruby_prism::parse(source);
    let mut listener = DeclarationListener::new(index, &parse_result, source, uri.clone(), collect_comments);
    listener.visit(&parse_result.node());

    if let Some(require_path) = require_path {
        index.insert_require_path(require_path, uri);
    }
}

/// Mirrors `RubyIndexer::Index#index_file`, ignoring paths that cannot be read
pub fn index_file(index: &mut Index, indexable_path: &IndexablePath, collect_comments: bool) {
    let Ok(source) = std::fs::read(&indexable_path.path) else { return };
    let uri: Arc<str> = Arc::from(crate::uri::from_path_buf(&indexable_path.path));
    index_single_with_require_path(index, uri, &source, collect_comments, indexable_path.require_path.as_deref());
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("The index is not empty. To prevent invalid entries, `index_all` can only be called once.")]
pub struct IndexNotEmptyError;

/// Indexes every path (in parallel) plus Ruby core RBS signatures, mirroring `RubyIndexer::Index#index_all`.
/// `progress` receives a percentage between 0 and 100. Calling it again after a completed indexing does nothing.
pub fn index_all(index: &mut Index, paths: &[IndexablePath], rbs_core_path: Option<&str>, progress: &(dyn Fn(u32) + Sync)) {
    if let Err(error) = try_index_all(index, paths, rbs_core_path, progress) {
        log::error!("{error}");
    }
}

/// Same as `index_all`, but fails like Ruby LSP when the initial indexing was already completed
pub fn try_index_all(
    index: &mut Index,
    paths: &[IndexablePath],
    rbs_core_path: Option<&str>,
    progress: &(dyn Fn(u32) + Sync),
) -> Result<(), IndexNotEmptyError> {
    if index.initial_indexing_completed {
        return Err(IndexNotEmptyError);
    }

    if let Some(rbs_core_path) = rbs_core_path {
        RbsIndexer::new(index).index_core_directory(std::path::Path::new(rbs_core_path));
    }

    let configuration = index.configuration.clone();
    let total = paths.len().max(1);
    let processed = AtomicUsize::new(0);
    let last_reported = AtomicUsize::new(0);
    progress(0);

    let worker_count = rayon::current_num_threads().max(1);
    let chunk_size = total.div_ceil(worker_count * 4).max(1);

    let index_chunk = |chunk: &[IndexablePath]| {
        let mut chunk_index = Index::new(configuration.clone());
        for indexable_path in chunk {
            index_file(&mut chunk_index, indexable_path, false);
            let done = processed.fetch_add(1, Ordering::Relaxed) + 1;
            let percentage = done * 100 / total;
            if last_reported.fetch_max(percentage, Ordering::Relaxed) < percentage {
                progress(percentage as u32);
            }
        }
        chunk_index
    };

    let chunk_indexes: Vec<Index> = match rayon::ThreadPoolBuilder::new().stack_size(INDEXING_THREAD_STACK_SIZE).build() {
        Ok(pool) => pool.install(|| paths.par_chunks(chunk_size).map(index_chunk).collect()),
        Err(_) => paths.par_chunks(chunk_size).map(index_chunk).collect(),
    };

    for chunk_index in chunk_indexes {
        index.absorb(chunk_index);
    }

    progress(100);
    index.initial_indexing_completed = true;
    Ok(())
}
