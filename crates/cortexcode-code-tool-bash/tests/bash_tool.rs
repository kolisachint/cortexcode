//! Port of the bash cases in hoocode `test/tools.test.ts` and
//! `test/bash-prompt-snippet.test.ts` (v0.5.89). Unix only (sh, seq).
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use cortexcode_agent_types::AgentToolResult;
use cortexcode_ai_types::{AbortSignal, Content};
use cortexcode_code_tool_api::{ToolDefinition, ToolError};
use cortexcode_code_tool_bash::*;
use serde_json::{json, Value};

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("bash-tool-test-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn text(result: &AgentToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|c| match c {
            Content::Text(t) => Some(t.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn run(definition: &ToolDefinition, args: Value) -> Result<AgentToolResult, ToolError> {
    (definition.execute)("call".into(), args, None, None, None)
}

fn bash(cwd: &Path, options: BashToolOptions) -> ToolDefinition {
    create_bash_tool_definition(cwd, options)
}

/// Operations that feed fixed chunks, then return or fail.
struct Scripted {
    chunks: Vec<Vec<u8>>,
    outcome: Result<Option<i32>, String>,
}

impl BashOperations for Scripted {
    fn exec(
        &self,
        _command: &str,
        _cwd: &Path,
        options: BashExecOptions<'_>,
    ) -> Result<Option<i32>, ToolError> {
        for chunk in &self.chunks {
            (options.on_data)(chunk);
        }
        self.outcome.clone().map_err(Into::into)
    }
}

fn scripted(chunks: Vec<Vec<u8>>, outcome: Result<Option<i32>, String>) -> BashToolOptions {
    BashToolOptions {
        operations: Some(Arc::new(Scripted { chunks, outcome })),
        ..Default::default()
    }
}

fn full_output_path(message: &str) -> String {
    let start = message.find("Full output: ").unwrap() + "Full output: ".len();
    message[start..]
        .split([']', '\n'])
        .next()
        .unwrap()
        .to_owned()
}

#[test]
fn executes_simple_commands() {
    let cwd = std::env::current_dir().unwrap();
    let result = run(
        &bash(&cwd, Default::default()),
        json!({"command": "echo 'test output'"}),
    )
    .unwrap();
    assert!(text(&result).contains("test output"));
    assert_eq!(result.details, Value::Null);
}

#[test]
fn handles_command_errors() {
    let cwd = std::env::current_dir().unwrap();
    let err = run(
        &bash(&cwd, Default::default()),
        json!({"command": "exit 1"}),
    )
    .unwrap_err();
    assert!(err.to_string().contains("code 1"), "{err}");
    assert_eq!(err.to_string(), "(no output)\n\nCommand exited with code 1");
}

#[test]
fn respects_timeout() {
    let cwd = std::env::current_dir().unwrap();
    let started = std::time::Instant::now();
    let err = run(
        &bash(&cwd, Default::default()),
        json!({"command": "sleep 5", "timeout": 1}),
    )
    .unwrap_err();
    assert_eq!(err.to_string(), "Command timed out after 1 seconds");
    assert!(started.elapsed() < std::time::Duration::from_secs(4));
}

#[test]
fn includes_full_output_path_for_truncated_timeout_and_abort_errors() {
    let dir = TestDir::new();
    for (error, expected) in [
        ("timeout:5", "Command timed out after 5 seconds"),
        ("aborted", "Command aborted"),
    ] {
        let chunks = (1..=3000).map(|i| format!("{i}\n").into_bytes()).collect();
        let tool = bash(&dir.0, scripted(chunks, Err(error.into())));
        let message = run(&tool, json!({"command": "chatty-fail"}))
            .unwrap_err()
            .to_string();
        assert!(message.contains(expected), "{message}");
        let re = regex_lite::Regex::new(r"\[Showing lines \d+-\d+ of \d+\. Full output: ").unwrap();
        assert!(re.is_match(&message), "{message}");
        assert!(!message.contains("Full output: undefined"));
        let path = full_output_path(&message);
        let full = std::fs::read_to_string(&path).unwrap();
        assert!(full.contains("1\n2\n3"));
        assert!(full.contains("2998\n2999\n3000"));
        let _ = std::fs::remove_file(path);
    }
}

#[test]
fn throws_when_cwd_does_not_exist() {
    let tool = bash(
        Path::new("/this/directory/definitely/does/not/exist/12345"),
        Default::default(),
    );
    let err = run(&tool, json!({"command": "echo test"})).unwrap_err();
    assert!(
        err.to_string().contains("Working directory does not exist"),
        "{err}"
    );
}

#[test]
fn handles_process_spawn_errors() {
    let dir = TestDir::new();
    // hoocode mocks getShellConfig to return a missing shell; a shellPath that
    // exists but cannot be executed as a program reaches spawn the same way.
    let not_a_program = dir.0.join("missing-dir/");
    std::fs::create_dir_all(&not_a_program).unwrap();
    let ops = LocalBashOperations::new(Some(not_a_program.to_string_lossy().into_owned()));
    let err = ops
        .exec(
            "echo test",
            &dir.0,
            BashExecOptions {
                on_data: &mut |_| {},
                signal: None,
                timeout: None,
                env: None,
            },
        )
        .unwrap_err();
    assert!(err.to_string().starts_with("spawn "), "{err}");
}

#[test]
fn passes_shell_path_through_to_shell_resolution() {
    let dir = TestDir::new();
    let tool = bash(
        &dir.0,
        BashToolOptions {
            shell_path: Some("/custom/bash".into()),
            ..scripted(Vec::new(), Ok(Some(0)))
        },
    );
    run(&tool, json!({"command": "echo test"})).unwrap();

    let ops = LocalBashOperations::new(Some("/custom/bash".into()));
    let err = ops
        .exec(
            "echo test",
            &dir.0,
            BashExecOptions {
                on_data: &mut |_| {},
                signal: None,
                timeout: None,
                env: None,
            },
        )
        .unwrap_err();
    assert_eq!(err.to_string(), "Custom shell path not found: /custom/bash");
}

#[test]
fn command_prefix() {
    let dir = TestDir::new();
    let with_prefix = |prefix: &str| {
        bash(
            &dir.0,
            BashToolOptions {
                command_prefix: Some(prefix.into()),
                ..Default::default()
            },
        )
    };
    let result = run(
        &with_prefix("export TEST_VAR=hello"),
        json!({"command": "echo $TEST_VAR"}),
    )
    .unwrap();
    assert_eq!(text(&result).trim(), "hello");
    let result = run(
        &with_prefix("echo prefix-output"),
        json!({"command": "echo command-output"}),
    )
    .unwrap();
    assert_eq!(text(&result).trim(), "prefix-output\ncommand-output");
    let result = run(
        &bash(&dir.0, Default::default()),
        json!({"command": "echo no-prefix"}),
    )
    .unwrap();
    assert_eq!(text(&result).trim(), "no-prefix");
}

#[test]
fn coalesces_streaming_updates_for_chatty_output() {
    let dir = TestDir::new();
    let chunks = (0..5000)
        .map(|i| format!("line {i}\n").into_bytes())
        .collect();
    let tool = bash(&dir.0, scripted(chunks, Ok(Some(0))));
    let updates = Arc::new(Mutex::new(Vec::new()));
    let sink = updates.clone();
    let result = (tool.execute)(
        "call".into(),
        json!({"command": "chatty"}),
        None,
        Some(Box::new(move |update| sink.lock().unwrap().push(update))),
        None,
    )
    .unwrap();
    let updates = updates.lock().unwrap();
    assert!(updates.len() < 25, "{} updates", updates.len());
    assert!(text(&result).contains("line 4999"));
    // The first update announces the call with no content.
    assert!(updates[0].content.is_empty());
}

#[test]
fn decodes_utf8_characters_split_across_chunks() {
    let dir = TestDir::new();
    let euro = "€\n".as_bytes();
    let tool = bash(
        &dir.0,
        scripted(vec![euro[..1].to_vec(), euro[1..].to_vec()], Ok(Some(0))),
    );
    let result = run(&tool, json!({"command": "split-utf8"})).unwrap();
    assert_eq!(text(&result).trim(), "€");
}

#[test]
fn exposes_local_bash_operations_for_extension_reuse() {
    let dir = TestDir::new();
    let mut env = get_shell_env();
    env.insert("TEST_LOCAL_BASH_OPS".into(), "from-local-ops".into());
    let mut out = Vec::new();
    let code = LocalBashOperations::default()
        .exec(
            "echo $TEST_LOCAL_BASH_OPS",
            &dir.0,
            BashExecOptions {
                on_data: &mut |data| out.extend_from_slice(data),
                signal: None,
                timeout: None,
                env: Some(env),
            },
        )
        .unwrap();
    assert_eq!(code, Some(0));
    assert_eq!(String::from_utf8(out).unwrap().trim(), "from-local-ops");
}

#[test]
fn execute_bash_preserves_sanitization_with_local_operations() {
    let cwd = std::env::current_dir().unwrap();
    let result = execute_bash_with_operations(
        r"printf '\033[31mred\033[0m\r\n'",
        &cwd,
        &LocalBashOperations::default(),
        BashExecutorOptions::default(),
    )
    .unwrap();
    assert_eq!(result.exit_code, Some(0));
    assert_eq!(result.output, "red\n");
}

#[test]
fn persists_full_output_when_truncation_is_by_line_count_only() {
    let dir = TestDir::new();
    let result = run(
        &bash(&dir.0, Default::default()),
        json!({"command": "seq 3000"}),
    )
    .unwrap();
    let output = text(&result);
    assert_eq!(result.details["truncation"]["truncated"], true);
    assert_eq!(result.details["truncation"]["truncatedBy"], "lines");
    let path = result.details["fullOutputPath"]
        .as_str()
        .unwrap()
        .to_owned();
    let re = regex_lite::Regex::new(r"\[Showing lines \d+-\d+ of \d+\. Full output: ").unwrap();
    assert!(re.is_match(&output), "{output}");
    assert!(!output.contains("Full output: undefined"));
    let full = std::fs::read_to_string(&path).unwrap();
    assert!(full.contains("1\n2\n3"));
    assert!(full.contains("2998\n2999\n3000"));
    let _ = std::fs::remove_file(path);
}

#[test]
fn execute_bash_persists_full_output_when_truncation_is_by_line_count_only() {
    let cwd = std::env::current_dir().unwrap();
    let result = execute_bash_with_operations(
        "seq 3000",
        &cwd,
        &LocalBashOperations::default(),
        BashExecutorOptions::default(),
    )
    .unwrap();
    assert!(result.truncated);
    let path = result.full_output_path.unwrap();
    let full = std::fs::read_to_string(&path).unwrap();
    assert!(full.contains("1\n2\n3"));
    assert!(full.contains("2998\n2999\n3000"));
    let _ = std::fs::remove_file(path);
}

// --- command-level enforcement (allowedCommands / deniedCommands) ---

fn filtered(allowed: &[&str], denied: &[&str]) -> BashToolOptions {
    BashToolOptions {
        allowed_commands: allowed.iter().map(|s| s.to_string()).collect(),
        denied_commands: denied.iter().map(|s| s.to_string()).collect(),
        ..Default::default()
    }
}

#[test]
fn allowed_and_denied_command_patterns() {
    let dir = TestDir::new();
    let ok = |options, command: &str| {
        text(&run(&bash(&dir.0, options), json!({"command": command})).unwrap())
            .trim()
            .to_owned()
    };
    let err = |options, command: &str| {
        run(&bash(&dir.0, options), json!({"command": command}))
            .unwrap_err()
            .to_string()
    };

    assert_eq!(ok(filtered(&[r"^echo\b"], &[]), "echo hello"), "hello");
    assert_eq!(
        err(filtered(&[r"^echo\b"], &[]), "ls"),
        "Command blocked: does not match any allowed command pattern"
    );
    assert!(err(filtered(&[], &[r"\brm\b"]), "rm -rf /")
        .starts_with("Command blocked: matches denied pattern"));
    assert_eq!(ok(filtered(&[], &[r"\brm\b"]), "echo safe"), "safe");
    assert_eq!(
        err(filtered(&[".*"], &[r"^echo\b"]), "echo hi"),
        "Command blocked: matches denied pattern \"^echo\\b\""
    );
    assert!(err(filtered(&[], &["[invalid", r"\brm\b"]), "rm foo")
        .starts_with("Command blocked: matches denied pattern"));
    assert_eq!(
        ok(filtered(&["[invalid", r"^echo\b"], &[]), "echo valid"),
        "valid"
    );
    assert_eq!(ok(filtered(&[], &[]), "echo unrestricted"), "unrestricted");
}

// --- bash-prompt-snippet.test.ts ---

#[test]
fn prompt_snippet_points_bash_at_its_own_lane() {
    let snippet = bash(Path::new("."), Default::default())
        .prompt_snippet
        .unwrap();
    let dedicated = regex_lite::Regex::new(r"\b(ls|grep|find|cat|head|tail|sed)\b").unwrap();
    assert!(!dedicated.is_match(&snippet));
    let lane = regex_lite::Regex::new(r"(?i)build|test|lint|git|package").unwrap();
    assert!(lane.is_match(&snippet));
}

// --- beyond the TS files ---

#[test]
fn aborts_kill_the_process_tree() {
    let dir = TestDir::new();
    let signal = AbortSignal::new();
    let aborter = signal.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(200));
        aborter.abort();
    });
    let started = std::time::Instant::now();
    let err = (bash(&dir.0, Default::default()).execute)(
        "call".into(),
        json!({"command": "echo started; sleep 5 & sleep 5"}),
        Some(signal),
        None,
        None,
    )
    .unwrap_err();
    assert_eq!(err.to_string(), "started\n\n\nCommand aborted");
    assert!(started.elapsed() < std::time::Duration::from_secs(3));
}

