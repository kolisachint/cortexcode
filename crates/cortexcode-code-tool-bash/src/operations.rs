//! `BashOperations` and `createLocalBashOperations` (core/tools/bash.ts).

use std::io::Read;
use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use cortexcode_ai_types::AbortSignal;
use cortexcode_code_tool_api::{js_number, node_fs_error, ToolError};
use process_wrap::std::CommandWrap;

use crate::shell::{
    get_shell_config, get_shell_env, track_detached_child_pid, untrack_detached_child_pid, ShellEnv,
};

/// Options for [`BashOperations::exec`].
pub struct BashExecOptions<'a> {
    /// Receives stdout and stderr chunks as they arrive.
    pub on_data: &'a mut dyn FnMut(&[u8]),
    pub signal: Option<AbortSignal>,
    /// Seconds; `None` or <= 0 means no timeout.
    pub timeout: Option<f64>,
    /// The environment; `None` means [`get_shell_env`].
    pub env: Option<ShellEnv>,
}

/// `BashOperations`: how a command runs. Override to run commands elsewhere
/// (for example over SSH).
///
/// `exec` returns the exit code (`None` when the process was killed by a
/// signal). It fails with `aborted` when the signal fired and
/// `timeout:<seconds>` when the timeout hit; the tool words those for the
/// model.
pub trait BashOperations: Send + Sync {
    fn exec(
        &self,
        command: &str,
        cwd: &Path,
        options: BashExecOptions<'_>,
    ) -> Result<Option<i32>, ToolError>;
}

/// `createLocalBashOperations`: the local shell.
#[derive(Debug, Clone, Default)]
pub struct LocalBashOperations {
    pub shell_path: Option<String>,
}

impl LocalBashOperations {
    pub fn new(shell_path: Option<String>) -> Self {
        Self { shell_path }
    }
}

/// How long to wait for stdout/stderr to close after the shell exits. A
/// backgrounded descendant can hold the pipes open forever
/// (`waitForChildProcess`'s grace period).
const EXIT_STDIO_GRACE: Duration = Duration::from_millis(100);
const POLL: Duration = Duration::from_millis(10);

fn spawn_reader(
    mut pipe: impl Read + Send + 'static,
    tx: mpsc::Sender<Option<Vec<u8>>>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut buf = [0u8; 16 * 1024];
        loop {
            match pipe.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tx.send(Some(buf[..n].to_vec())).is_err() {
                        return;
                    }
                }
            }
        }
        let _ = tx.send(None);
    })
}

impl BashOperations for LocalBashOperations {
    fn exec(
        &self,
        command: &str,
        cwd: &Path,
        options: BashExecOptions<'_>,
    ) -> Result<Option<i32>, ToolError> {
        let BashExecOptions {
            on_data,
            signal,
            timeout,
            env,
        } = options;
        let config = get_shell_config(self.shell_path.as_deref())?;
        if !cwd.exists() {
            return Err(format!(
                "Working directory does not exist: {}\nCannot execute bash commands.",
                cwd.display()
            )
            .into());
        }

        let env = env.unwrap_or_else(get_shell_env);
        let mut wrap = CommandWrap::with_new(&config.shell, |cmd| {
            cmd.args(&config.args)
                .arg(command)
                .current_dir(cwd)
                .env_clear()
                .envs(&env)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped());
        });
        #[cfg(unix)]
        wrap.wrap(process_wrap::std::ProcessGroup::leader());
        #[cfg(windows)]
        wrap.wrap(process_wrap::std::JobObject);
        let mut child = wrap.spawn().map_err(|e| {
            let code = node_fs_error(e, "spawn", None).code;
            format!("spawn {} {code}", config.shell)
        })?;
        let pid = child.id();
        track_detached_child_pid(pid);

        let (tx, rx) = mpsc::channel();
        let mut open_pipes = 0;
        if let Some(stdout) = child.stdout().take() {
            spawn_reader(stdout, tx.clone());
            open_pipes += 1;
        }
        if let Some(stderr) = child.stderr().take() {
            spawn_reader(stderr, tx.clone());
            open_pipes += 1;
        }
        drop(tx);

        let deadline = timeout
            .filter(|t| *t > 0.0)
            .map(|t| Instant::now() + Duration::from_secs_f64(t));
        let mut timed_out = false;
        let mut killed = false;
        let mut exited_at: Option<Instant> = None;
        let mut status = None;

        loop {
            if !killed {
                let aborted = signal.as_ref().is_some_and(AbortSignal::aborted);
                let expired = deadline.is_some_and(|d| Instant::now() >= d);
                if aborted || expired {
                    timed_out = expired && !aborted;
                    let _ = child.start_kill();
                    killed = true;
                }
            }
            if status.is_none() {
                match child.try_wait() {
                    Ok(Some(s)) => {
                        status = Some(s);
                        exited_at = Some(Instant::now());
                    }
                    Ok(None) => {}
                    Err(e) => {
                        untrack_detached_child_pid(pid);
                        return Err(e.into());
                    }
                }
            }
            if open_pipes == 0 && status.is_some() {
                break;
            }
            if exited_at.is_some_and(|t| t.elapsed() >= EXIT_STDIO_GRACE) {
                break;
            }
            match rx.recv_timeout(POLL) {
                Ok(Some(chunk)) => on_data(&chunk),
                Ok(None) => open_pipes -= 1,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => open_pipes = 0,
            }
        }
        // Whatever was already read before we stopped waiting.
        while let Ok(Some(chunk)) = rx.try_recv() {
            on_data(&chunk);
        }
        untrack_detached_child_pid(pid);

        if signal.as_ref().is_some_and(AbortSignal::aborted) {
            return Err("aborted".into());
        }
        if timed_out {
            return Err(format!("timeout:{}", js_number(timeout.unwrap_or_default())).into());
        }
        Ok(status.and_then(|s| s.code()))
    }
}
