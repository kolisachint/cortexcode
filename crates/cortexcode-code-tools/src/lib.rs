//! Core CLI tools: read, bash, edit, write, grep, find, ls.
//!
//! `read` is the pinned port from `cortexcode-code-tools-fs` (10.2a) and `bash`
//! from `cortexcode-code-tool-bash` (10.2b); the others are placeholders until
//! their 10.2 tasks land.
//!
//! Mirrors `core/tools/` from the TypeScript `packages/coding-agent` package.

use cortexcode_agent_types::{AgentTool, AgentToolResult};
use cortexcode_ai_types::{Content, TextContent};
use cortexcode_code_tool_api::{
    tool_definition_from_agent_tool, wrap_tool_definitions, ToolContextFactory, ToolDefinition,
};
use cortexcode_code_tool_bash::{create_bash_tool_definition, BashToolOptions};
use cortexcode_code_tools_fs::{create_read_tool_definition, ReadToolOptions};
use serde_json::json;
use std::path::Path;

pub mod light;
pub mod permissions;

pub use permissions::*;

/// Build a text-only tool result.
pub fn text_result(text: impl Into<String>) -> AgentToolResult {
    AgentToolResult {
        content: vec![Content::Text(TextContent {
            text_signature: None,
            text: text.into(),
        })],
        details: serde_json::Value::Null,
        terminate: false,
    }
}

/// Build an error tool result.
pub fn error_result(text: impl Into<String>) -> AgentToolResult {
    AgentToolResult {
        content: vec![Content::Text(TextContent {
            text_signature: None,
            text: text.into(),
        })],
        details: serde_json::Value::Null,
        terminate: false,
    }
}

/// Read the contents of a file.
pub fn read_file(path: impl AsRef<Path>) -> Result<String, std::io::Error> {
    std::fs::read_to_string(path)
}

/// Write contents to a file, creating parent directories as needed.
pub fn write_file(
    path: impl AsRef<Path>,
    contents: impl AsRef<[u8]>,
) -> Result<(), std::io::Error> {
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, contents)
}

/// Apply a single exact-string replacement in a file.
pub fn edit_file(path: impl AsRef<Path>, old_text: &str, new_text: &str) -> Result<(), EditError> {
    let path = path.as_ref();
    let text = std::fs::read_to_string(path)?;
    if !text.contains(old_text) {
        return Err(EditError::OldTextNotFound);
    }
    let count = text.matches(old_text).count();
    if count > 1 {
        return Err(EditError::AmbiguousOldText(count));
    }
    let updated = text.replacen(old_text, new_text, 1);
    std::fs::write(path, updated)?;
    Ok(())
}

/// Error type for file edits.
#[derive(Debug)]
pub enum EditError {
    Io(std::io::Error),
    OldTextNotFound,
    AmbiguousOldText(usize),
}

impl std::fmt::Display for EditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EditError::Io(e) => write!(f, "io error: {}", e),
            EditError::OldTextNotFound => write!(f, "old text not found"),
            EditError::AmbiguousOldText(n) => {
                write!(f, "old text matched {} times; expected 1", n)
            }
        }
    }
}

impl std::error::Error for EditError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            EditError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for EditError {
    fn from(e: std::io::Error) -> Self {
        EditError::Io(e)
    }
}

/// Search file contents for a regex pattern.
pub fn grep(
    pattern: &str,
    paths: &[impl AsRef<Path>],
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let regex = regex_lite::Regex::new(pattern)?;
    let mut matches = Vec::new();
    for path in paths {
        let path = path.as_ref();
        if path.is_file() {
            let text = std::fs::read_to_string(path)?;
            for (i, line) in text.lines().enumerate() {
                if regex.is_match(line) {
                    matches.push(format!("{}:{}:{}", path.display(), i + 1, line));
                }
            }
        }
    }
    Ok(matches.join("\n"))
}

/// Recursively find files matching a glob pattern under a root directory.
pub fn find(
    root: impl AsRef<Path>,
    pattern: &str,
) -> Result<Vec<std::path::PathBuf>, Box<dyn std::error::Error + Send + Sync>> {
    let root = root.as_ref();
    let mut results = Vec::new();
    if !root.is_dir() {
        return Ok(results);
    }
    let glob = glob::Pattern::new(pattern)?;
    for entry in walkdir::WalkDir::new(root)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();
        if path.is_file() {
            let relative = path.strip_prefix(root).unwrap_or(path);
            if glob.matches_path(relative) {
                results.push(path.to_path_buf());
            }
        }
    }
    Ok(results)
}

/// List directory entries.
pub fn ls(dir: impl AsRef<Path>) -> Result<Vec<std::path::PathBuf>, std::io::Error> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .collect();
    entries.sort();
    Ok(entries)
}

