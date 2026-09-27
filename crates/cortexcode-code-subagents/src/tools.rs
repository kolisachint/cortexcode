//! `core/tools/subagent.ts`: the `Task` tool (delegate a focused task to a
//! specialized subagent) and the `TaskOutput` tool (check on and collect
//! background subagents).
//!
//! The parent agent decides when to delegate and picks the agent with
//! `subagent_type`; the chosen agent runs in an isolated child process (the
//! cold pool, or a warm RPC worker when enabled) and only its final answer
//! comes back. Background dispatches leave their body in the inbox and return
//! a compact notification; `TaskOutput` pulls it. Interactive rendering
//! (`renderCall`/`renderResult`) arrives with the TUI (phase 11).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use cortexcode_agent_types::{AgentToolCall, AgentToolResult};
use cortexcode_ai_types::{AbortSignal, Content, Model};
use cortexcode_code_agent_session::provider_health::get_provider_exhaustion;
use cortexcode_code_resources::{load_agent_registry, LoadAgentRegistryOptions, TASK_TOOL_NAME};
use cortexcode_code_session::SessionManager;
use cortexcode_code_task_store::{
    task_store, AgentStats, CreateTaskOptions, TaskAgentKind, TaskAgentPatch, TaskAgentState,
    TaskPatch, TaskSource, TaskStatus, TaskUsage,
};
use cortexcode_code_tool_api::{ToolContext, ToolDefinition, ToolError};
use serde_json::{json, Map, Value};

use crate::agent_log::agent_log;
use crate::depth::{delegate_allow_list, is_delegate_allowed, ProcessEnv};
use crate::inbox::{subagent_inbox, InboxRecord, TaskLifecycle};
use crate::instance::get_subagent_pool;
use crate::pool::{DispatchOptions, ResultStatus, SubagentPool, SubagentResult, TaskResult};
use crate::warm::{
    get_warm_subagent_pool, warm_subagents_enabled, WarmDispatchOptions, WarmProgressCallback,
    WarmRunResult,
};

pub use cortexcode_code_resources::summarize_agent_description;

const TASK_MAIN_PROMPT: &str = include_str!("../templates/prompts/task-main.md");
const TASK_BACKGROUND_AGENTS_PROMPT: &str =
    include_str!("../templates/prompts/task-background-agents.md");
const TASK_BACKGROUND_NONE_PROMPT: &str =
    include_str!("../templates/prompts/task-background-none.md");

/// Default wait for `TaskOutput(wait: true)`.
const TASK_OUTPUT_DEFAULT_TIMEOUT_MS: u64 = 120_000;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn text_result(text: impl Into<String>, details: Value) -> AgentToolResult {
    AgentToolResult {
        content: vec![Content::text(text)],
        details,
        terminate: false,
    }
}

/// Names of agents configured to run in the background.
fn collect_background_agent_names(cwd: &Path) -> HashSet<String> {
    load_agent_registry(&LoadAgentRegistryOptions::new(cwd.to_string_lossy()))
        .list()
        .iter()
        .filter(|a| a.background == Some(true))
        .map(|a| a.name.clone())
        .collect()
}

/// `buildTaskMainPrompt`: the main-session delegation instructions appended
/// to the system prompt. The heavier background/barrier guidance is only
/// included when the project has background-capable agents.
pub fn build_task_main_prompt(cwd: &Path) -> String {
    let guidance = if collect_background_agent_names(cwd).is_empty() {
        TASK_BACKGROUND_NONE_PROMPT
    } else {
        TASK_BACKGROUND_AGENTS_PROMPT
    };
    TASK_MAIN_PROMPT
        .replacen("{{BACKGROUND_GUIDANCE}}", guidance.trim(), 1)
        .trim()
        .to_string()
}

/// JS `.length` (UTF-16 units) and `slice(0, n)`.
fn js_truncate(text: &str, max: usize) -> String {
    let units: Vec<u16> = text.encode_utf16().collect();
    if units.len() > max {
        format!("{}\u{2026}", String::from_utf16_lossy(&units[..max - 1]))
    } else {
        text.to_string()
    }
}

/// A glanceable task name: the first line, at most 8 words and 60 chars.
fn summarize(task: &str) -> String {
    let first = task.trim().split('\n').next().unwrap_or("").trim();
    if first.is_empty() {
        return "(task)".into();
    }
    let words: Vec<&str> = first.split_whitespace().collect();
    let name = if words.len() > 8 {
        format!("{}\u{2026}", words[..8].join(" "))
    } else {
        first.to_string()
    };
    js_truncate(&name, 60)
}

