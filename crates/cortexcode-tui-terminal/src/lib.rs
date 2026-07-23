//! Terminal abstraction for the cortex TUI.
//!
//! Ported from TypeScript `@kolisachint/hoocode-tui` -> `terminal.ts` /
//! `stdin-buffer.ts`. Raw-mode toggling and dimension queries are delegated
//! to `crossterm`; escape sequences that `crossterm` has no dedicated API
//! for (bracketed paste, Kitty keyboard protocol negotiation, OSC progress /
//! title) are written directly, matching the byte sequences hoocode used.

mod stdin_buffer;

pub use stdin_buffer::{StdinBuffer, StdinBufferOptions, StdinEvent};

use std::env;
use std::fs::OpenOptions;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const TERMINAL_PROGRESS_ACTIVE_SEQUENCE: &str = "\x1b]9;4;3\x07";
const TERMINAL_PROGRESS_CLEAR_SEQUENCE: &str = "\x1b]9;4;0;\x07";
const TERMINAL_PROGRESS_KEEPALIVE: Duration = Duration::from_millis(1000);
const KITTY_QUERY_FALLBACK_DELAY: Duration = Duration::from_millis(150);

/// Minimal terminal interface for the TUI.
///
/// Input/resize handling uses callbacks (rather than returning a stream)
/// to mirror the original event-driven `start(onInput, onResize)` API.
pub trait Terminal: Send {
    fn start(&mut self, on_input: Box<dyn FnMut(&str) + Send>, on_resize: Box<dyn FnMut() + Send>);
    fn stop(&mut self);
    fn drain_input(&mut self, max: Duration, idle: Duration);
    fn write(&mut self, data: &str);
    fn columns(&self) -> u16;
    fn rows(&self) -> u16;
    fn kitty_protocol_active(&self) -> bool;
    fn move_by(&mut self, lines: i32);
    fn hide_cursor(&mut self);
    fn show_cursor(&mut self);
    fn clear_line(&mut self);
    fn clear_from_cursor(&mut self);
    fn clear_screen(&mut self);
    fn set_title(&mut self, title: &str);
    fn set_progress(&mut self, active: bool);
}

/// Resolves terminal dimensions the same way hoocode's `columns`/`rows`
/// getters do: measured size, then an env var override, then a default.
fn resolve_dimension(measured: Option<u16>, env_var: Option<&str>, default: u16) -> u16 {
    if let Some(m) = measured {
        if m != 0 {
            return m;
        }
    }
    if let Some(v) = env_var {
        if let Ok(parsed) = v.parse::<u16>() {
            if parsed != 0 {
                return parsed;
            }
        }
    }
    default
}

/// Matches a Kitty keyboard-protocol query response: `\x1b[?<flags>u`.
fn parse_kitty_query_response(sequence: &str) -> bool {
    let Some(rest) = sequence.strip_prefix("\x1b[?") else {
        return false;
    };
    let Some(digits) = rest.strip_suffix('u') else {
        return false;
    };
    !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit())
}

fn resolve_write_log_path() -> Option<PathBuf> {
    let env = env::var("CORTEXCODE_TUI_WRITE_LOG").ok()?;
    if env.is_empty() {
        return None;
    }
    let path = PathBuf::from(&env);
    if path.is_dir() {
        let now = std::time::SystemTime::now();
        let ts = humantime_like_timestamp(now);
        Some(path.join(format!("tui-{ts}-{}.log", std::process::id())))
    } else {
        Some(path)
    }
}

fn humantime_like_timestamp(_now: std::time::SystemTime) -> String {
    // Coarse, dependency-free timestamp (no chrono dependency): seconds since epoch.
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    secs.to_string()
}

/// Real terminal backed by process stdin/stdout, raw mode via `crossterm`.
pub struct ProcessTerminal {
    was_raw: bool,
    started: bool,
    kitty_protocol_active: Arc<AtomicBool>,
    modify_other_keys_active: Arc<AtomicBool>,
    progress_active: Arc<AtomicBool>,
    forwarding: Arc<AtomicBool>,
    last_input_at: Arc<Mutex<Instant>>,
    reader_thread: Option<thread::JoinHandle<()>>,
    resize_thread: Option<thread::JoinHandle<()>>,
    stop_signal: Arc<AtomicBool>,
    progress_thread: Option<thread::JoinHandle<()>>,
    last_cols: Arc<AtomicU16>,
    last_rows: Arc<AtomicU16>,
    write_log_path: Option<PathBuf>,
}

