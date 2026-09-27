//! The harness execution environment: hoocode `harness/types.ts`
//! (`ExecutionEnv`, `FileInfo`, `FileError`) and `harness/env/nodejs.ts`
//! (`NodeExecutionEnv`, here [`LocalExecutionEnv`] on tokio), v0.5.89.
//!
//! Paths may be absolute or relative to the env's cwd; returned paths are
//! absolute and not canonicalized through symlinks (except
//! [`ExecutionEnv::real_path`]).

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use cortexcode_ai_types::AbortSignal;
use futures_util::future::BoxFuture;
use tokio::io::AsyncReadExt;

/// `FileKind`: symlinks are not followed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    File,
    Directory,
    Symlink,
}

impl FileKind {
    pub fn as_str(self) -> &'static str {
        match self {
            FileKind::File => "file",
            FileKind::Directory => "directory",
            FileKind::Symlink => "symlink",
        }
    }
}

/// `FileErrorCode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileErrorCode {
    NotFound,
    PermissionDenied,
    NotDirectory,
    IsDirectory,
    Invalid,
    NotSupported,
    Unknown,
}

impl FileErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            FileErrorCode::NotFound => "not_found",
            FileErrorCode::PermissionDenied => "permission_denied",
            FileErrorCode::NotDirectory => "not_directory",
            FileErrorCode::IsDirectory => "is_directory",
            FileErrorCode::Invalid => "invalid",
            FileErrorCode::NotSupported => "not_supported",
            FileErrorCode::Unknown => "unknown",
        }
    }
}

/// `FileError`: a file operation failure with a backend-independent code.
/// The message is the OS error text (Node's reads `ENOENT: ...`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileError {
    pub code: FileErrorCode,
    pub message: String,
    /// The absolute addressed path, when known.
    pub path: Option<String>,
}

impl FileError {
    pub fn new(code: FileErrorCode, message: impl Into<String>, path: Option<String>) -> Self {
        Self {
            code,
            message: message.into(),
            path,
        }
    }

    /// `toFileError`.
    fn from_io(error: &std::io::Error, path: &Path) -> Self {
        use std::io::ErrorKind;
        let code = match error.kind() {
            ErrorKind::NotFound => FileErrorCode::NotFound,
            ErrorKind::PermissionDenied => FileErrorCode::PermissionDenied,
            ErrorKind::NotADirectory => FileErrorCode::NotDirectory,
            ErrorKind::IsADirectory => FileErrorCode::IsDirectory,
            ErrorKind::InvalidInput => FileErrorCode::Invalid,
            _ => FileErrorCode::Unknown,
        };
        Self::new(code, error.to_string(), Some(path_string(path)))
    }
}

impl std::fmt::Display for FileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for FileError {}

/// `FileInfo`.
#[derive(Debug, Clone, PartialEq)]
pub struct FileInfo {
    /// Basename of `path`.
    pub name: String,
    /// Absolute addressed path (symlinks not followed).
    pub path: String,
    pub kind: FileKind,
    pub size: u64,
    /// Modification time in milliseconds since the epoch.
    pub mtime_ms: f64,
}

/// A stdout/stderr chunk callback.
pub type ChunkCallback = Arc<dyn Fn(&str) + Send + Sync>;

/// `ExecutionEnvExecOptions`.
#[derive(Clone, Default)]
pub struct ExecOptions {
    /// Working directory, relative to the env's cwd.
    pub cwd: Option<String>,
    /// Extra environment variables (override the defaults).
    pub env: Option<HashMap<String, String>>,
    /// Timeout in seconds.
    pub timeout: Option<f64>,
    pub signal: Option<AbortSignal>,
    pub on_stdout: Option<ChunkCallback>,
    pub on_stderr: Option<ChunkCallback>,
}

impl std::fmt::Debug for ExecOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExecOptions")
            .field("cwd", &self.cwd)
            .field("env", &self.env)
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

/// The result of [`ExecutionEnv::exec`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