/// `formatDurationSecs`.
pub fn format_duration_secs(secs: f64) -> String {
    let s = secs.max(0.0);
    let round = |x: f64| (x + 0.5).floor();
    if s < 10.0 {
        return format!("{:.1}s", (s * 10.0).round() / 10.0);
    }
    if s < 60.0 {
        return format!("{}s", round(s));
    }
    if s < 3600.0 {
        let mins = (s / 60.0).floor();
        let rem = round(s % 60.0);
        return format!("{mins}m{rem:02}s");
    }
    let hrs = (s / 3600.0).floor();
    let rem = round((s % 3600.0) / 60.0);
    format!("{hrs}h{rem:02}m")
}

/// A pool task id for a dispatch (the pool's own format).
fn new_dispatch_task_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
        ^ COUNTER
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let suffix: String = (0..6)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            DIGITS[(seed % 36) as usize] as char
        })
        .collect();
    format!("dispatch-{}-{suffix}", now_ms())
}

/// The single in_progress main-agent TodoWrite item, when unambiguous.
fn linked_todo_id() -> Option<u64> {
    let in_progress: Vec<u64> = task_store()
        .list()
        .into_iter()
        .filter(|t| {
            t.source.is_none()
                && t.agent.is_none()
                && t.parent_task_id.is_none()
                && t.status == TaskStatus::InProgress
        })
        .map(|t| t.id)
        .collect();
    (in_progress.len() == 1).then(|| in_progress[0])
}

/// A roster row per dispatch, keyed by its run (pool task) id.
fn register_subagent_dispatch(run_id: &str, label: &str) {
    task_store().upsert_agent(
        run_id,
        label,
        TaskAgentKind::Subagent,
        TaskAgentPatch {
            role: Some("subagent".into()),
            state: Some(TaskAgentState::Running),
            ..Default::default()
        },
    );
}

fn patch_agent_state(run_id: &str, state: TaskAgentState) {
    task_store().patch_agent(
        run_id,
        TaskAgentPatch {
            state: Some(state),
            activity: Some(String::new()),
            ..Default::default()
        },
    );
}

fn set_status(task_id: u64, status: TaskStatus) {
    task_store().update(
        task_id,
        TaskPatch {
            status: Some(status),
            ..Default::default()
        },
    );
}

/// Mark the dispatch failed unless it already settled (a cancel stays one).
fn mark_dispatch_failed(task_id: u64, run_id: &str) {
    let current = task_store().list().into_iter().find(|t| t.id == task_id);
    if current.is_some_and(|t| !matches!(t.status, TaskStatus::Pending | TaskStatus::InProgress)) {
        return;
    }
    set_status(task_id, TaskStatus::Failed);
    patch_agent_state(run_id, TaskAgentState::Failed);
}

fn create_dispatch_task(summary: &str, subagent_type: &str, run_id: &str) -> u64 {
    task_store()
        .create(
            summary,
            CreateTaskOptions {
                source: Some(TaskSource::Subagent),
                subagent_mode: Some(subagent_type.to_string()),
                agent: Some(run_id.to_string()),
                linked_task_id: linked_todo_id(),
                ..Default::default()
            },
        )
        .id
}

/// For a `fork: true` agent, fork the parent's session so the subagent
/// inherits its conversation; `None` falls back to a fresh session.
pub fn resolve_fork_session_file(
    fork: Option<bool>,
    parent_session: Option<&Path>,
    cwd: &Path,
) -> Option<PathBuf> {
    if fork != Some(true) {
        return None;
    }
    let parent = parent_session?;
    SessionManager::fork_from(parent, cwd.to_string_lossy(), None)
        .ok()?
        .session_file()
        .map(Path::to_path_buf)
}

fn usage_of(value: Option<&Value>) -> Option<TaskUsage> {
    let usage = value?.as_object()?;
    let num = |k: &str| usage.get(k).and_then(Value::as_f64).unwrap_or(0.0);
    Some(TaskUsage {
        input: num("input"),
        output: num("output"),
        cache_read: num("cacheRead"),
        cache_write: num("cacheWrite"),
        cost: num("cost"),
    })
}

fn parse_status(status: &str) -> Option<TaskStatus> {
    Some(match status {
        "pending" => TaskStatus::Pending,
        "in_progress" => TaskStatus::InProgress,
        "done" => TaskStatus::Done,
        "failed" => TaskStatus::Failed,
        "cancelled" => TaskStatus::Cancelled,
        _ => return None,
    })
}

