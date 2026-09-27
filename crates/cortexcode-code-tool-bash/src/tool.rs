//! The `bash` tool (`createBashToolDefinition` in core/tools/bash.ts).
//! Interactive rendering (`renderCall`/`renderResult`) arrives with phase 11.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cortexcode_agent_types::{AgentTool, AgentToolResult, AgentToolUpdateCallback};
use cortexcode_ai_types::{Content, TextContent};
use cortexcode_code_tool_api::{
    format_size, wrap_tool_definition, ToolContextFactory, ToolDefinition, ToolError, TruncatedBy,
    DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES,
};
use serde_json::{json, Map, Value};

use crate::accumulator::{OutputAccumulator, OutputAccumulatorOptions, OutputSnapshot};
use crate::operations::{BashExecOptions, BashOperations, LocalBashOperations};
use crate::shell::{get_shell_env, ShellEnv};

/// `BashSpawnContext`: what a spawn hook may adjust.
#[derive(Debug, Clone)]
pub struct BashSpawnContext {
    pub command: String,
    pub cwd: PathBuf,
    pub env: ShellEnv,
}

/// `BashSpawnHook`.
pub type BashSpawnHook = Arc<dyn Fn(BashSpawnContext) -> BashSpawnContext + Send + Sync>;

/// `BashToolOptions`.
#[derive(Clone, Default)]
pub struct BashToolOptions {
    /// How commands run. Default: the local shell.
    pub operations: Option<Arc<dyn BashOperations>>,
    /// Prepended to every command on its own line (the `shellCommandPrefix`
    /// setting).
    pub command_prefix: Option<String>,
    /// Explicit shell (the `shellPath` setting).
    pub shell_path: Option<String>,
    pub spawn_hook: Option<BashSpawnHook>,
    /// Regex patterns; when set, a command must match one of them.
    pub allowed_commands: Vec<String>,
    /// Regex patterns; a matching command is blocked (checked first).
    pub denied_commands: Vec<String>,
    /// Byte cap on the returned output (tail kept). Default 32 KiB.
    pub max_output_bytes: Option<usize>,
    /// Line cap on the returned output (tail kept). Default 800.
    pub max_output_lines: Option<usize>,
}

const BASH_UPDATE_THROTTLE: Duration = Duration::from_millis(100);

fn text_content(text: impl Into<String>) -> Content {
    Content::Text(TextContent {
        text_signature: None,
        text: text.into(),
    })
}

/// The TypeBox schema hoocode sends for `bash`.
pub fn bash_parameters_schema() -> Value {
    json!({
        "type": "object",
        "required": ["command"],
        "properties": {
            "command": {"type": "string", "description": "Bash command to execute"},
            "timeout": {"type": "number", "description": "Timeout in seconds (optional, no default timeout)"}
        }
    })
}

/// `new RegExp(pattern).test(command)`; an invalid pattern never matches.
fn matches(pattern: &str, command: &str) -> bool {
    regex_lite::Regex::new(pattern).is_ok_and(|re| re.is_match(command))
}

fn check_command(options: &BashToolOptions, command: &str) -> Result<(), ToolError> {
    if let Some(pattern) = options.denied_commands.iter().find(|p| matches(p, command)) {
        return Err(format!("Command blocked: matches denied pattern \"{pattern}\"").into());
    }
    if !options.allowed_commands.is_empty()
        && !options.allowed_commands.iter().any(|p| matches(p, command))
    {
        return Err("Command blocked: does not match any allowed command pattern".into());
    }
    Ok(())
}

/// Output plus the throttle state `emitOutputUpdate` works on.
struct Progress {
    output: OutputAccumulator,
    dirty: bool,
    last_update: Option<Instant>,
}

type SharedUpdate = Arc<Mutex<AgentToolUpdateCallback>>;

fn snapshot_details(snapshot: &OutputSnapshot) -> Value {
    let mut details = Map::new();
    if snapshot.truncation.truncated {
        details.insert(
            "truncation".into(),
            serde_json::to_value(&snapshot.truncation).expect("truncation serializes"),
        );
    }
    if let Some(path) = &snapshot.full_output_path {
        details.insert("fullOutputPath".into(), path.clone().into());
    }
    Value::Object(details)
}

