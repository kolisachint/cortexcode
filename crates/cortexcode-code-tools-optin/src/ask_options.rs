//! The `ask_options` tool (`extensions/core/ask-options.ts`): put decisions to
//! the user. The options pane is the host's (11.3); in an autonomous `/loop`
//! the tool answers from recommended defaults or halts the loop.

use std::sync::Arc;

use cortexcode_agent_types::AgentToolResult;
use cortexcode_ai_types::{AbortSignal, Content, TextContent};
use cortexcode_code_tool_api::{ToolDefinition, ToolError};
use serde_json::{json, Value};

/// One option (`AskQuestion.options[i]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AskOption {
    pub label: String,
    pub description: Option<String>,
    pub recommended: bool,
}

/// `AskQuestion`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AskQuestion {
    pub question: String,
    /// Label for the answered-step breadcrumb; defaults to `question`.
    pub short: Option<String>,
    pub detail: Option<String>,
    pub options: Vec<AskOption>,
    pub allow_custom: bool,
}

/// What the tool needs from the running session.
pub trait AskOptionsHost: Send + Sync {
    /// Whether an interactive UI can show the pane.
    fn has_ui(&self) -> bool;
    /// Show the pane; `None` when the user skipped. Blocks until answered.
    fn ask_options(
        &self,
        questions: &[AskQuestion],
        signal: Option<AbortSignal>,
    ) -> Option<Vec<Option<String>>>;
    /// Whether an autonomous `/loop` is running (no human to ask).
    fn auto_loop_active(&self) -> bool {
        false
    }
    /// Stop the autonomous loop (`LOOP_HALT`).
    fn halt_loop(&self, _reason: &str) {}
}

/// A host with no UI (print and RPC modes).
#[derive(Debug, Clone, Copy, Default)]
pub struct NoUi;

impl AskOptionsHost for NoUi {
    fn has_ui(&self) -> bool {
        false
    }
    fn ask_options(
        &self,
        _: &[AskQuestion],
        _: Option<AbortSignal>,
    ) -> Option<Vec<Option<String>>> {
        None
    }
}

/// The TypeBox schema hoocode sends for `ask_options`.
pub fn ask_options_parameters_schema() -> Value {
    json!({
        "type": "object",
        "required": ["questions"],
        "properties": {
            "questions": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": ["question", "options"],
                    "properties": {
                        "question": {"type": "string", "description": "The question to ask the user."},
                        "detail": {"type": "string", "description": "Optional clarifying sub-text shown under the question."},
                        "options": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "required": ["label"],
                                "properties": {
                                    "label": {"type": "string", "description": "The option text; returned verbatim when chosen."},
                                    "description": {"type": "string", "description": "Optional short description shown next to the option."},
                                    "recommended": {"type": "boolean", "description": "When true, the option is marked '(recommended)' to help the user choose."}
                                }
                            },
                            "description": "The options the user can choose from."
                        },
                        "allow_custom": {"type": "boolean", "description": "When true, the user can type a free-form answer instead of choosing an option."}
                    }
                },
                "description": "One or more decisions to ask the user, in order."
            }
        }
    })
}

fn parse_questions(args: &Value) -> Result<Vec<AskQuestion>, ToolError> {
    let questions = args
        .get("questions")
        .and_then(Value::as_array)
        .ok_or("questions must be an array")?;
    Ok(questions
        .iter()
        .map(|q| AskQuestion {
            question: q
                .get("question")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
            // The tool never sets it (the schema has no field for it).
            short: None,
            detail: q.get("detail").and_then(Value::as_str).map(str::to_owned),
            options: q
                .get("options")
                .and_then(Value::as_array)
                .map(|opts| {
                    opts.iter()
                        .map(|o| AskOption {
                            label: o
                                .get("label")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_owned(),
                            description: o
                                .get("description")
                                .and_then(Value::as_str)
                                .map(str::to_owned),
                            recommended: o.get("recommended").and_then(Value::as_bool)
                                == Some(true),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            allow_custom: q.get("allow_custom").and_then(Value::as_bool) == Some(true),
        })
        .collect())
}

fn text_result(text: String) -> AgentToolResult {
    AgentToolResult {
        content: vec![Content::Text(TextContent {
            text_signature: None,
            text,
        })],
        details: Value::Null,
        terminate: false,
    }
}

/// The `ask_options` tool definition.
pub fn create_ask_options_tool_definition(host: Arc<dyn AskOptionsHost>) -> ToolDefinition {
    ToolDefinition { background_when: None,
        name: "ask_options".into(),
        label: "Ask the user".into(),
        description: "Ask the user to make one or more decisions before continuing. Each question is presented in an interactive options pane where the user selects an option (or types a custom answer). Use this when you genuinely need input to proceed and cannot reasonably decide yourself. Returns the user's answer for each question; if the user skips, no answers are returned.".into(),
        prompt_snippet: Some("Put a decision to the user as selectable options".into()),
        prompt_guidelines: Vec::new(),
        parameters: ask_options_parameters_schema(),
        prepare_arguments: None,
        execution_mode: None,
        background: false,
        execute: Arc::new(move |_id, args, signal, _on_update, _ctx| {
            if !host.has_ui() {
                return Ok(text_result(
                    "Cannot ask the user: no interactive UI is available in this session. Proceed using your best judgement.".into(),
                ));
            }
            let questions = parse_questions(&args)?;
            if questions.is_empty() {
                return Ok(text_result("No questions were provided.".into()));
            }
            if host.auto_loop_active() {
                let blockers: Vec<&AskQuestion> = questions
                    .iter()
                    .filter(|q| !q.options.iter().any(|o| o.recommended))
                    .collect();
                if !blockers.is_empty() {
                    let list: Vec<String> = blockers.iter().map(|q| format!("  • {}", q.question)).collect();
                    host.halt_loop(&format!(
                        "ask_options had {} question(s) with no recommended default.",
                        blockers.len()
                    ));
                    return Ok(text_result(format!(
                        "Autonomous loop: no user is available to answer, and {} question(s) have no recommended default to fall back on:\n{}\n\nThe loop has been stopped. Do not guess — stop and report this blocker to the user, explaining what decision is needed and the options you were weighing.",
                        blockers.len(),
                        list.join("\n")
                    )));
                }
                let text: Vec<String> = questions
                    .iter()
                    .map(|q| {
                        let recommended = q.options.iter().find(|o| o.recommended).map(|o| o.label.as_str());
                        format!(
                            "{}\n  → {} (auto-selected recommended default; autonomous loop, no user present)",
                            q.question,
                            recommended.unwrap_or("undefined")
                        )
                    })
                    .collect();
                return Ok(text_result(text.join("\n\n")));
            }
            let Some(answers) = host.ask_options(&questions, signal) else {
                return Ok(text_result(
                    "The user skipped the question(s) without answering. Ask how they would like to proceed.".into(),
                ));
            };
            let text: Vec<String> = questions
                .iter()
                .enumerate()
                .map(|(i, q)| {
                    let answer = answers.get(i).cloned().flatten();
                    format!("{}\n  → {}", q.question, answer.as_deref().unwrap_or("(no answer)"))
                })
                .collect();
            Ok(text_result(text.join("\n\n")))
        }),
    }
}
