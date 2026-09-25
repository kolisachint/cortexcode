//! CLI for the `cortex` coding agent: argument parsing and mode dispatch.
//!
//! Port of hoocode `packages/coding-agent/src/cli/args.ts` (in [`args`]) and
//! the argument-handling half of `main.ts` (in [`run`]). Flags that parse but
//! whose behavior is not ported yet fail with a clear "not yet supported"
//! error instead of being silently ignored.

pub mod args;
pub mod auth;
mod help;
mod help_text;
pub mod initial_message;
mod permission_dialog;
mod runtime;

pub use args::{
    is_valid_thinking_level, parse_args, Args, Diagnostic, DiagnosticKind, FlagValue, ListModels,
    Mode,
};
pub use help::{print_help, render_help};

use cortexcode_code_config::Config;
use cortexcode_code_print::PrintMode;
use std::io::{IsTerminal, Read, Write};

/// `VERSION` printed by `--version`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Package-manager and config subcommands that hoocode handles before
/// `parseArgs` (`handlePackageCommand`, `handleConfigCommand`,
/// `handleResourcesCommand`). None are ported yet.
const SUBCOMMANDS: [&str; 7] = [
    "install",
    "remove",
    "uninstall",
    "update",
    "list",
    "config",
    "resources",
];

/// `resolveAppMode` in `main.ts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppMode {
    Interactive,
    Print,
    Json,
    Rpc,
}

pub fn resolve_app_mode(parsed: &Args, stdin_is_tty: bool) -> AppMode {
    match parsed.mode {
        Some(Mode::Rpc) => AppMode::Rpc,
        Some(Mode::Json) => AppMode::Json,
        _ if parsed.print == Some(true) || !stdin_is_tty => AppMode::Print,
        _ => AppMode::Interactive,
    }
}

/// Process environment the dispatcher depends on, injectable for tests.
#[derive(Debug, Clone, Copy)]
pub struct Env {
    pub stdin_is_tty: bool,
    /// Whether chalk-style color is enabled.
    pub color: bool,
}

impl Env {
    pub fn detect() -> Self {
        Self {
            stdin_is_tty: std::io::stdin().is_terminal(),
            color: color_enabled(),
        }
    }
}

/// chalk's default level comes from `supports-color` on stdout.
fn color_enabled() -> bool {
    match std::env::var("FORCE_COLOR") {
        Ok(v) if v == "0" || v == "false" => return false,
        Ok(_) => return true,
        Err(_) => {}
    }
    std::io::stdout().is_terminal() && std::env::var("TERM").map_or(true, |t| t != "dumb")
}

pub(crate) fn red(color: bool, text: &str) -> String {
    if color {
        format!("\x1b[31m{text}\x1b[39m")
    } else {
        text.to_string()
    }
}

fn yellow(env: Env, text: &str) -> String {
    if env.color {
        format!("\x1b[33m{text}\x1b[39m")
    } else {
        text.to_string()
    }
}

/// Flags from the pinned set that parse but are not implemented yet, in
/// `Args` field order.
pub fn unsupported_flags(a: &Args) -> Vec<&'static str> {
    let checks: [(bool, &'static str); 42] = [
        (a.thinking.is_some(), "--thinking"),
        (a.continue_.is_some(), "--continue"),
        (a.resume.is_some(), "--resume"),
        (a.max_turns.is_some(), "--max-turns"),
        (a.session.is_some(), "--session"),
        (a.team.is_some(), "--team"),
        (a.fork.is_some(), "--fork"),
        (a.session_dir.is_some(), "--session-dir"),
        (a.models.is_some(), "--models"),
        (a.tools.is_some(), "--tools"),
        (a.disallowed_tools.is_some(), "--disallowed-tools"),
        (a.no_tools.is_some(), "--no-tools"),
        (a.no_builtin_tools.is_some(), "--no-builtin-tools"),
        (a.subagent == Some(true), "--enable-subagents"),
        (a.subagent == Some(false), "--no-subagents"),
        (a.warm_subagents.is_some(), "--warm-subagents"),
        (a.max_subagent_depth.is_some(), "--max-subagent-depth"),
        (a.delegate_allow.is_some(), "--delegate-allow"),
        (a.todo_write.is_some(), "--enable-todowrite"),
        (a.enable_web_tools.is_some(), "--enable-webtools"),
        (a.enable_plugin_tools.is_some(), "--enable-plugintools"),
        (a.enable_semantic_index.is_some(), "--enable-semantic-index"),
        (a.light.is_some(), "--light"),
        (a.print_token_surface.is_some(), "--print-token-surface"),
        (a.platform.is_some(), "--platform"),
        (a.ca_cert.is_some(), "--ca-cert"),
        (a.use_system_ca.is_some(), "--use-system-ca"),
        (a.extensions.is_some(), "--extension"),
        (a.no_extensions.is_some(), "--no-extensions"),
        (a.no_skills.is_some(), "--no-skills"),
        (a.skills.is_some(), "--skill"),
        (a.agents.is_some(), "--agent"),
        (a.prompt_templates.is_some(), "--prompt-template"),
        (a.no_prompt_templates.is_some(), "--no-prompt-templates"),
        (a.slash_commands.is_some(), "--slash-command"),
        (a.no_slash_commands.is_some(), "--no-slash-commands"),
        (a.themes.is_some(), "--theme"),
        (a.no_themes.is_some(), "--no-themes"),
        (a.mode_paths.is_some(), "--mode-path"),
        (a.no_context_files.is_some(), "--no-context-files"),
        (a.list_models.is_some(), "--list-models"),
        (a.verbose.is_some(), "--verbose"),
    ];
    checks
        .into_iter()
        .filter(|(set, _)| *set)
        .map(|(_, flag)| flag)
        .collect()
}

