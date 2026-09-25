//! CLI argument parsing. Port of hoocode `packages/coding-agent/src/cli/args.ts`.
//!
//! This is a direct port of the hand-written TS parser rather than a `clap`
//! derive: the pinned grammar has multi-letter short aliases (`-nt`, `-nbt`,
//! `-nsc`), captures unknown `--flags` (with a greedy value) as extension
//! flags, lets `-p` take the next argument as the prompt, and silently drops
//! invalid values for several flags. `clap` cannot express these without
//! changing observable behavior.

use cortexcode_ai_types::ThinkingLevel;

/// Output mode requested with `--mode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Text,
    Json,
    Rpc,
}

/// Value of an unknown (potentially extension) flag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlagValue {
    Bool(bool),
    String(String),
}

/// `--list-models` with or without a search pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListModels {
    All,
    Search(String),
}

/// Severity of a CLI parsing diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticKind {
    Warning,
    Error,
}

/// A single CLI parsing diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub kind: DiagnosticKind,
    pub message: String,
}

/// Parsed command-line arguments. Field names follow the TS `Args` interface.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Args {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub api_key: Option<String>,
    pub system_prompt: Option<String>,
    pub thinking: Option<ThinkingLevel>,
    pub continue_: Option<bool>,
    pub resume: Option<bool>,
    pub help: Option<bool>,
    pub version: Option<bool>,
    pub mode: Option<Mode>,
    pub no_session: Option<bool>,
    /// Internal: task id assigned by the subagent pool when this process is a spawned subagent.
    pub task_id: Option<String>,
    /// Hard cap on assistant turns.
    pub max_turns: Option<u64>,
    pub session: Option<String>,
    /// Base URL of a hooteams server, or "auto".
    pub team: Option<String>,
    pub fork: Option<String>,
    pub session_dir: Option<String>,
    pub models: Option<Vec<String>>,
    pub tools: Option<Vec<String>>,
    pub disallowed_tools: Option<Vec<String>>,
    pub no_tools: Option<bool>,
    pub no_builtin_tools: Option<bool>,
    /// Tri-state override of the `enableSubagent` setting.
    pub subagent: Option<bool>,
    pub warm_subagents: Option<bool>,
    pub max_subagent_depth: Option<u64>,
    /// Internal: restricts which subagent types this process may delegate to.
    pub delegate_allow: Option<Vec<String>>,
    pub todo_write: Option<bool>,
    pub enable_web_tools: Option<bool>,
    pub enable_plugin_tools: Option<bool>,
    pub enable_semantic_index: Option<bool>,
    pub light: Option<bool>,
    pub print_token_surface: Option<bool>,
    /// Raw `--platform` tokens (comma-separated and/or repeated).
    pub platform: Option<Vec<String>>,
    pub ca_cert: Option<String>,
    pub use_system_ca: Option<bool>,
    pub extensions: Option<Vec<String>>,
    pub no_extensions: Option<bool>,
    pub print: Option<bool>,
    pub export: Option<String>,
    pub no_skills: Option<bool>,
    pub skills: Option<Vec<String>>,
    pub agents: Option<Vec<String>>,
    pub prompt_templates: Option<Vec<String>>,
    pub no_prompt_templates: Option<bool>,
    pub slash_commands: Option<Vec<String>>,
    pub no_slash_commands: Option<bool>,
    pub themes: Option<Vec<String>>,
    pub no_themes: Option<bool>,
    pub mode_paths: Option<Vec<String>>,
    pub no_context_files: Option<bool>,
    pub list_models: Option<ListModels>,
    pub offline: Option<bool>,
    pub verbose: Option<bool>,
    pub messages: Vec<String>,
    pub file_args: Vec<String>,
    /// Unknown flags (potentially extension flags), in first-seen order.
    pub unknown_flags: Vec<(String, FlagValue)>,
    pub diagnostics: Vec<Diagnostic>,
}

