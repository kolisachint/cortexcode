//! Process-wide store for the footer's transient progress lines
//! (`core/startup-progress.ts`): first-run tool downloads, the semantic index
//! build, `/learn`. Keyed so concurrent jobs each own a stable line; a caller
//! removes its key when the work settles. `clear` runs when the first user
//! turn begins.

use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

/// One progress line.
#[derive(Debug, Clone, PartialEq)]
pub enum StartupProgress {
    Download {
        key: String,
        label: String,
        received_bytes: u64,
        total_bytes: Option<u64>,
    },
    Work {
        key: String,
        label: String,
        done: u64,
        total: u64,
        unit: String,
    },
    Error {
        key: String,
        label: String,
        message: String,
    },
}

impl StartupProgress {
    pub fn key(&self) -> &str {
        match self {
            StartupProgress::Download { key, .. }
            | StartupProgress::Work { key, .. }
            | StartupProgress::Error { key, .. } => key,
        }
    }
}

type Listener = Arc<dyn Fn() + Send + Sync>;

#[derive(Default)]
struct Store {
    // Insertion order, so lines stay where their work started.
    entries: Vec<StartupProgress>,
    listeners: Vec<(u64, Listener)>,
    next_id: u64,
}

fn store() -> MutexGuard<'static, Store> {
    static STORE: OnceLock<Mutex<Store>> = OnceLock::new();
    STORE
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

fn emit() {
    let listeners: Vec<Listener> = store().listeners.iter().map(|(_, l)| l.clone()).collect();
    for listener in listeners {
        listener();
    }
}

/// Insert or replace the entry for its key (in place, keeping its position).
pub fn set(entry: StartupProgress) {
    {
        let mut s = store();
        match s.entries.iter_mut().find(|e| e.key() == entry.key()) {
            Some(slot) => *slot = entry,
            None => s.entries.push(entry),
        }
    }
    emit();
}

/// Drop a key's line once its work settled.
pub fn remove(key: &str) {
    let removed = {
        let mut s = store();
        let before = s.entries.len();
        s.entries.retain(|e| e.key() != key);
        s.entries.len() != before
    };
    if removed {
        emit();
    }
}

/// Wipe everything (the first user turn started).
pub fn clear() {
    let cleared = {
        let mut s = store();
        let had = !s.entries.is_empty();
        s.entries.clear();
        had
    };
    if cleared {
        emit();
    }
}

/// The current lines, in order.
pub fn list() -> Vec<StartupProgress> {
    store().entries.clone()
}

/// Handle from [`subscribe`]; dropping it does not unsubscribe, call
/// [`Subscription::unsubscribe`].
pub struct Subscription(u64);

impl Subscription {
    pub fn unsubscribe(self) {
        store().listeners.retain(|(id, _)| *id != self.0);
    }
}

/// Run `listener` after every change.
pub fn subscribe(listener: impl Fn() + Send + Sync + 'static) -> Subscription {
    let mut s = store();
    s.next_id += 1;
    let id = s.next_id;
    s.listeners.push((id, Arc::new(listener)));
    Subscription(id)
}
