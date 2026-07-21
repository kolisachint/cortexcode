//! Subagent orchestration for the cortex coding agent.
//!
//! Mirrors `core/subagent-pool.ts` and `core/tools/subagent.ts` from the
//! TypeScript `packages/coding-agent` package. The pool spawns child processes
//! running `cortex --mode subagent --task-id <id>` and communicates with them
//! via line-delimited JSON-RPC over stdin/stdout.

use cortexcode_agent_types::{AgentTool, AgentToolResult};
use cortexcode_ai_types::{Content, TextContent};
use cortexcode_code_rpc::{Request, Response, RpcError};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for a subagent pool.
#[derive(Debug, Clone)]
pub struct SubagentOptions {
    /// Command to spawn (typically the `cortex` binary path).
    pub command: PathBuf,
    /// Extra arguments to pass before the standard `--mode subagent` args.
    pub args: Vec<String>,
    /// Environment variables to set for the child process.
    pub env: HashMap<String, String>,
    /// Maximum number of concurrently running subagents.
    pub max_concurrent: usize,
    /// Default timeout for a single task.
    pub timeout: Duration,
    /// Working directory for spawned processes.
    pub cwd: Option<PathBuf>,
}

impl Default for SubagentOptions {
    fn default() -> Self {
        Self {
            command: PathBuf::from("cortex"),
            args: Vec::new(),
            env: HashMap::new(),
            max_concurrent: 4,
            timeout: Duration::from_secs(120),
            cwd: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Result / errors
// ---------------------------------------------------------------------------

/// Result returned by a subagent task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentResult {
    /// Text returned by the subagent.
    pub output: String,
    /// Whether the task completed successfully.
    pub success: bool,
    /// Optional structured details.
    #[serde(default)]
    pub details: Value,
}

/// Errors that can occur while running a subagent.
#[derive(Debug)]
pub enum SubagentError {
    Io(std::io::Error),
    Spawn(String),
    Json(serde_json::Error),
    Rpc(RpcError),
    TimedOut,
    TaskFailed(String),
}

impl std::fmt::Display for SubagentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SubagentError::Io(e) => write!(f, "io error: {}", e),
            SubagentError::Spawn(e) => write!(f, "spawn error: {}", e),
            SubagentError::Json(e) => write!(f, "json error: {}", e),
            SubagentError::Rpc(e) => write!(f, "rpc error: {}", e),
            SubagentError::TimedOut => write!(f, "subagent timed out"),
            SubagentError::TaskFailed(e) => write!(f, "task failed: {}", e),
        }
    }
}

impl std::error::Error for SubagentError {}

impl From<std::io::Error> for SubagentError {
    fn from(e: std::io::Error) -> Self {
        SubagentError::Io(e)
    }
}

impl From<serde_json::Error> for SubagentError {
    fn from(e: serde_json::Error) -> Self {
        SubagentError::Json(e)
    }
}

impl From<RpcError> for SubagentError {
    fn from(e: RpcError) -> Self {
        SubagentError::Rpc(e)
    }
}

// ---------------------------------------------------------------------------
// Subagent pool
// ---------------------------------------------------------------------------

/// A pool that limits concurrent subagent processes.
#[derive(Debug)]
pub struct SubagentPool {
    options: SubagentOptions,
    permits: Arc<(Mutex<usize>, Condvar)>,
}

impl SubagentPool {
    /// Create a new pool with the given options.
    pub fn new(options: SubagentOptions) -> Self {
        let max = options.max_concurrent.max(1);
        Self {
            options,
            permits: Arc::new((Mutex::new(max), Condvar::new())),
        }
    }

    /// Return the pool options.
    pub fn options(&self) -> &SubagentOptions {
        &self.options
    }

    /// Run a single task in a subagent process, respecting concurrency limits
    /// and the configured timeout.
    pub fn run_task(&self, task_id: &str, prompt: &str) -> Result<SubagentResult, SubagentError> {
        let _permit = Permit::acquire(self.permits.clone());
        let mut handle = self.spawn(task_id)?;
        handle.send_request(
            "messages/complete",
            Some(serde_json::json!({"prompt": prompt })),
        )?;
        let deadline = Instant::now() + self.options.timeout;
        let response = handle.receive_response(Some(deadline))?;
        handle.kill()?;
        Self::response_to_result(response)
    }

    fn spawn(&self, task_id: &str) -> Result<SubagentHandle, SubagentError> {
        let mut cmd = Command::new(&self.options.command);
        cmd.arg("--mode")
            .arg("subagent")
            .arg("--task-id")
            .arg(task_id)
            .args(&self.options.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        if let Some(cwd) = &self.options.cwd {
            cmd.current_dir(cwd);
        }

        for (key, value) in &self.options.env {
            cmd.env(key, value);
        }

        let mut child = cmd
            .spawn()
            .map_err(|e| SubagentError::Spawn(e.to_string()))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| SubagentError::Spawn("failed to capture subagent stdin".to_string()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| SubagentError::Spawn("failed to capture subagent stdout".to_string()))?;

        Ok(SubagentHandle {
            child,
            stdin,
            stdout: Some(BufReader::new(stdout)),
            task_id: task_id.to_string(),
        })
    }

    fn response_to_result(response: Response) -> Result<SubagentResult, SubagentError> {
        if let Some(error) = response.error {
            return Err(SubagentError::TaskFailed(error.message));
        }
        let result = response.result.unwrap_or(Value::Null);
        let output = result
            .get("content")
            .and_then(|c| c.as_str().map(|s| s.to_string()))
            .unwrap_or_default();
        Ok(SubagentResult {
            output,
            success: true,
            details: result,
        })
    }
}

// ---------------------------------------------------------------------------
// Subagent handle
// ---------------------------------------------------------------------------

/// A handle to a single spawned subagent process.
pub struct SubagentHandle {
    child: Child,
    stdin: ChildStdin,
    stdout: Option<BufReader<ChildStdout>>,
    task_id: String,
}

impl SubagentHandle {
    /// Send a JSON-RPC request to the subagent.
    pub fn send_request(
        &mut self,
        method: &str,
        params: Option<Value>,
    ) -> Result<(), SubagentError> {
        let request = Request {
            jsonrpc: "2.0".to_string(),
            id: Some(Value::String(self.task_id.clone())),
            method: method.to_string(),
            params,
        };
        let line = serde_json::to_string(&request)?;
        writeln!(self.stdin, "{}", line)?;
        self.stdin.flush()?;
        Ok(())
    }

