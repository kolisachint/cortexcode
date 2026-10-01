//! `hoocode app-server [--listen URL] [hoocode flags]`: serve this folder's
//! sessions over the Codex app-server protocol. See
//! `docs/design/app-server.md`.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use cortexcode_agent_types::PermissionGate;
use cortexcode_app_server::transport::{self, Listen};
use cortexcode_app_server::{AppServer, SavedSession, ServerConfig, SessionFactory};
use cortexcode_code_agent_session::AgentSession;

use crate::runtime::{app_server_auth, async_runtime, AppServerSessions};

const USAGE: &str = "Usage: hoocode app-server [--listen URL] [hoocode options]

Serve this folder's hoocode sessions over the Codex app-server protocol.

  --listen URL    stdio:// (default), unix:// (default socket) or unix://PATH
                  Default socket: ~/.cortexcode/app-server-control/app-server-control.sock

Other options (--model, --session-dir, --tools, ...) apply to every thread.
";

impl SessionFactory for AppServerSessions {
    fn create(
        &self,
        model: Option<&str>,
        gate: Arc<dyn PermissionGate>,
    ) -> Result<AgentSession, String> {
        AppServerSessions::create(self, model, gate)
    }

    fn open(
        &self,
        path: &Path,
        model: Option<&str>,
        gate: Arc<dyn PermissionGate>,
    ) -> Result<AgentSession, String> {
        AppServerSessions::open(self, path, model, gate)
    }

    fn list(&self) -> Vec<SavedSession> {
        let mut sessions: Vec<SavedSession> =
            cortexcode_code_session::list_sessions(self.sessions_dir(), None)
                .unwrap_or_default()
                .into_iter()
                .map(|s| SavedSession {
                    id: s.id,
                    path: s.path,
                    cwd: s.cwd,
                    name: s.name,
                    preview: s.first_message,
                    created: s.created.timestamp(),
                    modified: s.modified.timestamp(),
                })
                .collect();
        sessions.sort_by_key(|s| std::cmp::Reverse(s.modified));
        sessions
    }

    fn models(&self) -> Vec<(String, String, bool)> {
        AppServerSessions::models(self)
    }

    fn resolve_model(&self, name: &str) -> Option<cortexcode_ai_types::Model> {
        AppServerSessions::resolve_model(self, name)
    }
}

/// The default control socket.
pub fn default_socket() -> PathBuf {
    cortexcode_code_paths::agent_dir()
        .join("app-server-control")
        .join("app-server-control.sock")
}

/// Split `--listen URL` / `--listen=URL` out of `argv`.
fn take_listen(argv: &[String]) -> Result<(Option<String>, Vec<String>), String> {
    let mut listen = None;
    let mut rest = Vec::new();
    let mut iter = argv.iter();
    while let Some(arg) = iter.next() {
        if arg == "--listen" {
            listen = Some(
                iter.next()
                    .cloned()
                    .ok_or_else(|| "--listen needs a URL".to_string())?,
            );
        } else if let Some(url) = arg.strip_prefix("--listen=") {
            listen = Some(url.to_string());
        } else {
            rest.push(arg.clone());
        }
    }
    Ok((listen, rest))
}

/// `argv` is everything after `app-server`.
pub fn run(argv: &[String], err: &mut dyn Write) -> i32 {
    if argv.iter().any(|a| a == "--help" || a == "-h") {
        let _ = write!(err, "{USAGE}");
        return 0;
    }
    let (listen, rest) = match take_listen(argv) {
        Ok(v) => v,
        Err(e) => {
            let _ = writeln!(err, "Error: {e}\n\n{USAGE}");
            return 1;
        }
    };
    let listen =
        match transport::parse_listen(listen.as_deref().unwrap_or("stdio://"), &default_socket()) {
            Ok(l) => l,
            Err(e) => {
                let _ = writeln!(err, "Error: {e}");
                return 1;
            }
        };
    let args = crate::args::parse_args(&rest);
    if let Some((bad, _)) = args.unknown_flags.first() {
        let _ = writeln!(err, "Error: unknown option --{bad}\n\n{USAGE}");
        return 1;
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let settings = crate::load_settings();
    let session_dir = crate::session_flags::session_dir(&args, &settings);
    // stdout carries the protocol in stdio mode; nothing else may print there.
    cortexcode_code_agent_session::output_guard::take_over_stdout();
    crate::runtime::install_oauth_providers();

    let sessions = AppServerSessions {
        args,
        cwd: cwd.clone(),
        session_dir,
        auth: app_server_auth(),
    };
    let config = ServerConfig {
        version: crate::VERSION.to_string(),
        home: cortexcode_code_paths::agent_dir()
            .to_string_lossy()
            .into_owned(),
        cwd,
    };
    let result = async_runtime().block_on(async move {
        let server = AppServer::new(config, Arc::new(sessions));
        let result = match listen {
            Listen::Stdio => {
                transport::serve_stdio(server.handler()).await;
                Ok(())
            }
            #[cfg(unix)]
            Listen::Unix(path) => {
                eprintln!("hoocode app-server listening on unix://{}", path.display());
                transport::serve_unix_until_ctrl_c(server.handler(), &path).await
            }
            #[cfg(not(unix))]
            Listen::Unix(_) => Err(std::io::Error::other(
                "unix sockets are not supported on this platform",
            )),
        };
        server.shutdown().await;
        result
    });
    match result {
        Ok(()) => 0,
        Err(e) => {
            let _ = writeln!(err, "Error: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn listen_is_taken_out_of_the_hoocode_flags() {
        let (listen, rest) =
            take_listen(&strings(&["--model", "x", "--listen", "unix://"])).unwrap();
        assert_eq!(listen.as_deref(), Some("unix://"));
        assert_eq!(rest, strings(&["--model", "x"]));
        let (listen, _) = take_listen(&strings(&["--listen=stdio://"])).unwrap();
        assert_eq!(listen.as_deref(), Some("stdio://"));
        assert!(take_listen(&strings(&["--listen"])).is_err());
    }
}
