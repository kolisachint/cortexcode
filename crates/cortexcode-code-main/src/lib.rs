//! Main entry point and CLI argument parsing for the `cortex` coding-agent
//! binary.
//!
//! Mirrors `main.ts` and `cli/args.ts` from the TypeScript
//! `packages/coding-agent` package. This crate currently exposes the CLI
//! surface and command routing. The runtime-backed commands (interactive TUI,
//! RPC server, fully wired print mode) are dispatched to the `runtime` module
//! and the agent namespace crates.

mod auth;
mod permission_dialog;
mod runtime;

use cortexcode_code_config::Config;
use cortexcode_code_print::PrintMode;
use std::str::FromStr;

/// Parsed command-line arguments for `cortex`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Args {
    /// Output mode requested with `--mode`.
    pub mode: Option<String>,
    /// LLM provider override.
    pub provider: Option<String>,
    /// Model override.
    pub model: Option<String>,
    /// API key override.
    pub api_key: Option<String>,
    /// Interactive OAuth login for the named provider, then exit.
    pub login: Option<String>,
    /// Explicit config file path override.
    pub config: Option<String>,
    /// Custom system prompt.
    pub system_prompt: Option<String>,
    /// Continue the current session.
    pub continue_: bool,
    /// Resume a previous session.
    pub resume: bool,
    /// Do not persist or load a session.
    pub no_session: bool,
    /// Run in single-shot print mode.
    pub print: bool,
    /// Explicit session id or path.
    pub session: Option<String>,
    /// Directory used for session storage.
    pub session_dir: Option<String>,
    /// Internal task id when spawned as a subagent.
    pub task_id: Option<String>,
    /// Hard cap on assistant turns.
    pub max_turns: Option<usize>,
    /// Print help and exit.
    pub help: bool,
    /// Print version and exit.
    pub version: bool,
    /// Positional prompt messages.
    pub messages: Vec<String>,
    /// Positional `@file` arguments.
    pub file_args: Vec<String>,
    /// Warnings/errors produced while parsing.
    pub diagnostics: Vec<Diagnostic>,
}

/// Severity of a CLI parsing diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticKind {
    Warning,
    Error,
}

/// A single CLI parsing diagnostic.
#[derive(Debug, Clone, PartialEq)]
pub struct Diagnostic {
    pub kind: DiagnosticKind,
    pub message: String,
}

impl Diagnostic {
    pub fn warning(message: impl Into<String>) -> Self {
        Self {
            kind: DiagnosticKind::Warning,
            message: message.into(),
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            kind: DiagnosticKind::Error,
            message: message.into(),
        }
    }
}

