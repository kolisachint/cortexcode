//! Ctrl+Z: suspend to the background (`InteractiveMode.handleCtrlZ`).
//!
//! The TUI gives the terminal back, SIGINT is ignored while stopped (so a
//! Ctrl+C typed at the shell does not kill the backgrounded process), and
//! SIGTSTP goes to the process group. hoocode restores the TUI from a SIGCONT
//! handler; here `kill` returns once the process has been continued, so the
//! restore follows it directly.
//!
//! The steps go through [`SuspendOps`] so tests can run the sequence without
//! stopping the test process.

/// What suspending touches.
pub trait SuspendOps {
    fn stop_ui(&mut self);
    /// Restart the UI and force a full redraw.
    fn restart_ui(&mut self);
    fn ignore_sigint(&mut self);
    fn restore_sigint(&mut self);
    /// `process.kill(0, "SIGTSTP")`: returns after the process is continued.
    fn stop_process_group(&mut self) -> Result<(), String>;
}

/// What happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SuspendOutcome {
    /// Suspended and resumed; the UI is back.
    Resumed,
    /// Not supported here; show this status instead.
    Unsupported(&'static str),
    /// The signal could not be sent. The temporary SIGINT handling is undone
    /// and the UI is still stopped.
    Failed(String),
}

/// `handleCtrlZ`.
pub fn suspend_to_background(ops: &mut dyn SuspendOps, windows: bool) -> SuspendOutcome {
    if windows {
        return SuspendOutcome::Unsupported("Suspend to background is not supported on Windows");
    }
    ops.ignore_sigint();
    ops.stop_ui();
    if let Err(error) = ops.stop_process_group() {
        ops.restore_sigint();
        return SuspendOutcome::Failed(error);
    }
    ops.restore_sigint();
    ops.restart_ui();
    SuspendOutcome::Resumed
}

/// The process half of [`SuspendOps`] on Unix: SIGINT ignored and restored
/// with `sigaction`, SIGTSTP to the process group.
#[cfg(unix)]
#[derive(Default)]
pub struct ProcessSignals {
    previous_sigint: Option<libc::sigaction>,
}

#[cfg(unix)]
impl ProcessSignals {
    pub fn ignore_sigint(&mut self) {
        // SAFETY: plain sigaction calls with zeroed, then filled, structs.
        unsafe {
            let mut ignore: libc::sigaction = std::mem::zeroed();
            ignore.sa_sigaction = libc::SIG_IGN;
            libc::sigemptyset(&mut ignore.sa_mask);
            let mut previous: libc::sigaction = std::mem::zeroed();
            if libc::sigaction(libc::SIGINT, &ignore, &mut previous) == 0 {
                self.previous_sigint = Some(previous);
            }
        }
    }

    pub fn restore_sigint(&mut self) {
        if let Some(previous) = self.previous_sigint.take() {
            // SAFETY: restores the action saved by `ignore_sigint`.
            unsafe {
                libc::sigaction(libc::SIGINT, &previous, std::ptr::null_mut());
            }
        }
    }

    pub fn stop_process_group(&mut self) -> Result<(), String> {
        // SAFETY: kill(2) on our own process group.
        if unsafe { libc::kill(0, libc::SIGTSTP) } == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error().to_string())
        }
    }
}
