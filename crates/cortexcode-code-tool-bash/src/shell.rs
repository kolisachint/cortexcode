//! `utils/shell.ts`: shell resolution, the shell environment, output
//! sanitizing, and process-tree kills.

use std::collections::{BTreeMap, HashSet};
use std::ffi::OsString;
use std::path::Path;
use std::sync::{LazyLock, Mutex};

/// Environment for a spawned shell (`NodeJS.ProcessEnv`).
pub type ShellEnv = BTreeMap<OsString, OsString>;

/// `ShellConfig`: the shell binary and the args before the command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellConfig {
    pub shell: String,
    pub args: Vec<String>,
}

impl ShellConfig {
    fn with_c(shell: impl Into<String>) -> Self {
        Self {
            shell: shell.into(),
            args: vec!["-c".into()],
        }
    }
}

/// The first line of `which bash` (`where bash.exe` on Windows, which must
/// also exist).
fn find_bash_on_path() -> Option<String> {
    let (finder, name) = if cfg!(windows) {
        ("where", "bash.exe")
    } else {
        ("which", "bash")
    };
    let output = std::process::Command::new(finder)
        .arg(name)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let first = stdout
        .trim()
        .lines()
        .next()?
        .trim_end_matches('\r')
        .to_owned();
    if first.is_empty() || (cfg!(windows) && !Path::new(&first).exists()) {
        return None;
    }
    Some(first)
}

/// `getShellConfig`: the explicit `shellPath`, else (Unix) `/bin/bash`, bash
/// on `PATH`, `sh`; (Windows) Git Bash, then bash on `PATH`.
pub fn get_shell_config(custom_shell_path: Option<&str>) -> Result<ShellConfig, String> {
    if let Some(custom) = custom_shell_path.filter(|p| !p.is_empty()) {
        if Path::new(custom).exists() {
            return Ok(ShellConfig::with_c(custom));
        }
        return Err(format!("Custom shell path not found: {custom}"));
    }

    if cfg!(windows) {
        let mut paths = Vec::new();
        if let Ok(program_files) = std::env::var("ProgramFiles") {
            paths.push(format!("{program_files}\\Git\\bin\\bash.exe"));
        }
        if let Ok(program_files_x86) = std::env::var("ProgramFiles(x86)") {
            paths.push(format!("{program_files_x86}\\Git\\bin\\bash.exe"));
        }
        if let Some(path) = paths.iter().find(|p| Path::new(p).exists()) {
            return Ok(ShellConfig::with_c(path.clone()));
        }
        if let Some(bash) = find_bash_on_path() {
            return Ok(ShellConfig::with_c(bash));
        }
        let searched: Vec<String> = paths.iter().map(|p| format!("  {p}")).collect();
        return Err(format!(
            "No bash shell found. Options:\n  1. Install Git for Windows: https://git-scm.com/download/win\n  2. Add your bash to PATH (Cygwin, MSYS2, etc.)\n  3. Set shellPath in settings.json\n\nSearched Git Bash in:\n{}",
            searched.join("\n")
        ));
    }

    if Path::new("/bin/bash").exists() {
        return Ok(ShellConfig::with_c("/bin/bash"));
    }
    if let Some(bash) = find_bash_on_path() {
        return Ok(ShellConfig::with_c(bash));
    }
    Ok(ShellConfig::with_c("sh"))
}

/// `getShellEnv`: the process environment with the managed bin dir first on
/// `PATH` (unless already there).
pub fn get_shell_env() -> ShellEnv {
    let mut env: ShellEnv = std::env::vars_os().collect();
    let bin_dir = cortexcode_code_paths::bin_dir().into_os_string();
    let path_key = env
        .keys()
        .find(|k| k.to_string_lossy().eq_ignore_ascii_case("path"))
        .cloned()
        .unwrap_or_else(|| OsString::from("PATH"));
    let current = env.get(&path_key).cloned().unwrap_or_default();
    let has_bin_dir = std::env::split_paths(&current).any(|p| p.as_os_str() == bin_dir);
    if !has_bin_dir {
        let updated = if current.is_empty() {
            bin_dir
        } else {
            let separator = if cfg!(windows) { ";" } else { ":" };
            let mut joined = bin_dir;
            joined.push(separator);
            joined.push(&current);
            joined
        };
        env.insert(path_key, updated);
    }
    env
}

