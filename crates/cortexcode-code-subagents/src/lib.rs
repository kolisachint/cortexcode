//! Subagent orchestration for the cortex coding agent (hoocode's
//! `core/subagent-*.ts`, `lifeguard.ts`, `dispatch-evaluator.ts`,
//! `token-budget.ts`, `output-verifier.ts`, `model-categories.ts`).
//!
//! [`pool::SubagentPool`] runs each subagent as a child process
//! (`cortex --mode json --task-id <id> ...`): progress events and
//! `{"ping":true}` heartbeats on its stdout, a verified `result.json` in the
//! task's dispatch dir settles it. [`lifeguard`] reaps silent or overdue
//! children. The Task/TaskOutput tools and the warm (RPC) pool build on this.

pub mod agent_log;
pub mod depth;
pub mod dispatch;
pub mod events;
pub mod instance;
pub mod lifeguard;
pub mod model_categories;
pub mod output_verifier;
pub mod pool;
pub mod result;
pub mod token_budget;

pub use pool::{
    DispatchOptions, PoolError, PoolEvent, SubagentPool, SubagentPoolOptions, SubagentPoolTask,
    SubagentResult, TaskResult, DEFAULT_SUBAGENT_MAX_TURNS,
};
