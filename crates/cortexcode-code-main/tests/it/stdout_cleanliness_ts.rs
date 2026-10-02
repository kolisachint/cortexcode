//! Port of the pin's `coding-agent/test/stdout-cleanliness.test.ts`: in the
//! non-interactive modes `--help` leaves stdout empty and prints to stderr.
//! The TS also routes the startup `npm install` chatter of a settings
//! `packages` entry to stderr; package installs are 12.2 (deferred), so that
//! half is asserted there.

use std::process::Command;

fn run(args: &[&str]) -> (Option<i32>, String, String) {
    let home = std::env::temp_dir().join(format!("stdout-clean-{}", std::process::id()));
    std::fs::create_dir_all(home.join("project")).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_cortex"))
        .args(args)
        .current_dir(home.join("project"))
        .env("HOME", &home)
        .env("CORTEXCODE_CODING_AGENT_DIR", home.join("agent"))
        .output()
        .unwrap();
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn keeps_stdout_empty_for_mode_json_help() {
    let (code, stdout, stderr) = run(&["--mode", "json", "--help"]);
    assert_eq!(code, Some(0));
    assert_eq!(stdout, "");
    assert!(stderr.contains("Usage:"), "{stderr}");
}

#[test]
fn keeps_stdout_empty_for_print_help() {
    let (code, stdout, stderr) = run(&["-p", "--help"]);
    assert_eq!(code, Some(0));
    assert_eq!(stdout, "");
    assert!(stderr.contains("Usage:"), "{stderr}");
}
