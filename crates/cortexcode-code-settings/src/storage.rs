//! `settings-storage.ts`: read-modify-write of one scope's raw settings JSON
//! under an exclusive lock.

use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use cortexcode_code_paths::{CONFIG_DIR_NAME, LEGACY_CONFIG_DIR_NAME};

/// `SettingsScope`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SettingsScope {
    Global,
    Project,
}

impl SettingsScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Project => "project",
        }
    }
}

/// Why a settings file could not be read or written.
#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Json(serde_json::Error),
    /// The file parsed but is not a JSON object.
    NotAnObject,
    /// Another process held the lock for every retry (`ELOCKED`).
    Locked(PathBuf),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => e.fmt(f),
            Self::Json(e) => e.fmt(f),
            Self::NotAnObject => f.write_str("settings must be a JSON object"),
            Self::Locked(path) => write!(f, "Lock file is already being held: {}", path.display()),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

/// `SettingsError`: a load or write failure, kept for `drain_errors`.
#[derive(Debug)]
pub struct SettingsError {
    pub scope: SettingsScope,
    pub error: Error,
}

/// The update callback: current file content in, new content (or `None` to
/// leave the file alone) out.
pub type LockFn<'a> = &'a mut dyn FnMut(Option<&str>) -> Result<Option<String>, Error>;

/// `SettingsStorage`.
pub trait SettingsStorage: Send + Sync {
    fn with_lock(&self, scope: SettingsScope, f: LockFn<'_>) -> Result<(), Error>;
}

/// `FileSettingsStorage`: `<agentDir>/settings.json` and
/// `<cwd>/.cortexcode/settings.json`.
///
/// When a cortex file does not exist yet, its hoocode twin (`.hoocode`
/// beside the `.cortexcode` directory) is read in its place; the first write
/// then creates the cortex file with that content plus the change. The
/// hoocode file is never written or locked.
///
/// Locking uses an `fs4` advisory lock on a `settings.json.lock` file beside
/// the settings file (hoocode's proper-lockfile uses a `.lock` directory), with
/// the same 10 x 20 ms retry. Writes go through a temp file and a rename.
pub struct FileSettingsStorage {
    global_path: PathBuf,
    project_path: PathBuf,
}

impl FileSettingsStorage {
    pub fn new(cwd: impl AsRef<Path>, agent_dir: impl AsRef<Path>) -> Self {
        Self {
            global_path: agent_dir.as_ref().join("settings.json"),
            project_path: cwd.as_ref().join(CONFIG_DIR_NAME).join("settings.json"),
        }
    }

    pub fn path(&self, scope: SettingsScope) -> &Path {
        match scope {
            SettingsScope::Global => &self.global_path,
            SettingsScope::Project => &self.project_path,
        }
    }

    /// The hoocode twin of a `.cortexcode/settings.json` path.
    fn legacy_path(path: &Path) -> Option<PathBuf> {
        let dir = path.parent()?;
        if dir.file_name()? != CONFIG_DIR_NAME {
            return None;
        }
        Some(
            dir.parent()?
                .join(LEGACY_CONFIG_DIR_NAME)
                .join(path.file_name()?),
        )
    }

    fn lock(path: &Path) -> Result<LockGuard, Error> {
        let lock_path = lock_path(path);
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_path)?;
        const MAX_ATTEMPTS: u32 = 10;
        for attempt in 1..=MAX_ATTEMPTS {
            if fs4::fs_std::FileExt::try_lock_exclusive(&file)? {
                return Ok(LockGuard(file));
            }
            if attempt < MAX_ATTEMPTS {
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        Err(Error::Locked(path.to_path_buf()))
    }
}

fn lock_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".lock");
    path.with_file_name(name)
}

struct LockGuard(File);

impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = fs4::fs_std::FileExt::unlock(&self.0);
    }
}

fn write_atomic(path: &Path, content: &str) -> Result<(), Error> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    tmp.write_all(content.as_bytes())?;
    tmp.as_file().sync_all()?;
    tmp.persist(path).map_err(|e| Error::Io(e.error))?;
    Ok(())
}

impl SettingsStorage for FileSettingsStorage {
    fn with_lock(&self, scope: SettingsScope, f: LockFn<'_>) -> Result<(), Error> {
        let path = self.path(scope);
        // Only lock (and so create the lock file) when the file exists or we
        // are about to write it.
        let mut guard = None;
        let current = if path.exists() {
            guard = Some(Self::lock(path)?);
            Some(std::fs::read_to_string(path)?)
        } else {
            match Self::legacy_path(path).filter(|p| p.exists()) {
                Some(legacy) => Some(std::fs::read_to_string(legacy)?),
                None => None,
            }
        };
        if let Some(next) = f(current.as_deref())? {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            if guard.is_none() {
                guard = Some(Self::lock(path)?);
            }
            write_atomic(path, &next)?;
        }
        drop(guard);
        Ok(())
    }
}

/// `InMemorySettingsStorage`.
#[derive(Default)]
pub struct InMemorySettingsStorage {
    files: Mutex<[Option<String>; 2]>,
}

impl InMemorySettingsStorage {
    pub fn new() -> Self {
        Self::default()
    }

    /// The stored content of a scope.
    pub fn get(&self, scope: SettingsScope) -> Option<String> {
        self.files.lock().unwrap_or_else(|e| e.into_inner())[scope as usize].clone()
    }
}

impl SettingsStorage for InMemorySettingsStorage {
    fn with_lock(&self, scope: SettingsScope, f: LockFn<'_>) -> Result<(), Error> {
        let mut files = self.files.lock().unwrap_or_else(|e| e.into_inner());
        let slot = &mut files[scope as usize];
        if let Some(next) = f(slot.as_deref())? {
            *slot = Some(next);
        }
        Ok(())
    }
}