/// Parse the raw CLI arguments into an [`Args`] value.
///
/// Unknown flags are collected as diagnostics rather than causing a hard
/// failure, matching the TypeScript behavior for extension flags.
pub fn parse_args(raw: &[String]) -> Args {
    let mut args = Args::default();
    let mut i = 0;

    while i < raw.len() {
        let arg = raw[i].as_str();

        match arg {
            "--help" | "-h" => args.help = true,
            "--version" | "-v" => args.version = true,
            "--mode" => {
                if let Some(value) = raw.get(i + 1) {
                    i += 1;
                    if ["text", "json", "rpc", "subagent"].contains(&value.as_str()) {
                        args.mode = Some(value.clone());
                    } else {
                        args.diagnostics.push(Diagnostic::error(format!(
                            "unknown mode: {} (expected text, json, rpc, or subagent)",
                            value
                        )));
                    }
                } else {
                    args.diagnostics
                        .push(Diagnostic::error("--mode requires a value".to_string()));
                }
            }
            "--provider" => {
                if let Some(value) = raw.get(i + 1) {
                    i += 1;
                    args.provider = Some(value.clone());
                } else {
                    args.diagnostics
                        .push(Diagnostic::error("--provider requires a value".to_string()));
                }
            }
            "--model" => {
                if let Some(value) = raw.get(i + 1) {
                    i += 1;
                    args.model = Some(value.clone());
                } else {
                    args.diagnostics
                        .push(Diagnostic::error("--model requires a value".to_string()));
                }
            }
            "--api-key" => {
                if let Some(value) = raw.get(i + 1) {
                    i += 1;
                    args.api_key = Some(value.clone());
                } else {
                    args.diagnostics
                        .push(Diagnostic::error("--api-key requires a value".to_string()));
                }
            }
            "--login" => {
                if let Some(value) = raw.get(i + 1) {
                    i += 1;
                    args.login = Some(value.clone());
                } else {
                    args.diagnostics
                        .push(Diagnostic::error("--login requires a value".to_string()));
                }
            }
            "--config" => {
                if let Some(value) = raw.get(i + 1) {
                    i += 1;
                    args.config = Some(value.clone());
                } else {
                    args.diagnostics
                        .push(Diagnostic::error("--config requires a value".to_string()));
                }
            }
            "--system-prompt" => {
                if let Some(value) = raw.get(i + 1) {
                    i += 1;
                    args.system_prompt = Some(value.clone());
                } else {
                    args.diagnostics.push(Diagnostic::error(
                        "--system-prompt requires a value".to_string(),
                    ));
                }
            }
            "--session" => {
                if let Some(value) = raw.get(i + 1) {
                    i += 1;
                    args.session = Some(value.clone());
                } else {
                    args.diagnostics
                        .push(Diagnostic::error("--session requires a value".to_string()));
                }
            }
            "--session-dir" => {
                if let Some(value) = raw.get(i + 1) {
                    i += 1;
                    args.session_dir = Some(value.clone());
                } else {
                    args.diagnostics.push(Diagnostic::error(
                        "--session-dir requires a value".to_string(),
                    ));
                }
            }
            "--task-id" => {
                if let Some(value) = raw.get(i + 1) {
                    i += 1;
                    args.task_id = Some(value.clone());
                } else {
                    args.diagnostics
                        .push(Diagnostic::error("--task-id requires a value".to_string()));
                }
            }
            "--max-turns" => {
                if let Some(value) = raw.get(i + 1) {
                    i += 1;
                    match value.parse::<usize>() {
                        Ok(n) if n > 0 => args.max_turns = Some(n),
                        _ => args.diagnostics.push(Diagnostic::error(format!(
                            "--max-turns requires a positive integer, got {}",
                            value
                        ))),
                    }
                } else {
                    args.diagnostics.push(Diagnostic::error(
                        "--max-turns requires a value".to_string(),
                    ));
                }
            }
            "--continue" | "-c" => args.continue_ = true,
            "--resume" | "-r" => args.resume = true,
            "--no-session" => args.no_session = true,
            "--print" | "-p" => args.print = true,
            _ if arg.starts_with('-') => {
                args.diagnostics
                    .push(Diagnostic::warning(format!("unknown flag: {}", arg)));
            }
            _ => {
                if let Some(path) = arg.strip_prefix('@') {
                    args.file_args.push(path.to_string());
                } else {
                    args.messages.push(arg.to_string());
                }
            }
        }

        i += 1;
    }

    args
}

/// Print usage information.
pub fn print_help(output: &mut dyn std::io::Write) -> std::io::Result<()> {
    writeln!(
        output,
        "cortex {}

Usage: cortex [OPTIONS] [PROMPT]... [@FILE]...

Options:
  -h, --help                 Print this help message
  -v, --version              Print version information
  -p, --print                Run in single-shot print mode
      --mode <text|json|rpc|subagent> Output mode (default: text)
      --provider <NAME>      LLM provider to use
      --model <ID>           Model id to use
      --api-key <KEY>        API key for the provider
      --login <PROVIDER>     Sign in via OAuth (anthropic | github-copilot)
      --config <PATH>        Path to a config file (overrides the default)
      --system-prompt <TEXT> Override the system prompt
      --session <ID>         Session id or path
      --session-dir <PATH>   Directory for session storage
      --continue, -c         Continue the current session
      --resume, -r           Resume a previous session
      --no-session           Do not persist or load a session
      --task-id <ID>         Subagent task id (internal)
      --max-turns <N>        Hard cap on assistant turns

Positional arguments:
  PROMPT                     User prompt
  @FILE                      Read file content into the prompt
",
        env!("CARGO_PKG_VERSION")
    )
}