/// `ExecutionEnv`: the filesystem and process access the harness uses.
/// File operations fail with [`FileError`]; `exec` fails with a message
/// (`aborted`, `timeout:<seconds>`, or a spawn failure).
pub trait ExecutionEnv: Send + Sync {
    /// Working directory for relative paths and commands.
    fn cwd(&self) -> &str;
    /// Run `command` through the shell.
    fn exec<'a>(
        &'a self,
        command: &'a str,
        options: ExecOptions,
    ) -> BoxFuture<'a, Result<ExecResult, String>>;
    fn read_text_file<'a>(&'a self, path: &'a str) -> BoxFuture<'a, Result<String, FileError>>;
    fn read_binary_file<'a>(&'a self, path: &'a str) -> BoxFuture<'a, Result<Vec<u8>, FileError>>;
    /// Create or overwrite a file, creating parent directories.
    fn write_file<'a>(
        &'a self,
        path: &'a str,
        content: &'a [u8],
    ) -> BoxFuture<'a, Result<(), FileError>>;
    /// Metadata of the addressed path (symlinks not followed).
    fn file_info<'a>(&'a self, path: &'a str) -> BoxFuture<'a, Result<FileInfo, FileError>>;
    /// Direct children, symlinks not followed.
    fn list_dir<'a>(&'a self, path: &'a str) -> BoxFuture<'a, Result<Vec<FileInfo>, FileError>>;
    /// The canonical path, following symlinks.
    fn real_path<'a>(&'a self, path: &'a str) -> BoxFuture<'a, Result<String, FileError>>;
    /// False for missing paths; other failures are errors.
    fn exists<'a>(&'a self, path: &'a str) -> BoxFuture<'a, Result<bool, FileError>>;
    fn create_dir<'a>(
        &'a self,
        path: &'a str,
        recursive: bool,
    ) -> BoxFuture<'a, Result<(), FileError>>;
    fn remove<'a>(
        &'a self,
        path: &'a str,
        recursive: bool,
        force: bool,
    ) -> BoxFuture<'a, Result<(), FileError>>;
    /// A new temporary directory (default prefix `tmp-`).
    fn create_temp_dir<'a>(
        &'a self,
        prefix: Option<&'a str>,
    ) -> BoxFuture<'a, Result<String, FileError>>;
    /// A new empty temporary file.
    fn create_temp_file<'a>(
        &'a self,
        prefix: Option<&'a str>,
        suffix: Option<&'a str>,
    ) -> BoxFuture<'a, Result<String, FileError>>;
    /// Release resources owned by the environment.
    fn cleanup(&self) -> BoxFuture<'_, ()>;
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// `path.resolve(cwd, path)` for relative paths: joined and normalized
/// (`.` and `..` resolved lexically, no trailing slash).
fn resolve_path(cwd: &str, path: &str) -> PathBuf {
    let joined = if Path::new(path).is_absolute() {
        PathBuf::from(path)
    } else {
        Path::new(cwd).join(path)
    };
    let mut normalized = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

fn kind_of(metadata: &std::fs::Metadata) -> Option<FileKind> {
    let file_type = metadata.file_type();
    if file_type.is_file() {
        Some(FileKind::File)
    } else if file_type.is_dir() {
        Some(FileKind::Directory)
    } else if file_type.is_symlink() {
        Some(FileKind::Symlink)
    } else {
        None
    }
}

/// `fileInfoFromStats`: an unsupported kind (socket, fifo, device) is an
/// `invalid` error.
fn file_info_from(path: &Path, metadata: &std::fs::Metadata) -> Result<FileInfo, FileError> {
    let kind = kind_of(metadata)
        .ok_or_else(|| FileError::new(FileErrorCode::Invalid, "Unsupported file type", None))?;
    let path_str = path_string(path);
    let name = path_str
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(&path_str)
        .to_string();
    let mtime_ms = metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64() * 1000.0)
        .unwrap_or(0.0);
    Ok(FileInfo {
        name,
        path: path_str,
        kind,
        size: metadata.len(),
        mtime_ms,
    })
}

/// Six random characters, as `mkdtemp` appends.
fn random_suffix(len: usize) -> String {
    uuid::Uuid::new_v4().simple().to_string()[..len].to_string()
}

/// `NodeExecutionEnv`: the local filesystem and shell.
#[derive(Debug, Clone)]
pub struct LocalExecutionEnv {
    cwd: String,
    shell_path: Option<String>,
    shell_env: Option<HashMap<String, String>>,
}

/// hoocode's name for [`LocalExecutionEnv`].
pub type NodeExecutionEnv = LocalExecutionEnv;

impl LocalExecutionEnv {
    pub fn new(cwd: impl Into<String>) -> Self {
        Self {
            cwd: cwd.into(),
            shell_path: None,
            shell_env: None,
        }
    }

    /// Run commands with this shell (`<shell> -c <command>`).
    pub fn with_shell_path(mut self, shell_path: impl Into<String>) -> Self {
        self.shell_path = Some(shell_path.into());
        self
    }

