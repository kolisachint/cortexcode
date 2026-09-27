//! Port of hoocode `packages/coding-agent/test/paths.test.ts` (v0.5.89),
//! plus env-override / directory checks for the `config.ts` half.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use cortexcode_code_paths::*;

/// `createTempDir`: a fresh directory, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("pi-paths-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// --- canonicalizePath ---

#[test]
fn canonicalize_returns_the_real_path_for_a_regular_file() {
    let dir = TempDir::new();
    let file = dir.0.join("file.txt");
    std::fs::write(&file, "hello").unwrap();
    assert_eq!(
        canonicalize_path(&file),
        std::fs::canonicalize(&file).unwrap()
    );
}

#[cfg(unix)]
#[test]
fn canonicalize_resolves_symlinks_to_their_targets() {
    let dir = TempDir::new();
    let target = dir.0.join("target.txt");
    let link = dir.0.join("link.txt");
    std::fs::write(&target, "hello").unwrap();
    std::os::unix::fs::symlink(&target, &link).unwrap();
    assert_eq!(
        canonicalize_path(&link),
        std::fs::canonicalize(&target).unwrap()
    );
}

#[cfg(unix)]
#[test]
fn canonicalize_resolves_directory_symlinks() {
    let dir = TempDir::new();
    let target = dir.0.join("target-dir");
    let link = dir.0.join("link-dir");
    std::fs::create_dir(&target).unwrap();
    std::os::unix::fs::symlink(&target, &link).unwrap();
    assert_eq!(
        canonicalize_path(&link),
        std::fs::canonicalize(&target).unwrap()
    );
}

#[test]
fn canonicalize_falls_back_to_the_raw_path_when_the_target_does_not_exist() {
    let dir = TempDir::new();
    let missing = dir.0.join("no-such-file");
    assert_eq!(canonicalize_path(&missing), missing);
}

#[cfg(unix)]
#[test]
fn canonicalize_falls_back_to_the_raw_path_for_a_dangling_symlink() {
    let dir = TempDir::new();
    let link = dir.0.join("link.txt");
    std::os::unix::fs::symlink(dir.0.join("target.txt"), &link).unwrap();
    assert_eq!(canonicalize_path(&link), link);
}

// --- getCwdRelativePath ---

#[test]
fn cwd_relative_keeps_names_that_start_with_dots() {
    let cwd = std::env::temp_dir().join("pi-paths-cwd");
    assert_eq!(
        cwd_relative_path(&cwd.join("..config").join("AGENTS.md"), &cwd),
        Some(Path::new("..config").join("AGENTS.md"))
    );
}

#[test]
fn cwd_relative_rejects_parent_directory_traversals() {
    let cwd = std::env::temp_dir().join("pi-paths-cwd");
    assert_eq!(
        cwd_relative_path(&cwd.join("..").join("AGENTS.md"), &cwd),
        None
    );
}

// --- isLocalPath ---

#[test]
fn is_local_path_cases() {
    assert!(is_local_path("my-package"));
    assert!(is_local_path("./foo"));
    assert!(!is_local_path("npm:package"));
    assert!(!is_local_path("git://repo"));
    assert!(!is_local_path("https://example.com"));
}

// --- beyond the TS file ---

#[test]
fn cwd_relative_edge_cases_and_formatting() {
    let cwd = Path::new("/work/proj");
    assert_eq!(cwd_relative_path(cwd, cwd), Some(PathBuf::from(".")));
    assert_eq!(
        cwd_relative_path(Path::new("src/./a/../b.rs"), cwd),
        Some(PathBuf::from("src/b.rs"))
    );
    assert_eq!(cwd_relative_path(Path::new("/work/projx/a"), cwd), None);
    assert_eq!(
        format_path_relative_to_cwd_or_absolute(Path::new("/work/proj/src/a.rs"), cwd),
        "src/a.rs"
    );
    assert_eq!(
        format_path_relative_to_cwd_or_absolute(Path::new("../other/a.rs"), cwd),
        "/work/other/a.rs"
    );
    assert_eq!(format_path_relative_to_cwd_or_absolute(cwd, cwd), ".");
}

#[test]
fn is_path_inside_is_segment_aware() {
    assert!(is_path_inside(Path::new("/a/b"), Path::new("/a/b")));
    assert!(is_path_inside(Path::new("/a/b/c"), Path::new("/a/b/")));
    assert!(is_path_inside(Path::new("/a/b/../b/c"), Path::new("/a/b")));
    assert!(!is_path_inside(Path::new("/a/bc"), Path::new("/a/b")));
    assert!(!is_path_inside(Path::new("/a"), Path::new("/a/b")));
}

#[test]
fn collects_agents_dirs_up_to_the_git_root() {
    let root = TempDir::new();
    let repo = root.0.join("repo");
    let nested = repo.join("a").join("b");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::create_dir(repo.join(".git")).unwrap();
    assert_eq!(
        collect_agents_ancestor_dirs(&nested, "skills"),
        vec![
            nested.join(".agents/skills"),
            repo.join("a/.agents/skills"),
            repo.join(".agents/skills"),
        ]
    );
    // Outside a repo the walk reaches the filesystem root.
    let outside = collect_agents_ancestor_dirs(&root.0, "prompts");
    assert_eq!(outside.first(), Some(&root.0.join(".agents/prompts")));
    assert_eq!(outside.last(), Some(&PathBuf::from("/.agents/prompts")));
}

static ENV_LOCK: Mutex<()> = Mutex::new(());