/// `sanitizeBinaryOutput`: drop control characters (except tab, newline,
/// carriage return) and U+FFF9..=U+FFFB, which break width measurement.
pub fn sanitize_binary_output(text: &str) -> String {
    text.chars()
        .filter(|&c| {
            let code = c as u32;
            if matches!(code, 0x09 | 0x0a | 0x0d) {
                return true;
            }
            code > 0x1f && !(0xfff9..=0xfffb).contains(&code)
        })
        .collect()
}

static ANSI: LazyLock<regex_lite::Regex> = LazyLock::new(|| {
    // ansi-regex 6.2 (strip-ansi 7.2): OSC sequences, then CSI and friends.
    regex_lite::Regex::new(
        r"(?:\x1B\][\s\S]*?(?:\x07|\x1B\x5C|\x{9C}))|[\x1B\x{9B}][\[\]()#;?]*(?:\d{1,4}(?:[;:]\d{0,4})*)?[\dA-PR-TZcf-nq-uy=><~]",
    )
    .expect("ansi pattern")
});

/// `strip-ansi`.
pub fn strip_ansi(text: &str) -> String {
    if !text.contains(['\u{1b}', '\u{9b}']) {
        return text.to_owned();
    }
    ANSI.replace_all(text, "").into_owned()
}

static TRACKED_DETACHED_CHILDREN: LazyLock<Mutex<HashSet<u32>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

fn tracked() -> std::sync::MutexGuard<'static, HashSet<u32>> {
    TRACKED_DETACHED_CHILDREN
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// Remember a detached child so shutdown can kill it.
pub fn track_detached_child_pid(pid: u32) {
    tracked().insert(pid);
}

pub fn untrack_detached_child_pid(pid: u32) {
    tracked().remove(&pid);
}

/// Kill every tracked detached child tree (on SIGHUP/SIGTERM).
pub fn kill_tracked_detached_children() {
    let pids: Vec<u32> = tracked().drain().collect();
    for pid in pids {
        kill_process_tree(pid);
    }
}

/// `killProcessTree`: SIGKILL the process group (Unix, falling back to the
/// process itself), or `taskkill /F /T` (Windows).
pub fn kill_process_tree(pid: u32) {
    #[cfg(unix)]
    {
        let Ok(pid) = libc::pid_t::try_from(pid) else {
            return;
        };
        // SAFETY: kill(2) with a pid/pgid and a signal number has no memory effects.
        unsafe {
            if libc::kill(-pid, libc::SIGKILL) != 0 {
                libc::kill(pid, libc::SIGKILL);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_csi_and_osc_sequences() {
        assert_eq!(strip_ansi("\u{1b}[31mred\u{1b}[0m"), "red");
        assert_eq!(
            strip_ansi("a\u{1b}]8;;http://x\u{7}link\u{1b}]8;;\u{7}b"),
            "alinkb"
        );
        assert_eq!(strip_ansi("\u{1b}[1;2:3mX"), "X");
        assert_eq!(strip_ansi("plain"), "plain");
    }

    #[test]
    fn sanitizes_control_and_format_characters() {
        assert_eq!(
            sanitize_binary_output("a\u{0}b\tc\r\n\u{1f}d\u{fffa}e\u{7f}"),
            "ab\tc\r\nde\u{7f}"
        );
    }

    #[test]
    fn resolves_shells() {
        assert_eq!(
            get_shell_config(Some("/custom/bash")),
            Err("Custom shell path not found: /custom/bash".into())
        );
        let config = get_shell_config(None).unwrap();
        assert_eq!(config.args, ["-c"]);
        #[cfg(unix)]
        if Path::new("/bin/bash").exists() {
            assert_eq!(config.shell, "/bin/bash");
        }
    }

    #[test]
    fn shell_env_puts_the_bin_dir_first_once() {
        let env = get_shell_env();
        let path = env
            .iter()
            .find(|(k, _)| k.to_string_lossy().eq_ignore_ascii_case("path"))
            .map(|(_, v)| v.clone())
            .unwrap();
        let first = std::env::split_paths(&path).next().unwrap();
        assert_eq!(first, cortexcode_code_paths::bin_dir());
    }
}