impl Default for ProcessTerminal {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessTerminal {
    pub fn new() -> Self {
        Self {
            was_raw: false,
            started: false,
            kitty_protocol_active: Arc::new(AtomicBool::new(false)),
            modify_other_keys_active: Arc::new(AtomicBool::new(false)),
            progress_active: Arc::new(AtomicBool::new(false)),
            forwarding: Arc::new(AtomicBool::new(true)),
            last_input_at: Arc::new(Mutex::new(Instant::now())),
            reader_thread: None,
            resize_thread: None,
            stop_signal: Arc::new(AtomicBool::new(false)),
            progress_thread: None,
            last_cols: Arc::new(AtomicU16::new(0)),
            last_rows: Arc::new(AtomicU16::new(0)),
            write_log_path: resolve_write_log_path(),
        }
    }

    fn raw_write(&self, data: &str) {
        let _ = io::stdout().write_all(data.as_bytes());
        let _ = io::stdout().flush();
        if let Some(path) = &self.write_log_path {
            if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
                let _ = f.write_all(data.as_bytes());
            }
        }
    }
}

impl Terminal for ProcessTerminal {
    fn start(
        &mut self,
        mut on_input: Box<dyn FnMut(&str) + Send>,
        mut on_resize: Box<dyn FnMut() + Send>,
    ) {
        if self.started {
            return;
        }
        self.started = true;
        self.was_raw = crossterm::terminal::is_raw_mode_enabled().unwrap_or(false);
        let _ = crossterm::terminal::enable_raw_mode();

        // Windows: Enable virtual terminal input processing.
        // This allows Windows to process escape sequences properly,
        // including mouse events, bracketed paste, and Kitty keyboard protocol.
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::System::Console::{
                GetConsoleMode, SetConsoleMode, ENABLE_VIRTUAL_TERMINAL_INPUT,
            };

            let stdin = io::stdin();
            let handle = stdin.as_raw_handle();
            let mut mode: u32 = 0;
            unsafe {
                if GetConsoleMode(handle, &mut mode) != 0 {
                    mode |= ENABLE_VIRTUAL_TERMINAL_INPUT;
                    SetConsoleMode(handle, mode);
                }
            }
        }

        // Bracketed paste mode.
        self.raw_write("\x1b[?2004h");

        self.stop_signal.store(false, Ordering::SeqCst);
        self.forwarding.store(true, Ordering::SeqCst);

        // Query + (fallback) enable Kitty keyboard protocol / modifyOtherKeys.
        self.raw_write("\x1b[?u");
        {
            let kitty_active = self.kitty_protocol_active.clone();
            let modify_active = self.modify_other_keys_active.clone();
            let stop_signal = self.stop_signal.clone();
            thread::spawn(move || {
                thread::sleep(KITTY_QUERY_FALLBACK_DELAY);
                if stop_signal.load(Ordering::SeqCst) {
                    return;
                }
                if !kitty_active.load(Ordering::SeqCst) && !modify_active.load(Ordering::SeqCst) {
                    let _ = io::stdout().write_all(b"\x1b[>4;2m");
                    let _ = io::stdout().flush();
                    modify_active.store(true, Ordering::SeqCst);
                }
            });
        }