impl Args {
    /// `Map.get` on the unknown flags.
    pub fn unknown_flag(&self, name: &str) -> Option<&FlagValue> {
        self.unknown_flags
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v)
    }

    /// `Map.set` semantics: overwrite in place, otherwise append.
    fn set_unknown_flag(&mut self, name: String, value: FlagValue) {
        match self.unknown_flags.iter_mut().find(|(n, _)| *n == name) {
            Some(entry) => entry.1 = value,
            None => self.unknown_flags.push((name, value)),
        }
    }
}

pub const VALID_THINKING_LEVELS: [&str; 6] = ["off", "minimal", "low", "medium", "high", "xhigh"];

/// Parse a thinking level name (`isValidThinkingLevel`).
pub fn parse_thinking_level(level: &str) -> Option<ThinkingLevel> {
    match level {
        "off" => Some(ThinkingLevel::Off),
        "minimal" => Some(ThinkingLevel::Minimal),
        "low" => Some(ThinkingLevel::Low),
        "medium" => Some(ThinkingLevel::Medium),
        "high" => Some(ThinkingLevel::High),
        "xhigh" => Some(ThinkingLevel::XHigh),
        _ => None,
    }
}

pub fn is_valid_thinking_level(level: &str) -> bool {
    parse_thinking_level(level).is_some()
}

/// `Number.parseInt(s, 10)`: optional leading whitespace and sign, then the
/// longest digit prefix. `None` is `NaN`.
fn js_parse_int(s: &str) -> Option<i64> {
    let s = s.trim_start();
    let (neg, rest) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let digits: &str = &rest[..rest.bytes().take_while(u8::is_ascii_digit).count()];
    if digits.is_empty() {
        return None;
    }
    let n = digits.parse::<i64>().unwrap_or(i64::MAX);
    Some(if neg { -n } else { n })
}

/// `s.split(",").map((s) => s.trim())`.
fn split_trim(s: &str) -> Vec<String> {
    s.split(',').map(|p| p.trim().to_string()).collect()
}

/// `split_trim` followed by `.filter((name) => name.length > 0)`.
fn split_trim_nonempty(s: &str) -> Vec<String> {
    split_trim(s)
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect()
}

fn push(list: &mut Option<Vec<String>>, value: &str) {
    list.get_or_insert_with(Vec::new).push(value.to_string());
}