#[test]
fn does_not_hang_on_pipes_held_by_background_children() {
    let dir = TestDir::new();
    let started = std::time::Instant::now();
    let result = run(
        &bash(&dir.0, Default::default()),
        json!({"command": "(sleep 3 &) ; echo done"}),
    )
    .unwrap();
    assert_eq!(text(&result).trim(), "done");
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
}

#[test]
fn byte_truncation_notice_and_description() {
    let dir = TestDir::new();
    let tool = bash(
        &dir.0,
        BashToolOptions {
            max_output_bytes: Some(2048),
            max_output_lines: Some(1000),
            ..Default::default()
        },
    );
    assert!(tool
        .description
        .contains("truncated to last 1000 lines or 2KB"));
    let result = run(
        &tool,
        json!({"command": "for i in $(seq 1 400); do echo line-$i; done"}),
    )
    .unwrap();
    let output = text(&result);
    assert_eq!(result.details["truncation"]["truncatedBy"], "bytes");
    assert!(output.contains("(2.0KB limit). Full output: "), "{output}");
    let path = full_output_path(&output);
    let _ = std::fs::remove_file(path);
    // Signal-killed shells (no exit code) are not failures.
    let result = run(
        &bash(&dir.0, Default::default()),
        json!({"command": "echo x; kill -9 $$"}),
    )
    .unwrap();
    assert_eq!(text(&result).trim(), "x");
}
