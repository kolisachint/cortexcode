//! Interactive permission dialog for dangerous tool calls.
//!
//! Displays the tool name and arguments and asks the user to approve, deny,
//! or always approve the tool. Implemented with crossterm to match the
//! existing interactive mode in this crate.

use cortexcode_agent_types::{AgentToolCall, PermissionDecision, PermissionGate};
use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEventKind},
    style::{self, Stylize},
    terminal::{self, ClearType},
    QueueableCommand,
};
use std::io::Write;

/// Interactive permission gate that prompts the user in the terminal.
#[derive(Debug, Default)]
pub struct InteractivePermissionGate;

impl PermissionGate for InteractivePermissionGate {
    fn request(&self, tool_call: &AgentToolCall) -> PermissionDecision {
        match prompt(tool_call) {
            PromptResult::Yes => PermissionDecision::Grant,
            PromptResult::Always => PermissionDecision::GrantAlways,
            PromptResult::No => PermissionDecision::Deny {
                reason: "User denied the tool call".into(),
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PromptResult {
    Yes,
    No,
    Always,
}

fn prompt(tool_call: &AgentToolCall) -> PromptResult {
    // Best-effort terminal UI. If raw mode is already enabled, keep it; otherwise
    // clear and draw a dialog. If anything goes wrong, default to deny.
    let mut stdout = std::io::stdout();
    let mut draw = || -> std::io::Result<()> {
        stdout
            .queue(cursor::MoveToColumn(0))?
            .queue(terminal::Clear(ClearType::CurrentLine))?
            .queue(style::Print("\n"))?
            .queue(style::Print("Tool call requires approval:\n".bold()))?
            .queue(style::Print(format!(
                "  {}\n",
                tool_call.name.clone().yellow()
            )))?;

        if let Ok(args) = serde_json::to_string_pretty(&tool_call.arguments) {
            for line in args.lines() {
                stdout.queue(style::Print(format!("  {}\n", line.dim())))?;
            }
        }

        stdout
            .queue(style::Print("\n"))?
            .queue(style::Print("[Y] Yes  [N] No  [A] Always approve\n".bold()))?
            .queue(style::Print("Choice: "))?
            .flush()?;
        Ok(())
    };

    if draw().is_err() {
        return PromptResult::No;
    }

    loop {
        if event::poll(std::time::Duration::from_millis(100)).unwrap_or(false) {
            if let Ok(Event::Key(key)) = event::read() {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                match key.code {
                    KeyCode::Char('y') | KeyCode::Char('Y') => {
                        let _ = stdout.queue(style::Print("Yes\n"));
                        let _ = stdout.flush();
                        return PromptResult::Yes;
                    }
                    KeyCode::Char('n') | KeyCode::Char('N') => {
                        let _ = stdout.queue(style::Print("No\n"));
                        let _ = stdout.flush();
                        return PromptResult::No;
                    }
                    KeyCode::Char('a') | KeyCode::Char('A') => {
                        let _ = stdout.queue(style::Print("Always\n"));
                        let _ = stdout.flush();
                        return PromptResult::Always;
                    }
                    _ => {}
                }
            }
        }
    }
}
