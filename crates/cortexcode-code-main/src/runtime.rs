//! Runtime glue between the `cortex` CLI and the agent namespace.
//!
//! This module builds an `Agent` from CLI arguments, wires up the default
//! coding tools, and dispatches to print or interactive mode. It is the
//! integration point that turns the previously stubbed CLI commands into
//! actual LLM-backed sessions.

use crate::Args;
use cortexcode_agent_core::PromptInput;
use cortexcode_agent_core::{Agent, AgentOptions};
use cortexcode_agent_types::{AgentMessage, AgentState, PermissionGate};
use cortexcode_ai_env::get_env_api_key;
use cortexcode_ai_models::get_model;
use cortexcode_ai_types::{Content, Message, TextContent, UserMessage};
use cortexcode_code_config::Config;
use cortexcode_code_print::{format_text_output, PrintFormatter, PrintMode};
use cortexcode_code_prompts::{initial_user_prompt, system_prompt, Mode};
use cortexcode_code_tools::{permissions::PermissionPolicy, PolicyPermissionGate};
use std::io::Write;
use std::sync::Arc;

/// Error type for runtime operations.
#[derive(Debug)]
pub enum RuntimeError {
    Setup(String),
    Agent(String),
    Print(cortexcode_code_print::PrintError),
}

impl std::fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RuntimeError::Setup(e) => write!(f, "setup error: {}", e),
            RuntimeError::Agent(e) => write!(f, "agent error: {}", e),
            RuntimeError::Print(e) => write!(f, "print error: {}", e),
        }
    }
}

impl std::error::Error for RuntimeError {}

impl From<cortexcode_code_print::PrintError> for RuntimeError {
    fn from(e: cortexcode_code_print::PrintError) -> Self {
        RuntimeError::Print(e)
    }
}

impl From<Box<dyn std::error::Error + Send + Sync>> for RuntimeError {
    fn from(e: Box<dyn std::error::Error + Send + Sync>) -> Self {
        RuntimeError::Agent(e.to_string())
    }
}

impl From<std::io::Error> for RuntimeError {
    fn from(e: std::io::Error) -> Self {
        RuntimeError::Print(e.into())
    }
}

/// Resolve the provider and model from CLI arguments, falling back to the
/// persisted/migrated config file and finally to hardcoded defaults.
fn resolve_provider_model(args: &Args) -> Result<(String, String), RuntimeError> {
    let config = crate::config_or_default(args);
    Ok(resolve_provider_model_with_config(args, &config))
}

/// Pure variant of [`resolve_provider_model`] taking an explicit config, so
/// the fallback precedence can be unit-tested without touching the
/// filesystem.
fn resolve_provider_model_with_config(args: &Args, config: &Config) -> (String, String) {
    let provider = args
        .provider
        .clone()
        .or_else(|| config.provider.clone())
        .unwrap_or_else(|| "anthropic".to_string());
    let model = args
        .model
        .clone()
        .or_else(|| {
            // Only trust the config's model if it was paired with the same
            // provider (or no provider override was requested at all).
            if args.provider.is_none() || args.provider.as_deref() == config.provider.as_deref() {
                config.model.clone()
            } else {
                None
            }
        })
        .unwrap_or_else(|| default_model_for_provider(&provider));
    (provider, model)
}

fn default_model_for_provider(provider: &str) -> String {
    match provider {
        "anthropic" => "claude-sonnet-4-5".to_string(),
        "openai" => "gpt-4o".to_string(),
        "google" => "gemini-2.5-pro".to_string(),
        "azure" => "gpt-4o".to_string(),
        _ => "unknown".to_string(),
    }
}

/// Resolve the API key for the provider.
fn resolve_api_key(provider: &str, args: &Args) -> Option<String> {
    let config = crate::config_or_default(args);
    resolve_api_key_with_config(provider, args, &config)
}

