//! Core CLI tools: read, bash, edit, write, grep, find, ls.
//!
//! Mirrors `core/tools/` from the TypeScript `packages/coding-agent` package.

use cortexcode_agent_types::{AgentTool, AgentToolResult};
use cortexcode_ai_types::{Content, TextContent};
use serde_json::json;
use std::path::Path;

pub mod permissions;

pub use permissions::*;

/// Build a text-only tool result.
pub fn text_result(text: impl Into<String>) -> AgentToolResult {
    AgentToolResult {
        content: vec![Content::Text(TextContent {
            text: text.into(),
            cache_control: None,
        })],
        details: serde_json::Value::Null,
        terminate: false,
    }
}

/// Build an error tool result.
pub fn error_result(text: impl Into<String>) -> AgentToolResult {
    AgentToolResult {
        content: vec![Content::Text(TextContent {
            text: text.into(),
            cache_control: None,
        })],
        details: serde_json::Value::Null,
        terminate: false,
    }
}

/// Read the contents of a file.
pub fn read_file(path: impl AsRef<Path>) -> Result<String, std::io::Error> {
    std::fs::read_to_string(path)
}

/// Read a specific line range from a file (1-indexed, inclusive).
pub fn read_file_range(
    path: impl AsRef<Path>,
    start_line: usize,
    end_line: usize,
) -> Result<String, std::io::Error> {
    let text = std::fs::read_to_string(path)?;
    let lines: Vec<&str> = text.lines().collect();
    let start = start_line.saturating_sub(1).min(lines.len());
    let end = end_line.min(lines.len());
    Ok(lines[start..end].join("\n"))
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

/// Execute a shell command and return stdout/stderr.
pub fn bash(command: &str, cwd: Option<&Path>) -> Result<std::process::Output, std::io::Error> {
    let mut cmd = if cfg!(target_os = "windows") {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", command]);
        c
    } else {
        let mut c = std::process::Command::new("sh");
        c.args(["-c", command]);
        c
    };
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    cmd.output()
}

/// Format command output as a string.
pub fn format_output(output: &std::process::Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let mut result = String::new();
    if !stdout.is_empty() {
        result.push_str(&stdout);
    }
    if !stderr.is_empty() {
        if !result.is_empty() {
            result.push('\n');
        }
        result.push_str("stderr:\n");
        result.push_str(&stderr);
    }
    if result.is_empty() {
        result.push_str("(no output)");
    }
    result
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

/// Return the default set of coding tools ready to register with an agent.
pub fn default_tools(cwd: std::path::PathBuf, _permissions: PermissionPolicy) -> Vec<AgentTool> {
    let cwd_read = cwd.clone();
    let cwd_bash = cwd.clone();
    let cwd_write = cwd.clone();
    let cwd_edit = cwd.clone();
    let cwd_grep = cwd.clone();
    let cwd_find = cwd.clone();
    let cwd_ls = cwd;

    vec![
        AgentTool::new(
            "read",
            "Read file contents. Args: {\"path\": string, \"offset\"?: number, \"limit\"?: number}",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "offset": { "type": "number" },
                    "limit": { "type": "number" }
                },
                "required": ["path"]
            }),
            Box::new(move |_id, args, _signal, _update| {
                let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
                let full = cwd_read.join(path);
                let result = match (
                    args.get("offset").and_then(|v| v.as_u64()),
                    args.get("limit").and_then(|v| v.as_u64()),
                ) {
                    (Some(offset), Some(limit)) => {
                        read_file_range(&full, offset as usize, (offset + limit) as usize)
                    }
                    _ => read_file(&full),
                };
                match result {
                    Ok(text) => Ok(text_result(text)),
                    Err(e) => Ok(error_result(format!("Error reading file: {}", e))),
                }
            }),
        ),
        AgentTool::new(
            "bash",
            "Run a shell command. Args: {\"command\": string}",
            json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string" }
                },
                "required": ["command"]
            }),
            Box::new(move |_id, args, _signal, _update| {
                let command = args.get("command").and_then(|v| v.as_str()).unwrap_or("");
                match bash(command, Some(&cwd_bash)) {
                    Ok(output) => Ok(text_result(format_output(&output))),
                    Err(e) => Ok(error_result(format!("Error running command: {}", e))),
                }
            }),
        ),
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
    fn test_bash_echo() {
        let output = bash("echo hello", None).unwrap();
        assert!(String::from_utf8_lossy(&output.stdout).contains("hello"));
    }
}