/// Print version information.
pub fn print_version(output: &mut dyn std::io::Write) -> std::io::Result<()> {
    writeln!(output, "cortex {}", env!("CARGO_PKG_VERSION"))
}

/// Run the CLI and return an exit code.
///
/// This function owns command dispatch. Runtime-backed modes are currently
/// placeholders that point to the leaf crate responsible for their full
/// implementation.
pub fn run(
    args: &Args,
    output: &mut dyn std::io::Write,
    err: &mut dyn std::io::Write,
) -> std::io::Result<u8> {
    if args.help {
        print_help(output)?;
        return Ok(0);
    }

    if args.version {
        print_version(output)?;
        return Ok(0);
    }

    for diag in &args.diagnostics {
        let prefix = match diag.kind {
            DiagnosticKind::Warning => "warning",
            DiagnosticKind::Error => "error",
        };
        writeln!(err, "{}: {}", prefix, diag.message)?;
    }

    if args
        .diagnostics
        .iter()
        .any(|d| d.kind == DiagnosticKind::Error)
    {
        return Ok(2);
    }

    if let Some(provider) = &args.login {
        return match auth::login(provider, output) {
            Ok(()) => Ok(0),
            Err(e) => {
                writeln!(err, "{}", e)?;
                Ok(1)
            }
        };
    }

    let mode = args
        .mode
        .as_deref()
        .and_then(|m| PrintMode::from_str(m).ok());

    if args.print || mode == Some(PrintMode::Text) || mode == Some(PrintMode::Json) {
        let print_mode = mode.unwrap_or_default();
        return match runtime::run_print_mode(args, print_mode, output, err) {
            Ok(()) => Ok(0),
            Err(e) => {
                writeln!(err, "{}", e)?;
                Ok(1)
            }
        };
    }

    if args.mode.as_deref() == Some("rpc") || args.mode.as_deref() == Some("subagent") {
        if let Err(e) = cortexcode_code_rpc::start_stdio_server() {
            let label = args.mode.as_deref().unwrap_or("rpc");
            writeln!(err, "{} error: {}", label, e)?;
            return Ok(1);
        }
        return Ok(0);
    }

    // No prompt provided → enter interactive TUI mode.
    // If an explicit --mode was given without a prompt, that is an error.
    if args.messages.is_empty() && args.file_args.is_empty() {
        if let Some(mode) = &args.mode {
            writeln!(err, "error: --mode {} requires a prompt argument", mode)?;
            return Ok(2);
        }
        return match runtime::run_interactive_mode(args, output, err) {
            Ok(()) => Ok(0),
            Err(e) => {
                writeln!(err, "{}", e)?;
                Ok(1)
            }
        };
    }

    match runtime::run_interactive_mode(args, output, err) {
        Ok(()) => Ok(0),
        Err(e) => {
            writeln!(err, "{}", e)?;
            Ok(1)
        }
    }
}