/// Pure variant of [`resolve_api_key`] taking an explicit config, so the
/// fallback precedence (CLI flag > per-provider config > global config >
/// environment variable) can be unit-tested without touching the
/// filesystem.
fn resolve_api_key_with_config(provider: &str, args: &Args, config: &Config) -> Option<String> {
    if let Some(key) = &args.api_key {
        return Some(key.clone());
    }
    if let Some(provider_config) = config.providers.get(provider) {
        if let Some(key) = &provider_config.api_key {
            return Some(key.clone());
        }
    }
    if let Some(key) = &config.api_key {
        return Some(key.clone());
    }
    if let Some(key) = get_env_api_key(provider) {
        return Some(key);
    }
    oauth_api_key(provider)
}

/// Fall back to an OAuth access token persisted by `cortex --login <provider>`,
/// refreshing it first if it has expired.
fn oauth_api_key(provider: &str) -> Option<String> {
    let store_key = match provider {
        "anthropic" | "claude" => "anthropic",
        "github-copilot" | "github" | "copilot" => "github-copilot",
        _ => return None,
    };
    let store = crate::auth::CredentialStore::default_location();
    let credentials = store.get(store_key)?;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);

    if !credentials.is_expired(now) {
        return Some(credentials.access);
    }

    // Expired: attempt a refresh, persisting the new tokens on success.
    let refreshed = match store_key {
        "anthropic" => cortexcode_ai_oauth::anthropic::refresh_token(&credentials.refresh).ok(),
        "github-copilot" => {
            let enterprise = credentials
                .extra
                .get("enterprise_url")
                .and_then(|v| v.as_str());
            cortexcode_ai_oauth::github_copilot::refresh_token(&credentials.refresh, enterprise)
                .ok()
        }
        _ => None,
    };
    match refreshed {
        Some(fresh) => {
            let _ = store.save(store_key, &fresh);
            Some(fresh.access)
        }
        // Refresh failed (offline, revoked); fall back to the stale token.
        None => Some(credentials.access),
    }
}

/// Build the system prompt from CLI arguments.
fn build_system_prompt(args: &Args) -> String {
    if let Some(prompt) = &args.system_prompt {
        return prompt.clone();
    }
    let config = crate::config_or_default(args);
    let mode = args
        .mode
        .as_deref()
        .and_then(|m| m.parse::<Mode>().ok())
        .unwrap_or_default();
    system_prompt(mode, &config)
}

/// Build the default set of coding tools.
fn build_tools() -> Vec<cortexcode_agent_types::AgentTool> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    cortexcode_code_tools::default_tools(cwd, PermissionPolicy::default())
}

/// Build the permission gate for the current CLI mode.
fn build_permission_gate(args: &Args, interactive: bool) -> Arc<dyn PermissionGate> {
    let config = crate::config_or_default(args);

    if interactive {
        let inner = Arc::new(crate::permission_dialog::InteractivePermissionGate);
        return Arc::new(PolicyPermissionGate::new(
            PermissionPolicy::Ask,
            config.auto_approve_read_only(),
            Some(inner),
        ));
    }

    // Non-interactive (print) mode: auto-approve dangerous tools.
    Arc::new(PolicyPermissionGate::new(
        PermissionPolicy::Auto,
        config.auto_approve_read_only(),
        None,
    ))
}

/// Build an `Agent` from CLI arguments with a configured permission gate.
fn build_agent_with_gate(args: &Args, interactive: bool) -> Result<Agent, RuntimeError> {
    let (provider, model_id) = resolve_provider_model(args)?;
    let model = get_model(&provider, &model_id)
        .cloned()
        .ok_or_else(|| RuntimeError::Setup(format!("unknown model {}:{}", provider, model_id)))?;

    let api_key = resolve_api_key(&provider, args);
    if api_key.is_none() {
        return Err(RuntimeError::Setup(format!(
            "no API key found for provider {}",
            provider
        )));
    }

    let system_prompt = build_system_prompt(args);
    let tools = build_tools();

    let state = AgentState {
        system_prompt,
        model,
        thinking_level: cortexcode_ai_types::ThinkingLevel::Off,
        tools: cortexcode_agent_types::AgentTools::new(tools),
        messages: Vec::new(),
        is_streaming: false,
        streaming_message: None,
        pending_tool_calls: std::collections::HashSet::new(),
        error_message: None,
    };

    let permission_gate = Some(build_permission_gate(args, interactive));

    let agent = Agent::with_options(AgentOptions {
        initial_state: Some(state),
        api_key,
        permission_gate,
        ..Default::default()
    });

    Ok(agent)
}

