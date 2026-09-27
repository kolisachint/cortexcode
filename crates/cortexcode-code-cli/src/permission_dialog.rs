//! The permission gate's prompt (`ctx.ui.select` with the gate's three
//! choices) for the placeholder interactive mode, drawn with crossterm until
//! the TUI selectors arrive (11.3).

use cortexcode_code_permissions::PermissionUi;
use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEventKind},
    style::{self, Stylize},
    terminal::{self, ClearType},
    QueueableCommand,
};
use std::io::Write;

/// Terminal prompt for the permission gate.
#[derive(Debug, Default)]
pub struct TerminalPermissionUi;

impl PermissionUi for TerminalPermissionUi {
    /// Y / N / A pick the gate's options in order.
    fn select(&self, title: &str, options: &[&str]) -> Option<String> {
        let index = match prompt(title) {
            PromptResult::Yes => 0,
            PromptResult::No => 1,
            PromptResult::Always => 2,
        };
        options.get(index).map(|o| o.to_string())
    }

    fn notify(&self, message: &str) {
        println!("\r{message}");
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PromptResult {
    Yes,
    No,
    Always,
}

fn prompt(title: &str) -> PromptResult {
    // Best-effort terminal UI. If raw mode is already enabled, keep it; otherwise
    // clear and draw a dialog. If anything goes wrong, default to deny.
    let mut stdout = std::io::stdout();
    let mut draw = || -> std::io::Result<()> {
        stdout
            .queue(cursor::MoveToColumn(0))?
            .queue(terminal::Clear(ClearType::CurrentLine))?
            .queue(style::Print("\n"))?
            .queue(style::Print(format!("{}\n", title.to_string().bold())))?;

        stdout
            .queue(style::Print("\n"))?
            .queue(style::Print(
                "[Y] Yes (once)  [N] No (block)  [A] Always (add to auto-allow for this mode)\n"
                    .bold(),
            ))?
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