/// Process entry point used by the `cortex` binary. Returns the exit code.
pub fn main(argv: &[String]) -> i32 {
    let env = Env::detect();
    let mut stdout = std::io::stdout();
    let mut stderr = std::io::stderr();

    if let Some(cmd) = argv.first().filter(|a| SUBCOMMANDS.contains(&a.as_str())) {
        let _ = writeln!(
            stderr,
            "{}",
            red(
                env.color,
                &format!("Error: `cortex {cmd}` is not yet supported by cortex")
            )
        );
        return 1;
    }

    // Auto-migrate settings from the legacy hoocode `~/.hoocode/settings.json`
    // on first run. Best-effort: a failure must not block the CLI.
    if let Err(e) = cortexcode_code_config::migrate::auto_migrate() {
        let _ = writeln!(
            stderr,
            "{}",
            yellow(
                env,
                &format!("Warning: failed to load or migrate config: {e}")
            )
        );
    }

    let parsed = parse_args(argv);
    match run(&parsed, env, &mut stdout, &mut stderr) {
        Ok(code) => code,
        Err(e) => {
            let _ = writeln!(stderr, "{}", red(env.color, &format!("Error: {e}")));
            1
        }
    }
}

/// The argument-handling part of `main()` in `main.ts`, then mode dispatch.
pub fn run(
    parsed: &Args,
    env: Env,
    output: &mut dyn Write,
    err: &mut dyn Write,
) -> std::io::Result<i32> {
    if !parsed.diagnostics.is_empty() {
        for d in &parsed.diagnostics {
            let line = match d.kind {
                DiagnosticKind::Error => red(env.color, &format!("Error: {}", d.message)),
                DiagnosticKind::Warning => yellow(env, &format!("Warning: {}", d.message)),
            };
            writeln!(err, "{line}")?;
        }
        if parsed
            .diagnostics
            .iter()
            .any(|d| d.kind == DiagnosticKind::Error)
        {
            return Ok(1);
        }
    }

    let app_mode = resolve_app_mode(parsed, env.stdin_is_tty);

    if parsed.version == Some(true) {
        writeln!(output, "{VERSION}")?;
        return Ok(0);
    }

    if parsed.export.is_some() {
        writeln!(
            err,
            "{}",
            red(env.color, "Error: --export is not yet supported by cortex")
        )?;
        return Ok(1);
    }

    if parsed.mode == Some(Mode::Rpc) && !parsed.file_args.is_empty() {
        writeln!(
            err,
            "{}",
            red(
                env.color,
                "Error: @file arguments are not supported in RPC mode"
            )
        )?;
        return Ok(1);
    }

    if parsed.help == Some(true) {
        print_help(output, env.color)?;
        return Ok(0);
    }

    let unsupported = unsupported_flags(parsed);
    if !unsupported.is_empty() {
        for flag in &unsupported {
            writeln!(
                err,
                "{}",
                red(
                    env.color,
                    &format!("Error: {flag} is not yet supported by cortex")
                )
            )?;
        }
        return Ok(1);
    }

    // No extensions are ported yet, so no extension registers flags: every
    // unknown long flag is reported the way `applyExtensionFlagValues` does.
    if !parsed.unknown_flags.is_empty() {
        let names: Vec<String> = parsed
            .unknown_flags
            .iter()
            .map(|(name, _)| format!("--{name}"))
            .collect();
        let plural = if names.len() == 1 { "" } else { "s" };
        writeln!(
            err,
            "{}",
            red(
                env.color,
                &format!("Error: Unknown option{plural}: {}", names.join(", "))
            )
        )?;
        return Ok(1);
    }

    // `readPipedStdin`: RPC mode owns stdin; otherwise a non-TTY stdin is read
    // whole and becomes the start of the initial message.
    let stdin_content = if app_mode != AppMode::Rpc && !env.stdin_is_tty {
        let mut data = String::new();
        let _ = std::io::stdin().read_to_string(&mut data);
        initial_message::normalize_piped_stdin(&data)
    } else {
        None
    };

    match app_mode {
        AppMode::Json => runtime::run_print_mode(
            parsed,
            PrintMode::Json,
            stdin_content,
            env.color,
            output,
            err,
        ),
        AppMode::Print => runtime::run_print_mode(
            parsed,
            PrintMode::Text,
            stdin_content,
            env.color,
            output,
            err,
        ),
        AppMode::Rpc => match cortexcode_code_rpc::start_stdio_server() {
            Ok(()) => Ok(0),
            Err(e) => {
                writeln!(err, "rpc error: {e}")?;
                Ok(1)
            }
        },
        AppMode::Interactive => match runtime::run_interactive_mode(parsed, output, err) {
            Ok(()) => Ok(0),
            Err(e) => {
                writeln!(err, "{e}")?;
                Ok(1)
            }
        },
    }
}

