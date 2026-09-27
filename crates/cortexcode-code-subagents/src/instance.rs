//! `core/subagent-pool-instance.ts`: the process-wide [`SubagentPool`].
//!
//! The Task tool and `/subagent` share one pool, so concurrency limits,
//! lifeguard monitoring and token budgets span every delegation in the
//! session. Created lazily on first use (inside a tokio runtime).

use std::path::Path;
use std::sync::{LazyLock, Mutex, MutexGuard};

use cortexcode_ai_types::Model;
use cortexcode_code_settings::SettingsManager;
use cortexcode_code_task_store::{task_store, TaskAgentPatch};
use serde_json::Value;

use crate::depth::{pool_concurrency_for_depth, ProcessEnv};
use crate::model_categories::CategorySettings;
use crate::pool::{SubagentPool, SubagentPoolOptions};

#[derive(Default)]
struct Instance {
    pool: Option<SubagentPool>,
    override_pool: Option<SubagentPool>,
    /// Latest non-default skill paths, kept in sync with the resource loader.
    skill_paths: Vec<String>,
}

static INSTANCE: LazyLock<Mutex<Instance>> = LazyLock::new(Mutex::default);

fn instance() -> MutexGuard<'static, Instance> {
    INSTANCE.lock().unwrap_or_else(|e| e.into_inner())
}

/// `getSubagentSpawnCommand`: this executable, no prefix args.
fn spawn_command() -> std::path::PathBuf {
    std::env::current_exe().unwrap_or_else(|_| "cortex".into())
}

/// The shared pool for `cwd`, created on first use. `available_models` (the
/// caller's available models) is snapshotted then, for deriving model
/// categories; later calls reuse the pool.
pub fn get_subagent_pool(cwd: &Path, available_models: &[Model]) -> SubagentPool {
    let mut instance = instance();
    if let Some(pool) = &instance.override_pool {
        return pool.clone();
    }
    if let Some(pool) = &instance.pool {
        return pool.clone();
    }
    let manager = SettingsManager::create(cwd, cortexcode_code_paths::agent_dir());
    // Project settings shallow-override global ones.
    let mut settings = manager.global_settings();
    settings.extend(manager.project_settings());
    let pool = SubagentPool::new(SubagentPoolOptions {
        executable: spawn_command(),
        cwd: Some(cwd.to_path_buf()),
        skill_paths: instance.skill_paths.clone(),
        // Nested pools (depth >= 1) run with the reduced cap.
        max_concurrency: pool_concurrency_for_depth(&ProcessEnv).map(|n| n as usize),
        settings: Some(CategorySettings::from_settings(&settings)),
        available_models: available_models.to_vec(),
        ..Default::default()
    });
    wire_progress_to_task_store(&pool);
    instance.pool = Some(pool.clone());
    pool
}

/// Live subagent progress on the task panel's roster row (keyed by the pool
/// task id): the running tool, "thinking" between turns, cleared at the end.
fn wire_progress_to_task_store(pool: &SubagentPool) {
    pool.on(|event| {
        let Some(task_id) = event.data.get("task_id").and_then(Value::as_str) else {
            return;
        };
        let activity = match event.name {
            "task_progress" => match event.data["event"].get("type").and_then(Value::as_str) {
                Some("tool_execution_start") => Some(
                    event.data["event"]
                        .get("toolName")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                ),
                Some("turn_end") => Some("thinking".to_string()),
                Some("tool_execution_end") => Some(String::new()),
                _ => None,
            },
            "task_done" | "task_failed" | "task_stalled" | "task_timeout" | "task_cancelled" => {
                Some(String::new())
            }
            _ => None,
        };
        if let Some(activity) = activity {
            task_store().patch_agent(
                task_id,
                TaskAgentPatch {
                    activity: Some(activity),
                    ..Default::default()
                },
            );
        }
    });
}

/// The shared pool if one exists (never creates it).
pub fn peek_subagent_pool() -> Option<SubagentPool> {
    let instance = instance();
    instance
        .override_pool
        .clone()
        .or_else(|| instance.pool.clone())
}

/// Update the skill paths forwarded to every subagent.
pub fn update_subagent_skill_paths(paths: Vec<String>) {
    let mut instance = instance();
    instance.skill_paths = paths.clone();
    if let Some(pool) = &instance.pool {
        pool.update_skill_paths(paths);
    }
}

/// Dispose and clear the shared pool (shutdown, test isolation).
pub fn dispose_subagent_pool() {
    let pool = {
        let mut instance = instance();
        instance.skill_paths.clear();
        instance.pool.take()
    };
    if let Some(pool) = pool {
        pool.dispose();
    }
}

/// Inject a pool for tests; `None` clears the override.
pub fn set_subagent_pool_for_testing(pool: Option<SubagentPool>) {
    instance().override_pool = pool;
}