/// Merge a child's task subtree under the dispatching task.
fn merge_child_task_tree(nodes: Option<&Value>, parent_task_id: u64) {
    let Some(nodes) = nodes.and_then(Value::as_array) else {
        return;
    };
    let store = task_store();
    store.batch(|store| {
        for node in nodes {
            let created = store.create(
                node.get("title").and_then(Value::as_str).unwrap_or(""),
                CreateTaskOptions {
                    source: match node.get("source").and_then(Value::as_str) {
                        Some("subagent") => Some(TaskSource::Subagent),
                        Some("mcp") => Some(TaskSource::Mcp),
                        _ => None,
                    },
                    subagent_mode: node
                        .get("subagentMode")
                        .and_then(Value::as_str)
                        .map(String::from),
                    parent_task_id: Some(parent_task_id),
                    ..Default::default()
                },
            );
            store.update(
                created.id,
                TaskPatch {
                    status: node
                        .get("status")
                        .and_then(Value::as_str)
                        .and_then(parse_status),
                    usage: usage_of(node.get("usage")),
                    ..Default::default()
                },
            );
            merge_child_task_tree(node.get("children"), created.id);
        }
    });
}

/// A warm run as the TaskResult the shared finish path consumes.
fn warm_result_to_task_result(warm: &WarmRunResult, agent_type: &str, task_id: u64) -> TaskResult {
    let mut data = Map::new();
    data.insert(
        "summary".into(),
        Value::from(if warm.summary.is_empty() {
            "(subagent returned no output)".to_string()
        } else {
            warm.summary.clone()
        }),
    );
    data.insert("files_changed".into(), json!([]));
    data.insert("confidence".into(), json!(if warm.ok { 1 } else { 0 }));
    data.insert("status".into(), Value::from(warm.status));
    if let Some(usage) = &warm.usage {
        data.insert(
            "usage".into(),
            json!({
                "input": usage.input, "output": usage.output, "cacheRead": usage.cache_read,
                "cacheWrite": usage.cache_write, "cost": usage.cost,
            }),
        );
    }
    TaskResult {
        handled_inline: false,
        agent_type: Some(agent_type.to_string()),
        result: Some(SubagentResult {
            task_id: task_id.to_string(),
            ok: warm.ok,
            exit_code: Some(if warm.ok { 0 } else { 1 }),
            status: Some(if warm.ok {
                ResultStatus::Complete
            } else {
                ResultStatus::Failed
            }),
            error: warm.error.clone(),
            result_data: Some(data),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// A background dispatch's notification handle.
struct Background {
    task_id: String,
    label: String,
}

fn details(
    subagent_type: &str,
    ok: bool,
    error: Option<&str>,
    task_id: u64,
    pool_task_id: Option<&str>,
    background: bool,
) -> Value {
    let mut d = Map::new();
    d.insert("subagent_type".into(), Value::from(subagent_type));
    d.insert("ok".into(), Value::from(ok));
    if let Some(error) = error {
        d.insert("error".into(), Value::from(error));
    }
    d.insert("taskId".into(), Value::from(task_id));
    if let Some(id) = pool_task_id {
        d.insert("poolTaskId".into(), Value::from(id));
    }
    if background {
        d.insert("background".into(), Value::from(true));
    }
    Value::Object(d)
}

/// Update the task panel from a finished dispatch and shape the tool result.
/// A foreground failure is an error; a background one is a notification.
fn finalize_dispatch_result(
    dispatch: &TaskResult,
    subagent_type: &str,
    run_id: &str,
    task_id: u64,
    resume_handle: Option<&str>,
    background: Option<&Background>,
) -> Result<AgentToolResult, ToolError> {
    let result = dispatch.result.as_ref();
    let data = result.and_then(|r| r.result_data.as_ref());
    let usage = usage_of(data.and_then(|d| d.get("usage")));
    merge_child_task_tree(data.and_then(|d| d.get("task_tree")), task_id);
    if let Some(usage) = &usage {
        task_store().add_agent_stats(
            run_id,
            AgentStats {
                input: usage.input,
                output: usage.output,
                cost: usage.cost,
            },
        );
    }

    let Some(result) = result.filter(|r| r.ok) else {
        let cancelled = result.and_then(|r| r.status) == Some(ResultStatus::Cancelled);
        let fallback = result.and_then(|r| r.used_inherited_model_fallback) == Some(true);
        task_store().update(
            task_id,
            TaskPatch {
                status: Some(if cancelled {
                    TaskStatus::Cancelled
                } else {
                    TaskStatus::Failed
                }),
                usage,
                note: Some(fallback.then(|| "inherited-model retry failed".to_string())),
                ..Default::default()
            },
        );
        patch_agent_state(
            run_id,
            if cancelled {
                TaskAgentState::Cancelled
            } else {
                TaskAgentState::Failed
            },
        );
        let reason = result.and_then(|r| r.error.clone()).unwrap_or_else(|| {
            match result.and_then(|r| r.status) {
                Some(status) => format!("subagent {}", status.as_str()),
                None => "unknown error".into(),
            }
        });
        if let Some(background) = background {
            let verdict = if cancelled {
                "cancelled ⊘"
            } else {
                "failed ✗"
            };
            return Ok(text_result(
                format!("{} {verdict} — {reason}", background.label),
                details(
                    subagent_type,
                    false,
                    Some(&reason),
                    task_id,
                    Some(&background.task_id),
                    true,
                ),
            ));
        }
        if cancelled {
            return Err(format!("Subagent ({subagent_type}) cancelled by user.").into());
        }
        let stderr = result.map(|r| r.stderr.trim()).unwrap_or("");
        let stderr = if stderr.is_empty() {
            String::new()
        } else {
            let units: Vec<u16> = stderr.encode_utf16().collect();
            format!(
                "\nstderr: {}",
                String::from_utf16_lossy(&units[units.len().saturating_sub(500)..])
            )
        };
        return Err(format!("Subagent ({subagent_type}) failed: {reason}{stderr}").into());
    };

    let fallback_note = (result.used_inherited_model_fallback == Some(true))
        .then(|| "ran on inherited model".to_string());
    task_store().update(
        task_id,
        TaskPatch {
            status: Some(TaskStatus::Done),
            usage,
            note: Some(fallback_note),
            ..Default::default()
        },
    );
    patch_agent_state(run_id, TaskAgentState::Done);
    let mut answer = data
        .and_then(|d| d.get("summary"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or("(subagent returned no output)")
        .to_string();
    // `result.status === "partial"` in hoocode (the pool itself only reports
    // complete/failed, so result.json's own status does not count).
    let partial = result.status == Some(ResultStatus::Partial);
    if partial {
        if let Some(handle) = resume_handle {
            answer.push_str(&format!(
                "\n\n[Partial result. To continue this subagent, call Task again with resume_task_id=\"{handle}\".]"
            ));
        }
    }

    if let Some(background) = background {
        let partial_note = if partial {
            " (partial — resume to continue)"
        } else {
            ""
        };
        let outstanding = subagent_inbox().outstanding().len();
        let tail = if outstanding > 0 {
            format!(" {outstanding} still running.")
        } else {
            String::new()
        };
        let text = format!(
            "{} finished ✓{partial_note} — {}.{tail}\nRead the full result with TaskOutput(\"{}\").",
            background.label,
            summarize(&answer),
            background.label
        );
        return Ok(text_result(
            text,
            details(
                subagent_type,
                true,
                None,
                task_id,
                Some(&background.task_id),
                true,
            ),
        ));
    }
    Ok(text_result(
        answer,
        details(subagent_type, true, None, task_id, resume_handle, false),
    ))
}

/// Run a dispatch, cancelling its run (process tree) when the call aborts.
async fn cancel_on_abort<F>(
    pool: &SubagentPool,
    run_id: &str,
    signal: Option<AbortSignal>,
    run: F,
) -> F::Output
where
    F: std::future::Future,
{
    let Some(signal) = signal else {
        return run.await;
    };
    tokio::pin!(run);
    let mut cancelled = false;
    loop {
        tokio::select! {
            output = &mut run => return output,
            _ = signal.cancelled(), if !cancelled => {
                cancelled = true;
                pool.cancel(run_id);
            }
        }
    }
}

/// Block on a future from a tool's (blocking) execute.
fn block_on<F: std::future::Future>(future: F) -> F::Output {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => tokio::task::block_in_place(|| handle.block_on(future)),
        Err(_) => tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime")
            .block_on(future),
    }
}

fn str_param<'a>(params: &'a Value, key: &str) -> Option<&'a str> {
    params.get(key).and_then(Value::as_str)
}

fn task_parameters() -> Value {
    json!({
        "type": "object",
        "required": ["description", "prompt", "subagent_type"],
        "properties": {
            "description": {"type": "string", "description": "A short (3-5 word) description of the task, shown in the task panel."},
            "prompt": {"type": "string", "description": "The full, self-contained task for the subagent. It cannot see this conversation, so include all needed context, files, and constraints."},
            "subagent_type": {"type": "string", "description": "The name of the specialized agent to delegate to. Must be one of the available agents."},
            "complexity": {
                "anyOf": [
                    {"type": "string", "const": "fast"},
                    {"type": "string", "const": "standard"},
                    {"type": "string", "const": "capable"}
                ],
                "description": "Model tier for this dispatch: fast (quick reads/lookups), standard (multi-file edits), capable (deep architecture). Maps to settings.modelCategories. Ignored if the chosen agent pins its own model; omit to use the agent's default."
            },
            "background": {"type": "boolean", "description": "Set true to run non-blocking: you get a short notification when it finishes and pull the full result with TaskOutput; set false to wait and get the answer inline. Defaults to the agent's own background setting."},
            "resume_task_id": {"type": "string", "description": "Optional. To continue a previous subagent run, pass its task_id (returned by an earlier Task or TaskOutput call). The subagent resumes with its full prior transcript and `prompt` is your follow-up instruction."}
        }
    })
}

/// `createTaskToolDefinition`.
pub fn create_task_tool_definition(cwd: &Path) -> ToolDefinition {
    let background_agents = Arc::new(collect_background_agent_names(cwd));
    let predicate_agents = background_agents.clone();
    let default_cwd = cwd.to_path_buf();
    ToolDefinition {
        name: TASK_TOOL_NAME.into(),
        label: TASK_TOOL_NAME.into(),
        description: "Delegate a focused task to a specialized subagent that runs in a fresh, isolated context (it cannot see this conversation). Choose one of the available agents (listed in the system prompt) via `subagent_type` and pass everything it needs via `prompt`; the subagent returns only its final answer.".into(),
        prompt_snippet: Some(
            "delegate a self-contained task to a specialized subagent (choose via subagent_type)"
                .into(),
        ),
        prompt_guidelines: Vec::new(),
        parameters: task_parameters(),
        prepare_arguments: None,
        execution_mode: None,
        background: false,
        // A per-call `background` wins; else the agent's own default.
        background_when: Some(Arc::new(move |call: &AgentToolCall| {
            match call.arguments.get("background").and_then(Value::as_bool) {
                Some(value) => value,
                None => predicate_agents.contains(
                    call.arguments
                        .get("subagent_type")
                        .and_then(Value::as_str)
                        .unwrap_or(""),
                ),
            }
        })),
        execute: Arc::new(move |_id, params, signal, _on_update, ctx| {
            let cwd = ctx
                .and_then(|c| c.cwd.clone())
                .unwrap_or_else(|| default_cwd.clone());
            block_on(execute_task(
                params,
                signal,
                ctx.cloned(),
                cwd,
                background_agents.clone(),
            ))
        }),
    }
}

async fn execute_task(
    params: Value,
    signal: Option<AbortSignal>,
    ctx: Option<ToolContext>,
    cwd: PathBuf,
    background_agents: Arc<HashSet<String>>,
) -> Result<AgentToolResult, ToolError> {
    let subagent_type = str_param(&params, "subagent_type")
        .unwrap_or("")
        .to_string();
    let prompt = str_param(&params, "prompt").unwrap_or("").to_string();
    let description = str_param(&params, "description")
        .map(str::trim)
        .unwrap_or("");
    let summary = if description.is_empty() {
        summarize(&prompt)
    } else {
        description.to_string()
    };
    let available: Vec<Model> = ctx
        .as_ref()
        .map(|c| c.available_models.clone())
        .unwrap_or_default();
    let model = ctx.as_ref().and_then(|c| c.model.clone());
    let pool = get_subagent_pool(&cwd, &available);

    // Pre-flight: the inherited provider just exhausted its quota; a subagent
    // on the same provider would fail too.
    if let Some(provider) = model.as_ref().map(|m| m.provider.clone()) {
        if let Some(exhaustion) = get_provider_exhaustion(&provider) {
            let run_id = new_dispatch_task_id();
            let task_id = create_dispatch_task(&summary, &subagent_type, &run_id);
            register_subagent_dispatch(&run_id, &subagent_inbox().next_label(&subagent_type));
            task_store().update(
                task_id,
                TaskPatch {
                    status: Some(TaskStatus::Failed),
                    note: Some(Some(format!("{provider} exhausted"))),
                    ..Default::default()
                },
            );
            task_store().patch_agent(
                &run_id,
                TaskAgentPatch {
                    state: Some(TaskAgentState::Failed),
                    ..Default::default()
                },
            );
            return Ok(text_result(
                format!(
                    "Did not dispatch subagent \"{subagent_type}\": the \"{provider}\" provider appears exhausted or rate-limited (this session just failed with: {}). Subagents run on the same provider, so dispatching would fail too. Wait for the quota to reset or switch model/provider, then retry — or complete the work directly in this session.",
                    exhaustion.message
                ),
                details(&subagent_type, false, None, task_id, None, false),
            ));
        }
    }

    // Scoped delegation (`delegate: <types>`); the root is unrestricted.
    if !is_delegate_allowed(&subagent_type, &ProcessEnv) {
        let allowed = delegate_allow_list(&ProcessEnv)
            .map(|l| l.join(", "))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "(none)".into());
        return Err(format!(
            "This agent may not delegate to \"{subagent_type}\". Allowed subagent types: {allowed}."
        )
        .into());
    }

    let model_id = model.as_ref().map(|m| m.id.clone());
    let provider = model.as_ref().map(|m| m.provider.clone());

    // Resume: continue a previous subagent with its persisted transcript.
    if let Some(resume_id) = str_param(&params, "resume_task_id")
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let run_id = new_dispatch_task_id();
        let label = subagent_inbox().next_label(&subagent_type);
        let task_id = create_dispatch_task(&summary, &subagent_type, &run_id);
        register_subagent_dispatch(&run_id, &label);
        set_status(task_id, TaskStatus::InProgress);
        let outcome = cancel_on_abort(
            &pool,
            &run_id,
            signal,
            pool.resume(
                resume_id,
                &prompt,
                DispatchOptions {
                    model: model_id,
                    provider,
                    task_id: Some(run_id.clone()),
                    ..Default::default()
                },
            ),
        )
        .await;
        return match outcome {
            // The session lives under the original id: keep it as the handle.
            Ok(dispatch) => finalize_dispatch_result(
                &dispatch,
                &subagent_type,
                &run_id,
                task_id,
                Some(resume_id),
                None,
            ),
            Err(error) => {
                mark_dispatch_failed(task_id, &run_id);
                Err(error.0.into())
            }
        };
    }

    // The model already chose the agent: validate it against the registry.
    let registry = load_agent_registry(&LoadAgentRegistryOptions::new(cwd.to_string_lossy()));
    let Some(def) = registry.get(&subagent_type).cloned() else {
        let available = registry
            .list()
            .iter()
            .map(|a| a.name.clone())
            .collect::<Vec<_>>()
            .join(", ");
        let available = if available.is_empty() {
            "(none)".into()
        } else {
            available
        };
        return Err(format!(
            "Unknown subagent_type: \"{subagent_type}\". Available agents: {available}."
        )
        .into());
    };

    let pool_task_id = new_dispatch_task_id();
    let label = subagent_inbox().next_label(&subagent_type);
    let task_id = create_dispatch_task(&summary, &subagent_type, &pool_task_id);
    register_subagent_dispatch(&pool_task_id, &label);
    set_status(task_id, TaskStatus::InProgress);
    // Fork agents inherit the parent's conversation via a forked session.
    let fork_session_file = if def.fork == Some(true) {
        resolve_fork_session_file(
            def.fork,
            ctx.as_ref().and_then(|c| c.session_file.as_deref()),
            &cwd,
        )
    } else {
        None
    };
    // `complexity` goes in as the model: a pinned agent model still wins, and
    // the pool resolves a category.
    let dispatch_model = str_param(&params, "complexity")
        .map(String::from)
        .or_else(|| model_id.clone());
    let is_background = params
        .get("background")
        .and_then(Value::as_bool)
        .unwrap_or_else(|| background_agents.contains(&subagent_type));
    let warm_options = WarmDispatchOptions {
        agent_type: subagent_type.clone(),
        cwd: cwd.clone(),
        model: dispatch_model.clone(),
        provider: provider.clone(),
    };
    let progress: WarmProgressCallback = {
        let run_id = pool_task_id.clone();
        Arc::new(move |activity: &str| {
            task_store().patch_agent(
                &run_id,
                TaskAgentPatch {
                    activity: Some(activity.to_string()),
                    ..Default::default()
                },
            );
        })
    };
    let dispatch_options = DispatchOptions {
        force_agent: Some(subagent_type.clone()),
        context: Some(String::new()),
        model: dispatch_model.clone(),
        provider: provider.clone(),
        session_file: fork_session_file.clone(),
        task_id: Some(pool_task_id.clone()),
    };
    let use_warm = warm_subagents_enabled(&ProcessEnv) && fork_session_file.is_none();

    if is_background {
        // Notify-and-pull: the body stays in the inbox.
        subagent_inbox().observe(&pool);
        subagent_inbox().start(&pool_task_id, &label, &subagent_type);
        let background = Background {
            task_id: pool_task_id.clone(),
            label: label.clone(),
        };
        if use_warm {
            let warm = get_warm_subagent_pool(&cwd, &available);
            if warm.is_poolable(&subagent_type) {
                match warm
                    .dispatch(&prompt, &warm_options, Some(progress.clone()))
                    .await
                {
                    Ok(warm_result) => {
                        let dispatch =
                            warm_result_to_task_result(&warm_result, &subagent_type, task_id);
                        subagent_inbox().finish(&pool_task_id, &dispatch);
                        return finalize_dispatch_result(
                            &dispatch,
                            &subagent_type,
                            &pool_task_id,
                            task_id,
                            Some(&pool_task_id),
                            Some(&background),
                        );
                    }
                    Err(error) => agent_log(&format!(
                        "[WARM] {subagent_type} fell back to cold spawn: {error}"
                    )),
                }
            }
        }
        let outcome = cancel_on_abort(
            &pool,
            &pool_task_id,
            signal,
            pool.dispatch(&prompt, dispatch_options),
        )
        .await;
        return match outcome {
            Ok(dispatch) => {
                subagent_inbox().finish(&pool_task_id, &dispatch);
                finalize_dispatch_result(
                    &dispatch,
                    &subagent_type,
                    &pool_task_id,
                    task_id,
                    Some(&pool_task_id),
                    Some(&background),
                )
            }
            Err(error) => {
                let reason = error.0;
                mark_dispatch_failed(task_id, &pool_task_id);
                subagent_inbox().fail(&pool_task_id, &reason, TaskLifecycle::Failed);
                // Already answered by a placeholder: a notification, not an error.
                Ok(text_result(
                    format!("{label} failed ✗ — {reason}"),
                    details(
                        &subagent_type,
                        false,
                        Some(&reason),
                        task_id,
                        Some(&pool_task_id),
                        true,
                    ),
                ))
            }
        };
    }

    // Warm path for a foreground dispatch; infra failures fall back to cold.
    if use_warm {
        let warm = get_warm_subagent_pool(&cwd, &available);
        if warm.is_poolable(&subagent_type) {
            match warm.dispatch(&prompt, &warm_options, Some(progress)).await {
                Ok(warm_result) => {
                    let dispatch =
                        warm_result_to_task_result(&warm_result, &subagent_type, task_id);
                    return finalize_dispatch_result(
                        &dispatch,
                        &subagent_type,
                        &pool_task_id,
                        task_id,
                        None,
                        None,
                    );
                }
                Err(error) => agent_log(&format!(
                    "[WARM] {subagent_type} fell back to cold spawn: {error}"
                )),
            }
        }
    }

    // Foreground: block and return the full answer inline.
    let outcome = cancel_on_abort(
        &pool,
        &pool_task_id,
        signal,
        pool.dispatch(&prompt, dispatch_options),
    )
    .await;
    match outcome {
        Ok(dispatch) => {
            let handle = dispatch.task_id.clone();
            finalize_dispatch_result(
                &dispatch,
                &subagent_type,
                &pool_task_id,
                task_id,
                handle.as_deref(),
                None,
            )
        }
        Err(error) => {
            mark_dispatch_failed(task_id, &pool_task_id);
            Err(error.0.into())
        }
    }
}

// ---------------------------------------------------------------------------
// TaskOutput
// ---------------------------------------------------------------------------

/// How long a record has run (so far, or until it settled).
fn record_elapsed(rec: &InboxRecord) -> String {
    let end = rec.ended_at.unwrap_or_else(now_ms);
    format_duration_secs(end.saturating_sub(rec.started_at) as f64 / 1000.0)
}

fn output_details(
    task_id: Option<&str>,
    status: &str,
    ok: bool,
    outstanding: Option<usize>,
) -> Value {
    let mut d = Map::new();
    if let Some(id) = task_id {
        d.insert("task_id".into(), Value::from(id));
    }
    d.insert("status".into(), Value::from(status));
    d.insert("ok".into(), Value::from(ok));
    if let Some(n) = outstanding {
        d.insert("outstanding".into(), Value::from(n));
    }
    Value::Object(d)
}

/// Every known background subagent: status and activity, no bodies.
fn format_task_roster() -> AgentToolResult {
    let inbox = subagent_inbox();
    let all = inbox.list();
    let outstanding = inbox.outstanding().len();
    if all.is_empty() {
        return text_result(
            "No background subagents have been dispatched.",
            output_details(None, "empty", true, Some(0)),
        );
    }
    let lines: Vec<String> = all
        .iter()
        .map(|r| {
            let when = record_elapsed(r);
            let summary = r.summary_line.as_deref().unwrap_or("");
            match r.lifecycle {
                TaskLifecycle::Running => format!(
                    "- {}  running  {when}{}",
                    r.label,
                    r.last_activity
                        .as_deref()
                        .filter(|a| !a.is_empty())
                        .map(|a| format!("  · {a}"))
                        .unwrap_or_default()
                ),
                TaskLifecycle::Done => {
                    format!("- {}  done (uncollected)  {when} — {summary}", r.label)
                }
                TaskLifecycle::Collected => format!("- {}  collected  {when} — {summary}", r.label),
                TaskLifecycle::Cancelled => format!(
                    "- {}  cancelled ⊘  — {}",
                    r.label,
                    r.error.as_deref().unwrap_or("cancelled by user")
                ),
                other => format!(
                    "- {}  {} ✗  — {}",
                    r.label,
                    other.as_str(),
                    r.error.as_deref().unwrap_or("unknown error")
                ),
            }
        })
        .collect();
    let header = format!(
        "{} background subagent{} ({outstanding} running):",
        all.len(),
        if all.len() == 1 { "" } else { "s" }
    );
    let hint = if all.iter().any(|r| r.lifecycle == TaskLifecycle::Done) {
        "\nRead a finished one with TaskOutput(\"<label>\")."
    } else {
        ""
    };
    text_result(
        format!("{header}\n{}{hint}", lines.join("\n")),
        output_details(None, "list", true, Some(outstanding)),
    )
}

fn task_output_parameters() -> Value {
    json!({
        "type": "object",
        "properties": {
            "task_id": {"type": "string", "description": "Handle of a background subagent — its task_id or friendly label (e.g. \"explore#1\") from a Task notification. Omit (or set list:true) to see every background task."},
            "list": {"type": "boolean", "description": "List all background subagents with their status (running/done/failed/cancelled) and current activity. No result bodies are returned."},
            "wait": {"type": "boolean", "description": "Block until the named task finishes — or, with no task_id, until all outstanding subagents finish (a swarm barrier) — before returning. Bounded by timeout_ms."},
            "timeout_ms": {"type": "number", "description": "Maximum time to block in wait mode, in milliseconds (default 120000)."}
        }
    })
}

/// `createTaskOutputToolDefinition`.
pub fn create_task_output_tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "TaskOutput".into(),
        label: "TaskOutput".into(),
        description: [
            "Check on background subagents dispatched via Task, and pull their results.",
            "Pass a task_id/label (e.g. \"explore#1\") to read a finished subagent's full result, or to see its status while it runs.",
            "Set list:true (or omit task_id) to list every background subagent with its status and current activity.",
            "Set wait:true to block until that task finishes — or, with no task_id, until all outstanding subagents finish (a swarm barrier).",
            "It never errors on a valid handle: a running task reports status, a finished one returns its result, an already-read one says so.",
        ]
        .join("\n"),
        prompt_snippet: Some(
            "check status / list / collect the results of background subagents".into(),
        ),
        prompt_guidelines: Vec::new(),
        parameters: task_output_parameters(),
        prepare_arguments: None,
        execution_mode: None,
        background: false,
        background_when: None,
        execute: Arc::new(|_id, params, _signal, _on_update, ctx| {
            let ctx = ctx.cloned();
            Ok(block_on(execute_task_output(params, ctx)))
        }),
    }
}