    /// Extra variables for every command (under the per-call `env`).
    pub fn with_shell_env(mut self, env: HashMap<String, String>) -> Self {
        self.shell_env = Some(env);
        self
    }

    fn resolve(&self, path: &str) -> PathBuf {
        resolve_path(&self.cwd, path)
    }

    async fn stat(&self, path: &str) -> Result<FileInfo, FileError> {
        let resolved = self.resolve(path);
        let metadata = tokio::fs::symlink_metadata(&resolved)
            .await
            .map_err(|e| FileError::from_io(&e, &resolved))?;
        file_info_from(&resolved, &metadata).map_err(|e| FileError {
            path: Some(path_string(&resolved)),
            ..e
        })
    }
}

/// `which bash` / `where bash.exe`: the first existing match.
async fn find_bash_on_path() -> Option<String> {
    let (program, arg) = if cfg!(windows) {
        ("where", "bash.exe")
    } else {
        ("which", "bash")
    };
    let output = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::process::Command::new(program)
            .arg(arg)
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let first = stdout.trim().lines().next()?.trim().to_string();
    (!first.is_empty() && Path::new(&first).exists()).then_some(first)
}

/// `getShellConfig`: the custom shell, else bash (Git Bash on Windows),
/// else `sh`.
async fn shell_config(custom: Option<&str>) -> Result<String, String> {
    if let Some(custom) = custom {
        if Path::new(custom).exists() {
            return Ok(custom.to_string());
        }
        return Err(format!("Custom shell path not found: {custom}"));
    }
    if cfg!(windows) {
        for var in ["ProgramFiles", "ProgramFiles(x86)"] {
            if let Ok(dir) = std::env::var(var) {
                let candidate = format!("{dir}\\Git\\bin\\bash.exe");
                if Path::new(&candidate).exists() {
                    return Ok(candidate);
                }
            }
        }
        return find_bash_on_path()
            .await
            .ok_or_else(|| "No bash shell found".to_string());
    }
    if Path::new("/bin/bash").exists() {
        return Ok("/bin/bash".to_string());
    }
    Ok(find_bash_on_path()
        .await
        .unwrap_or_else(|| "sh".to_string()))
}

/// `killProcessTree`: SIGKILL the process group (the child leads its own),
/// or `taskkill /T` on Windows.
fn kill_process_tree(pid: u32) {
    #[cfg(unix)]
    {
        // SAFETY: plain kill(2) calls with a pid we spawned.
        unsafe {
            if libc::kill(-(pid as i32), libc::SIGKILL) != 0 {
                libc::kill(pid as i32, libc::SIGKILL);
            }
        }
    }
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("taskkill")
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }
}

/// Read a stream as UTF-8 text chunks (`setEncoding("utf8")`: a character
/// split across reads is held back), calling `on_chunk` for each.
async fn read_stream<R: tokio::io::AsyncRead + Unpin>(
    mut reader: R,
    on_chunk: Option<ChunkCallback>,
) -> String {
    let mut all = String::new();
    let mut pending: Vec<u8> = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = match reader.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        pending.extend_from_slice(&buf[..n]);
        let valid = match std::str::from_utf8(&pending) {
            Ok(_) => pending.len(),
            Err(e) if e.error_len().is_none() => e.valid_up_to(),
            Err(_) => pending.len(),
        };
        if valid == 0 {
            continue;
        }
        let chunk = String::from_utf8_lossy(&pending[..valid]).into_owned();
        pending.drain(..valid);
        if let Some(callback) = &on_chunk {
            callback(&chunk);
        }
        all.push_str(&chunk);
    }
    if !pending.is_empty() {
        let chunk = String::from_utf8_lossy(&pending).into_owned();
        if let Some(callback) = &on_chunk {
            callback(&chunk);
        }
        all.push_str(&chunk);
    }
    all
}