    /// Read a single JSON-RPC response from the subagent.
    ///
    /// If `deadline` is provided, the call returns `SubagentError::TimedOut`
    /// when the deadline is reached.
    pub fn receive_response(
        &mut self,
        deadline: Option<Instant>,
    ) -> Result<Response, SubagentError> {
        let mut stdout = self
            .stdout
            .take()
            .ok_or_else(|| SubagentError::TaskFailed("stdout already consumed".to_string()))?;
        let mut line = String::new();

        if let Some(deadline) = deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let mut buf = String::new();
                let result = stdout.read_line(&mut buf).map(|_| buf);
                let _ = tx.send((result, stdout));
            });
            match rx.recv_timeout(remaining) {
                Ok((Ok(read_line), stdout)) => {
                    self.stdout = Some(stdout);
                    line = read_line;
                }
                Ok((Err(e), stdout)) => {
                    self.stdout = Some(stdout);
                    return Err(SubagentError::Io(e));
                }
                Err(_) => {
                    let _ = self.child.kill();
                    return Err(SubagentError::TimedOut);
                }
            }
        } else {
            let n = stdout.read_line(&mut line)?;
            if n == 0 {
                return Err(SubagentError::TaskFailed(
                    "subagent closed stdout without response".to_string(),
                ));
            }
            self.stdout = Some(stdout);
        }

        if line.is_empty() {
            return Err(SubagentError::TaskFailed(
                "subagent closed stdout without response".to_string(),
            ));
        }
        let response: Response = serde_json::from_str(&line)?;
        Ok(response)
    }

    /// Kill the subagent process.
    pub fn kill(&mut self) -> Result<(), SubagentError> {
        let _ = self.child.kill();
        let _ = self.child.wait();
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Concurrency permit
// ---------------------------------------------------------------------------

struct Permit {
    permits: Arc<(Mutex<usize>, Condvar)>,
}

impl Permit {
    fn acquire(permits: Arc<(Mutex<usize>, Condvar)>) -> Self {
        let (lock, cvar) = &*permits;
        let mut guard = lock.lock().unwrap();
        while *guard == 0 {
            guard = cvar.wait(guard).unwrap();
        }
        *guard -= 1;
        drop(guard);
        Self { permits }
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        let (lock, cvar) = &*self.permits;
        let mut guard = lock.lock().unwrap();
        *guard += 1;
        cvar.notify_one();
    }
}

// ---------------------------------------------------------------------------
// Task tool factory
// ---------------------------------------------------------------------------

/// Build the `task` agent tool that dispatches work to a subagent pool.
///
/// The tool accepts a `prompt` parameter and returns the subagent's output.
pub fn task_tool(pool: SubagentPool) -> AgentTool {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "prompt": { "type": "string", "description": "The task to hand off to a subagent." },
            "task_id": { "type": "string", "description": "Optional task id." }
        },
        "required": ["prompt"],
    });

    AgentTool::new(
        "task",
        "Spawn a subagent to complete an independent coding task.",
        schema,
        Box::new(move |id, args, _signal, _update| {
            let prompt = args
                .get("prompt")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let task_id = args
                .get("task_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| id);
            let result = pool.run_task(&task_id, &prompt)?;
            let text = if result.success {
                result.output
            } else {
                format!("subagent failed: {}", result.output)
            };
            Ok(AgentToolResult {
                content: vec![Content::Text(TextContent {
                    text,
                    cache_control: None,
                })],
                details: result.details,
                terminate: false,
            })
        }),
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_options() {
        let opts = SubagentOptions::default();
        assert_eq!(opts.command, PathBuf::from("cortex"));
        assert_eq!(opts.max_concurrent, 4);
    }

    #[test]
    fn test_permit_limits_concurrency() {
        let opts = SubagentOptions {
            max_concurrent: 2,
            ..Default::default()
        };
        let pool = SubagentPool::new(opts);
        let p1 = Permit::acquire(pool.permits.clone());
        let p2 = Permit::acquire(pool.permits.clone());

        // With 2 permits taken, a third acquisition would block. Verify the
        // counter is correct rather than deadlocking the test.
        {
            let (lock, _) = &*pool.permits;
            let guard = lock.lock().unwrap();
            assert_eq!(*guard, 0);
        }
        drop(p1);
        drop(p2);
        {
            let (lock, _) = &*pool.permits;
            let guard = lock.lock().unwrap();
            assert_eq!(*guard, 2);
        }
    }

    #[test]
    fn test_task_tool_schema() {
        let tool = task_tool(SubagentPool::new(SubagentOptions::default()));
        assert_eq!(tool.name, "task");
        assert!(tool.parameters.get("properties").is_some());
    }
}