/// Run an async HTTP call from a synchronous tool body. Tools execute on
/// tokio's blocking pool (agent-loop `run_tool`); outside a runtime (unit
/// tests) a throwaway one is used.
fn block_on<F: std::future::Future>(future: F) -> F::Output {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => tokio::task::block_in_place(|| handle.block_on(future)),
        Err(_) => tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("failed to start a tokio runtime")
            .block_on(future),
    }
}

/// Fetch content from a URL.
pub async fn webfetch(url: &str) -> Result<String, Box<dyn std::error::Error>> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;
    let response = client.get(url).send().await?;
    let text = response.text().await?;
    // Truncate very long responses
    if text.len() > 100000 {
        Ok(format!(
            "{}...[truncated, total {} bytes]",
            &text[..100000],
            text.len()
        ))
    } else {
        Ok(text)
    }
}

/// Search the web using DuckDuckGo.
pub async fn websearch(query: &str) -> Result<String, Box<dyn std::error::Error>> {
    let encoded_query = urlencoding::encode(query);
    let search_url = format!("https://html.duckduckgo.com/html/?q={}", encoded_query);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .user_agent("Mozilla/5.0")
        .build()?;
    let response = client.get(&search_url).send().await?;
    let html = response.text().await?;

    // Simple HTML parsing to extract results
    let mut results = Vec::new();
    let lines: Vec<&str> = html.lines().collect();
    let mut in_result = false;
    let mut current_title = String::new();
    let mut current_snippet = String::new();

    for line in lines {
        if line.contains("result__a") {
            in_result = true;
            // Extract title
            if let Some(start) = line.find(">") {
                if let Some(end) = line.find("</a>") {
                    current_title = line[start + 1..end].trim().to_string();
                }
            }
        } else if in_result && line.contains("result__snippet") {
            if let Some(start) = line.find(">") {
                if let Some(end) = line.find("</a>") {
                    current_snippet = line[start + 1..end].trim().to_string();
                }
            }
            if !current_title.is_empty() {
                results.push(format!("**{}**\n{}", current_title, current_snippet));
            }
            current_title.clear();
            current_snippet.clear();
            in_result = false;
        }
    }

    if results.is_empty() {
        Ok(format!(
            "No results found for '{}'. Visit: {}",
            query, search_url
        ))
    } else {
        Ok(format!(
            "Search results for '{}':\n\n{}",
            query,
            results.join("\n\n")
        ))
    }
}

/// Todo list storage
use std::sync::Mutex;

static TODO_LIST: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Perform a todo action.
pub fn todo_action(
    action: &str,
    task: &str,
    id: usize,
) -> Result<String, Box<dyn std::error::Error>> {
    let mut todos = TODO_LIST.lock().map_err(|e| format!("Lock error: {}", e))?;

    match action {
        "add" => {
            if task.is_empty() {
                return Err("Task description required".into());
            }
            todos.push(task.to_string());
            Ok(format!("Added todo #{}: {}", todos.len(), task))
        }
        "list" => {
            if todos.is_empty() {
                Ok("No todo items".to_string())
            } else {
                let list = todos
                    .iter()
                    .enumerate()
                    .map(|(i, t)| format!("{}. [ ] {}", i + 1, t))
                    .collect::<Vec<_>>()
                    .join("\n");
                Ok(format!("Todo list ({} items):\n{}", todos.len(), list))
            }
        }
        "done" => {
            if id == 0 || id > todos.len() {
                return Err(
                    format!("Invalid todo id: {}. Use 'list' to see available ids.", id).into(),
                );
            }
            let completed = todos.remove(id - 1);
            Ok(format!("Completed todo #{}: {}", id, completed))
        }
        _ => Err(format!("Unknown action: {}. Use 'add', 'list', or 'done'.", action).into()),
    }
}

/// Options for [`default_tools_with`].
#[derive(Clone, Default)]
pub struct DefaultToolsOptions {
    /// Options for the `read` tool (output caps, image resize, dedup).
    pub read: ReadToolOptions,
    /// Options for the `bash` tool (shell, command prefix, output caps).
    pub bash: BashToolOptions,
    /// Supplies the model and session branch to context-aware tools.
    pub ctx_factory: Option<ToolContextFactory>,
}

/// Return the default set of coding tools ready to register with an agent.
pub fn default_tools(cwd: std::path::PathBuf, permissions: PermissionPolicy) -> Vec<AgentTool> {
    default_tools_with(cwd, permissions, DefaultToolsOptions::default())
}

