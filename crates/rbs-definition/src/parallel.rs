use std::{
    panic,
    sync::atomic::{AtomicUsize, Ordering},
    thread,
};

const WORKER_STACK_SIZE: usize = 256 * 1024 * 1024;

pub fn run_in_parallel<Item: Sync, Output: Send, State>(
    items: &[Item],
    jobs: usize,
    new_state: impl Fn() -> State + Sync,
    work: impl Fn(&State, &Item) -> Output + Sync,
    forget_states: bool,
) -> Vec<Output> {
    let finish = |state: State| {
        if forget_states {
            std::mem::forget(state);
        }
    };
    if jobs <= 1 || items.len() <= 1 {
        let state = new_state();
        let outputs = items.iter().map(|item| work(&state, item)).collect();
        finish(state);
        return outputs;
    }
    let next = AtomicUsize::new(0);
    let mut outputs: Vec<(usize, Output)> = thread::scope(|scope| {
        let workers: Vec<_> = (0..jobs.min(items.len()))
            .map(|_| {
                thread::Builder::new()
                    .stack_size(WORKER_STACK_SIZE)
                    .spawn_scoped(scope, || {
                        let state = new_state();
                        let mut outputs = Vec::new();
                        loop {
                            let index = next.fetch_add(1, Ordering::Relaxed);
                            let Some(item) = items.get(index) else {
                                break;
                            };
                            outputs.push((index, work(&state, item)));
                        }
                        finish(state);
                        outputs
                    })
                    .expect("a worker thread starts")
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|worker| {
                worker
                    .join()
                    .unwrap_or_else(|payload| panic::resume_unwind(payload))
            })
            .collect()
    });
    outputs.sort_by_key(|(index, _)| *index);
    outputs.into_iter().map(|(_, output)| output).collect()
}

pub fn map_in_parallel<Item: Sync, Output: Send>(
    items: &[Item],
    jobs: usize,
    work: impl Fn(&Item) -> Output + Sync,
) -> Vec<Output> {
    run_in_parallel(items, jobs, || (), |_, item| work(item), false)
}