/// Build the initial user messages from CLI arguments.
fn build_user_messages(args: &Args) -> Vec<AgentMessage> {
    let mode = args
        .mode
        .as_deref()
        .and_then(|m| m.parse::<Mode>().ok())
        .unwrap_or_default();

    let mut messages = Vec::new();

    // Add file args as user messages.
    for path in &args.file_args {
        if let Ok(content) = std::fs::read_to_string(path) {
            let text = format!("File {}:\n```\n{}\n```", path, content);
            messages.push(text_message(&text));
        }
    }

    // Add explicit prompt messages.
    for text in &args.messages {
        messages.push(text_message(&initial_user_prompt(mode, text)));
    }

    messages
}

fn text_message(text: &str) -> AgentMessage {
    AgentMessage::from_message(Message::User(UserMessage {
        content: vec![Content::Text(TextContent {
            text: text.to_string(),
            cache_control: None,
        })],
        timestamp: None,
    }))
}

/// Run the agent once in print mode and write the result to `output`.
pub fn run_print_mode(
    args: &Args,
    mode: PrintMode,
    output: &mut dyn Write,
    err: &mut dyn Write,
) -> Result<(), RuntimeError> {
    let agent = build_agent_with_gate(args, false)?;
    let messages = build_user_messages(args);

    let formatter = std::sync::Arc::new(std::sync::Mutex::new(PrintFormatter::new(mode)));
    let formatter_for_sub = formatter.clone();
    let _sub = agent.subscribe(Box::new(move |event| {
        if let Ok(mut fmt) = formatter_for_sub.lock() {
            fmt.record(event);
        }
    }));

    let result = agent.prompt(PromptInput::Messages(messages))?;

    match mode {
        PrintMode::Text => {
            writeln!(output, "{}", format_text_output(&result))?;
        }
        PrintMode::Json => {
            let formatter = std::sync::Arc::try_unwrap(formatter)
                .ok()
                .and_then(|m| m.into_inner().ok())
                .unwrap_or_default();
            formatter.finalize(output)?;
        }
    }

    if let Some(last) = result.last() {
        if let Some(Message::Assistant(am)) = last.extract_message() {
            if let Some(error) = &am.error_message {
                writeln!(err, "error: {}", error)?;
            }
        }
    }

    Ok(())
}

