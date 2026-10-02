//! Port of the pin's `test/interactive-mode-suspend.test.ts`
//! (`InteractiveMode.handleCtrlZ`) over a recording [`SuspendOps`].
//! hoocode's keep-alive interval and SIGCONT listener have no counterpart:
//! `kill` returns once the process is continued, and the restore follows it.

use cortexcode_code_tui_app::suspend::{suspend_to_background, SuspendOps, SuspendOutcome};

#[derive(Default)]
struct Recorder {
    calls: Vec<&'static str>,
    fail: bool,
}

impl SuspendOps for Recorder {
    fn stop_ui(&mut self) {
        self.calls.push("stop_ui");
    }
    fn restart_ui(&mut self) {
        self.calls.push("restart_ui");
    }
    fn ignore_sigint(&mut self) {
        self.calls.push("ignore_sigint");
    }
    fn restore_sigint(&mut self) {
        self.calls.push("restore_sigint");
    }
    fn stop_process_group(&mut self) -> Result<(), String> {
        self.calls.push("SIGTSTP");
        if self.fail {
            Err("suspend failed".into())
        } else {
            Ok(())
        }
    }
}

#[test]
fn shows_a_status_message_and_skips_suspend_on_windows() {
    let mut ops = Recorder::default();
    assert_eq!(
        suspend_to_background(&mut ops, true),
        SuspendOutcome::Unsupported("Suspend to background is not supported on Windows")
    );
    assert!(ops.calls.is_empty());
}

#[test]
fn ignores_sigint_while_suspended_and_restores_the_tui_on_resume() {
    let mut ops = Recorder::default();
    assert_eq!(
        suspend_to_background(&mut ops, false),
        SuspendOutcome::Resumed
    );
    assert_eq!(
        ops.calls,
        [
            "ignore_sigint",
            "stop_ui",
            "SIGTSTP",
            "restore_sigint",
            "restart_ui"
        ]
    );
}

#[test]
fn cleans_up_the_temporary_handlers_if_suspension_fails() {
    let mut ops = Recorder {
        fail: true,
        ..Default::default()
    };
    assert_eq!(
        suspend_to_background(&mut ops, false),
        SuspendOutcome::Failed("suspend failed".into())
    );
    assert_eq!(
        ops.calls,
        ["ignore_sigint", "stop_ui", "SIGTSTP", "restore_sigint"]
    );
}