/// Port of `parseArgs`.
pub fn parse_args(args: &[String]) -> Args {
    let mut result = Args::default();
    let n = args.len();
    let mut i = 0;

    while i < n {
        let arg = args[i].as_str();
        let has_next = i + 1 < n;

        match arg {
            "--help" | "-h" => result.help = Some(true),
            "--version" | "-v" => result.version = Some(true),
            "--mode" if has_next => {
                i += 1;
                match args[i].as_str() {
                    "text" => result.mode = Some(Mode::Text),
                    "json" => result.mode = Some(Mode::Json),
                    "rpc" => result.mode = Some(Mode::Rpc),
                    _ => {}
                }
            }
            "--continue" | "-c" => result.continue_ = Some(true),
            "--resume" | "-r" => result.resume = Some(true),
            "--provider" if has_next => {
                i += 1;
                result.provider = Some(args[i].clone());
            }
            "--model" if has_next => {
                i += 1;
                result.model = Some(args[i].clone());
            }
            "--api-key" if has_next => {
                i += 1;
                result.api_key = Some(args[i].clone());
            }
            "--system-prompt" if has_next => {
                i += 1;
                result.system_prompt = Some(args[i].clone());
            }
            "--no-session" => result.no_session = Some(true),
            "--task-id" if has_next => {
                i += 1;
                result.task_id = Some(args[i].clone());
            }
            "--max-turns" if has_next => {
                i += 1;
                if let Some(v) = js_parse_int(&args[i]).filter(|v| *v > 0) {
                    result.max_turns = Some(v as u64);
                }
            }
            "--session" if has_next => {
                i += 1;
                result.session = Some(args[i].clone());
            }
            "--team" if has_next => {
                i += 1;
                result.team = Some(args[i].clone());
            }
            "--fork" if has_next => {
                i += 1;
                result.fork = Some(args[i].clone());
            }
            "--session-dir" if has_next => {
                i += 1;
                result.session_dir = Some(args[i].clone());
            }
            "--models" if has_next => {
                i += 1;
                result.models = Some(split_trim(&args[i]));
            }
            "--no-tools" | "-nt" => result.no_tools = Some(true),
            "--no-builtin-tools" | "-nbt" => result.no_builtin_tools = Some(true),
            "--enable-subagents" => result.subagent = Some(true),
            "--no-subagents" | "--disable-subagents" => result.subagent = Some(false),
            "--warm-subagents" => result.warm_subagents = Some(true),
            "--max-subagent-depth" if has_next => {
                i += 1;
                if let Some(v) = js_parse_int(&args[i]).filter(|v| *v >= 1) {
                    result.max_subagent_depth = Some(v as u64);
                }
            }
            "--delegate-allow" if has_next => {
                i += 1;
                result.delegate_allow = Some(split_trim_nonempty(&args[i]));
            }
            "--enable-todowrite" => result.todo_write = Some(true),
            "--enable-webtools" => result.enable_web_tools = Some(true),
            "--enable-semantic-index" => result.enable_semantic_index = Some(true),
            "--enable-plugintools" => result.enable_plugin_tools = Some(true),
            "--light" => result.light = Some(true),
            "--print-token-surface" => result.print_token_surface = Some(true),
            "--platform" if has_next => {
                i += 1;
                result
                    .platform
                    .get_or_insert_with(Vec::new)
                    .extend(split_trim_nonempty(&args[i]));
            }
            "--ca-cert" if has_next => {
                i += 1;
                result.ca_cert = Some(args[i].clone());
            }
            "--use-system-ca" => result.use_system_ca = Some(true),
            "--tools" | "-t" if has_next => {
                i += 1;
                result.tools = Some(split_trim_nonempty(&args[i]));
            }
            "--disallowed-tools" if has_next => {
                i += 1;
                result.disallowed_tools = Some(split_trim_nonempty(&args[i]));
            }
            "--thinking" if has_next => {
                i += 1;
                let level = &args[i];
                match parse_thinking_level(level) {
                    Some(l) => result.thinking = Some(l),
                    None => result.diagnostics.push(Diagnostic {
                        kind: DiagnosticKind::Warning,
                        message: format!(
                            "Invalid thinking level \"{}\". Valid values: {}",
                            level,
                            VALID_THINKING_LEVELS.join(", ")
                        ),
                    }),
                }
            }
            "--print" | "-p" => {
                result.print = Some(true);
                if let Some(next) = args.get(i + 1) {
                    if !next.starts_with('@') && (!next.starts_with('-') || next.starts_with("---"))
                    {
                        result.messages.push(next.clone());
                        i += 1;
                    }
                }
            }
            "--export" if has_next => {
                i += 1;
                result.export = Some(args[i].clone());
            }
            "--extension" | "-e" if has_next => {
                i += 1;
                push(&mut result.extensions, &args[i]);
            }
            "--no-extensions" | "-ne" => result.no_extensions = Some(true),
            "--skill" if has_next => {
                i += 1;
                push(&mut result.skills, &args[i]);
            }
            "--agent" if has_next => {
                i += 1;
                push(&mut result.agents, &args[i]);
            }
            "--prompt-template" if has_next => {
                i += 1;
                push(&mut result.prompt_templates, &args[i]);
            }
            "--slash-command" if has_next => {
                i += 1;
                push(&mut result.slash_commands, &args[i]);
            }
            "--theme" if has_next => {
                i += 1;
                push(&mut result.themes, &args[i]);
            }
            "--mode-path" if has_next => {
                i += 1;
                push(&mut result.mode_paths, &args[i]);
            }
            "--no-skills" | "-ns" => result.no_skills = Some(true),
            "--no-prompt-templates" | "-np" => result.no_prompt_templates = Some(true),
            "--no-slash-commands" | "-nsc" => result.no_slash_commands = Some(true),
            "--no-themes" => result.no_themes = Some(true),
            "--no-context-files" | "-nc" => result.no_context_files = Some(true),
            "--list-models" => {
                // A following arg is a search pattern unless it is a flag or file arg.
                match args.get(i + 1) {
                    Some(next) if !next.starts_with('-') && !next.starts_with('@') => {
                        result.list_models = Some(ListModels::Search(next.clone()));
                        i += 1;
                    }
                    _ => result.list_models = Some(ListModels::All),
                }
            }
            "--verbose" => result.verbose = Some(true),
            "--offline" => result.offline = Some(true),
            _ if arg.starts_with('@') => result.file_args.push(arg[1..].to_string()),
            _ if arg.starts_with("--") => {
                if let Some(eq) = arg.find('=') {
                    result.set_unknown_flag(
                        arg[2..eq].to_string(),
                        FlagValue::String(arg[eq + 1..].to_string()),
                    );
                } else {
                    let flag_name = arg[2..].to_string();
                    match args.get(i + 1) {
                        Some(next) if !next.starts_with('-') && !next.starts_with('@') => {
                            result.set_unknown_flag(flag_name, FlagValue::String(next.clone()));
                            i += 1;
                        }
                        _ => result.set_unknown_flag(flag_name, FlagValue::Bool(true)),
                    }
                }
            }
            _ if arg.starts_with('-') => result.diagnostics.push(Diagnostic {
                kind: DiagnosticKind::Error,
                message: format!("Unknown option: {}", arg),
            }),
            _ => result.messages.push(arg.to_string()),
        }

        i += 1;
    }

    result
}

