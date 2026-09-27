//! The `bash` tool: hoocode `core/tools/bash.ts`, `core/bash-executor.ts`,
//! `core/tools/output-accumulator.ts` and `utils/shell.ts` (v0.5.89).

pub mod accumulator;
pub mod executor;
pub mod operations;
pub mod shell;
pub mod tool;

pub use accumulator::{OutputAccumulator, OutputAccumulatorOptions, OutputSnapshot};
pub use executor::{execute_bash_with_operations, BashExecutorOptions, BashResult};
pub use operations::{BashExecOptions, BashOperations, LocalBashOperations};
pub use shell::{
    get_shell_config, get_shell_env, kill_process_tree, kill_tracked_detached_children,
    sanitize_binary_output, strip_ansi, ShellConfig, ShellEnv,
};
pub use tool::{
    bash_parameters_schema, create_bash_tool, create_bash_tool_definition, BashSpawnContext,
    BashSpawnHook, BashToolOptions,
};
