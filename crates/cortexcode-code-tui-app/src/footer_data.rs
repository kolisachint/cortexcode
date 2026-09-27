//! `FooterDataProvider` (`core/footer-data-provider.ts`): the git branch
//! (watched), extension statuses, provider count, active mode and the
//! subagent flag — data the footer shows that the session does not hold.
//!
//! The branch watcher polls HEAD (and a reftable directory) rather than using
//! OS notifications; changes are debounced 500ms like hoocode's.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

use cortexcode_code_paths::git_branch::{find_git_paths, read_branch_from_head, GitPaths};

type Callback = Arc<dyn Fn() + Send + Sync>;

const WATCH_DEBOUNCE: Duration = Duration::from_millis(500);
const WATCH_POLL: Duration = Duration::from_millis(100);

struct State {
    cwd: PathBuf,
    extension_statuses: BTreeMap<String, String>,
    /// `None` = not resolved yet; `Some(None)` = not in a repo.
    cached_branch: Option<Option<String>>,
    git_paths: Option<GitPaths>,
    branch_callbacks: Vec<(u64, Callback)>,
    next_callback: u64,
    available_provider_count: usize,
    active_mode: String,
    subagent_enabled: bool,
}

struct Inner {
    state: Mutex<State>,
    /// Bumped on `set_cwd`/`dispose`; a watcher of an older generation exits.
    generation: AtomicU64,
}

/// Shared handle; clones see the same data.
#[derive(Clone)]
pub struct FooterDataProvider {
    inner: Arc<Inner>,
}

impl FooterDataProvider {
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        let cwd = cwd.into();
        let provider = Self {
            inner: Arc::new(Inner {
                state: Mutex::new(State {
                    git_paths: find_git_paths(&cwd),
                    cwd,
                    extension_statuses: BTreeMap::new(),
                    cached_branch: None,
                    branch_callbacks: Vec::new(),
                    next_callback: 0,
                    available_provider_count: 0,
                    active_mode: "build".into(),
                    subagent_enabled: false,
                }),
                generation: AtomicU64::new(0),
            }),
        };
        provider.setup_git_watcher();
        provider
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.inner.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Current git branch; `None` outside a repo, `"detached"` on a detached HEAD.
    pub fn get_git_branch(&self) -> Option<String> {
        let mut s = self.state();
        if s.cached_branch.is_none() {
            let branch = s.git_paths.as_ref().and_then(read_branch_from_head);
            s.cached_branch = Some(branch);
        }
        s.cached_branch.clone().flatten()
    }

    /// Extension statuses, sorted by key.
    pub fn get_extension_statuses(&self) -> BTreeMap<String, String> {
        self.state().extension_statuses.clone()
    }

    pub fn set_extension_status(&self, key: &str, text: Option<&str>) {
        let mut s = self.state();
        match text {
            Some(text) => {
                s.extension_statuses
                    .insert(key.to_string(), text.to_string());
            }
            None => {
                s.extension_statuses.remove(key);
            }
        }
    }

    pub fn clear_extension_statuses(&self) {
        self.state().extension_statuses.clear();
    }

    /// Run `callback` when the branch changes; returns an id for
    /// [`FooterDataProvider::off_branch_change`].
    pub fn on_branch_change(&self, callback: impl Fn() + Send + Sync + 'static) -> u64 {
        let mut s = self.state();
        s.next_callback += 1;
        let id = s.next_callback;
        s.branch_callbacks.push((id, Arc::new(callback)));
        id
    }

    pub fn off_branch_change(&self, id: u64) {
        self.state().branch_callbacks.retain(|(i, _)| *i != id);
    }

    pub fn get_available_provider_count(&self) -> usize {
        self.state().available_provider_count
    }

    pub fn set_available_provider_count(&self, count: usize) {
        self.state().available_provider_count = count;
    }

    pub fn get_active_mode(&self) -> String {
        self.state().active_mode.clone()
    }

    pub fn set_active_mode(&self, mode: &str) {
        self.state().active_mode = mode.to_string();
    }

    pub fn get_subagent_enabled(&self) -> bool {
        self.state().subagent_enabled
    }

    pub fn set_subagent_enabled(&self, enabled: bool) {
        self.state().subagent_enabled = enabled;
    }

    pub fn cwd(&self) -> PathBuf {
        self.state().cwd.clone()
    }

    /// Point at another working directory: re-find the repo, forget the
    /// branch, restart the watcher, and notify listeners.
    pub fn set_cwd(&self, cwd: impl Into<PathBuf>) {
        let cwd = cwd.into();
        {
            let mut s = self.state();
            if s.cwd == cwd {
                return;
            }
            s.git_paths = find_git_paths(&cwd);
            s.cwd = cwd;
            s.cached_branch = None;
        }
        self.setup_git_watcher();
        self.notify_branch_change();
    }

    /// Stop watching and drop the listeners.
    pub fn dispose(&self) {
        self.inner.generation.fetch_add(1, Ordering::SeqCst);
        self.state().branch_callbacks.clear();
    }

    fn notify_branch_change(&self) {
        let callbacks: Vec<Callback> = self
            .state()
            .branch_callbacks
            .iter()
            .map(|(_, c)| c.clone())
            .collect();
        for callback in callbacks {
            callback();
        }
    }

    /// Re-read the branch; notify when a previously known branch changed.
    fn refresh(&self) {
        let (paths, known) = {
            let s = self.state();
            (s.git_paths.clone(), s.cached_branch.is_some())
        };
        let next = paths.as_ref().and_then(read_branch_from_head);
        let changed = {
            let mut s = self.state();
            let changed = known && s.cached_branch.as_ref() != Some(&next);
            s.cached_branch = Some(next);
            changed
        };
        if changed {
            self.notify_branch_change();
        }
    }

    fn setup_git_watcher(&self) {
        let generation = self.inner.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let Some(paths) = self.state().git_paths.clone() else {
            return;
        };
        let weak = Arc::downgrade(&self.inner);
        std::thread::spawn(move || {
            let reftable = paths.common_git_dir.join("reftable");
            let watched = [
                paths.head_path.clone(),
                reftable.clone(),
                reftable.join("tables.list"),
            ];
            let signature = || {
                watched
                    .iter()
                    .map(|p| file_signature(p))
                    .collect::<Vec<_>>()
            };
            let mut last = signature();
            let mut pending: Option<Instant> = None;
            loop {
                std::thread::sleep(WATCH_POLL);
                let Some(inner) = weak.upgrade() else { return };
                if inner.generation.load(Ordering::SeqCst) != generation {
                    return;
                }
                let now = signature();
                if now != last {
                    last = now;
                    pending.get_or_insert_with(Instant::now);
                }
                if pending.is_some_and(|at| at.elapsed() >= WATCH_DEBOUNCE) {
                    pending = None;
                    FooterDataProvider { inner }.refresh();
                }
            }
        });
    }
}

fn file_signature(path: &Path) -> Option<(SystemTime, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}