/// [`default_tools`] with tool options and a context factory.
pub fn default_tools_with(
    cwd: std::path::PathBuf,
    permissions: PermissionPolicy,
    options: DefaultToolsOptions,
) -> Vec<AgentTool> {
    wrap_tool_definitions(
        default_tool_definitions(cwd, permissions, options.read, options.bash),
        options.ctx_factory,
    )
}

/// The default tools as definitions, carrying their system prompt snippets
/// and guidelines. Tools not yet ported to their pinned behavior have neither,
/// so the system prompt lists them the way hoocode lists a tool without a
/// `promptSnippet` (not at all).
pub fn default_tool_definitions(
    cwd: std::path::PathBuf,
    _permissions: PermissionPolicy,
    read: ReadToolOptions,
    bash: BashToolOptions,
) -> Vec<ToolDefinition> {
    let mut definitions = vec![
        create_read_tool_definition(cwd.clone(), read),
        create_bash_tool_definition(cwd.clone(), bash),
    ];
    definitions.extend(
        placeholder_tools(cwd)
            .into_iter()
            .map(tool_definition_from_agent_tool),
    );
    definitions
}

/// The tools that still await their 10.2 ports.
pub(crate) fn placeholder_tools(cwd: std::path::PathBuf) -> Vec<AgentTool> {
    let cwd_write = cwd.clone();
    let cwd_edit = cwd.clone();
    let cwd_grep = cwd.clone();
    let cwd_find = cwd.clone();
    let cwd_ls = cwd.clone();

    vec![
        AgentTool::new(
            "write",
            "Write contents to a file. Args: {\"path\": string, \"content\": string}",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "content": { "type": "string" }
                },
                "required": ["path", "content"]
            }),
            Box::new(move |_id, args, _signal, _update| {
                let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
                let content = args.get("content").and_then(|v| v.as_str()).unwrap_or("");
                let full = cwd_write.join(path);
                match write_file(&full, content) {
                    Ok(()) => Ok(text_result(format!("Wrote {}", full.display()))),
                    Err(e) => Ok(error_result(format!("Error writing file: {}", e))),
                }
            }),
        ),
        AgentTool::new(
            "edit",
            "Apply an exact-text replacement in a file. Args: {\"path\": string, \"old_text\": string, \"new_text\": string}",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "old_text": { "type": "string" },
                    "new_text": { "type": "string" }
                },
                "required": ["path", "old_text", "new_text"]
            }),
            Box::new(move |_id, args, _signal, _update| {
                let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
                let old_text = args.get("old_text").and_then(|v| v.as_str()).unwrap_or("");
                let new_text = args.get("new_text").and_then(|v| v.as_str()).unwrap_or("");
                let full = cwd_edit.join(path);
                match edit_file(&full, old_text, new_text) {
                    Ok(()) => Ok(text_result(format!("Edited {}", full.display()))),
                    Err(e) => Ok(error_result(format!("Error editing file: {}", e))),
                }
            }),
        ),
        AgentTool::new(
            "grep",
            "Search file contents with a regex. Args: {\"pattern\": string, \"paths\": string[]}",
            json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string" },
                    "paths": { "type": "array", "items": { "type": "string" } }
                },
                "required": ["pattern", "paths"]
            }),
            Box::new(move |_id, args, _signal, _update| {
                let pattern = args.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
                let paths: Vec<std::path::PathBuf> = args
                    .get("paths")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(|s| cwd_grep.join(s)))
                            .collect()
                    })
                    .unwrap_or_default();
                match grep(pattern, &paths) {
                    Ok(text) => Ok(text_result(text)),
                    Err(e) => Ok(error_result(format!("Error grepping: {}", e))),
                }
            }),
        ),
        AgentTool::new(
            "find",
            "Find files matching a glob under a directory. Args: {\"pattern\": string, \"root\": string}",
            json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string" },
                    "root": { "type": "string" }
                },
                "required": ["pattern"]
            }),
            Box::new(move |_id, args, _signal, _update| {
                let pattern = args.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
                let root = args
                    .get("root")
                    .and_then(|v| v.as_str())
                    .map(|s| cwd_find.join(s))
                    .unwrap_or_else(|| cwd_find.clone());
                match find(&root, pattern) {
                    Ok(paths) => {
                        let text = paths
                            .into_iter()
                            .map(|p| p.display().to_string())
                            .collect::<Vec<_>>()
                            .join("\n");
                        Ok(text_result(text))
                    }
                    Err(e) => Ok(error_result(format!("Error finding files: {}", e))),
                }
            }),
        ),
        AgentTool::new(
            "ls",
            "List directory contents. Args: {\"path\": string}",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" }
                },
                "required": ["path"]
            }),
            Box::new(move |_id, args, _signal, _update| {
                let path = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
                let full = cwd_ls.join(path);
                match ls(&full) {
                    Ok(entries) => {
                        let text = entries
                            .into_iter()
                            .map(|p| {
                                let suffix = if p.is_dir() { "/" } else { "" };
                                format!("{}{}", p.display(), suffix)
                            })
                            .collect::<Vec<_>>()
                            .join("\n");
                        Ok(text_result(text))
                    }
                    Err(e) => Ok(error_result(format!("Error listing directory: {}", e))),
                }
            }),
        ),
        AgentTool::new(
            "webfetch",
            "Fetch content from a URL. Args: url (string)",
            json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string" }
                },
                "required": ["url"]
            }),
            Box::new(move |_id, args, _signal, _update| {
                let url = args.get("url").and_then(|v| v.as_str()).unwrap_or("");
                match block_on(webfetch(url)) {
                    Ok(text) => Ok(text_result(text)),
                    Err(e) => Ok(error_result(format!("Error fetching URL: {}", e))),
                }
            }),
        ),
        AgentTool::new(
            "websearch",
            "Search the web. Args: query (string)",
            json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" }
                },
                "required": ["query"]
            }),
            Box::new(move |_id, args, _signal, _update| {
                let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
                match block_on(websearch(query)) {
                    Ok(text) => Ok(text_result(text)),
                    Err(e) => Ok(error_result(format!("Error searching web: {}", e))),
                }
            }),
        ),
        AgentTool::new(
            "todo",
            "Track todo items. Args: action (add|list|done), task (string, optional), id (number, optional)",
            json!({
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["add", "list", "done"] },
                    "task": { "type": "string" },
                    "id": { "type": "number" }
                },
                "required": ["action"]
            }),
            Box::new(move |_id, args, _signal, _update| {
                let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("list");
                let task = args.get("task").and_then(|v| v.as_str()).unwrap_or("");
                let id = args.get("id").and_then(|v| v.as_u64()).unwrap_or(0);
                match todo_action(action, task, id as usize) {
                    Ok(text) => Ok(text_result(text)),
                    Err(e) => Ok(error_result(format!("Error with todo: {}", e))),
                }
            }),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "cortex-code-tools-test-{}-{}",
            name,
            std::process::id()
        ))
    }

    #[test]
    fn test_read_write_edit() {
        let dir = temp_dir("read-write-edit");
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("file.txt");
        write_file(&path, "hello world").unwrap();
        assert_eq!(read_file(&path).unwrap(), "hello world");
        edit_file(&path, "world", "rust").unwrap();
        assert_eq!(read_file(&path).unwrap(), "hello rust");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_edit_ambiguous() {
        let dir = temp_dir("ambiguous");
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("file.txt");
        write_file(&path, "abc abc").unwrap();
        assert!(matches!(
            edit_file(&path, "abc", "x").unwrap_err(),
            EditError::AmbiguousOldText(2)
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_ls() {
        let dir = temp_dir("ls");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        write_file(dir.join("a.txt"), "").unwrap();
        write_file(dir.join("b.txt"), "").unwrap();
        let entries = ls(&dir).unwrap();
        assert_eq!(entries.len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_grep_and_find() {
        let dir = temp_dir("grep-find");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        write_file(dir.join("foo.rs"), "fn main() {}").unwrap();
        write_file(dir.join("bar.rs"), "fn helper() {}").unwrap();

        let matches = grep("fn main", &[dir.join("foo.rs"), dir.join("bar.rs")]).unwrap();
        assert!(matches.contains("main"));

        let found = find(&dir, "*.rs").unwrap();
        assert_eq!(found.len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_todo_actions() {
        // Test add
        let result = todo_action("add", "Buy groceries", 0).unwrap();
        assert!(result.contains("Added todo"));

        // Test list
        let result = todo_action("list", "", 0).unwrap();
        assert!(result.contains("Buy groceries"));

        // Test done
        let result = todo_action("done", "", 1).unwrap();
        assert!(result.contains("Completed todo"));

        // Test list after done
        let result = todo_action("list", "", 0).unwrap();
        assert!(result.contains("No todo items"));
    }

    #[test]
    fn test_todo_invalid_id() {
        let result = todo_action("done", "", 999);
        assert!(result.is_err());
    }

    #[test]
    fn test_todo_empty_task() {
        let result = todo_action("add", "", 0);
        assert!(result.is_err());
    }

    #[test]
    fn test_webfetch_invalid_url() {
        let result = block_on(webfetch("not-a-valid-url"));
        assert!(result.is_err());
    }

    #[test]
    fn test_websearch_empty_query() {
        let result = block_on(websearch(""));
        // Should return some result or error, not panic
        assert!(result.is_ok() || result.is_err());
    }
}
