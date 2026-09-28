//! Keep protocol stdout clean: port of hoocode `core/output-guard.ts`.
//!
//! hoocode monkeypatches `process.stdout.write` so stray writes (extensions,
//! `console.log`) land on stderr while rpc / json / print output goes through
//! `writeRawStdout`. Rust cannot intercept `println!`, so the redirect is
//! explicit: incidental output goes through [`write_stdout`], which honors the
//! takeover, and protocol output through [`write_raw_stdout`].

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};

static TAKEN_OVER: AtomicBool = AtomicBool::new(false);

/// `takeOverStdout`: from now on [`write_stdout`] goes to stderr.
pub fn take_over_stdout() {
    TAKEN_OVER.store(true, Ordering::SeqCst);
}

/// `restoreStdout`.
pub fn restore_stdout() {
    TAKEN_OVER.store(false, Ordering::SeqCst);
}

/// `isStdoutTakenOver`.
pub fn is_stdout_taken_over() -> bool {
    TAKEN_OVER.load(Ordering::SeqCst)
}

/// Incidental output (the patched `process.stdout.write`): stdout normally,
/// stderr while stdout is taken over.
pub fn write_stdout(text: &str) {
    if is_stdout_taken_over() {
        let _ = std::io::stderr().lock().write_all(text.as_bytes());
    } else {
        let _ = std::io::stdout().lock().write_all(text.as_bytes());
    }
}

/// `writeRawStdout`: protocol output, always to the real stdout.
pub fn write_raw_stdout(text: &str) {
    let _ = std::io::stdout().lock().write_all(text.as_bytes());
}

/// `flushRawStdout`.
pub fn flush_raw_stdout() -> std::io::Result<()> {
    std::io::stdout().lock().flush()
}