const ENV_VARS: [&str; 9] = [
    "CORTEXCODE_CODING_AGENT_DIR",
    "CORTEX_CODING_AGENT_DIR",
    "HOOCODE_CODING_AGENT_DIR",
    "CORTEXCODE_CODING_AGENT_SESSION_DIR",
    "HOOCODE_CODING_AGENT_SESSION_DIR",
    "CORTEXCODE_USER_AGENTS_DIR",
    "CORTEXCODE_SHARE_VIEWER_URL",
    "HOOCODE_SHARE_VIEWER_URL",
    "HOME",
];

/// Runs `f` with only `vars` set among [`ENV_VARS`], restoring afterwards.
fn with_env(vars: &[(&str, &str)], f: impl FnOnce()) {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let saved: Vec<_> = ENV_VARS.iter().map(|k| (*k, std::env::var_os(k))).collect();
    for k in ENV_VARS {
        if k != "HOME" {
            std::env::remove_var(k);
        }
    }
    for (k, v) in vars {
        std::env::set_var(k, v);
    }
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    for (k, v) in saved {
        match v {
            Some(v) => std::env::set_var(k, v),
            None => std::env::remove_var(k),
        }
    }
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

#[test]
fn env_overrides_prefer_cortexcode_then_cortex_then_hoocode() {
    with_env(
        &[("HOME", "/home/u"), ("HOOCODE_CODING_AGENT_DIR", "/h")],
        || {
            assert_eq!(agent_dir(), PathBuf::from("/h"));
        },
    );
    with_env(
        &[
            ("HOME", "/home/u"),
            ("HOOCODE_CODING_AGENT_DIR", "/h"),
            ("CORTEX_CODING_AGENT_DIR", "/c"),
        ],
        || assert_eq!(agent_dir(), PathBuf::from("/c")),
    );
    with_env(
        &[
            ("HOME", "/home/u"),
            ("HOOCODE_CODING_AGENT_DIR", "/h"),
            ("CORTEXCODE_CODING_AGENT_DIR", "~/agent"),
            ("CORTEX_CODING_AGENT_DIR", ""),
        ],
        || {
            assert_eq!(agent_dir(), PathBuf::from("/home/u/agent"));
            assert_eq!(auth_path(), PathBuf::from("/home/u/agent/auth.json"));
            assert_eq!(sessions_dir(), PathBuf::from("/home/u/agent/sessions"));
            assert_eq!(bin_dir(), PathBuf::from("/home/u/agent/bin"));
            assert_eq!(custom_themes_dir(), PathBuf::from("/home/u/agent/themes"));
            assert_eq!(
                debug_log_path(),
                PathBuf::from("/home/u/agent/cortex-debug.log")
            );
        },
    );
    with_env(&[("HOME", "/home/u")], || {
        assert_eq!(agent_dir(), PathBuf::from("/home/u/.cortexcode"));
        assert_eq!(legacy_agent_dir(), PathBuf::from("/home/u/.hoocode"));
        assert_eq!(user_agents_dir(), PathBuf::from("/home/u/.agents"));
        assert_eq!(session_dir_override(), None);
        assert_eq!(share_viewer_url("abc"), None);
        assert_eq!(expand_tilde_path("~"), PathBuf::from("/home/u"));
        assert_eq!(expand_tilde_path("~x/y"), PathBuf::from("~x/y"));
    });
    with_env(
        &[
            ("HOME", "/home/u"),
            ("HOOCODE_CODING_AGENT_SESSION_DIR", "~/s"),
            ("CORTEXCODE_USER_AGENTS_DIR", "/ua"),
            ("HOOCODE_SHARE_VIEWER_URL", " https://view.example/ "),
        ],
        || {
            assert_eq!(session_dir_override(), Some(PathBuf::from("/home/u/s")));
            assert_eq!(user_agents_dir(), PathBuf::from("/ua"));
            assert_eq!(
                share_viewer_url("abc").as_deref(),
                Some("https://view.example/#abc")
            );
        },
    );
}

#[test]
fn agent_files_fall_back_to_the_hoocode_dir() {
    let home = TempDir::new();
    let home_str = home.0.to_string_lossy().into_owned();
    with_env(&[("HOME", &home_str)], || {
        assert_eq!(
            resolve_agent_file("settings.json"),
            home.0.join(".cortexcode/settings.json")
        );
        std::fs::create_dir_all(home.0.join(".hoocode")).unwrap();
        std::fs::write(home.0.join(".hoocode/settings.json"), "{}").unwrap();
        assert_eq!(
            resolve_agent_file("settings.json"),
            home.0.join(".hoocode/settings.json")
        );
        std::fs::create_dir_all(home.0.join(".cortexcode")).unwrap();
        std::fs::write(home.0.join(".cortexcode/settings.json"), "{}").unwrap();
        assert_eq!(
            resolve_agent_file("settings.json"),
            home.0.join(".cortexcode/settings.json")
        );
    });
    with_env(
        &[("HOME", &home_str), ("HOOCODE_CODING_AGENT_DIR", "/x")],
        || {
            assert_eq!(
                resolve_agent_file("auth.json"),
                PathBuf::from("/x/auth.json")
            );
        },
    );
}

#[test]
fn dispatch_dirs_live_in_the_project_config_dir() {
    let cwd = Path::new("/p");
    assert_eq!(dispatch_root(cwd), PathBuf::from("/p/.cortexcode/dispatch"));
    assert_eq!(
        dispatch_task_dir(cwd, "t1"),
        PathBuf::from("/p/.cortexcode/dispatch/t1")
    );
}
