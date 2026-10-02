//! `core/warm-subagent-pool.ts` and `warm-subagent-pool-instance.ts`: warm
//! subagent workers (experimental, opt-in via `--warm-subagents` /
//! `warmSubagents`; default off).
//!
//! The cold pool re-execs the CLI for every dispatch. A warm worker keeps a
//! child running in RPC mode and hands it one task at a time: `new_session`
//! resets it between tasks, `prompt` runs a task, `agent_end` ends it, and the
//! answer and usage are pulled inline (no result.json round-trip). Workers are
//! pinned per (agent type, model, provider): RPC cannot swap the system prompt
//! or tools per prompt. Any worker failure is a [`WarmWorkerError`] so the
//! caller falls back to the cold pool.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::time::Duration;

use cortexcode_ai_types::Model;
use cortexcode_code_resources::{
    load_agent_registry, AgentRegistry, LoadAgentRegistryOptions, MODEL_INHERIT,
};
use cortexcode_code_rpc::client::{RpcClient, RpcClientOptions};
use cortexcode_code_settings::SettingsManager;
use serde_json::Value;
use tokio::task::JoinHandle;

use crate::depth::{
    current_subagent_depth, resolve_max_subagent_depth, tool_allowlist_needs_mcp, ProcessEnv,
    SubagentEnv, SUBAGENT_DEPTH_ENV, SUBAGENT_SKIP_MCP_ENV,
};
use crate::lifeguard;
use crate::model_categories::{resolve_model_reference, CategorySettings};
use crate::pool::DEFAULT_SUBAGENT_MAX_TURNS;

/// Usage totals pulled from a worker after a task.
#[derive(Debug, Clone, PartialEq)]
pub struct WarmUsage {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
    pub cost: f64,
}

/// Outcome of one task on a warm worker.
#[derive(Debug, Clone, PartialEq)]
pub struct WarmRunResult {
    pub ok: bool,
    /// `complete` or `failed`.
    pub status: &'static str,
    /// The subagent's final assistant text.
    pub summary: String,
    pub usage: Option<WarmUsage>,
    pub error: Option<String>,
}

/// What selects and configures a worker.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WarmDispatchOptions {
    pub agent_type: String,
    pub cwd: PathBuf,
    /// A model id or category.
    pub model: Option<String>,
    pub provider: Option<String>,
}

/// An infrastructure failure (crash, timeout, protocol error).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WarmWorkerError(pub String);

impl std::fmt::Display for WarmWorkerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for WarmWorkerError {}

/// Reports the tool a worker is running (`""` between tools).
pub type WarmProgressCallback = Arc<dyn Fn(&str) + Send + Sync>;

/// Deviation: how long a warm run may take before it is treated as stalled.
/// hoocode's `WARM_RUN_TIMEOUT_MS` is a flat 180s for every agent; we use the
/// same per-agent table the cold pool's lifeguard uses, which is never tighter.
/// A flat 3 minutes killed `code-review` runs that the cold pool would have
/// allowed 15.
fn warm_run_timeout(agent_type: &str) -> Duration {
    Duration::from_millis(lifeguard::base_timeout_ms(agent_type))
}

/// The spawn command: executable and prefix args.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnCommand {
    pub executable: PathBuf,
    pub prefix_args: Vec<String>,
}

static NEXT_WORKER: AtomicU64 = AtomicU64::new(1);

/// One long-lived RPC child pinned to a single agent configuration.
pub struct WarmSubagentWorker {
    pub key: String,
    id: u64,
    client: RpcClient,
    alive: bool,
}

impl WarmSubagentWorker {
    #[allow(clippy::too_many_arguments)]
    fn new(
        key: String,
        options: &WarmDispatchOptions,
        env: Vec<(String, String)>,
        registry: &AgentRegistry,
        skill_paths: &[String],
        settings: Option<&CategorySettings>,
        available_models: &[Model],
        spawn: Option<&SpawnCommand>,
    ) -> Self {
        let (executable, prefix_args) = match spawn {
            Some(spawn) => (Some(spawn.executable.clone()), spawn.prefix_args.clone()),
            None => (None, Vec::new()),
        };
        let client = RpcClient::new(RpcClientOptions {
            executable,
            prefix_args,
            cwd: Some(options.cwd.clone()),
            env,
            args: build_worker_args(options, registry, skill_paths, settings, available_models),
            ..Default::default()
        });
        Self {
            key,
            id: NEXT_WORKER.fetch_add(1, Ordering::Relaxed),
            client,
            alive: true,
        }
    }