#[cfg(test)]
mod tests {
    //! Ported from hoocode `packages/coding-agent/test/args.test.ts` (same titles).
    use super::*;

    fn parse(args: &[&str]) -> Args {
        parse_args(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    fn strs(v: &[&str]) -> Option<Vec<String>> {
        Some(v.iter().map(|s| s.to_string()).collect())
    }

    // --version flag
    #[test]
    fn parses_version_flag() {
        assert_eq!(parse(&["--version"]).version, Some(true));
    }
    #[test]
    fn parses_v_shorthand() {
        assert_eq!(parse(&["-v"]).version, Some(true));
    }
    #[test]
    fn version_takes_precedence_over_other_args() {
        let r = parse(&["--version", "--help", "some message"]);
        assert_eq!(r.version, Some(true));
        assert_eq!(r.help, Some(true));
        assert!(r.messages.contains(&"some message".to_string()));
    }

    // --help flag
    #[test]
    fn parses_help_flag() {
        assert_eq!(parse(&["--help"]).help, Some(true));
    }
    #[test]
    fn parses_h_shorthand() {
        assert_eq!(parse(&["-h"]).help, Some(true));
    }

    // --print flag
    #[test]
    fn parses_print_flag() {
        assert_eq!(parse(&["--print"]).print, Some(true));
    }
    #[test]
    fn parses_p_shorthand() {
        assert_eq!(parse(&["-p"]).print, Some(true));
    }
    #[test]
    fn parses_prompt_after_p_even_when_it_starts_with_yaml_frontmatter() {
        let prompt = "---\ntitle: hello\n---\nSay hi.";
        let r = parse(&["-p", prompt]);
        assert_eq!(r.print, Some(true));
        assert_eq!(r.messages, vec![prompt.to_string()]);
        assert!(r.unknown_flags.is_empty());
    }
    #[test]
    fn does_not_consume_options_after_p_as_prompts() {
        let r = parse(&["-p", "--provider", "openai", "Say hi."]);
        assert_eq!(r.print, Some(true));
        assert_eq!(r.provider.as_deref(), Some("openai"));
        assert_eq!(r.messages, vec!["Say hi.".to_string()]);
    }

    // --continue / --resume
    #[test]
    fn parses_continue_flag() {
        assert_eq!(parse(&["--continue"]).continue_, Some(true));
    }
    #[test]
    fn parses_c_shorthand() {
        assert_eq!(parse(&["-c"]).continue_, Some(true));
    }
    #[test]
    fn parses_resume_flag() {
        assert_eq!(parse(&["--resume"]).resume, Some(true));
    }
    #[test]
    fn parses_r_shorthand() {
        assert_eq!(parse(&["-r"]).resume, Some(true));
    }

    // flags with values
    #[test]
    fn parses_provider() {
        assert_eq!(
            parse(&["--provider", "openai"]).provider.as_deref(),
            Some("openai")
        );
    }
    #[test]
    fn parses_model() {
        assert_eq!(
            parse(&["--model", "gpt-4o"]).model.as_deref(),
            Some("gpt-4o")
        );
    }
    #[test]
    fn parses_api_key() {
        assert_eq!(
            parse(&["--api-key", "sk-test-key"]).api_key.as_deref(),
            Some("sk-test-key")
        );
    }
    #[test]
    fn parses_system_prompt() {
        assert_eq!(
            parse(&["--system-prompt", "You are a helpful assistant"])
                .system_prompt
                .as_deref(),
            Some("You are a helpful assistant")
        );
    }
    #[test]
    fn parses_mode() {
        assert_eq!(parse(&["--mode", "json"]).mode, Some(Mode::Json));
    }
    #[test]
    fn parses_mode_rpc() {
        assert_eq!(parse(&["--mode", "rpc"]).mode, Some(Mode::Rpc));
    }
    #[test]
    fn parses_session() {
        assert_eq!(
            parse(&["--session", "/path/to/session.jsonl"])
                .session
                .as_deref(),
            Some("/path/to/session.jsonl")
        );
    }
    #[test]
    fn parses_fork() {
        let r = parse(&["--fork", "1234abcd"]);
        assert_eq!(r.fork.as_deref(), Some("1234abcd"));
        assert!(r.messages.is_empty());
    }
    #[test]
    fn parses_export() {
        assert_eq!(
            parse(&["--export", "session.jsonl"]).export.as_deref(),
            Some("session.jsonl")
        );
    }
    #[test]
    fn parses_thinking() {
        assert_eq!(
            parse(&["--thinking", "high"]).thinking,
            Some(ThinkingLevel::High)
        );
    }
    #[test]
    fn parses_models_as_comma_separated_list() {
        assert_eq!(
            parse(&["--models", "gpt-4o,claude-sonnet,gemini-pro"]).models,
            strs(&["gpt-4o", "claude-sonnet", "gemini-pro"])
        );
    }

    // --no-session
    #[test]
    fn parses_no_session_flag() {
        assert_eq!(parse(&["--no-session"]).no_session, Some(true));
    }

    // --extension
    #[test]
    fn parses_single_extension() {
        assert_eq!(
            parse(&["--extension", "./my-extension.ts"]).extensions,
            strs(&["./my-extension.ts"])
        );
    }
    #[test]
    fn parses_e_shorthand() {
        assert_eq!(
            parse(&["-e", "./my-extension.ts"]).extensions,
            strs(&["./my-extension.ts"])
        );
    }
    #[test]
    fn parses_multiple_extension_flags() {
        assert_eq!(
            parse(&["--extension", "./ext1.ts", "-e", "./ext2.ts"]).extensions,
            strs(&["./ext1.ts", "./ext2.ts"])
        );
    }

    // --no-extensions
    #[test]
    fn parses_no_extensions_flag() {
        assert_eq!(parse(&["--no-extensions"]).no_extensions, Some(true));
    }
    #[test]
    fn parses_no_extensions_with_explicit_e_flags() {
        let r = parse(&["--no-extensions", "-e", "foo.ts", "-e", "bar.ts"]);
        assert_eq!(r.no_extensions, Some(true));
        assert_eq!(r.extensions, strs(&["foo.ts", "bar.ts"]));
    }

    // --skill
    #[test]
    fn parses_single_skill() {
        assert_eq!(
            parse(&["--skill", "./skill-dir"]).skills,
            strs(&["./skill-dir"])
        );
    }
    #[test]
    fn parses_multiple_skill_flags() {
        assert_eq!(
            parse(&["--skill", "./skill-a", "--skill", "./skill-b"]).skills,
            strs(&["./skill-a", "./skill-b"])
        );
    }

    // --prompt-template
    #[test]
    fn parses_single_prompt_template() {
        assert_eq!(
            parse(&["--prompt-template", "./prompts"]).prompt_templates,
            strs(&["./prompts"])
        );
    }
    #[test]
    fn parses_multiple_prompt_template_flags() {
        assert_eq!(
            parse(&["--prompt-template", "./one", "--prompt-template", "./two"]).prompt_templates,
            strs(&["./one", "./two"])
        );
    }

    // --theme
    #[test]
    fn parses_single_theme() {
        assert_eq!(
            parse(&["--theme", "./theme.json"]).themes,
            strs(&["./theme.json"])
        );
    }
    #[test]
    fn parses_multiple_theme_flags() {
        assert_eq!(
            parse(&["--theme", "./dark.json", "--theme", "./light.json"]).themes,
            strs(&["./dark.json", "./light.json"])
        );
    }

    #[test]
    fn parses_no_skills_flag() {
        assert_eq!(parse(&["--no-skills"]).no_skills, Some(true));
    }
    #[test]
    fn parses_no_prompt_templates_flag() {
        assert_eq!(
            parse(&["--no-prompt-templates"]).no_prompt_templates,
            Some(true)
        );
    }

    // --enable-subagents
    #[test]
    fn parses_enable_subagents_flag() {
        assert_eq!(parse(&["--enable-subagents"]).subagent, Some(true));
    }
    #[test]
    fn subagent_defaults_to_undefined_when_absent() {
        assert_eq!(parse(&[]).subagent, None);
    }
    #[test]
    fn parses_no_subagents_flag_as_false() {
        assert_eq!(parse(&["--no-subagents"]).subagent, Some(false));
        assert_eq!(parse(&["--disable-subagents"]).subagent, Some(false));
    }

    // --disallowed-tools
    #[test]
    fn parses_a_comma_separated_denylist() {
        assert_eq!(
            parse(&["--disallowed-tools", "bash, write"]).disallowed_tools,
            strs(&["bash", "write"])
        );
    }
    #[test]
    fn disallowed_tools_defaults_to_undefined_when_absent() {
        assert_eq!(parse(&[]).disallowed_tools, None);
    }

    // --max-subagent-depth
    #[test]
    fn parses_a_valid_depth() {
        assert_eq!(
            parse(&["--max-subagent-depth", "2"]).max_subagent_depth,
            Some(2)
        );
    }
    #[test]
    fn ignores_non_positive_or_non_numeric_values() {
        assert_eq!(
            parse(&["--max-subagent-depth", "0"]).max_subagent_depth,
            None
        );
        assert_eq!(
            parse(&["--max-subagent-depth", "x"]).max_subagent_depth,
            None
        );
    }
    #[test]
    fn max_subagent_depth_defaults_to_undefined_when_absent() {
        assert_eq!(parse(&[]).max_subagent_depth, None);
    }

    // --enable-todowrite / --enable-webtools
    #[test]
    fn parses_enable_todowrite_flag() {
        assert_eq!(parse(&["--enable-todowrite"]).todo_write, Some(true));
    }
    #[test]
    fn todo_write_defaults_to_undefined_when_absent() {
        assert_eq!(parse(&[]).todo_write, None);
    }
    #[test]
    fn parses_enable_webtools_flag() {
        assert_eq!(parse(&["--enable-webtools"]).enable_web_tools, Some(true));
    }
    #[test]
    fn enable_web_tools_defaults_to_undefined_when_absent() {
        assert_eq!(parse(&[]).enable_web_tools, None);
    }

    // --slash-command / --no-slash-commands
    #[test]
    fn parses_single_slash_command() {
        assert_eq!(
            parse(&["--slash-command", "./commands"]).slash_commands,
            strs(&["./commands"])
        );
    }
    #[test]
    fn parses_multiple_slash_command_flags() {
        assert_eq!(
            parse(&["--slash-command", "./one", "--slash-command", "./two"]).slash_commands,
            strs(&["./one", "./two"])
        );
    }
    #[test]
    fn parses_no_slash_commands_flag() {
        assert_eq!(
            parse(&["--no-slash-commands"]).no_slash_commands,
            Some(true)
        );
    }
    #[test]
    fn parses_nsc_shorthand() {
        assert_eq!(parse(&["-nsc"]).no_slash_commands, Some(true));
    }

    #[test]
    fn parses_no_themes_flag() {
        assert_eq!(parse(&["--no-themes"]).no_themes, Some(true));
    }
    #[test]
    fn parses_no_context_files_flag() {
        assert_eq!(parse(&["--no-context-files"]).no_context_files, Some(true));
    }
    #[test]
    fn parses_nc_shorthand() {
        assert_eq!(parse(&["-nc"]).no_context_files, Some(true));
    }
    #[test]
    fn parses_verbose_flag() {
        assert_eq!(parse(&["--verbose"]).verbose, Some(true));
    }
    #[test]
    fn parses_offline_flag() {
        assert_eq!(parse(&["--offline"]).offline, Some(true));
    }

    // tool flags
    #[test]
    fn parses_no_tools_flag() {
        assert_eq!(parse(&["--no-tools"]).no_tools, Some(true));
    }
    #[test]
    fn parses_nt_shorthand() {
        assert_eq!(parse(&["-nt"]).no_tools, Some(true));
    }
    #[test]
    fn parses_no_builtin_tools_flag() {
        assert_eq!(parse(&["--no-builtin-tools"]).no_builtin_tools, Some(true));
    }
    #[test]
    fn parses_nbt_shorthand() {
        assert_eq!(parse(&["-nbt"]).no_builtin_tools, Some(true));
    }
    #[test]
    fn parses_tools_flag() {
        assert_eq!(
            parse(&["--tools", "read,bash"]).tools,
            strs(&["read", "bash"])
        );
    }
    #[test]
    fn parses_t_shorthand() {
        assert_eq!(parse(&["-t", "read,bash"]).tools, strs(&["read", "bash"]));
    }
    #[test]
    fn parses_no_tools_with_explicit_tools_flags() {
        let r = parse(&["--no-tools", "--tools", "read,bash"]);
        assert_eq!(r.no_tools, Some(true));
        assert_eq!(r.tools, strs(&["read", "bash"]));
    }
    #[test]
    fn parses_no_builtin_tools_with_explicit_tools_flags() {
        let r = parse(&["--no-builtin-tools", "--tools", "read,bash"]);
        assert_eq!(r.no_builtin_tools, Some(true));
        assert_eq!(r.tools, strs(&["read", "bash"]));
    }

    // messages and file args
    #[test]
    fn parses_plain_text_messages() {
        assert_eq!(parse(&["hello", "world"]).messages, vec!["hello", "world"]);
    }
    #[test]
    fn parses_file_arguments() {
        assert_eq!(
            parse(&["@README.md", "@src/main.ts"]).file_args,
            vec!["README.md", "src/main.ts"]
        );
    }
    #[test]
    fn parses_mixed_messages_and_file_args() {
        let r = parse(&["@file.txt", "explain this", "@image.png"]);
        assert_eq!(r.file_args, vec!["file.txt", "image.png"]);
        assert_eq!(r.messages, vec!["explain this"]);
    }
    #[test]
    fn captures_unknown_long_flags_with_string_values() {
        let r = parse(&["--unknown-flag", "message"]);
        assert!(r.messages.is_empty());
        assert_eq!(
            r.unknown_flag("unknown-flag"),
            Some(&FlagValue::String("message".into()))
        );
    }
    #[test]
    fn captures_unknown_boolean_long_flags() {
        assert_eq!(
            parse(&["--unknown-flag"]).unknown_flag("unknown-flag"),
            Some(&FlagValue::Bool(true))
        );
    }
    #[test]
    fn captures_unknown_long_flags_with_equals_syntax() {
        assert_eq!(
            parse(&["--unknown-flag=value"]).unknown_flag("unknown-flag"),
            Some(&FlagValue::String("value".into()))
        );
    }

    // --mode-path
    #[test]
    fn collects_mode_path_values_into_array() {
        assert_eq!(
            parse(&["--mode-path", "/team/modes", "--mode-path", "~/extra/modes"]).mode_paths,
            strs(&["/team/modes", "~/extra/modes"])
        );
    }
    #[test]
    fn mode_paths_is_undefined_when_no_flags_passed() {
        assert_eq!(parse(&[]).mode_paths, None);
    }

    // complex combinations
    #[test]
    fn parses_multiple_flags_together() {
        let r = parse(&[
            "--provider",
            "anthropic",
            "--model",
            "claude-sonnet",
            "--print",
            "--thinking",
            "high",
            "@prompt.md",
            "Do the task",
        ]);
        assert_eq!(r.provider.as_deref(), Some("anthropic"));
        assert_eq!(r.model.as_deref(), Some("claude-sonnet"));
        assert_eq!(r.print, Some(true));
        assert_eq!(r.thinking, Some(ThinkingLevel::High));
        assert_eq!(r.file_args, vec!["prompt.md"]);
        assert_eq!(r.messages, vec!["Do the task"]);
    }

    // Behavior of args.ts beyond args.test.ts.
    #[test]
    fn invalid_mode_is_silently_ignored() {
        let r = parse(&["--mode", "subagent"]);
        assert_eq!(r.mode, None);
        assert!(r.diagnostics.is_empty());
    }
    #[test]
    fn value_flag_without_value_falls_through_to_unknown_flag() {
        // `--model` as the last arg does not match the `i + 1 < length` branch.
        let r = parse(&["--model"]);
        assert_eq!(r.model, None);
        assert_eq!(r.unknown_flag("model"), Some(&FlagValue::Bool(true)));
    }
    #[test]
    fn unknown_short_option_is_an_error() {
        let r = parse(&["-x"]);
        assert_eq!(
            r.diagnostics,
            vec![Diagnostic {
                kind: DiagnosticKind::Error,
                message: "Unknown option: -x".into()
            }]
        );
    }
    #[test]
    fn invalid_thinking_level_warns() {
        let r = parse(&["--thinking", "max"]);
        assert_eq!(r.thinking, None);
        assert_eq!(
            r.diagnostics[0].message,
            "Invalid thinking level \"max\". Valid values: off, minimal, low, medium, high, xhigh"
        );
        assert_eq!(r.diagnostics[0].kind, DiagnosticKind::Warning);
    }
    #[test]
    fn max_turns_uses_parse_int_semantics() {
        assert_eq!(parse(&["--max-turns", "12abc"]).max_turns, Some(12));
        assert_eq!(parse(&["--max-turns", "-3"]).max_turns, None);
        assert_eq!(parse(&["--max-turns", "abc"]).max_turns, None);
    }
    #[test]
    fn platform_accumulates_across_repeats() {
        assert_eq!(
            parse(&["--platform", "claude, gh", "--platform", ",agents"]).platform,
            strs(&["claude", "gh", "agents"])
        );
    }
    #[test]
    fn list_models_optional_pattern() {
        assert_eq!(parse(&["--list-models"]).list_models, Some(ListModels::All));
        assert_eq!(
            parse(&["--list-models", "sonnet"]).list_models,
            Some(ListModels::Search("sonnet".into()))
        );
        assert_eq!(
            parse(&["--list-models", "@x"]).list_models,
            Some(ListModels::All)
        );
    }
    #[test]
    fn unknown_flag_set_overwrites_in_place() {
        let r = parse(&["--a", "--b", "x", "--a=2"]);
        assert_eq!(
            r.unknown_flags,
            vec![
                ("a".to_string(), FlagValue::String("2".into())),
                ("b".to_string(), FlagValue::String("x".into())),
            ]
        );
    }
}
