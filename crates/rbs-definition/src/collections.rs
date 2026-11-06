use rustc_hash::FxBuildHasher;

// Why not the standard hasher: SipHash took a noticeable part of the check, and the
// keys are type names and method names of the signatures, not input from attackers.
pub type HashMap<Key, Value> = std::collections::HashMap<Key, Value, FxBuildHasher>;
pub type HashSet<Item> = std::collections::HashSet<Item, FxBuildHasher>;
pub type IndexMap<Key, Value> = indexmap::IndexMap<Key, Value, FxBuildHasher>;
pub type IndexSet<Item> = indexmap::IndexSet<Item, FxBuildHasher>;