/// A JS number as `${n}` prints it (`5`, `0.5`).
fn js_number(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e21 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

impl LocalExecutionEnv {
    async fn run(&self, command: &str, options: ExecOptions) -> Result<ExecResult, String> {
        let cwd = match &options.cwd {
            Some(dir) => self.resolve(dir),
            None => PathBuf::from(&self.cwd),
        };
        let shell = shell_config(self.shell_path.as_deref()).await?;
        let mut cmd = tokio::process::Command::new(&shell);
        cmd.arg("-c")
            .arg(command)
            .current_dir(&cwd)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(false);
        for (k, v) in self.shell_env.iter().flatten() {
            cmd.env(k, v);
        }
        for (k, v) in options.env.iter().flatten() {
            cmd.env(k, v);
        }
        #[cfg(unix)]
        cmd.process_group(0);
        let mut child = cmd.spawn().map_err(|e| e.to_string())?;
        let pid = child.id();
        let stdout = tokio::spawn(read_stream(
            child.stdout.take().expect("piped stdout"),
            options.on_stdout.clone(),
        ));
        let stderr = tokio::spawn(read_stream(
            child.stderr.take().expect("piped stderr"),
            options.on_stderr.clone(),
        ));

        let signal = options.signal.clone();
        let timeout = options.timeout;
        let mut timed_out = false;
        let kill = || {
            if let Some(pid) = pid {
                kill_process_tree(pid);
            }
        };
        if signal.as_ref().is_some_and(AbortSignal::aborted) {
            kill();
        }
        let wait = child.wait();
        tokio::pin!(wait);
        let status = loop {
            tokio::select! {
                status = &mut wait => break status,
                _ = async {
                    match &signal {
                        Some(s) if !s.aborted() => s.cancelled().await,
                        _ => std::future::pending().await,
                    }
                } => kill(),
                _ = async {
                    match timeout {
                        Some(t) if !timed_out => tokio::time::sleep(Duration::from_secs_f64(t.max(0.0))).await,
                        _ => std::future::pending().await,
                    }
                } => {
                    timed_out = true;
                    kill();
                }
            }
        };
        // `close`: the streams are drained too.
        let stdout = stdout.await.unwrap_or_default();
        let stderr = stderr.await.unwrap_or_default();
        let status = status.map_err(|e| e.to_string())?;
        if signal.as_ref().is_some_and(AbortSignal::aborted) {
            return Err("aborted".to_string());
        }
        if timed_out {
            return Err(format!(
                "timeout:{}",
                js_number(timeout.unwrap_or_default())
            ));
        }
        Ok(ExecResult {
            stdout,
            stderr,
            exit_code: status.code().unwrap_or(0),
        })
    }

    async fn list(&self, path: &str) -> Result<Vec<FileInfo>, FileError> {
        let resolved = self.resolve(path);
        let mut entries = tokio::fs::read_dir(&resolved)
            .await
            .map_err(|e| FileError::from_io(&e, &resolved))?;
        let mut infos = Vec::new();
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| FileError::from_io(&e, &resolved))?
        {
            let entry_path = resolved.join(entry.file_name());
            let metadata = tokio::fs::symlink_metadata(&entry_path)
                .await
                .map_err(|e| FileError::from_io(&e, &resolved))?;
            match file_info_from(&entry_path, &metadata) {
                Ok(info) => infos.push(info),
                // Sockets, fifos and devices are skipped.
                Err(e) if e.code == FileErrorCode::Invalid => continue,
                Err(e) => return Err(e),
            }
        }
        Ok(infos)
    }

    async fn remove_path(&self, path: &str, recursive: bool, force: bool) -> Result<(), FileError> {
        let resolved = self.resolve(path);
        let metadata = match tokio::fs::symlink_metadata(&resolved).await {
            Ok(metadata) => metadata,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && force => return Ok(()),
            Err(e) => return Err(FileError::from_io(&e, &resolved)),
        };
        let result = if metadata.is_dir() {
            if !recursive {
                // Node's `ERR_FS_EISDIR`, which `toFileError` maps to unknown.
                return Err(FileError::new(
                    FileErrorCode::Unknown,
                    format!(
                        "Path is a directory: rm returned EISDIR (is a directory) {}",
                        path_string(&resolved)
                    ),
                    Some(path_string(&resolved)),
                ));
            }
            tokio::fs::remove_dir_all(&resolved).await
        } else {
            tokio::fs::remove_file(&resolved).await
        };
        result.map_err(|e| FileError::from_io(&e, &resolved))
    }

    async fn temp_dir(prefix: &str) -> Result<String, FileError> {
        let base = std::env::temp_dir();
        loop {
            let path = base.join(format!("{prefix}{}", random_suffix(6)));
            match tokio::fs::create_dir(&path).await {
                Ok(()) => return Ok(path_string(&path)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(FileError::from_io(&e, &path)),
            }
        }
    }
}

impl ExecutionEnv for LocalExecutionEnv {
    fn cwd(&self) -> &str {
        &self.cwd
    }

    fn exec<'a>(
        &'a self,
        command: &'a str,
        options: ExecOptions,
    ) -> BoxFuture<'a, Result<ExecResult, String>> {
        Box::pin(self.run(command, options))
    }

    fn read_text_file<'a>(&'a self, path: &'a str) -> BoxFuture<'a, Result<String, FileError>> {
        Box::pin(async move {
            let resolved = self.resolve(path);
            let bytes = tokio::fs::read(&resolved)
                .await
                .map_err(|e| FileError::from_io(&e, &resolved))?;
            Ok(String::from_utf8_lossy(&bytes).into_owned())
        })
    }

    fn read_binary_file<'a>(&'a self, path: &'a str) -> BoxFuture<'a, Result<Vec<u8>, FileError>> {
        Box::pin(async move {
            let resolved = self.resolve(path);
            tokio::fs::read(&resolved)
                .await
                .map_err(|e| FileError::from_io(&e, &resolved))
        })
    }

    fn write_file<'a>(
        &'a self,
        path: &'a str,
        content: &'a [u8],
    ) -> BoxFuture<'a, Result<(), FileError>> {
        Box::pin(async move {
            let resolved = self.resolve(path);
            if let Some(parent) = resolved.parent() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(|e| FileError::from_io(&e, &resolved))?;
            }
            tokio::fs::write(&resolved, content)
                .await
                .map_err(|e| FileError::from_io(&e, &resolved))
        })
    }

    fn file_info<'a>(&'a self, path: &'a str) -> BoxFuture<'a, Result<FileInfo, FileError>> {
        Box::pin(self.stat(path))
    }

    fn list_dir<'a>(&'a self, path: &'a str) -> BoxFuture<'a, Result<Vec<FileInfo>, FileError>> {
        Box::pin(self.list(path))
    }

    fn real_path<'a>(&'a self, path: &'a str) -> BoxFuture<'a, Result<String, FileError>> {
        Box::pin(async move {
            let resolved = self.resolve(path);
            tokio::fs::canonicalize(&resolved)
                .await
                .map(|p| path_string(&p))
                .map_err(|e| FileError::from_io(&e, &resolved))
        })
    }

    fn exists<'a>(&'a self, path: &'a str) -> BoxFuture<'a, Result<bool, FileError>> {
        Box::pin(async move {
            match self.stat(path).await {
                Ok(_) => Ok(true),
                Err(e) if e.code == FileErrorCode::NotFound => Ok(false),
                Err(e) => Err(e),
            }
        })
    }

    fn create_dir<'a>(
        &'a self,
        path: &'a str,
        recursive: bool,
    ) -> BoxFuture<'a, Result<(), FileError>> {
        Box::pin(async move {
            let resolved = self.resolve(path);
            let result = if recursive {
                tokio::fs::create_dir_all(&resolved).await
            } else {
                tokio::fs::create_dir(&resolved).await
            };
            result.map_err(|e| FileError::from_io(&e, &resolved))
        })
    }

    fn remove<'a>(
        &'a self,
        path: &'a str,
        recursive: bool,
        force: bool,
    ) -> BoxFuture<'a, Result<(), FileError>> {
        Box::pin(self.remove_path(path, recursive, force))
    }

    fn create_temp_dir<'a>(
        &'a self,
        prefix: Option<&'a str>,
    ) -> BoxFuture<'a, Result<String, FileError>> {
        Box::pin(Self::temp_dir(prefix.unwrap_or("tmp-")))
    }

    fn create_temp_file<'a>(
        &'a self,
        prefix: Option<&'a str>,
        suffix: Option<&'a str>,
    ) -> BoxFuture<'a, Result<String, FileError>> {
        Box::pin(async move {
            let dir = Self::temp_dir("tmp-").await?;
            let path = Path::new(&dir).join(format!(
                "{}{}{}",
                prefix.unwrap_or(""),
                uuid::Uuid::new_v4(),
                suffix.unwrap_or("")
            ));
            tokio::fs::write(&path, b"")
                .await
                .map_err(|e| FileError::from_io(&e, &path))?;
            Ok(path_string(&path))
        })
    }

    fn cleanup(&self) -> BoxFuture<'_, ()> {
        Box::pin(async {})
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_relative_paths_lexically() {
        assert_eq!(
            resolve_path("/a/b", "c/../d/./e"),
            PathBuf::from("/a/b/d/e")
        );
        assert_eq!(resolve_path("/a/b", "/x/y/"), PathBuf::from("/x/y"));
        assert_eq!(resolve_path("/a/b", "."), PathBuf::from("/a/b"));
    }

    #[test]
    fn js_numbers_print_like_template_literals() {
        assert_eq!(js_number(5.0), "5");
        assert_eq!(js_number(0.5), "0.5");
    }
}
