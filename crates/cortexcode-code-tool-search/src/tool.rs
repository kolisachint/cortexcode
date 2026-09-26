//! The `SearchCodebase` tool (`core/tools/search.ts`). Interactive rendering
//! arrives with phase 11.

use std::path::PathBuf;
use std::sync::Arc;

use cortexcode_agent_types::{AgentTool, AgentToolResult};
use cortexcode_ai_types::{AbortSignal, Content, TextContent};
use cortexcode_code_tool_api::{wrap_tool_definition, ToolContextFactory, ToolDefinition};
use serde_json::{json, Map, Value};

use crate::hybrid::{run_search, RetrieveOptions};
use crate::service::EmbsearchService;
use crate::types::SearchMode;

const DEFAULT_RESULTS: f64 = 5.0;
const MAX_RESULTS: f64 = 30.0;

/// Resolves the per-session embsearch service at call time.
pub type ServiceProvider = Arc<dyn Fn() -> Option<Arc<dyn EmbsearchService>> + Send + Sync>;

/// `SearchToolOptions`.
#[derive(Clone, Default)]
pub struct SearchToolOptions {
    /// The embsearch service, when semantic indexing runs. Without one every
    /// mode resolves to lexical.
    pub get_service: Option<ServiceProvider>,
}

/// The TypeBox schema hoocode sends for `SearchCodebase`.
pub fn search_parameters_schema() -> Value {
    json!({
        "type": "object",
        "required": ["query"],
        "properties": {
            "query": {"type": "string", "description": "What to find: an identifier, error text, or a natural-language description of the code, e.g. 'where sessions are persisted to disk'"},
            "mode": {
                "anyOf": [
                    {"type": "string", "const": "auto"},
                    {"type": "string", "const": "lexical"},
                    {"type": "string", "const": "semantic"},
                    {"type": "string", "const": "hybrid"}
                ],
                "description": "Retrieval mode (default: auto, which is almost always right). auto = hybrid when the index is available, else lexical; hybrid = keyword and meaning over the index plus exact text for anything indexed later; semantic = index only, skipping exact text; lexical = exact text only, the one mode that works with no index."
            },
            "glob": {"type": "string", "description": "Optional glob filter applied to file paths. Only file paths matching the glob are searched. Supports both slashless patterns (match base name anywhere) and slash patterns (match full path)."},
            "limit": {"type": "number", "description": "Maximum number of results (default: 5, max: 30)"}
        }
    })
}

fn aborted(signal: &Option<AbortSignal>) -> bool {
    signal.as_ref().is_some_and(AbortSignal::aborted)
}

fn text(text: String) -> Vec<Content> {
    vec![Content::Text(TextContent {
        text_signature: None,
        text,
    })]
}

/// `createSearchToolDefinition`.
pub fn create_search_tool_definition(
    cwd: impl Into<PathBuf>,
    options: SearchToolOptions,
) -> ToolDefinition {
    let cwd: PathBuf = cwd.into();
    ToolDefinition {
        name: "SearchCodebase".into(),
        label: "SearchCodebase".into(),
        description: "Find where code lives: ranked file:line-range results, fusing keyword and semantic retrieval over a local index with exact-text search of files the index has not read yet. The query is plain text, not a regex — regex metacharacters are matched literally. Falls back to exact-text retrieval automatically when the index is unavailable, and still finds code written moments ago that no index has seen.".into(),
        prompt_snippet: Some("Ranked code search (keyword + semantic, rank-fused)".into()),
        prompt_guidelines: vec![
            "SearchCodebase defaults to mode=auto, which is almost always right. Use limit=3 for targeted lookups, 10–20 when exploring a broad topic — past ~15 results the deeper ones arrive as ranked file:line-range headers without a snippet, which is still enough to choose what to read.".into(),
        ],
        parameters: search_parameters_schema(),
        prepare_arguments: None,
        execution_mode: None,
        background: false,
        execute: Arc::new(move |_id, args, signal, _on_update, _ctx| {
            if aborted(&signal) {
                return Err("Operation aborted".into());
            }
            let query = args
                .get("query")
                .and_then(Value::as_str)
                .ok_or("The \"query\" argument must be of type string")?;
            let mode = args
                .get("mode")
                .and_then(Value::as_str)
                .and_then(SearchMode::parse)
                .unwrap_or_default();
            let glob = args.get("glob").and_then(Value::as_str);
            let requested = args.get("limit").and_then(Value::as_f64).unwrap_or(DEFAULT_RESULTS);
            // Math.min(30, Math.max(1, limit)); slice() truncates fractions.
            let limit = requested.clamp(1.0, MAX_RESULTS) as usize;
            let service = options.get_service.as_ref().and_then(|f| f());
            let result = run_search(
                &RetrieveOptions {
                    mode,
                    glob,
                    limit: Some(limit),
                    service,
                    signal: signal.clone(),
                    ..RetrieveOptions::new(&cwd, query)
                },
                None,
            )?;
            if aborted(&signal) {
                return Err("Operation aborted".into());
            }
            let mut details = Map::new();
            details.insert("resultCount".into(), result.result_count.into());
            details.insert("resolvedMode".into(), result.resolved_mode.as_str().into());
            if let Some((done, total)) = result.indexing {
                details.insert("indexing".into(), json!({"done": done, "total": total}));
            }
            let mut notices = Vec::new();
            if let Some(reason) = &result.degraded_reason {
                notices.push(reason.clone());
            }
            if let Some((done, total)) = result.indexing {
                notices.push(format!(
                    "index still building: {done}/{total} chunks embedded — results may be incomplete"
                ));
            }
            let notice = if notices.is_empty() {
                String::new()
            } else {
                format!("\n\n[{}]", notices.join(". "))
            };
            let body = if result.result_count == 0 {
                format!("No results for \"{query}\" ({}){notice}", result.resolved_mode.as_str())
            } else {
                format!("{}{notice}", result.text)
            };
            Ok(AgentToolResult {
                content: text(body),
                details: Value::Object(details),
                terminate: false,
            })
        }),
    }
}

/// `createSearchTool`.
pub fn create_search_tool(
    cwd: impl Into<PathBuf>,
    options: SearchToolOptions,
    ctx_factory: Option<ToolContextFactory>,
) -> AgentTool {
    wrap_tool_definition(create_search_tool_definition(cwd, options), ctx_factory)
}