        // Reader thread: parses raw stdin bytes into complete sequences via
        // StdinBuffer and forwards them to `on_input`, intercepting the
        // Kitty protocol query response before it reaches the caller.
        let stop_signal = self.stop_signal.clone();
        let forwarding = self.forwarding.clone();
        let kitty_active = self.kitty_protocol_active.clone();
        let last_input_at = self.last_input_at.clone();
        self.reader_thread = Some(thread::spawn(move || {
            let mut buf = StdinBuffer::new(StdinBufferOptions::default());
            let mut chunk = [0u8; 4096];
            let mut stdin = io::stdin();
            loop {
                if stop_signal.load(Ordering::SeqCst) {
                    break;
                }
                let n = match stdin.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(_) => break,
                };
                *last_input_at.lock().unwrap() = Instant::now();

                let text = if n == 1 && chunk[0] > 127 {
                    format!("\x1b{}", (chunk[0] - 128) as char)
                } else {
                    String::from_utf8_lossy(&chunk[..n]).into_owned()
                };

                let events = buf.process(&text);
                for event in events {
                    if !forwarding.load(Ordering::SeqCst) {
                        continue;
                    }
                    match event {
                        StdinEvent::Data(seq) => {
                            if !kitty_active.load(Ordering::SeqCst)
                                && parse_kitty_query_response(&seq)
                            {
                                kitty_active.store(true, Ordering::SeqCst);
                                let _ = io::stdout().write_all(b"\x1b[>7u");
                                let _ = io::stdout().flush();
                                continue;
                            }
                            on_input(&seq);
                        }
                        StdinEvent::Paste(content) => {
                            on_input(&format!("\x1b[200~{content}\x1b[201~"));
                        }
                    }
                }
            }
        }));

        // Resize watcher: crossterm has no cross-platform SIGWINCH hook, so
        // poll terminal size and fire `on_resize` on change.
        let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
        self.last_cols.store(cols, Ordering::SeqCst);
        self.last_rows.store(rows, Ordering::SeqCst);
        let stop_signal = self.stop_signal.clone();
        let last_cols = self.last_cols.clone();
        let last_rows = self.last_rows.clone();
        self.resize_thread = Some(thread::spawn(move || loop {
            if stop_signal.load(Ordering::SeqCst) {
                break;
            }
            thread::sleep(Duration::from_millis(100));
            if let Ok((c, r)) = crossterm::terminal::size() {
                if c != last_cols.load(Ordering::SeqCst) || r != last_rows.load(Ordering::SeqCst) {
                    last_cols.store(c, Ordering::SeqCst);
                    last_rows.store(r, Ordering::SeqCst);
                    on_resize();
                }
            }
        }));
    }

    fn stop(&mut self) {
        if !self.started {
            return;
        }
        self.started = false;
        self.stop_signal.store(true, Ordering::SeqCst);
        self.forwarding.store(false, Ordering::SeqCst);

        if self.progress_active.swap(false, Ordering::SeqCst) {
            self.raw_write(TERMINAL_PROGRESS_CLEAR_SEQUENCE);
        }
        if let Some(handle) = self.progress_thread.take() {
            let _ = handle.join();
        }

        self.raw_write("\x1b[?2004l");

        if self.kitty_protocol_active.swap(false, Ordering::SeqCst) {
            self.raw_write("\x1b[<u");
        }
        if self.modify_other_keys_active.swap(false, Ordering::SeqCst) {
            self.raw_write("\x1b[>4;0m");
        }

        // Reader/resize threads are left detached: they read from stdin,
        // which cannot be interrupted from another thread without OS-level
        // signalling, so they exit naturally once no more input arrives
        // (matching the fire-and-forget cleanup semantics of a background
        // Node listener being removed).
        self.reader_thread = None;
        self.resize_thread = None;

        let _ = crossterm::terminal::disable_raw_mode();
        if self.was_raw {
            let _ = crossterm::terminal::enable_raw_mode();
        }
    }

    fn drain_input(&mut self, max: Duration, idle: Duration) {
        if self.kitty_protocol_active.swap(false, Ordering::SeqCst) {
            self.raw_write("\x1b[<u");
        }
        if self.modify_other_keys_active.swap(false, Ordering::SeqCst) {
            self.raw_write("\x1b[>4;0m");
        }

        self.forwarding.store(false, Ordering::SeqCst);
        let start = Instant::now();
        loop {
            let last = *self.last_input_at.lock().unwrap();
            if start.elapsed() >= max {
                break;
            }
            if last.elapsed() >= idle {
                break;
            }
            thread::sleep(idle.min(Duration::from_millis(10)));
        }
        self.forwarding.store(true, Ordering::SeqCst);
    }

    fn write(&mut self, data: &str) {
        self.raw_write(data);
    }

    fn columns(&self) -> u16 {
        let measured = crossterm::terminal::size().ok().map(|(c, _)| c);
        resolve_dimension(measured, env::var("COLUMNS").ok().as_deref(), 80)
    }

    fn rows(&self) -> u16 {
        let measured = crossterm::terminal::size().ok().map(|(_, r)| r);
        resolve_dimension(measured, env::var("LINES").ok().as_deref(), 24)
    }

    fn kitty_protocol_active(&self) -> bool {
        self.kitty_protocol_active.load(Ordering::SeqCst)
    }

    fn move_by(&mut self, lines: i32) {
        match lines.cmp(&0) {
            std::cmp::Ordering::Greater => self.raw_write(&format!("\x1b[{lines}B")),
            std::cmp::Ordering::Less => self.raw_write(&format!("\x1b[{}A", -lines)),
            std::cmp::Ordering::Equal => {}
        }
    }

    fn hide_cursor(&mut self) {
        self.raw_write("\x1b[?25l");
    }

    fn show_cursor(&mut self) {
        self.raw_write("\x1b[?25h");
    }

    fn clear_line(&mut self) {
        self.raw_write("\x1b[K");
    }

    fn clear_from_cursor(&mut self) {
        self.raw_write("\x1b[J");
    }

    fn clear_screen(&mut self) {
        self.raw_write("\x1b[2J\x1b[H");
    }

    fn set_title(&mut self, title: &str) {
        self.raw_write(&format!("\x1b]0;{title}\x07"));
    }

    fn set_progress(&mut self, active: bool) {
        if active {
            self.raw_write(TERMINAL_PROGRESS_ACTIVE_SEQUENCE);
            if !self.progress_active.swap(true, Ordering::SeqCst) {
                let stop_signal = self.stop_signal.clone();
                let progress_active = self.progress_active.clone();
                self.progress_thread = Some(thread::spawn(move || loop {
                    thread::sleep(TERMINAL_PROGRESS_KEEPALIVE);
                    if stop_signal.load(Ordering::SeqCst) || !progress_active.load(Ordering::SeqCst)
                    {
                        break;
                    }
                    let _ = io::stdout().write_all(TERMINAL_PROGRESS_ACTIVE_SEQUENCE.as_bytes());
                    let _ = io::stdout().flush();
                }));
            }
        } else {
            self.progress_active.store(false, Ordering::SeqCst);
            if let Some(handle) = self.progress_thread.take() {
                let _ = handle.join();
            }
            self.raw_write(TERMINAL_PROGRESS_CLEAR_SEQUENCE);
        }
    }
}

