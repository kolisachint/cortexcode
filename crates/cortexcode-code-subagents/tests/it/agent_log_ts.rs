//! Port of the pin's `test/subagent-dispatch-tui-output.test.ts`: operational
//! log lines (`agentLog`, the pool's `[DISPATCH]` line) reach stderr only
//! while no TUI owns the terminal.
//!
//! Stderr is captured at fd 2, so the cases share one lock. The TS
//! end-to-end case (a dispatch under a live differential renderer painting
//! one `Agent [explore]` row, not two) runs for real in the L2 scenario
//! `subagent-command`: a real child dispatched from the interactive TUI,
//! every rendered row compared with hoocode.
#![cfg(unix)]

use std::io::{Read, Seek, Write};
use std::os::fd::AsRawFd;
use std::sync::Mutex;
use std::time::Duration;

use cortexcode_code_subagents::agent_log::{
    agent_log, is_terminal_owned_by_tui, set_terminal_owned_by_tui,
};
use cortexcode_code_subagents::pool::{DispatchOptions, SubagentPool, SubagentPoolOptions};

static FD2: Mutex<()> = Mutex::new(());

/// Everything written to fd 2 while `run` executes.
fn capture_stderr(run: impl FnOnce()) -> String {
    let mut file = tempfile::tempfile().unwrap();
    let _ = std::io::stderr().flush();
    // SAFETY: fd 2 is duplicated, pointed at the temp file, then restored.
    let saved = unsafe { libc::dup(2) };
    assert!(saved >= 0);
    unsafe { libc::dup2(file.as_raw_fd(), 2) };
    run();
    let _ = std::io::stderr().flush();
    unsafe {
        libc::dup2(saved, 2);
        libc::close(saved);
    }
    let mut out = String::new();
    file.rewind().unwrap();
    file.read_to_string(&mut out).unwrap();
    out
}

fn with_tui_owned<T>(owned: bool, run: impl FnOnce() -> T) -> T {
    let _g = FD2.lock().unwrap_or_else(|e| e.into_inner());
    set_terminal_owned_by_tui(owned);
    let out = run();
    set_terminal_owned_by_tui(false);
    out
}

/// A pool that spawns a trivial child, so a dispatch is real but cheap.
fn dispatch_output() -> String {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _enter = runtime.enter();
    let cwd = tempfile::tempdir().unwrap();
    let pool = SubagentPool::new(SubagentPoolOptions {
        executable: "/bin/true".into(),
        cwd: Some(cwd.path().to_path_buf()),
        max_concurrency: Some(1),
        ..Default::default()
    });
    let output = capture_stderr(|| {
        let _ = pool.dispatch_detached(
            "find the bug",
            DispatchOptions {
                force_agent: Some("explore".into()),
                ..Default::default()
            },
        );
        std::thread::sleep(Duration::from_millis(200));
    });
    pool.dispose();
    output
}

#[test]
fn agent_log_writes_to_stderr_when_no_tui_owns_the_terminal() {
    let output = with_tui_owned(false, || {
        capture_stderr(|| agent_log("[DISPATCH] agent=explore"))
    });
    assert!(output.contains("[DISPATCH] agent=explore"), "{output:?}");
}

#[test]
fn agent_log_stays_silent_while_a_tui_owns_the_terminal() {
    let (output, owned) = with_tui_owned(true, || {
        (
            capture_stderr(|| agent_log("[DISPATCH] agent=explore")),
            is_terminal_owned_by_tui(),
        )
    });
    assert_eq!(output, "");
    assert!(owned);
}

#[test]
fn the_pool_logs_the_dispatch_line_in_non_interactive_modes() {
    let output = with_tui_owned(false, dispatch_output);
    assert!(output.contains("[DISPATCH] agent=explore"), "{output:?}");
    assert!(output.contains("depth=1"), "{output:?}");
}

#[test]
fn the_pool_writes_nothing_while_the_interactive_tui_owns_the_terminal() {
    let output = with_tui_owned(true, dispatch_output);
    assert_eq!(output, "");
}
