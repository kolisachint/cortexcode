//! main.ts `resolveSessionPath` / `validateForkFlags` / `createSessionManager`:
//! `--no-session`, `--fork`, `--session`, `--continue` and the session dir
//! (`--session-dir`, else `CORTEXCODE_CODING_AGENT_SESSION_DIR`, else the
//! `sessionDir` setting), and `--resume` (the session picker).

use crate::args::Args;
use crate::{red, Env};
use cortexcode_code_session::{list_all_sessions, list_sessions, SessionManager};
use cortexcode_code_settings::SettingsManager;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

/// `ResolvedSession`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedSession {
    /// A direct file path.
    Path(PathBuf),
    /// Found in the current project.
    Local(PathBuf),
    /// Found in a different project.
    Global {
        path: PathBuf,
        cwd: String,
    },
    NotFound(String),
}

/// `resolveSessionPath`: a path as given, else an id prefix in this
/// project's sessions (newest first), else across all projects.
pub fn resolve_session_path(arg: &str, cwd: &str, session_dir: Option<&Path>) -> ResolvedSession {
    if arg.contains('/') || arg.contains('\\') || arg.ends_with(".jsonl") {
        return ResolvedSession::Path(PathBuf::from(arg));
    }
    let dir = session_dir
        .map(Path::to_path_buf)
        .unwrap_or_else(|| cortexcode_code_session::default_session_dir(cwd));
    if let Some(info) = list_sessions(&dir, None)
        .unwrap_or_default()
        .into_iter()
        .find(|s| s.id.starts_with(arg))
    {
        return ResolvedSession::Local(info.path);
    }
    if let Some(info) = list_all_sessions(None)
        .unwrap_or_default()
        .into_iter()
        .find(|s| s.id.starts_with(arg))
    {
        return ResolvedSession::Global {
            path: info.path,
            cwd: info.cwd,
        };
    }
    ResolvedSession::NotFound(arg.to_string())
}

/// `validateForkFlags`: the error message, if `--fork` has a conflicting flag.
pub fn fork_conflicts(args: &Args) -> Option<String> {
    args.fork.as_ref()?;
    let conflicting: Vec<&str> = [
        (args.session.is_some(), "--session"),
        (args.continue_ == Some(true), "--continue"),
        (args.resume == Some(true), "--resume"),
        (args.no_session == Some(true), "--no-session"),
    ]
    .into_iter()
    .filter_map(|(set, flag)| set.then_some(flag))
    .collect();
    (!conflicting.is_empty()).then(|| {
        format!(
            "Error: --fork cannot be combined with {}",
            conflicting.join(", ")
        )
    })
}

/// The session dir: `--session-dir`, the env override, the setting.
pub fn session_dir(args: &Args, settings: &SettingsManager) -> Option<PathBuf> {
    args.session_dir
        .as_ref()
        .map(PathBuf::from)
        .or_else(|| {
            cortexcode_code_paths::env_override("CODING_AGENT_SESSION_DIR")
                .map(|d| cortexcode_code_paths::expand_tilde_path(&d))
        })
        .or_else(|| settings.session_dir())
}

/// `promptConfirm`: `message [y/N]` on stdout, an answer line from stdin.
fn prompt_confirm(message: &str) -> bool {
    print!("{message} [y/N] ");
    let _ = std::io::stdout().flush();
    let mut answer = String::new();
    let _ = std::io::stdin().lock().read_line(&mut answer);
    let answer = answer.trim().to_lowercase();
    answer == "y" || answer == "yes"
}

fn exit_with(env: Env, message: &str) -> ! {
    eprintln!("{}", red(env.color, message));
    std::process::exit(1)
}

fn fork_or_exit(env: Env, source: &Path, cwd: &str, dir: Option<PathBuf>) -> SessionManager {
    SessionManager::fork_from(source, cwd, dir)
        .unwrap_or_else(|e| exit_with(env, &format!("Error: {e}")))
}