    /// Boot the child.
    pub async fn start(&mut self) -> Result<(), WarmWorkerError> {
        self.client.start().await.map_err(|e| {
            self.alive = false;
            WarmWorkerError(e.0)
        })
    }

    pub fn is_alive(&self) -> bool {
        self.alive
    }

    /// Run one task to completion. An infra failure is a
    /// [`WarmWorkerError`] (the worker is then dead); a task that ran but
    /// failed is `ok: false`.
    pub async fn run(
        &mut self,
        prompt: &str,
        on_activity: Option<WarmProgressCallback>,
        timeout: Duration,
    ) -> Result<WarmRunResult, WarmWorkerError> {
        if !self.alive {
            return Err(WarmWorkerError("worker is not alive".into()));
        }
        // The tool the child is running, cleared between tools and at turn end.
        let progress = on_activity.clone().map(|on_activity| {
            self.client.on_event(move |event: &Value| {
                match event.get("type").and_then(Value::as_str) {
                    Some("tool_execution_start") => {
                        on_activity(event.get("toolName").and_then(Value::as_str).unwrap_or(""))
                    }
                    Some("tool_execution_end") | Some("turn_end") => on_activity(""),
                    _ => {}
                }
            })
        });
        let outcome = self.run_inner(prompt, timeout).await;
        drop(progress);
        if let Some(on_activity) = on_activity {
            on_activity("");
        }
        outcome.map_err(|e| {
            // The child is no longer trustworthy: the pool discards it.
            self.alive = false;
            WarmWorkerError(e)
        })
    }

    async fn run_inner(
        &mut self,
        prompt: &str,
        timeout: Duration,
    ) -> Result<WarmRunResult, String> {
        let events = self
            .client
            .prompt_and_wait(prompt, None, timeout)
            .await
            .map_err(|e| e.0)?;
        let failure = first_turn_error(&events);
        let summary = self
            .client
            .get_last_assistant_text()
            .await
            .map_err(|e| e.0)?
            .unwrap_or_default();
        let usage = self.read_usage().await;
        Ok(match failure {
            Some(error) => WarmRunResult {
                ok: false,
                status: "failed",
                summary,
                usage,
                error: Some(error),
            },
            None => WarmRunResult {
                ok: true,
                status: "complete",
                summary,
                usage,
                error: None,
            },
        })
    }

    /// Reset the conversation for the next task.
    pub async fn reset(&mut self) -> Result<(), WarmWorkerError> {
        if !self.alive {
            return Ok(());
        }
        self.client
            .new_session(None)
            .await
            .map(|_| ())
            .map_err(|e| {
                self.alive = false;
                WarmWorkerError(e.0)
            })
    }

    pub async fn dispose(&mut self) {
        self.alive = false;
        self.client.stop().await;
    }

    async fn read_usage(&self) -> Option<WarmUsage> {
        let stats = self.client.get_session_stats().await.ok()?;
        let tokens = &stats["tokens"];
        let num = |v: &Value| v.as_f64().unwrap_or(0.0);
        Some(WarmUsage {
            input: num(&tokens["input"]),
            output: num(&tokens["output"]),
            cache_read: num(&tokens["cacheRead"]),
            cache_write: num(&tokens["cacheWrite"]),
            cost: num(&stats["cost"]),
        })
    }
}

/// The first turn that ended in error/abort: its message, else `turn <reason>`.
fn first_turn_error(events: &[Value]) -> Option<String> {
    events.iter().find_map(|event| {
        if event.get("type").and_then(Value::as_str) != Some("turn_end") {
            return None;
        }
        let message = event.get("message")?;
        let reason = message.get("stopReason").and_then(Value::as_str)?;
        if reason != "error" && reason != "aborted" {
            return None;
        }
        Some(
            message
                .get("errorMessage")
                .and_then(Value::as_str)
                .filter(|m| !m.is_empty())
                .map(String::from)
                .unwrap_or_else(|| format!("turn {reason}")),
        )
    })
}

