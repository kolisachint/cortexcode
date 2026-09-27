//! `core/tools/file-mutation-queue.ts`: edits and writes to the same file
//! run one at a time, in arrival order; different files run in parallel.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, LazyLock, Mutex};

/// A FIFO ticket lock for one file.
#[derive(Default)]
struct Queue {
    state: Mutex<(u64, u64)>,
    turn: Condvar,
}

static QUEUES: LazyLock<Mutex<HashMap<PathBuf, Arc<Queue>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

static REALPATHS: LazyLock<Mutex<HashMap<PathBuf, PathBuf>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// The queue key: the real path (cached per absolute path), else the path.
fn queue_key(file_path: &Path) -> PathBuf {
    let resolved = cortexcode_code_tool_api::resolve_to_cwd(
        &file_path.to_string_lossy(),
        &std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")),
    );
    let mut cache = REALPATHS.lock().unwrap_or_else(|e| e.into_inner());
    cache
        .entry(resolved.clone())
        .or_insert_with(|| std::fs::canonicalize(&resolved).unwrap_or(resolved))
        .clone()
}

/// `withFileMutationQueue`: run `f` once every earlier mutation of the same
/// file (symlinks resolved) has finished.
pub fn with_file_mutation_queue<T>(file_path: &Path, f: impl FnOnce() -> T) -> T {
    let key = queue_key(file_path);
    // Take the ticket while holding the map lock, so a release cannot drop
    // this queue between our lookup and our ticket.
    let mut queues = QUEUES.lock().unwrap_or_else(|e| e.into_inner());
    let queue = queues.entry(key.clone()).or_default().clone();
    let mut state = queue.state.lock().unwrap_or_else(|e| e.into_inner());
    drop(queues);
    let ticket = state.0;
    state.0 += 1;
    while state.1 != ticket {
        state = queue.turn.wait(state).unwrap_or_else(|e| e.into_inner());
    }
    drop(state);

    struct Release<'a> {
        queue: &'a Arc<Queue>,
        key: &'a PathBuf,
        ticket: u64,
    }
    impl Drop for Release<'_> {
        fn drop(&mut self) {
            let mut queues = QUEUES.lock().unwrap_or_else(|e| e.into_inner());
            let mut state = self.queue.state.lock().unwrap_or_else(|e| e.into_inner());
            state.1 = self.ticket + 1;
            if state.0 == state.1 {
                queues.remove(self.key);
            }
            self.queue.turn.notify_all();
        }
    }
    let _release = Release {
        queue: &queue,
        key: &key,
        ticket,
    };
    f()
}
