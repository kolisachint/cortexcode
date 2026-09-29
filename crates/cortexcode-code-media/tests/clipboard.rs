//! Port of `test/clipboard.test.ts`, with a recording host in place of the
//! mocked native addon, `child_process` and stdout.

use std::cell::RefCell;
use std::collections::HashMap;

use cortexcode_code_media::clipboard::{copy_to_clipboard, ClipboardHost, Platform};

#[derive(Default)]
struct Host {
    env: HashMap<String, String>,
    native_fails: bool,
    exec_fails: bool,
    native_calls: RefCell<Vec<String>>,
    exec_calls: RefCell<Vec<(String, String)>>,
    spawn_calls: RefCell<Vec<String>>,
    /// Everything written, in order, tagged with what wrote it.
    log: RefCell<Vec<String>>,
}

impl ClipboardHost for Host {
    fn platform(&self) -> Platform {
        Platform::Darwin
    }
    fn env(&self, name: &str) -> Option<String> {
        self.env.get(name).cloned()
    }
    fn native_set_text(&self, text: &str) -> Option<Result<(), String>> {
        self.native_calls.borrow_mut().push(text.into());
        self.log.borrow_mut().push("native".into());
        Some(if self.native_fails {
            Err("native failed".into())
        } else {
            Ok(())
        })
    }
    fn exec(&self, command: &str, input: &str) -> Result<(), String> {
        self.exec_calls
            .borrow_mut()
            .push((command.into(), input.into()));
        if self.exec_fails {
            Err(format!("{command} failed"))
        } else {
            Ok(())
        }
    }
    fn spawn_detached(&self, program: &str, _input: &str) -> Result<(), String> {
        self.spawn_calls.borrow_mut().push(program.into());
        Ok(())
    }
    fn write_stdout(&self, data: &str) {
        if data.starts_with("\x1b]52;c;") {
            self.log.borrow_mut().push("osc52".into());
        }
    }
}

impl Host {
    fn osc52_writes(&self) -> usize {
        self.log.borrow().iter().filter(|e| *e == "osc52").count()
    }
}

#[test]
fn local_native_success_skips_osc52_and_shell_fallbacks() {
    let host = Host::default();
    copy_to_clipboard(&host, "hello").unwrap();
    assert_eq!(*host.native_calls.borrow(), vec!["hello".to_string()]);
    assert_eq!(host.osc52_writes(), 0);
    assert!(host.exec_calls.borrow().is_empty());
    assert!(host.spawn_calls.borrow().is_empty());
}

#[test]
fn remote_native_success_emits_osc52_after_native_write() {
    let mut host = Host::default();
    host.env
        .insert("SSH_CONNECTION".into(), "client server".into());
    copy_to_clipboard(&host, "hello").unwrap();
    assert_eq!(*host.log.borrow(), vec!["native", "osc52"]);
    assert!(host.exec_calls.borrow().is_empty());
}

#[test]
fn local_shell_fallback_success_skips_osc52() {
    let host = Host {
        native_fails: true,
        ..Default::default()
    };
    copy_to_clipboard(&host, "hello").unwrap();
    assert_eq!(
        *host.exec_calls.borrow(),
        vec![("pbcopy".to_string(), "hello".to_string())]
    );
    assert_eq!(host.osc52_writes(), 0);
}

#[test]
fn uses_osc52_fallback_when_native_and_shell_tools_fail() {
    let host = Host {
        native_fails: true,
        exec_fails: true,
        ..Default::default()
    };
    copy_to_clipboard(&host, "hello").unwrap();
    assert_eq!(host.osc52_writes(), 1);
}

#[test]
fn does_not_emit_oversized_osc52_payloads() {
    let host = Host {
        native_fails: true,
        exec_fails: true,
        ..Default::default()
    };
    let error = copy_to_clipboard(&host, &"x".repeat(80_000)).unwrap_err();
    assert!(error.contains("Failed to copy to clipboard"));
    assert_eq!(host.osc52_writes(), 0);
}

/// Linux skips the native write and goes to the display's tools.
#[test]
fn linux_uses_xclip_with_a_display_and_never_the_native_write() {
    struct Linux(Host);
    impl ClipboardHost for Linux {
        fn platform(&self) -> Platform {
            Platform::Linux
        }
        fn env(&self, name: &str) -> Option<String> {
            self.0.env(name)
        }
        fn native_set_text(&self, text: &str) -> Option<Result<(), String>> {
            self.0.native_set_text(text)
        }
        fn exec(&self, command: &str, input: &str) -> Result<(), String> {
            self.0.exec(command, input)
        }
        fn spawn_detached(&self, program: &str, input: &str) -> Result<(), String> {
            self.0.spawn_detached(program, input)
        }
        fn write_stdout(&self, data: &str) {
            self.0.write_stdout(data)
        }
    }
    let mut host = Host::default();
    host.env.insert("DISPLAY".into(), ":0".into());
    let host = Linux(host);
    copy_to_clipboard(&host, "hello").unwrap();
    assert!(host.0.native_calls.borrow().is_empty());
    assert_eq!(
        host.0.exec_calls.borrow()[0].0,
        "xclip -selection clipboard".to_string()
    );
    assert_eq!(host.0.osc52_writes(), 0);
}
