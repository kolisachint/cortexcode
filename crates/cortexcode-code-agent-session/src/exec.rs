//! Shared command execution for extensions and custom tools: port of hoocode
//! `core/exec.ts` (`execCommand`) with `utils/child-process.ts`'s
//! `waitForChildProcess` exit handling.

use cortexcode_ai_types::AbortSignal;
use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt};

/// After `exit`, how long to wait for stdout/stderr to end before we stop
/// tracking handles inherited by detached descendants (`EXIT_STDIO_GRACE_MS`).
const EXIT_STDIO_GRACE: Duration = Duration::from_millis(100);

/// SIGTERM is escalated to SIGKILL after this long.
const FORCE_KILL_AFTER: Duration = Duration::from_secs(5);

/// `ExecOptions`.
#[derive(Clone, Default)]
pub struct ExecOptions {
    /// Cancels the command.
    pub signal: Option<AbortSignal>,
    /// Timeout in milliseconds; `0` or `None` means none.
    pub timeout_ms: Option<u64>,
}

/// `ExecResult`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ExecResult {
    pub stdout: String,
    pub stderr: String,
    /// Exit code. A signal-terminated process reports `0` (Node's `code ?? 0`),
    /// a spawn failure reports `1`.
    pub code: i32,
    /// True when the timeout or the abort signal killed the command.
    pub killed: bool,
}

type Buf = Arc<Mutex<Vec<u8>>>;

async fn drain(mut stream: impl AsyncRead + Unpin, buf: Buf) {
    let mut chunk = [0u8; 8192];
    loop {
        match stream.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => buf.lock().unwrap().extend_from_slice(&chunk[..n]),
        }
    }
}

fn decode(buf: &Buf) -> String {
    // `TextDecoder` (streaming, then flushed): invalid bytes become U+FFFD.
    String::from_utf8_lossy(&buf.lock().unwrap()).into_owned()
}

#[cfg(unix)]
fn send_signal(pid: u32, signal: i32) {
    // SAFETY: a plain kill(2) on the pid of a child we spawned.
    unsafe {
        libc::kill(pid as i32, signal);
    }
}

/// Execute `command` with `args` (no shell) in `cwd` and collect its output.
/// Never fails: a spawn error resolves with `code: 1`, like hoocode.
pub async fn exec_command(
    command: &str,
    args: &[String],
    cwd: &Path,
    options: ExecOptions,
) -> ExecResult {
    let stdout_buf: Buf = Arc::default();
    let stderr_buf: Buf = Arc::default();
    let mut child = match tokio::process::Command::new(command)
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => {
            return ExecResult {
                code: 1,
                ..ExecResult::default()
            }
        }
    };
    let mut readers = Vec::new();
    if let Some(out) = child.stdout.take() {
        readers.push(tokio::spawn(drain(out, stdout_buf.clone())));
    }
    if let Some(err) = child.stderr.take() {
        readers.push(tokio::spawn(drain(err, stderr_buf.clone())));
    }

    let pid = child.id();
    let mut killed = false;
    let mut force_kill_at: Option<tokio::time::Instant> = None;
    let signal = options.signal.clone();
    let deadline = options
        .timeout_ms
        .filter(|t| *t > 0)
        .map(|t| tokio::time::Instant::now() + Duration::from_millis(t));

    // `killProcess`: SIGTERM once, SIGKILL five seconds later if still alive.
    let kill = |killed: &mut bool, force_kill_at: &mut Option<tokio::time::Instant>| {
        if *killed {
            return;
        }
        *killed = true;
        #[cfg(unix)]
        if let Some(pid) = pid {
            send_signal(pid, libc::SIGTERM);
        }
        *force_kill_at = Some(tokio::time::Instant::now() + FORCE_KILL_AFTER);
    };
    if signal.as_ref().is_some_and(AbortSignal::aborted) {
        kill(&mut killed, &mut force_kill_at);
    }

    let status = loop {
        tokio::select! {
            status = child.wait() => break status,
            _ = async {
                match &signal {
                    Some(s) if !killed => s.cancelled().await,
                    _ => std::future::pending().await,
                }
            } => kill(&mut killed, &mut force_kill_at),
            _ = async {
                match deadline {
                    Some(d) if !killed => tokio::time::sleep_until(d).await,
                    _ => std::future::pending().await,
                }
            } => kill(&mut killed, &mut force_kill_at),
            _ = async {
                match force_kill_at {
                    Some(at) => tokio::time::sleep_until(at).await,
                    None => std::future::pending().await,
                }
            } => {
                force_kill_at = None;
                let _ = child.start_kill();
            }
        }
    };

    // Wait briefly for stdio to end; a detached descendant may hold the pipes
    // open, in which case we stop tracking them.
    let aborts: Vec<_> = readers.iter().map(|r| r.abort_handle()).collect();
    let all_ended = async {
        for reader in readers {
            let _ = reader.await;
        }
    };
    if tokio::time::timeout(EXIT_STDIO_GRACE, all_ended)
        .await
        .is_err()
    {
        for handle in aborts {
            handle.abort();
        }
    }

    let code = match status {
        Ok(status) => status.code().unwrap_or(0),
        Err(_) => 1,
    };
    ExecResult {
        stdout: decode(&stdout_buf),
        stderr: decode(&stderr_buf),
        code,
        killed,
    }
}