/// The RPC child's arguments: the agent-config part of the cold pool's
/// (system prompt, tools, model/provider, turn cap, skills), without the
/// one-shot `--mode json` / `--session` / `--task-id` / prompt.
fn build_worker_args(
    options: &WarmDispatchOptions,
    registry: &AgentRegistry,
    skill_paths: &[String],
    settings: Option<&CategorySettings>,
    available_models: &[Model],
) -> Vec<String> {
    let mut args = Vec::new();
    let def = registry.get(&options.agent_type);
    if let Some(prompt) = def.map(|d| &d.prompt).filter(|p| !p.is_empty()) {
        args.extend(["--system-prompt".into(), prompt.clone()]);
    }
    let child_depth = current_subagent_depth(&ProcessEnv) + 1;
    let can_child_delegate = def.and_then(|d| d.delegate) == Some(true)
        && child_depth < resolve_max_subagent_depth(None, &ProcessEnv);
    let mut tools = def.and_then(|d| d.tools.clone());
    if can_child_delegate {
        if let Some(tools) = &mut tools {
            for t in ["Task", "TaskOutput"] {
                if !tools.iter().any(|x| x == t) {
                    tools.push(t.into());
                }
            }
        }
    }
    if let Some(tools) = tools.filter(|t| !t.is_empty()) {
        args.extend(["--tools".into(), tools.join(",")]);
    }
    if let Some(disallowed) = def
        .and_then(|d| d.disallowed_tools.as_ref())
        .filter(|t| !t.is_empty())
    {
        args.extend(["--disallowed-tools".into(), disallowed.join(",")]);
    }
    if can_child_delegate {
        args.push("--enable-subagents".into());
        if let Some(to) = def
            .and_then(|d| d.delegate_to.as_ref())
            .filter(|t| !t.is_empty())
        {
            args.extend(["--delegate-allow".into(), to.join(",")]);
        }
    }
    let explicit = def
        .and_then(|d| d.model.clone())
        .filter(|m| !m.is_empty() && m != MODEL_INHERIT);
    let raw = explicit.or_else(|| options.model.clone().filter(|m| !m.is_empty()));
    let model = raw.and_then(|m| resolve_model_reference(&m, settings, Some(available_models)));
    if let Some(model) = &model {
        args.extend(["--model".into(), model.clone()]);
    }
    if let Some(provider) = options.provider.as_ref().filter(|p| !p.is_empty()) {
        if model.as_ref().is_none_or(|m| !m.contains('/')) {
            args.extend(["--provider".into(), provider.clone()]);
        }
    }
    let max_turns = def
        .and_then(|d| d.max_turns)
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT_SUBAGENT_MAX_TURNS);
    args.extend(["--max-turns".into(), max_turns.to_string()]);
    for path in skill_paths {
        args.extend(["--skill".into(), path.clone()]);
    }
    args
}

#[derive(Default)]
struct WarmState {
    idle: HashMap<String, Vec<WarmSubagentWorker>>,
    reclaim_timers: HashMap<u64, JoinHandle<()>>,
    live_count: HashMap<String, usize>,
    skill_paths: Vec<String>,
    registry: Option<Arc<AgentRegistry>>,
    disposed: bool,
}

struct WarmInner {
    cwd: PathBuf,
    settings: Option<CategorySettings>,
    available_models: Vec<Model>,
    max_per_key: usize,
    idle_ttl: Duration,
    spawn: Option<SpawnCommand>,
    state: Mutex<WarmState>,
}

/// `WarmSubagentPool` options.
#[derive(Debug, Clone)]
pub struct WarmSubagentPoolOptions {
    pub cwd: PathBuf,
    pub settings: Option<CategorySettings>,
    pub skill_paths: Vec<String>,
    pub available_models: Vec<Model>,
    /// Idle workers kept per configuration (default 2).
    pub max_per_key: usize,
    /// An idle worker is reclaimed after this long (default 30s).
    pub idle_ttl: Duration,
    /// Spawn command override (tests); default: this executable.
    pub spawn: Option<SpawnCommand>,
}

impl WarmSubagentPoolOptions {
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        Self {
            cwd: cwd.into(),
            settings: None,
            skill_paths: Vec::new(),
            available_models: Vec::new(),
            max_per_key: 2,
            idle_ttl: Duration::from_millis(30_000),
            spawn: None,
        }
    }
}

/// Warm workers keyed by configuration: hands out an idle worker (or boots
/// one), and on release resets it and parks it with an idle-TTL reclaim.
#[derive(Clone)]
pub struct WarmSubagentPool {
    inner: Arc<WarmInner>,
}

impl WarmSubagentPool {
    pub fn new(options: WarmSubagentPoolOptions) -> Self {
        Self {
            inner: Arc::new(WarmInner {
                cwd: options.cwd,
                settings: options.settings,
                available_models: options.available_models,
                max_per_key: options.max_per_key,
                idle_ttl: options.idle_ttl,
                spawn: options.spawn,
                state: Mutex::new(WarmState {
                    skill_paths: options.skill_paths,
                    ..Default::default()
                }),
            }),
        }
    }

