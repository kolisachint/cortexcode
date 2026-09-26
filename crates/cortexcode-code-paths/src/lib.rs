//! App identity and paths: hoocode `packages/coding-agent/src/config.ts`
//! (v0.5.89; the install-method / self-update half is ledger 12.7) and
//! `src/utils/paths.ts`.
//!
//! cortex writes under `~/.cortexcode` and reads hoocode's `~/.hoocode` as a
//! fallback. Env overrides take `CORTEXCODE_`, `CORTEX_` or hoocode's
//! `HOOCODE_` prefix, in that order.

use std::path::{Component, Path, PathBuf};

/// `APP_NAME`.
pub const APP_NAME: &str = "cortex";
/// `APP_TITLE`.
pub const APP_TITLE: &str = "Cortex";
/// `CONFIG_DIR_NAME`: the global (`~/…`) and project config directory.
pub const CONFIG_DIR_NAME: &str = ".cortexcode";
/// hoocode's config directory, read as a fallback.
pub const LEGACY_CONFIG_DIR_NAME: &str = ".hoocode";
/// `VERSION`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Env override prefixes, most specific first.
pub const ENV_PREFIXES: [&str; 3] = ["CORTEXCODE_", "CORTEX_", "HOOCODE_"];

/// `ENV_AGENT_DIR` (suffix).
pub const ENV_AGENT_DIR: &str = "CODING_AGENT_DIR";
/// `ENV_SESSION_DIR` (suffix).
pub const ENV_SESSION_DIR: &str = "CODING_AGENT_SESSION_DIR";
/// `ENV_USER_AGENTS_DIR` (suffix).
pub const ENV_USER_AGENTS_DIR: &str = "USER_AGENTS_DIR";

/// The first non-empty `<prefix><suffix>` variable.
pub fn env_override(suffix: &str) -> Option<String> {
    ENV_PREFIXES.iter().find_map(|prefix| {
        std::env::var(format!("{prefix}{suffix}"))
            .ok()
            .filter(|v| !v.is_empty())
    })
}

fn home_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

/// `expandTildePath`: a leading `~` or `~/` becomes the home directory.
pub fn expand_tilde_path(path: &str) -> PathBuf {
    if path == "~" {
        return home_dir();
    }
    match path.strip_prefix("~/") {
        Some(rest) => home_dir().join(rest),
        None => PathBuf::from(path),
    }
}

/// `getShareViewerUrl`: `<base>#<gistId>` when a viewer is configured.
pub fn share_viewer_url(gist_id: &str) -> Option<String> {
    let base = env_override("SHARE_VIEWER_URL")?;
    let base = base.trim();
    (!base.is_empty()).then(|| format!("{base}#{gist_id}"))
}

/// `getAgentDir`: the env override, else `~/.cortexcode`.
pub fn agent_dir() -> PathBuf {
    match env_override(ENV_AGENT_DIR) {
        Some(dir) => expand_tilde_path(&dir),
        None => home_dir().join(CONFIG_DIR_NAME),
    }
}

/// hoocode's agent directory (`~/.hoocode`).
pub fn legacy_agent_dir() -> PathBuf {
    home_dir().join(LEGACY_CONFIG_DIR_NAME)
}

/// A file under the agent dir for reading: with an env override, that
/// directory; otherwise `~/.cortexcode/<name>`, or `~/.hoocode/<name>` when
/// only the hoocode one exists.
pub fn resolve_agent_file(name: &str) -> PathBuf {
    let primary = agent_dir().join(name);
    if env_override(ENV_AGENT_DIR).is_some() {
        return primary;
    }
    let legacy = legacy_agent_dir().join(name);
    if !primary.exists() && legacy.exists() {
        legacy
    } else {
        primary
    }
}

/// `getUserAgentsDir`: the cross-vendor `~/.agents` scope (read only).
pub fn user_agents_dir() -> PathBuf {
    match env_override(ENV_USER_AGENTS_DIR) {
        Some(dir) => expand_tilde_path(&dir),
        None => home_dir().join(".agents"),
    }
}

/// The session directory override (`*_CODING_AGENT_SESSION_DIR`).
pub fn session_dir_override() -> Option<PathBuf> {
    env_override(ENV_SESSION_DIR).map(|dir| expand_tilde_path(&dir))
}