/// `createSessionManager` (exits like main.ts on a bad flag).
pub fn create_session_manager(
    args: &Args,
    cwd: &str,
    session_dir: Option<PathBuf>,
    env: Env,
    theme: Option<&str>,
) -> SessionManager {
    if let Some(message) = fork_conflicts(args) {
        exit_with(env, &message);
    }
    if args.no_session == Some(true) {
        return SessionManager::in_memory(cwd);
    }
    if let Some(fork) = &args.fork {
        return match resolve_session_path(fork, cwd, session_dir.as_deref()) {
            ResolvedSession::Path(p)
            | ResolvedSession::Local(p)
            | ResolvedSession::Global { path: p, .. } => fork_or_exit(env, &p, cwd, session_dir),
            ResolvedSession::NotFound(arg) => {
                exit_with(env, &format!("No session found matching '{arg}'"))
            }
        };
    }
    if let Some(session) = &args.session {
        return match resolve_session_path(session, cwd, session_dir.as_deref()) {
            ResolvedSession::Path(p) | ResolvedSession::Local(p) => {
                SessionManager::open(p, session_dir, None)
            }
            ResolvedSession::Global { path, cwd: other } => {
                let yellow = |t: &str| {
                    if env.color {
                        format!("\x1b[33m{t}\x1b[39m")
                    } else {
                        t.to_string()
                    }
                };
                println!(
                    "{}",
                    yellow(&format!("Session found in different project: {other}"))
                );
                if !prompt_confirm("Fork this session into current directory?") {
                    let dim = if env.color {
                        "\x1b[2mAborted.\x1b[22m"
                    } else {
                        "Aborted."
                    };
                    println!("{dim}");
                    std::process::exit(0);
                }
                fork_or_exit(env, &path, cwd, session_dir)
            }
            ResolvedSession::NotFound(arg) => {
                exit_with(env, &format!("No session found matching '{arg}'"))
            }
        };
    }
    if args.resume == Some(true) {
        let dir = session_dir
            .clone()
            .unwrap_or_else(|| cortexcode_code_session::default_session_dir(cwd));
        let Some(selected) = cortexcode_code_tui_app::session_picker::resume_picker(theme, dir)
        else {
            let dim = if env.color {
                "\x1b[2mNo session selected\x1b[22m"
            } else {
                "No session selected"
            };
            println!("{dim}");
            std::process::exit(0);
        };
        return SessionManager::open(selected, session_dir, None);
    }
    if args.continue_ == Some(true) {
        return SessionManager::continue_recent(cwd, session_dir);
    }
    SessionManager::create(cwd, session_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(argv: &[&str]) -> Args {
        crate::args::parse_args(&argv.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn fork_rejects_conflicting_session_flags_in_order() {
        assert_eq!(fork_conflicts(&parse(&["--fork", "abc"])), None);
        assert_eq!(fork_conflicts(&parse(&["--continue"])), None);
        assert_eq!(
            fork_conflicts(&parse(&[
                "--fork",
                "abc",
                "--no-session",
                "-c",
                "--session",
                "x"
            ])),
            Some(
                "Error: --fork cannot be combined with --session, --continue, --no-session".into()
            )
        );
    }

    #[test]
    fn a_path_like_argument_is_used_as_given() {
        for arg in ["dir/s.jsonl", "a\\b", "s.jsonl"] {
            assert_eq!(
                resolve_session_path(arg, "/w", None),
                ResolvedSession::Path(PathBuf::from(arg))
            );
        }
    }

    #[test]
    fn an_id_prefix_is_looked_up_in_the_project_session_dir() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("2026-01-01_abc123.jsonl");
        std::fs::write(
            &file,
            r#"{"type":"session","version":3,"id":"abc123-0000","timestamp":"2026-01-01T00:00:00.000Z","cwd":"/w"}
{"type":"message","id":"m1","parentId":null,"timestamp":"2026-01-01T00:00:01.000Z","message":{"role":"user","content":"hi","timestamp":1}}
"#,
        )
        .unwrap();
        assert_eq!(
            resolve_session_path("abc", "/w", Some(dir.path())),
            ResolvedSession::Local(file)
        );
        assert_eq!(
            resolve_session_path("zz-no-such-session-zz", "/w", Some(dir.path())),
            ResolvedSession::NotFound("zz-no-such-session-zz".into())
        );
    }
}
