//! Plain-text clipboard writes, hoocode `utils/clipboard.ts`.
//!
//! Direct writes first (the native clipboard off Linux, then `pbcopy`, `clip`,
//! `termux-clipboard-set`, `wl-copy`, `xclip`/`xsel`), and OSC 52 through the
//! terminal when nothing local worked or the session is remote. Everything
//! that touches the machine goes through [`ClipboardHost`], so the order of
//! attempts is testable; [`SystemClipboardHost`] is the real one.

use std::io::Write as _;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use base64::Engine as _;

/// OSC 52 payloads past this (base64 bytes) are not sent: very large ones
/// desynchronize terminal rendering.
pub const MAX_OSC52_ENCODED_LENGTH: usize = 100_000;

/// Timeout for the platform tools (`execSync`'s `timeout: 5000`).
const EXEC_TIMEOUT: Duration = Duration::from_millis(5000);

/// The OS as `process.platform` names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Darwin,
    Win32,
    Linux,
    Other,
}

impl Platform {
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::Darwin
        } else if cfg!(windows) {
            Self::Win32
        } else if cfg!(target_os = "linux") {
            Self::Linux
        } else {
            Self::Other
        }
    }
}

/// What the copy needs from the machine.
pub trait ClipboardHost {
    fn platform(&self) -> Platform;
    /// An environment variable, `None` when unset.
    fn env(&self, name: &str) -> Option<String>;
    /// The native clipboard write; `None` when there is no native clipboard.
    fn native_set_text(&self, text: &str) -> Option<Result<(), String>>;
    /// `execSync(command, { input })`: a shell command fed `input`, to exit 0.
    fn exec(&self, command: &str, input: &str) -> Result<(), String>;
    /// Spawn `program` detached, write `input` to its stdin, don't wait.
    fn spawn_detached(&self, program: &str, input: &str) -> Result<(), String>;
    /// Write to the terminal (OSC 52).
    fn write_stdout(&self, data: &str);
}

