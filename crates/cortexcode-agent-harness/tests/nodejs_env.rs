//! Port of hoocode `packages/agent/test/harness/nodejs-env.test.ts`
//! (v0.5.89) against [`LocalExecutionEnv`]. Unix only (symlinks, `sh`).
#![cfg(unix)]

use std::sync::{Arc, Mutex};

use cortexcode_agent_harness::{
    ExecOptions, ExecResult, ExecutionEnv, FileErrorCode, FileKind, LocalExecutionEnv,
};
use cortexcode_ai_types::AbortSignal;

/// `createTempDir`: a fresh directory, removed on drop.
struct TempDir(std::path::PathBuf);

impl TempDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("harness-env-{}", uuid()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn path(&self) -> String {
        self.0.to_string_lossy().into_owned()
    }
    fn join(&self, rel: &str) -> String {
        self.0.join(rel).to_string_lossy().into_owned()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn uuid() -> String {
    format!(
        "{:x}{:x}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

#[tokio::test]
async fn reads_writes_lists_and_removes_files_and_directories() {
    let root = TempDir::new();
    let env = LocalExecutionEnv::new(root.path());
    env.create_dir("nested", true).await.unwrap();
    env.write_file("nested/file.txt", b"hello").await.unwrap();
    assert_eq!(
        env.read_text_file("nested/file.txt").await.unwrap(),
        "hello"
    );
    assert_eq!(
        env.read_binary_file("nested/file.txt").await.unwrap(),
        b"hello"
    );

    let entries = env.list_dir("nested").await.unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "file.txt");
    assert_eq!(entries[0].path, root.join("nested/file.txt"));
    assert_eq!(entries[0].kind, FileKind::File);
    assert_eq!(entries[0].size, 5);
    assert!(entries[0].mtime_ms > 0.0);

    assert!(env.exists("nested/file.txt").await.unwrap());
    env.remove("nested/file.txt", false, false).await.unwrap();
    assert!(!env.exists("nested/file.txt").await.unwrap());
}

#[tokio::test]
async fn returns_file_info_for_files_directories_and_symlinks_without_following_them() {
    let root = TempDir::new();
    let env = LocalExecutionEnv::new(root.path());
    env.create_dir("dir", true).await.unwrap();
    env.write_file("dir/file.txt", b"hello").await.unwrap();
    std::os::unix::fs::symlink(root.join("dir/file.txt"), root.join("file-link")).unwrap();
    std::os::unix::fs::symlink(root.join("dir"), root.join("dir-link")).unwrap();

    let dir = env.file_info("dir").await.unwrap();
    assert_eq!((dir.name.as_str(), dir.kind), ("dir", FileKind::Directory));
    assert_eq!(dir.path, root.join("dir"));
    let file = env.file_info("dir/file.txt").await.unwrap();
    assert_eq!(
        (file.name.as_str(), file.kind, file.size),
        ("file.txt", FileKind::File, 5)
    );
    for link in ["file-link", "dir-link"] {
        let info = env.file_info(link).await.unwrap();
        assert_eq!((info.name.as_str(), info.kind), (link, FileKind::Symlink));
        assert_eq!(info.path, root.join(link));
    }
    assert_eq!(
        env.real_path("file-link").await.unwrap(),
        std::fs::canonicalize(root.join("dir/file.txt"))
            .unwrap()
            .to_string_lossy()
    );
}

#[tokio::test]
async fn lists_symlinks_as_symlinks() {
    let root = TempDir::new();
    let env = LocalExecutionEnv::new(root.path());
    env.write_file("target.txt", b"hello").await.unwrap();
    std::os::unix::fs::symlink(root.join("target.txt"), root.join("link.txt")).unwrap();
    let mut entries: Vec<(String, FileKind)> = env
        .list_dir(".")
        .await
        .unwrap()
        .into_iter()
        .map(|e| (e.name, e.kind))
        .collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        entries,
        [
            ("link.txt".to_string(), FileKind::Symlink),
            ("target.txt".to_string(), FileKind::File)
        ]
    );
}

#[tokio::test]
async fn missing_paths_are_not_found_errors_and_exists_is_false() {
    let root = TempDir::new();
    let env = LocalExecutionEnv::new(root.path());
    let err = env.file_info("missing.txt").await.unwrap_err();
    assert_eq!(err.code, FileErrorCode::NotFound);
    assert_eq!(err.path, Some(root.join("missing.txt")));
    assert!(!env.exists("missing.txt").await.unwrap());
}

#[tokio::test]
async fn listing_a_file_is_a_not_directory_error() {
    let root = TempDir::new();
    let env = LocalExecutionEnv::new(root.path());
    env.write_file("file.txt", b"hello").await.unwrap();
    assert_eq!(
        env.list_dir("file.txt").await.unwrap_err().code,
        FileErrorCode::NotDirectory
    );
}

#[tokio::test]
async fn creates_temporary_directories_and_files() {
    let root = TempDir::new();
    let env = LocalExecutionEnv::new(root.path());
    let temp_dir = env.create_temp_dir(Some("node-env-test-")).await.unwrap();
    assert!(std::path::Path::new(&temp_dir).is_dir());
    assert!(temp_dir.contains("node-env-test-"));
    let temp_file = env
        .create_temp_file(Some("prefix-"), Some(".txt"))
        .await
        .unwrap();
    assert!(std::path::Path::new(&temp_file).is_file());
    assert!(temp_file.ends_with(".txt"));
    let _ = std::fs::remove_dir_all(temp_dir);
    let _ = std::fs::remove_dir_all(std::path::Path::new(&temp_file).parent().unwrap());
}

#[tokio::test]
async fn executes_commands_in_cwd_with_env_overrides() {
    let root = TempDir::new();
    let env = LocalExecutionEnv::new(root.path());
    let result = env
        .exec(
            r#"printf '%s:%s' "$PWD" "$NODE_ENV_TEST""#,
            ExecOptions {
                env: Some([("NODE_ENV_TEST".to_string(), "ok".to_string())].into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let real_root = std::fs::canonicalize(&root.0).unwrap();
    assert_eq!(
        result,
        ExecResult {
            stdout: format!("{}:ok", real_root.to_string_lossy()),
            stderr: String::new(),
            exit_code: 0
        }
    );
}

#[tokio::test]
async fn streams_stdout_and_stderr_chunks() {
    let root = TempDir::new();
    let env = LocalExecutionEnv::new(root.path());
    let stdout = Arc::new(Mutex::new(String::new()));
    let stderr = Arc::new(Mutex::new(String::new()));
    let (out, err) = (stdout.clone(), stderr.clone());
    let result = env
        .exec(
            "printf out; printf err >&2",
            ExecOptions {
                on_stdout: Some(Arc::new(move |chunk| out.lock().unwrap().push_str(chunk))),
                on_stderr: Some(Arc::new(move |chunk| err.lock().unwrap().push_str(chunk))),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        result,
        ExecResult {
            stdout: "out".into(),
            stderr: "err".into(),
            exit_code: 0
        }
    );
    assert_eq!(*stdout.lock().unwrap(), "out");
    assert_eq!(*stderr.lock().unwrap(), "err");
}

#[tokio::test]
async fn rejects_aborted_commands() {
    let root = TempDir::new();
    let env = LocalExecutionEnv::new(root.path());
    let signal = AbortSignal::new();
    let aborter = signal.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        aborter.abort();
    });
    let started = std::time::Instant::now();
    let err = env
        .exec(
            "sleep 5",
            ExecOptions {
                signal: Some(signal),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert_eq!(err, "aborted");
    assert!(started.elapsed() < std::time::Duration::from_secs(4));
}

// --- beyond the TS file ---

#[tokio::test]
async fn times_out_exit_codes_and_removal_rules() {
    let root = TempDir::new();
    let env = LocalExecutionEnv::new(root.path());
    let err = env
        .exec(
            "sleep 5",
            ExecOptions {
                timeout: Some(0.2),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert_eq!(err, "timeout:0.2");
    let result = env.exec("exit 3", ExecOptions::default()).await.unwrap();
    assert_eq!(result.exit_code, 3);

    env.create_dir("d/e", true).await.unwrap();
    assert_eq!(
        env.remove("d", false, false).await.unwrap_err().code,
        FileErrorCode::Unknown
    );
    env.remove("d", true, false).await.unwrap();
    env.remove("d", true, true).await.unwrap();
    assert_eq!(
        env.remove("d", true, false).await.unwrap_err().code,
        FileErrorCode::NotFound
    );
    let custom = LocalExecutionEnv::new(root.path()).with_shell_path("/no/such/shell");
    assert_eq!(
        custom
            .exec("true", ExecOptions::default())
            .await
            .unwrap_err(),
        "Custom shell path not found: /no/such/shell"
    );
}