/// Return the effective configuration for this invocation.
///
/// Loads `~/.cortexcode/config.json` if present, falling back to
/// [`Config::default`] otherwise (missing file, unreadable file, or malformed
/// JSON are all treated as "no config" rather than a fatal error).
///
/// Legacy `~/.hoocode/settings.json` data migration runs separately at CLI
/// startup (see [`crate::main`] / `bin/main.rs`) via
/// `cortexcode_code_config::migrate::auto_migrate`, so by the time this is
/// called `~/.cortexcode/config.json` already reflects any migrated legacy
/// settings. When `--config <path>` is supplied, that file is loaded instead of
/// the default location (a missing or malformed file still falls back to
/// [`Config::default`]).
pub fn config_or_default(args: &Args) -> Config {
    if let Some(path) = &args.config {
        return Config::from_file(std::path::Path::new(path)).unwrap_or_default();
    }
    cortexcode_code_config::load_default().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_empty_args() {
        let args = parse_args(&[]);
        assert!(!args.help);
        assert!(args.messages.is_empty());
    }

    #[test]
    fn test_parse_help_and_version() {
        let args = parse_args(&["--help".to_string()]);
        assert!(args.help);

        let args = parse_args(&["-v".to_string()]);
        assert!(args.version);
    }

    #[test]
    fn test_parse_print_mode() {
        let args = parse_args(&[
            "-p".to_string(),
            "--model".to_string(),
            "gpt-4".to_string(),
            "hello".to_string(),
        ]);
        assert!(args.print);
        assert_eq!(args.model, Some("gpt-4".to_string()));
        assert_eq!(args.messages, vec!["hello"]);
    }

    #[test]
    fn test_parse_mode() {
        let args = parse_args(&["--mode".to_string(), "json".to_string()]);
        assert_eq!(args.mode, Some("json".to_string()));

        let args = parse_args(&["--mode".to_string(), "unknown".to_string()]);
        assert!(args.mode.is_none());
        assert!(args
            .diagnostics
            .iter()
            .any(|d| d.kind == DiagnosticKind::Error));
    }

    #[test]
    fn test_parse_config_flag() {
        let args = parse_args(&["--config".to_string(), "/tmp/custom.json".to_string()]);
        assert_eq!(args.config, Some("/tmp/custom.json".to_string()));
        assert!(args.diagnostics.is_empty());
    }

    #[test]
    fn test_parse_config_flag_missing_value() {
        let args = parse_args(&["--config".to_string()]);
        assert!(args.config.is_none());
        assert!(args
            .diagnostics
            .iter()
            .any(|d| d.kind == DiagnosticKind::Error));
    }

    #[test]
    fn test_parse_file_args() {
        let args = parse_args(&["@README.md".to_string(), "explain".to_string()]);
        assert_eq!(args.file_args, vec!["README.md"]);
        assert_eq!(args.messages, vec!["explain"]);
    }

    #[test]
    fn test_unknown_flag_warning() {
        let args = parse_args(&["--future-flag".to_string()]);
        assert_eq!(args.diagnostics.len(), 1);
        assert_eq!(args.diagnostics[0].kind, DiagnosticKind::Warning);
    }

    #[test]
    fn test_run_help() {
        let args = parse_args(&["--help".to_string()]);
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run(&args, &mut out, &mut err).unwrap();
        assert_eq!(code, 0);
        let out = String::from_utf8(out).unwrap();
        assert!(out.contains("Usage:"));
    }

    #[test]
    fn test_run_print_missing_key() {
        // Use an empty config file so no API key is available.
        let empty_config = std::env::temp_dir().join("cortex-test-empty-config.json");
        std::fs::write(&empty_config, "{}").unwrap();
        let args = parse_args(&[
            "-p".to_string(),
            "hi".to_string(),
            "--config".to_string(),
            empty_config.to_str().unwrap().to_string(),
        ]);
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run(&args, &mut out, &mut err).unwrap();
        let err = String::from_utf8(err).unwrap();
        let out = String::from_utf8(out).unwrap();
        assert_eq!(code, 1, "stdout: {:?}, stderr: {:?}", out, err);
        assert!(
            err.contains("API key")
                || err.contains("not supported")
                || err.contains("Unauthorized")
                || err.contains("401"),
            "unexpected error: {:?}",
            err
        );
        let _ = std::fs::remove_file(&empty_config);
    }

    #[test]
    fn test_run_no_args_shows_help() {
        // With no prompt and no --mode, cortex enters interactive mode,
        // which requires a TTY.  Verify it exits with an error instead
        // of printing help (that was the old behavior).
        let args = parse_args(&[]);
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run(&args, &mut out, &mut err).unwrap();
        assert_eq!(code, 1);
        let err = String::from_utf8(err).unwrap();
        // In a non-TTY test env we get a terminal error;
        // in a real terminal we'd get an API-key error.
        assert!(
            err.contains("API key")
                || err.contains("not supported")
                || err.contains("error")
                || err.contains("Device"),
            "unexpected error: {:?}",
            err
        );
    }
}