/// `emitOutputUpdate`.
fn emit(progress: &mut Progress, on_update: &SharedUpdate) {
    if !progress.dirty {
        return;
    }
    progress.dirty = false;
    progress.last_update = Some(Instant::now());
    let snapshot = progress.output.snapshot(true);
    let update = AgentToolResult {
        content: vec![text_content(snapshot.content.clone())],
        details: snapshot_details(&snapshot),
        terminate: false,
    };
    let callback = on_update.lock().unwrap_or_else(|e| e.into_inner());
    callback(update);
}

/// `formatOutput`: the snapshot text plus the truncation notice.
fn format_output(
    snapshot: &OutputSnapshot,
    last_line_bytes: usize,
    max_bytes: usize,
    empty_text: &str,
) -> (String, Value) {
    let truncation = &snapshot.truncation;
    let mut text = if snapshot.content.is_empty() {
        empty_text.to_owned()
    } else {
        snapshot.content.clone()
    };
    let mut details = Value::Null;
    if truncation.truncated {
        let mut map = Map::new();
        map.insert(
            "truncation".into(),
            serde_json::to_value(truncation).expect("truncation serializes"),
        );
        if let Some(path) = &snapshot.full_output_path {
            map.insert("fullOutputPath".into(), path.clone().into());
        }
        details = Value::Object(map);
        let path = snapshot.full_output_path.as_deref().unwrap_or("undefined");
        let start_line = truncation.total_lines + 1 - truncation.output_lines;
        let end_line = truncation.total_lines;
        if truncation.last_line_partial {
            text.push_str(&format!(
                "\n\n[Showing last {} of line {end_line} (line is {}). Full output: {path}]",
                format_size(truncation.output_bytes),
                format_size(last_line_bytes)
            ));
        } else if truncation.truncated_by == Some(TruncatedBy::Lines) {
            text.push_str(&format!(
                "\n\n[Showing lines {start_line}-{end_line} of {}. Full output: {path}]",
                truncation.total_lines
            ));
        } else {
            text.push_str(&format!(
                "\n\n[Showing lines {start_line}-{end_line} of {} ({} limit). Full output: {path}]",
                truncation.total_lines,
                format_size(max_bytes)
            ));
        }
    }
    (text, details)
}

fn append_status(text: &str, status: &str) -> String {
    if text.is_empty() {
        status.to_owned()
    } else {
        format!("{text}\n\n{status}")
    }
}

struct BashCall<'a> {
    cwd: &'a PathBuf,
    ops: &'a dyn BashOperations,
    options: &'a BashToolOptions,
    max_bytes: usize,
    max_lines: usize,
}