    fn state(&self) -> MutexGuard<'_, WarmState> {
        self.inner.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn update_skill_paths(&self, paths: Vec<String>) {
        self.state().skill_paths = paths;
    }

    /// Use this registry instead of loading one for the pool's cwd.
    pub fn set_registry(&self, registry: AgentRegistry) {
        self.state().registry = Some(Arc::new(registry));
    }

    fn registry(&self) -> Arc<AgentRegistry> {
        if let Some(registry) = self.state().registry.clone() {
            return registry;
        }
        let registry = Arc::new(load_agent_registry(&LoadAgentRegistryOptions::new(
            self.inner.cwd.to_string_lossy(),
        )));
        self.state().registry = Some(registry.clone());
        registry
    }

    /// A registry agent is poolable unless it is a fork agent.
    pub fn is_poolable(&self, agent_type: &str) -> bool {
        self.registry()
            .get(agent_type)
            .is_some_and(|d| d.fork != Some(true))
    }

    fn key_for(&self, options: &WarmDispatchOptions) -> String {
        let resolved = options.model.as_deref().and_then(|m| {
            resolve_model_reference(
                m,
                self.inner.settings.as_ref(),
                Some(self.inner.available_models.as_slice()),
            )
        });
        format!(
            "{}::{}::{}",
            options.agent_type,
            resolved.as_deref().unwrap_or("default"),
            options.provider.as_deref().unwrap_or("default")
        )
    }

    /// The child's environment additions: depth + 1, MCP skipped for an
    /// MCP-free allowlist. (hoocode also deletes the deferred-MCP flag, but
    /// its RpcClient spreads the parent environment back over it.)
    fn child_env(&self, agent_type: &str) -> Vec<(String, String)> {
        let mut env = vec![(
            format!("CORTEXCODE_{SUBAGENT_DEPTH_ENV}"),
            (current_subagent_depth(&ProcessEnv) + 1).to_string(),
        )];
        let registry = self.registry();
        let tools = registry.get(agent_type).and_then(|d| d.tools.as_deref());
        if !tool_allowlist_needs_mcp(tools) {
            env.push((format!("CORTEXCODE_{SUBAGENT_SKIP_MCP_ENV}"), "1".into()));
        }
        env
    }

    /// Run a task on a warm worker: acquire (reuse or boot), run, release.
    pub async fn dispatch(
        &self,
        prompt: &str,
        options: &WarmDispatchOptions,
        on_activity: Option<WarmProgressCallback>,
    ) -> Result<WarmRunResult, WarmWorkerError> {
        if self.state().disposed {
            return Err(WarmWorkerError("warm pool disposed".into()));
        }
        let mut worker = self.acquire(options).await?;
        let timeout = warm_run_timeout(&options.agent_type);
        match worker.run(prompt, on_activity, timeout).await {
            Ok(result) => {
                self.release(worker).await;
                Ok(result)
            }
            Err(error) => {
                self.discard(worker).await;
                Err(error)
            }
        }
    }

    async fn acquire(
        &self,
        options: &WarmDispatchOptions,
    ) -> Result<WarmSubagentWorker, WarmWorkerError> {
        let key = self.key_for(options);
        loop {
            let parked = {
                let mut state = self.state();
                let worker = state.idle.get_mut(&key).and_then(Vec::pop);
                if let Some(worker) = &worker {
                    if let Some(timer) = state.reclaim_timers.remove(&worker.id) {
                        timer.abort();
                    }
                }
                worker
            };
            let Some(mut worker) = parked else { break };
            if worker.is_alive() {
                return Ok(worker);
            }
            // Died while parked: drop it and try the next.
            self.dec_live(&key);
            worker.dispose().await;
        }
        let skill_paths = self.state().skill_paths.clone();
        let mut worker = WarmSubagentWorker::new(
            key.clone(),
            options,
            self.child_env(&options.agent_type),
            &self.registry(),
            &skill_paths,
            self.inner.settings.as_ref(),
            &self.inner.available_models,
            self.inner.spawn.as_ref(),
        );
        *self.state().live_count.entry(key.clone()).or_insert(0) += 1;
        if let Err(error) = worker.start().await {
            self.dec_live(&key);
            return Err(error);
        }
        Ok(worker)
    }

