//! `AuthStorageBackend`: where `auth.json` lives and how it is locked.

use cortexcode_ai_oauth::BoxFuture;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

/// Given the current file contents, return the contents to write (`None` leaves
/// the file untouched). An `Err` aborts without writing.
pub type LockFn<'a> = &'a mut dyn FnMut(Option<&str>) -> Result<Option<String>, String>;

/// The async variant of [`LockFn`] (an OAuth refresh runs while the lock is held).
pub type AsyncLockFn<'a> =
    Box<dyn FnOnce(Option<String>) -> BoxFuture<'a, Result<Option<String>, String>> + Send + 'a>;

/// `AuthStorageBackend`: read-modify-write under a lock.
pub trait AuthStorageBackend: Send + Sync {
    /// `withLock`.
    fn with_lock(&self, f: LockFn<'_>) -> Result<(), String>;
    /// `withLockAsync`.
    fn with_lock_async<'a>(&'a self, f: AsyncLockFn<'a>) -> BoxFuture<'a, Result<(), String>>;
}

/// `InMemoryAuthStorageBackend`.
#[derive(Default)]
pub struct InMemoryAuthStorageBackend {
    value: Mutex<Option<String>>,
}

impl AuthStorageBackend for InMemoryAuthStorageBackend {
    fn with_lock(&self, f: LockFn<'_>) -> Result<(), String> {
        let mut value = self.value.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(next) = f(value.as_deref())? {
            *value = Some(next);
        }
        Ok(())
    }

    fn with_lock_async<'a>(&'a self, f: AsyncLockFn<'a>) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let current = self.value.lock().unwrap_or_else(|e| e.into_inner()).clone();
            if let Some(next) = f(current).await? {
                *self.value.lock().unwrap_or_else(|e| e.into_inner()) = Some(next);
            }
            Ok(())
        })
    }
}

/// `FileAuthStorageBackend`: `auth.json` guarded by a `proper-lockfile` compatible
/// lock (an `auth.json.lock` directory), so cortex and hoocode can share the file.
pub struct FileAuthStorageBackend {
    auth_path: PathBuf,
}

/// Sync lock: `lockSync` (stale after 10 s), retried 10 times 20 ms apart.
const SYNC_STALE: Duration = Duration::from_secs(10);
const SYNC_ATTEMPTS: u32 = 10;
const SYNC_DELAY: Duration = Duration::from_millis(20);
/// Async lock: `lock(..., {retries: 10, factor: 2, minTimeout: 100, maxTimeout: 10000,
/// randomize: true}, stale: 30000})`.
const ASYNC_STALE: Duration = Duration::from_secs(30);
const ASYNC_RETRIES: u32 = 10;

enum LockAttempt {
    Acquired(LockGuard),
    Locked,
}

/// Removes the lock directory on drop (`release()`).
struct LockGuard(PathBuf);

impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir(&self.0);
    }
}

fn is_stale(lock_dir: &Path, stale: Duration) -> bool {
    std::fs::metadata(lock_dir)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|mtime| SystemTime::now().duration_since(mtime).ok())
        .is_some_and(|age| age > stale)
}

/// One `proper-lockfile` acquisition: `mkdir <file>.lock`; an existing lock older
/// than `stale` is removed and taken over.
fn try_lock(lock_dir: &Path, stale: Duration) -> Result<LockAttempt, String> {
    match std::fs::create_dir(lock_dir) {
        Ok(()) => Ok(LockAttempt::Acquired(LockGuard(lock_dir.to_path_buf()))),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            if is_stale(lock_dir, stale) {
                let _ = std::fs::remove_dir(lock_dir);
                match std::fs::create_dir(lock_dir) {
                    Ok(()) => Ok(LockAttempt::Acquired(LockGuard(lock_dir.to_path_buf()))),
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                        Ok(LockAttempt::Locked)
                    }
                    Err(e) => Err(e.to_string()),
                }
            } else {
                Ok(LockAttempt::Locked)
            }
        }
        Err(e) => Err(e.to_string()),
    }
}

fn locked_error() -> String {
    "Lock file is already being held".to_string()
}

impl FileAuthStorageBackend {
    pub fn new(auth_path: impl Into<PathBuf>) -> Self {
        Self {
            auth_path: auth_path.into(),
        }
    }

    pub fn path(&self) -> &Path {
        &self.auth_path
    }

    fn lock_dir(&self) -> PathBuf {
        let mut name = self.auth_path.as_os_str().to_owned();
        name.push(".lock");
        PathBuf::from(name)
    }

    /// `ensureParentDir` (mode 0700) + `ensureFileExists` (`{}`, mode 0600).
    fn ensure_file(&self) -> Result<(), String> {
        if let Some(dir) = self.auth_path.parent() {
            if !dir.exists() {
                create_private_dir(dir).map_err(|e| e.to_string())?;
            }
        }
        if !self.auth_path.exists() {
            write_private(&self.auth_path, "{}")?;
        }
        Ok(())
    }

    fn read(&self) -> Result<Option<String>, String> {
        match std::fs::read_to_string(&self.auth_path) {
            Ok(s) => Ok(Some(s)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    fn acquire_sync(&self) -> Result<LockGuard, String> {
        let lock_dir = self.lock_dir();
        for attempt in 1..=SYNC_ATTEMPTS {
            match try_lock(&lock_dir, SYNC_STALE)? {
                LockAttempt::Acquired(guard) => return Ok(guard),
                LockAttempt::Locked if attempt < SYNC_ATTEMPTS => std::thread::sleep(SYNC_DELAY),
                LockAttempt::Locked => {}
            }
        }
        Err(locked_error())
    }

    async fn acquire_async(&self) -> Result<LockGuard, String> {
        let lock_dir = self.lock_dir();
        for retry in 0..=ASYNC_RETRIES {
            if let LockAttempt::Acquired(guard) = try_lock(&lock_dir, ASYNC_STALE)? {
                return Ok(guard);
            }
            if retry < ASYNC_RETRIES {
                // `retry` package timeouts: min(random(1..2) * 100ms * 2^n, 10s).
                let factor: f64 = 1.0 + rand::random::<f64>();
                let ms = (factor * 100.0 * 2f64.powi(retry as i32)).min(10_000.0);
                tokio::time::sleep(Duration::from_millis(ms as u64)).await;
            }
        }
        Err(locked_error())
    }
}

#[cfg(unix)]
fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
}

#[cfg(not(unix))]
fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)
}

/// `writeFileSync` + `chmodSync(0o600)`.
fn write_private(path: &Path, contents: &str) -> Result<(), String> {
    std::fs::write(path, contents).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

impl AuthStorageBackend for FileAuthStorageBackend {
    fn with_lock(&self, f: LockFn<'_>) -> Result<(), String> {
        self.ensure_file()?;
        let _guard = self.acquire_sync()?;
        let current = self.read()?;
        if let Some(next) = f(current.as_deref())? {
            write_private(&self.auth_path, &next)?;
        }
        Ok(())
    }

    fn with_lock_async<'a>(&'a self, f: AsyncLockFn<'a>) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            self.ensure_file()?;
            let _guard = self.acquire_async().await?;
            let current = self.read()?;
            if let Some(next) = f(current).await? {
                write_private(&self.auth_path, &next)?;
            }
            Ok(())
        })
    }
}