/// Load `~/.cortexcode/config.json`, falling back to [`Config::default`] when
/// it is missing or malformed. Legacy `~/.hoocode/settings.json` migration
/// runs first in [`main`].
pub fn config_or_default() -> Config {
    cortexcode_code_config::load_default().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TTY: Env = Env {
        stdin_is_tty: true,
        color: false,
    };

    fn run_with(argv: &[&str], env: Env) -> (i32, String, String) {
        let parsed = parse_args(&argv.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = run(&parsed, env, &mut out, &mut err).unwrap();
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    #[test]
    fn version_prints_bare_version() {
        assert_eq!(
            run_with(&["--version", "--help"], TTY),
            (0, format!("{VERSION}\n"), String::new())
        );
    }

    #[test]
    fn help_prints_help() {
        let (code, out, _) = run_with(&["-h"], TTY);
        assert_eq!(code, 0);
        assert!(out.contains("Usage:"));
    }

    #[test]
    fn help_wins_over_unsupported_and_unknown_flags() {
        let (code, out, err) = run_with(&["--continue", "--future", "-h"], TTY);
        assert_eq!(code, 0);
        assert!(out.contains("Options:"));
        assert!(err.is_empty());
    }

    #[test]
    fn error_diagnostics_exit_1() {
        assert_eq!(
            run_with(&["-x"], TTY),
            (1, String::new(), "Error: Unknown option: -x\n".into())
        );
    }

    #[test]
    fn warnings_are_printed_and_colored() {
        let color = Env {
            stdin_is_tty: true,
            color: true,
        };
        let (code, _, err) = run_with(&["--thinking", "max", "--version"], color);
        assert_eq!(code, 0);
        assert!(err.starts_with("\x1b[33mWarning: Invalid thinking level \"max\""));
    }

    #[test]
    fn unsupported_flags_fail_clearly() {
        let (code, out, err) = run_with(&["--continue", "--tools", "read", "-p", "hi"], TTY);
        assert_eq!(code, 1);
        assert!(out.is_empty());
        assert_eq!(
            err,
            "Error: --continue is not yet supported by cortex\n\
             Error: --tools is not yet supported by cortex\n"
        );
    }

    #[test]
    fn export_is_not_yet_supported() {
        let (code, _, err) = run_with(&["--export", "s.jsonl"], TTY);
        assert_eq!(code, 1);
        assert_eq!(err, "Error: --export is not yet supported by cortex\n");
    }

    #[test]
    fn unknown_long_flags_are_errors_without_extensions() {
        assert_eq!(
            run_with(&["--login", "anthropic", "-p", "hi"], TTY).2,
            "Error: Unknown option: --login\n"
        );
        assert_eq!(
            run_with(&["--a", "--b=1"], TTY).2,
            "Error: Unknown options: --a, --b\n"
        );
    }

    #[test]
    fn rpc_mode_rejects_file_args() {
        assert_eq!(
            run_with(&["--mode", "rpc", "@x.md"], TTY),
            (
                1,
                String::new(),
                "Error: @file arguments are not supported in RPC mode\n".into()
            )
        );
    }

    #[test]
    fn supported_flags_are_not_flagged() {
        let parsed = parse_args(
            &[
                "--offline",
                "--provider",
                "p",
                "--model",
                "m",
                "--api-key",
                "k",
                "--system-prompt",
                "s",
                "--no-session",
                "--task-id",
                "t",
                "--mode",
                "json",
                "-p",
                "hi",
                "@f",
            ]
            .map(String::from),
        );
        assert!(unsupported_flags(&parsed).is_empty());
        assert!(parsed.unknown_flags.is_empty());
    }

    #[test]
    fn app_mode_resolution() {
        let p = |a: &[&str]| parse_args(&a.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(
            resolve_app_mode(&p(&["--mode", "rpc", "-p"]), true),
            AppMode::Rpc
        );
        assert_eq!(
            resolve_app_mode(&p(&["--mode", "json"]), true),
            AppMode::Json
        );
        assert_eq!(resolve_app_mode(&p(&["-p", "x"]), true), AppMode::Print);
        assert_eq!(resolve_app_mode(&p(&["x"]), false), AppMode::Print);
        assert_eq!(resolve_app_mode(&p(&["x"]), true), AppMode::Interactive);
        assert_eq!(
            resolve_app_mode(&p(&["--mode", "text"]), true),
            AppMode::Interactive
        );
    }
}