impl BashCall<'_> {
    fn run(
        &self,
        command: &str,
        timeout: Option<f64>,
        signal: Option<cortexcode_ai_types::AbortSignal>,
        on_update: Option<AgentToolUpdateCallback>,
    ) -> Result<AgentToolResult, ToolError> {
        check_command(self.options, command)?;

        let resolved = match &self.options.command_prefix {
            Some(prefix) if !prefix.is_empty() => format!("{prefix}\n{command}"),
            _ => command.to_owned(),
        };
        let base = BashSpawnContext {
            command: resolved,
            cwd: self.cwd.clone(),
            env: get_shell_env(),
        };
        let spawn = match &self.options.spawn_hook {
            Some(hook) => hook(base),
            None => base,
        };

        let progress = Arc::new(Mutex::new(Progress {
            output: OutputAccumulator::new(OutputAccumulatorOptions {
                max_lines: Some(self.max_lines),
                max_bytes: Some(self.max_bytes),
                temp_file_prefix: Some("cortex-bash".into()),
                command: None,
            }),
            dirty: false,
            last_update: None,
        }));
        let on_update: Option<SharedUpdate> = on_update.map(|f| Arc::new(Mutex::new(f)));
        if let Some(callback) = &on_update {
            let callback = callback.lock().unwrap_or_else(|e| e.into_inner());
            callback(AgentToolResult {
                content: Vec::new(),
                details: Value::Null,
                terminate: false,
            });
        }

        // The throttle timer: flush pending output once 100 ms have passed
        // since the last update, even when no new data arrives.
        let running = Arc::new(AtomicBool::new(true));
        let flusher = on_update.clone().map(|callback| {
            let progress = progress.clone();
            let running = running.clone();
            std::thread::spawn(move || {
                while running.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(10));
                    let mut p = progress.lock().unwrap_or_else(|e| e.into_inner());
                    let due = p
                        .last_update
                        .is_none_or(|t| t.elapsed() >= BASH_UPDATE_THROTTLE);
                    if p.dirty && due {
                        emit(&mut p, &callback);
                    }
                }
            })
        });

        let mut on_data = |data: &[u8]| {
            let mut p = progress.lock().unwrap_or_else(|e| e.into_inner());
            p.output.append(data);
            if let Some(callback) = &on_update {
                p.dirty = true;
                let due = p
                    .last_update
                    .is_none_or(|t| t.elapsed() >= BASH_UPDATE_THROTTLE);
                if due {
                    emit(&mut p, callback);
                }
            }
        };
        let result = self.ops.exec(
            &spawn.command,
            &spawn.cwd,
            BashExecOptions {
                on_data: &mut on_data,
                signal,
                timeout,
                env: Some(spawn.env),
            },
        );

        running.store(false, Ordering::Relaxed);
        if let Some(flusher) = flusher {
            let _ = flusher.join();
        }

        // finishOutput
        let mut p = progress.lock().unwrap_or_else(|e| e.into_inner());
        p.output.finish();
        if let Some(callback) = &on_update {
            emit(&mut p, callback);
        }
        let snapshot = p.output.snapshot(true);
        p.output.close_temp_file()?;
        let last_line_bytes = p.output.last_line_bytes();
        drop(p);

        match result {
            Err(error) => {
                let (text, _) = format_output(&snapshot, last_line_bytes, self.max_bytes, "");
                let message = error.to_string();
                if message == "aborted" {
                    return Err(append_status(&text, "Command aborted").into());
                }
                if let Some(secs) = message.strip_prefix("timeout:") {
                    let status = format!("Command timed out after {secs} seconds");
                    return Err(append_status(&text, &status).into());
                }
                Err(error)
            }
            Ok(exit_code) => {
                let (text, details) =
                    format_output(&snapshot, last_line_bytes, self.max_bytes, "(no output)");
                if let Some(code) = exit_code.filter(|c| *c != 0) {
                    let status = format!("Command exited with code {code}");
                    return Err(append_status(&text, &status).into());
                }
                Ok(AgentToolResult {
                    content: vec![text_content(text)],
                    details,
                    terminate: false,
                })
            }
        }
    }
}

/// `createBashToolDefinition`.
pub fn create_bash_tool_definition(
    cwd: impl Into<PathBuf>,
    options: BashToolOptions,
) -> ToolDefinition {
    let cwd: PathBuf = cwd.into();
    let ops: Arc<dyn BashOperations> = options
        .operations
        .clone()
        .unwrap_or_else(|| Arc::new(LocalBashOperations::new(options.shell_path.clone())));
    let max_bytes = options.max_output_bytes.unwrap_or(DEFAULT_MAX_BYTES);
    let max_lines = options.max_output_lines.unwrap_or(DEFAULT_MAX_LINES);
    // Math.round(maxBytes / 1024)
    let kb = (max_bytes * 2 + 1024) / 2048;
    let description = format!(
        "Execute a bash command in the current working directory. Returns stdout and stderr. Output is truncated to last {max_lines} lines or {kb}KB (whichever is hit first). If truncated, full output is saved to a temp file. Optionally provide a timeout in seconds."
    );
    ToolDefinition {
        name: "bash".into(),
        label: "bash".into(),
        description,
        prompt_snippet: Some("Run builds, tests, linters, git, and package managers".into()),
        prompt_guidelines: Vec::new(),
        parameters: bash_parameters_schema(),
        prepare_arguments: None,
        execution_mode: None,
        background: false,
        execute: Arc::new(move |_tool_call_id, args, signal, on_update, _ctx| {
            let command = args
                .get("command")
                .and_then(Value::as_str)
                .ok_or("The \"command\" argument must be of type string")?;
            let timeout = args.get("timeout").and_then(Value::as_f64);
            BashCall {
                cwd: &cwd,
                ops: ops.as_ref(),
                options: &options,
                max_bytes,
                max_lines,
            }
            .run(command, timeout, signal, on_update)
        }),
    }
}

/// `createBashTool`: the definition wrapped for the agent loop.
pub fn create_bash_tool(
    cwd: impl Into<PathBuf>,
    options: BashToolOptions,
    ctx_factory: Option<ToolContextFactory>,
) -> AgentTool {
    wrap_tool_definition(create_bash_tool_definition(cwd, options), ctx_factory)
}
