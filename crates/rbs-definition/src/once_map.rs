use std::{
    cell::RefCell,
    hash::{BuildHasher, Hash},
    sync::{Arc, OnceLock, RwLock},
};

use rustc_hash::FxBuildHasher;

use crate::collections::HashMap;

thread_local! {
    static INITIALIZING: RefCell<Vec<(usize, u64)>> = const { RefCell::new(Vec::new()) };
}

const SHARD_COUNT: usize = 64;

type Shard<Key, Value> = RwLock<HashMap<Key, Arc<OnceLock<Value>>>>;

// Why not one map behind one lock: every thread registers new keys at the start of a
// check, and they spent a noticeable time waiting for each other on the lock.
pub struct OnceMap<Key, Value> {
    shards: Box<[Shard<Key, Value>]>,
    hasher: FxBuildHasher,
}

impl<Key, Value> Default for OnceMap<Key, Value> {
    fn default() -> OnceMap<Key, Value> {
        OnceMap {
            shards: (0..SHARD_COUNT).map(|_| RwLock::default()).collect(),
            hasher: FxBuildHasher,
        }
    }
}

impl<Key: Eq + Hash + Clone, Value: Clone> OnceMap<Key, Value> {
    // Why not build the value and then insert it: threads that ask for the same key
    // at once would each build it, and a definition of a class with thousands of
    // inherited methods is costly to build.
    pub fn get_or_init(&self, key: &Key, init: impl FnOnce() -> Value) -> Value {
        let hash = self.hasher.hash_one(key);
        let initializing = (self as *const OnceMap<Key, Value> as usize, hash);
        // Why not wait for the value when this thread is building the same key: the
        // thread would wait for itself. Building it without the map leaves a cycle to
        // the recursion checks of the builders.
        let reentered = INITIALIZING.with_borrow(|keys| keys.contains(&initializing));
        if reentered {
            return init();
        }
        let cell = self.cell(key, hash);
        if let Some(value) = cell.get() {
            return value.clone();
        }
        INITIALIZING.with_borrow_mut(|keys| keys.push(initializing));
        let guard = PopOnDrop(initializing);
        let value = cell.get_or_init(init).clone();
        drop(guard);
        value
    }

    fn cell(&self, key: &Key, hash: u64) -> Arc<OnceLock<Value>> {
        let shard = &self.shards[hash as usize % SHARD_COUNT];
        if let Some(cell) = shard
            .read()
            .expect("no thread panics while holding the entries")
            .get(key)
        {
            return cell.clone();
        }
        shard
            .write()
            .expect("no thread panics while holding the entries")
            .entry(key.clone())
            .or_default()
            .clone()
    }
}

struct PopOnDrop((usize, u64));

impl Drop for PopOnDrop {
    fn drop(&mut self) {
        INITIALIZING.with_borrow_mut(|keys| {
            if let Some(index) = keys.iter().rposition(|key| *key == self.0) {
                keys.remove(index);
            }
        });
    }
}