/// `isWaylandSession`.
pub fn is_wayland_session(env: impl Fn(&str) -> Option<String>) -> bool {
    env("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty())
        || env("XDG_SESSION_TYPE").as_deref() == Some("wayland")
}

fn env_set(host: &dyn ClipboardHost, name: &str) -> bool {
    host.env(name).is_some_and(|v| !v.is_empty())
}

fn is_remote_session(host: &dyn ClipboardHost) -> bool {
    ["SSH_CONNECTION", "SSH_CLIENT", "MOSH_CONNECTION"]
        .iter()
        .any(|name| env_set(host, name))
}

/// The OSC 52 sequence for `text`, or `None` when it is too large to send.
pub fn osc52_sequence(text: &str) -> Option<String> {
    let encoded = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
    (encoded.len() <= MAX_OSC52_ENCODED_LENGTH).then(|| format!("\x1b]52;c;{encoded}\x07"))
}

fn emit_osc52(host: &dyn ClipboardHost, text: &str) -> bool {
    match osc52_sequence(text) {
        Some(sequence) => {
            host.write_stdout(&sequence);
            true
        }
        None => false,
    }
}

fn copy_to_x11_clipboard(host: &dyn ClipboardHost, text: &str) -> Result<(), String> {
    host.exec("xclip -selection clipboard", text)
        .or_else(|_| host.exec("xsel --clipboard --input", text))
}

/// The platform tools, in the pin's order.
fn copy_with_platform_tools(host: &dyn ClipboardHost, text: &str) -> Result<bool, String> {
    match host.platform() {
        Platform::Darwin => host.exec("pbcopy", text).map(|_| true),
        Platform::Win32 => host.exec("clip", text).map(|_| true),
        _ => {
            if env_set(host, "TERMUX_VERSION") && host.exec("termux-clipboard-set", text).is_ok() {
                return Ok(true);
            }
            let has_wayland_display = env_set(host, "WAYLAND_DISPLAY");
            let has_x11_display = env_set(host, "DISPLAY");
            if is_wayland_session(|name| host.env(name)) && has_wayland_display {
                let wl_copy = host
                    .exec("which wl-copy", "")
                    .and_then(|_| host.spawn_detached("wl-copy", text));
                match wl_copy {
                    Ok(()) => Ok(true),
                    Err(_) if has_x11_display => copy_to_x11_clipboard(host, text).map(|_| true),
                    Err(_) => Ok(false),
                }
            } else if has_x11_display {
                copy_to_x11_clipboard(host, text).map(|_| true)
            } else {
                Ok(false)
            }
        }
    }
}

/// `copyToClipboard`.
pub fn copy_to_clipboard(host: &dyn ClipboardHost, text: &str) -> Result<(), String> {
    let mut copied = false;

    // Direct writes first. On Linux the native clipboard is skipped: it does
    // not keep selection ownership, so the platform tools below do it.
    if host.platform() != Platform::Linux {
        if let Some(Ok(())) = host.native_set_text(text) {
            copied = true;
        }
    }

    let remote = is_remote_session(host);
    if copied && !remote {
        return Ok(());
    }

    if !copied {
        // A tool that fails falls through to OSC 52.
        copied = copy_with_platform_tools(host, text).unwrap_or(false);
    }

    if remote || !copied {
        let osc52_copied = emit_osc52(host, text);
        copied = copied || osc52_copied;
    }

    if copied {
        Ok(())
    } else {
        Err("Failed to copy to clipboard".into())
    }
}

/// A native clipboard text write.
pub type NativeWriter = Box<dyn Fn(&str) -> Result<(), String> + Send>;

/// The machine itself. `native` is the platform clipboard write, supplied by
/// the crate that owns the clipboard library.
pub struct SystemClipboardHost {
    pub native: Option<NativeWriter>,
}

impl SystemClipboardHost {
    /// `clipboard-native.ts` loads the addon only with a display to talk to
    /// (and never on Termux).
    pub fn has_native_display() -> bool {
        let set = |name: &str| std::env::var(name).is_ok_and(|v| !v.is_empty());
        !set("TERMUX_VERSION")
            && (Platform::current() != Platform::Linux || set("DISPLAY") || set("WAYLAND_DISPLAY"))
    }
}

fn shell_command(command: &str) -> Command {
    if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.args(["/C", command]);
        c
    } else {
        let mut c = Command::new("/bin/sh");
        c.args(["-c", command]);
        c
    }
}

impl ClipboardHost for SystemClipboardHost {
    fn platform(&self) -> Platform {
        Platform::current()
    }

    fn env(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }

    fn native_set_text(&self, text: &str) -> Option<Result<(), String>> {
        self.native.as_ref().map(|set| set(text))
    }

    fn exec(&self, command: &str, input: &str) -> Result<(), String> {
        let mut child = shell_command(command)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| e.to_string())?;
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(input.as_bytes());
        }
        let started = Instant::now();
        loop {
            match child.try_wait().map_err(|e| e.to_string())? {
                Some(status) if status.success() => return Ok(()),
                Some(status) => return Err(format!("{command} exited with {status}")),
                None if started.elapsed() >= EXEC_TIMEOUT => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!("{command} timed out"));
                }
                None => std::thread::sleep(Duration::from_millis(10)),
            }
        }
    }

    fn spawn_detached(&self, program: &str, input: &str) -> Result<(), String> {
        let mut child = Command::new(program)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| e.to_string())?;
        if let Some(mut stdin) = child.stdin.take() {
            // EPIPE when the tool exits early is ignored.
            let _ = stdin.write_all(input.as_bytes());
        }
        // Reap it off-thread; wl-copy daemonizes and keeps ownership.
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(())
    }

    fn write_stdout(&self, data: &str) {
        let mut stdout = std::io::stdout();
        let _ = stdout.write_all(data.as_bytes());
        let _ = stdout.flush();
    }
}