/// Run the agent in an interactive TUI loop.
pub fn run_interactive_mode(
    args: &Args,
    output: &mut dyn Write,
    err: &mut dyn Write,
) -> Result<(), RuntimeError> {
    use crossterm::{
        cursor, event,
        style::{self, Stylize},
        terminal, QueueableCommand,
    };
    use std::io::Write as _;

    let agent = build_agent_with_gate(args, true)?;
    let mut stdout = std::io::stdout();
    terminal::enable_raw_mode().map_err(|e| RuntimeError::Setup(e.to_string()))?;
    let _ = stdout
        .queue(terminal::Clear(terminal::ClearType::All))?
        .queue(cursor::MoveTo(0, 0))?
        .flush();

    writeln!(
        output,
        "{} Interactive Cortex mode. /quit or Ctrl+C to exit.",
        "TUI".bold()
    )?;

    let mut input = String::new();
    loop {
        let _ = stdout
            .queue(cursor::MoveToColumn(0))?
            .queue(terminal::Clear(terminal::ClearType::CurrentLine))?
            .queue(style::Print("cortex> "))?
            .queue(style::Print(&input))?
            .flush();

        if event::poll(std::time::Duration::from_millis(100)).unwrap_or(false) {
            if let Ok(event::Event::Key(key)) = event::read() {
                match key.code {
                    event::KeyCode::Enter => {
                        let line = input.trim();
                        if line == "/quit" {
                            break;
                        }
                        if !line.is_empty() {
                            writeln!(output, "\nYou: {}", line)?;
                            let user_msg = text_message(&initial_user_prompt(
                                args.mode
                                    .as_deref()
                                    .and_then(|m| m.parse::<Mode>().ok())
                                    .unwrap_or_default(),
                                line,
                            ));
                            match agent.prompt(PromptInput::Messages(vec![user_msg])) {
                                Ok(messages) => {
                                    let text = format_text_output(&messages);
                                    if !text.is_empty() {
                                        writeln!(output, "Cortex: {}\n", text)?;
                                    } else {
                                        writeln!(output, "Cortex: (no response)\n")?;
                                    }
                                }
                                Err(e) => writeln!(output, "Cortex error: {}\n", e)?,
                            }
                        }
                        input.clear();
                    }
                    event::KeyCode::Char(c) => {
                        if key.modifiers == event::KeyModifiers::CONTROL && c == 'c' {
                            break;
                        }
                        input.push(c);
                    }
                    event::KeyCode::Backspace => {
                        input.pop();
                    }
                    _ => {}
                }
            }
        }
    }

    let _ = terminal::disable_raw_mode();
    writeln!(err, "interactive session ended")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_model_for_provider() {
        assert!(!default_model_for_provider("anthropic").is_empty());
    }

    #[test]
    fn test_text_message() {
        let msg = text_message("hello");
        assert!(msg.extract_message().is_some());
    }

    #[test]
    fn test_resolve_provider_model_cli_args_win_over_config() {
        let args = Args {
            provider: Some("openai".into()),
            model: Some("gpt-4".into()),
            ..Default::default()
        };
        let config = Config {
            provider: Some("anthropic".into()),
            model: Some("claude-sonnet-4".into()),
            ..Default::default()
        };
        assert_eq!(
            resolve_provider_model_with_config(&args, &config),
            ("openai".to_string(), "gpt-4".to_string())
        );
    }

    #[test]
    fn test_resolve_provider_model_falls_back_to_config() {
        let args = Args::default();
        let config = Config {
            provider: Some("anthropic".into()),
            model: Some("claude-opus-4".into()),
            ..Default::default()
        };
        assert_eq!(
            resolve_provider_model_with_config(&args, &config),
            ("anthropic".to_string(), "claude-opus-4".to_string())
        );
    }

    #[test]
    fn test_resolve_provider_model_ignores_mismatched_config_model() {
        // Config's default model belongs to a different provider than the
        // one requested on the CLI, so it must not leak across providers.
        let args = Args {
            provider: Some("openai".into()),
            ..Default::default()
        };
        let config = Config {
            provider: Some("anthropic".into()),
            model: Some("claude-opus-4".into()),
            ..Default::default()
        };
        let (provider, model) = resolve_provider_model_with_config(&args, &config);
        assert_eq!(provider, "openai");
        assert_eq!(model, default_model_for_provider("openai"));
    }

    #[test]
    fn test_resolve_provider_model_no_args_no_config_uses_defaults() {
        let args = Args::default();
        let config = Config::default();
        let (provider, model) = resolve_provider_model_with_config(&args, &config);
        assert_eq!(provider, "anthropic");
        assert_eq!(model, default_model_for_provider("anthropic"));
    }

    #[test]
    fn test_resolve_api_key_cli_arg_wins() {
        let args = Args {
            api_key: Some("cli-key".into()),
            ..Default::default()
        };
        let config = Config {
            api_key: Some("config-key".into()),
            ..Default::default()
        };
        assert_eq!(
            resolve_api_key_with_config("anthropic", &args, &config),
            Some("cli-key".to_string())
        );
    }

    #[test]
    fn test_resolve_api_key_prefers_provider_specific_config() {
        let args = Args::default();
        let mut config = Config {
            api_key: Some("global-key".into()),
            ..Default::default()
        };
        config.providers.insert(
            "anthropic".into(),
            cortexcode_code_config::ProviderConfig {
                api_key: Some("provider-key".into()),
                ..Default::default()
            },
        );
        assert_eq!(
            resolve_api_key_with_config("anthropic", &args, &config),
            Some("provider-key".to_string())
        );
    }

    #[test]
    fn test_resolve_api_key_falls_back_to_global_config() {
        let args = Args::default();
        let config = Config {
            api_key: Some("global-key".into()),
            ..Default::default()
        };
        assert_eq!(
            resolve_api_key_with_config("anthropic", &args, &config),
            Some("global-key".to_string())
        );
    }
}