async fn execute_task_output(params: Value, ctx: Option<ToolContext>) -> AgentToolResult {
    // Wire the inbox to the pool's progress stream for live activity.
    if let Some(cwd) = ctx.as_ref().and_then(|c| c.cwd.clone()) {
        let models = ctx
            .as_ref()
            .map(|c| c.available_models.clone())
            .unwrap_or_default();
        subagent_inbox().observe(&get_subagent_pool(&cwd, &models));
    }
    let inbox = subagent_inbox();
    let handle = str_param(&params, "task_id")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from);

    if params.get("wait").and_then(Value::as_bool) == Some(true) {
        let timeout = Duration::from_millis(
            params
                .get("timeout_ms")
                .and_then(Value::as_f64)
                .map(|n| n.max(0.0) as u64)
                .unwrap_or(TASK_OUTPUT_DEFAULT_TIMEOUT_MS),
        );
        match &handle {
            Some(handle) => {
                inbox.wait_for(handle, timeout).await;
            }
            None => inbox.wait_for_all(timeout).await,
        }
    }

    let Some(handle) = handle.filter(|_| params.get("list").and_then(Value::as_bool) != Some(true))
    else {
        return format_task_roster();
    };

    let Some(rec) = inbox.get(&handle) else {
        return text_result(
            format!("No background task \"{handle}\". Call TaskOutput with list:true to see active tasks."),
            output_details(Some(&handle), "unknown", false, None),
        );
    };
    match rec.lifecycle {
        TaskLifecycle::Running => {
            let activity = rec
                .last_activity
                .as_deref()
                .filter(|a| !a.is_empty())
                .map(|a| format!(" (currently: {a})"))
                .unwrap_or_default();
            text_result(
                format!(
                    "{} is still running — {} elapsed{activity}. Call TaskOutput again, or with wait:true to block until it finishes.",
                    rec.label,
                    record_elapsed(&rec)
                ),
                output_details(Some(&handle), "running", true, None),
            )
        }
        TaskLifecycle::Done => {
            let body = inbox
                .collect(&handle)
                .map(|(_, body)| body)
                .or(rec.summary_line.clone())
                .unwrap_or_else(|| "(subagent returned no output)".into());
            text_result(body, output_details(Some(&handle), "done", true, None))
        }
        TaskLifecycle::Collected => text_result(
            format!(
                "{} was already delivered — {}.",
                rec.label,
                rec.summary_line.as_deref().unwrap_or("(no summary kept)")
            ),
            output_details(Some(&handle), "collected", true, None),
        ),
        other => {
            let glyph = if other == TaskLifecycle::Cancelled {
                "⊘"
            } else {
                "✗"
            };
            text_result(
                format!(
                    "{} {} {glyph} — {}.",
                    rec.label,
                    other.as_str(),
                    rec.error.as_deref().unwrap_or("unknown error")
                ),
                output_details(Some(&handle), other.as_str(), false, None),
            )
        }
    }
}