const DISPATCH_DIR_NAME: &str = "dispatch";

/// `getDispatchRoot`: subagent runtime state within a project.
pub fn dispatch_root(cwd: &Path) -> PathBuf {
    cwd.join(CONFIG_DIR_NAME).join(DISPATCH_DIR_NAME)
}

/// `getDispatchTaskDir`.
pub fn dispatch_task_dir(cwd: &Path, task_id: &str) -> PathBuf {
    dispatch_root(cwd).join(task_id)
}

/// `getCustomThemesDir`.
pub fn custom_themes_dir() -> PathBuf {
    agent_dir().join("themes")
}

/// `getAuthPath`.
pub fn auth_path() -> PathBuf {
    agent_dir().join("auth.json")
}

/// `getBinDir`: managed binaries (fd, rg).
pub fn bin_dir() -> PathBuf {
    agent_dir().join("bin")
}

/// `getSessionsDir`.
pub fn sessions_dir() -> PathBuf {
    agent_dir().join("sessions")
}

/// `getDebugLogPath`.
pub fn debug_log_path() -> PathBuf {
    agent_dir().join(format!("{APP_NAME}-debug.log"))
}

// ---------------------------------------------------------------------------
// utils/paths.ts
// ---------------------------------------------------------------------------

/// `path.resolve`: absolute (against the process cwd) and normalized.
fn resolve(path: &Path) -> PathBuf {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("/"))
            .join(path)
    };
    normalize(&joined)
}

/// Lexical `.`/`..` resolution.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// `canonicalizePath`: the real path, or the path itself when it cannot be
/// resolved (missing target, dangling symlink).
pub fn canonicalize_path(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// `isPathInside`: `target` is `root` or below it (segment-aware).
pub fn is_path_inside(target: &Path, root: &Path) -> bool {
    resolve(target).starts_with(resolve(root))
}

/// `isLocalPath`: not a package source (`npm:`, `git:`, `github:`) or URL.
pub fn is_local_path(value: &str) -> bool {
    // JS `trim` also strips U+FEFF.
    let trimmed = value.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}');
    !["npm:", "git:", "github:", "http:", "https:", "ssh:"]
        .iter()
        .any(|prefix| trimmed.starts_with(prefix))
}

fn resolve_against_cwd(file_path: &Path, cwd: &Path) -> PathBuf {
    if file_path.is_absolute() {
        resolve(file_path)
    } else {
        resolve(&cwd.join(file_path))
    }
}

/// `getCwdRelativePath`: the path relative to `cwd` (`.` for cwd itself),
/// or `None` outside it.
pub fn cwd_relative_path(file_path: &Path, cwd: &Path) -> Option<PathBuf> {
    let cwd = resolve(cwd);
    let path = resolve_against_cwd(file_path, &cwd);
    let relative = path.strip_prefix(&cwd).ok()?;
    Some(if relative.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        relative.to_path_buf()
    })
}

/// `formatPathRelativeToCwdOrAbsolute`: `/`-separated.
pub fn format_path_relative_to_cwd_or_absolute(file_path: &Path, cwd: &Path) -> String {
    let absolute = resolve_against_cwd(file_path, cwd);
    let shown = cwd_relative_path(&absolute, cwd).unwrap_or(absolute);
    shown
        .to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/")
}

/// The nearest ancestor with a `.git` entry.
fn find_git_repo_root(start: &Path) -> Option<PathBuf> {
    let mut dir = resolve(start);
    loop {
        if dir.join(".git").exists() {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// `collectAgentsAncestorDirs`: `.agents/<subdir>` from `start` up to the
/// git root (or the filesystem root outside a repo), closest first.
pub fn collect_agents_ancestor_dirs(start: &Path, subdir: &str) -> Vec<PathBuf> {
    let start = resolve(start);
    let git_root = find_git_repo_root(&start);
    let mut dirs = Vec::new();
    let mut dir = start;
    loop {
        dirs.push(dir.join(".agents").join(subdir));
        if git_root.as_ref() == Some(&dir) || !dir.pop() {
            break;
        }
    }
    dirs
}
