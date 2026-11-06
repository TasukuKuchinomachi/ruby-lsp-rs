use std::{
    sync::atomic::{AtomicUsize, Ordering},
    thread,
};

use rbs_definition::{once_map::OnceMap, parallel::map_in_parallel};

#[test]
fn map_in_parallel_returns_outputs_in_the_order_of_the_items() {
    let items: Vec<usize> = (0..1000).collect();
    let outputs = map_in_parallel(&items, 8, |item| item * 2);
    assert_eq!(
        outputs,
        items.iter().map(|item| item * 2).collect::<Vec<_>>()
    );
}

#[test]
fn once_map_builds_the_value_of_a_key_once_across_threads() {
    let map: OnceMap<String, usize> = OnceMap::default();
    let builds = AtomicUsize::new(0);
    thread::scope(|scope| {
        for _ in 0..8 {
            scope.spawn(|| {
                let value = map.get_or_init(&"key".to_string(), || {
                    builds.fetch_add(1, Ordering::SeqCst);
                    42
                });
                assert_eq!(value, 42);
            });
        }
    });
    assert_eq!(builds.load(Ordering::SeqCst), 1);
}

#[test]
fn once_map_builds_the_value_again_when_the_same_thread_asks_for_the_key_while_building_it() {
    let map: OnceMap<String, usize> = OnceMap::default();
    let value = map.get_or_init(&"key".to_string(), || {
        map.get_or_init(&"key".to_string(), || 1) + 1
    });
    assert_eq!(value, 2);
    assert_eq!(map.get_or_init(&"key".to_string(), || 3), 2);
}