    async fn release(&self, mut worker: WarmSubagentWorker) {
        if self.state().disposed || !worker.is_alive() {
            return self.discard(worker).await;
        }
        if worker.reset().await.is_err() {
            return self.discard(worker).await;
        }
        let rejected = {
            let mut state = self.state();
            let parked = state.idle.entry(worker.key.clone()).or_default();
            if parked.len() >= self.inner.max_per_key {
                Some(worker)
            } else {
                let (id, key) = (worker.id, worker.key.clone());
                parked.push(worker);
                let pool = self.clone();
                let ttl = self.inner.idle_ttl;
                let timer = tokio::spawn(async move {
                    tokio::time::sleep(ttl).await;
                    pool.reclaim(&key, id).await;
                });
                state.reclaim_timers.insert(id, timer);
                None
            }
        };
        if let Some(worker) = rejected {
            self.discard(worker).await;
        }
    }

    async fn reclaim(&self, key: &str, id: u64) {
        let worker = {
            let mut state = self.state();
            state.reclaim_timers.remove(&id);
            let parked = state.idle.get_mut(key);
            parked.and_then(|p| p.iter().position(|w| w.id == id).map(|i| p.remove(i)))
        };
        if let Some(worker) = worker {
            self.discard(worker).await;
        }
    }

    async fn discard(&self, mut worker: WarmSubagentWorker) {
        if let Some(timer) = self.state().reclaim_timers.remove(&worker.id) {
            timer.abort();
        }
        self.dec_live(&worker.key);
        worker.dispose().await;
    }

    fn dec_live(&self, key: &str) {
        let mut state = self.state();
        let n = state
            .live_count
            .get(key)
            .copied()
            .unwrap_or(1)
            .saturating_sub(1);
        if n == 0 {
            state.live_count.remove(key);
        } else {
            state.live_count.insert(key.to_string(), n);
        }
    }

    /// Currently parked workers.
    pub fn idle_count(&self) -> usize {
        self.state().idle.values().map(Vec::len).sum()
    }

    pub async fn dispose(&self) {
        let workers: Vec<WarmSubagentWorker> = {
            let mut state = self.state();
            if state.disposed {
                return;
            }
            state.disposed = true;
            for (_, timer) in state.reclaim_timers.drain() {
                timer.abort();
            }
            state.live_count.clear();
            state.idle.drain().flat_map(|(_, w)| w).collect()
        };
        for mut worker in workers {
            worker.dispose().await;
        }
    }
}

/// Warm dispatch is enabled (suffix of `HOOCODE_WARM_SUBAGENTS`, set to "1"
/// at the root from the flag or setting).
pub const WARM_SUBAGENTS_ENV: &str = "WARM_SUBAGENTS";

/// Whether warm-subagent dispatch is enabled for this process.
pub fn warm_subagents_enabled(env: &impl SubagentEnv) -> bool {
    env.var(WARM_SUBAGENTS_ENV).as_deref() == Some("1")
}

#[derive(Default)]
struct Instance {
    pool: Option<WarmSubagentPool>,
    override_pool: Option<WarmSubagentPool>,
    skill_paths: Vec<String>,
}

static INSTANCE: LazyLock<Mutex<Instance>> = LazyLock::new(Mutex::default);

fn instance() -> MutexGuard<'static, Instance> {
    INSTANCE.lock().unwrap_or_else(|e| e.into_inner())
}

/// The shared warm pool for `cwd`, created on first use.
pub fn get_warm_subagent_pool(cwd: &Path, available_models: &[Model]) -> WarmSubagentPool {
    let mut instance = instance();
    if let Some(pool) = instance
        .override_pool
        .clone()
        .or_else(|| instance.pool.clone())
    {
        return pool;
    }
    let manager = SettingsManager::create(cwd, cortexcode_code_paths::agent_dir());
    let mut settings = manager.global_settings();
    settings.extend(manager.project_settings());
    let pool = WarmSubagentPool::new(WarmSubagentPoolOptions {
        settings: Some(CategorySettings::from_settings(&settings)),
        skill_paths: instance.skill_paths.clone(),
        available_models: available_models.to_vec(),
        ..WarmSubagentPoolOptions::new(cwd)
    });
    instance.pool = Some(pool.clone());
    pool
}

/// Update the skill paths forwarded to new warm workers.
pub fn update_warm_subagent_skill_paths(paths: Vec<String>) {
    let mut instance = instance();
    instance.skill_paths = paths.clone();
    if let Some(pool) = &instance.pool {
        pool.update_skill_paths(paths);
    }
}

/// Inject a warm pool for tests; `None` clears the override.
pub fn set_warm_subagent_pool_for_testing(pool: Option<WarmSubagentPool>) {
    instance().override_pool = pool;
}