impl Drop for ProcessTerminal {
    fn drop(&mut self) {
        if self.started {
            self.stop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_dimension_prefers_measured_value() {
        assert_eq!(resolve_dimension(Some(120), Some("999"), 80), 120);
    }

    #[test]
    fn resolve_dimension_falls_back_to_env_var() {
        assert_eq!(resolve_dimension(None, Some("123"), 80), 123);
        assert_eq!(resolve_dimension(Some(0), Some("45"), 24), 45);
    }

    #[test]
    fn resolve_dimension_falls_back_to_default() {
        assert_eq!(resolve_dimension(None, None, 80), 80);
        assert_eq!(resolve_dimension(None, Some("not-a-number"), 80), 80);
    }

    #[test]
    fn kitty_query_response_matching() {
        assert!(parse_kitty_query_response("\x1b[?7u"));
        assert!(parse_kitty_query_response("\x1b[?0u"));
        assert!(!parse_kitty_query_response("\x1b[?u"));
        assert!(!parse_kitty_query_response("\x1b[97u"));
        assert!(!parse_kitty_query_response("\x1b[A"));
    }

    #[test]
    fn new_terminal_is_not_kitty_active_by_default() {
        let term = ProcessTerminal::new();
        assert!(!term.kitty_protocol_active());
    }
}
