//! `components/tool-chain.ts`: a run of consecutive tool calls, one line in
//! a collapsed radar view and a plain pass-through everywhere else.
//!
//! A chain closes when the agent next speaks or the turn settles; closing
//! while it is still the bottom of the screen keeps the rewrite to one line.

use std::cell::RefCell;
use std::rc::Rc;

use cortexcode_code_tui_theme::theme;
use cortexcode_tui_render::Component;
use cortexcode_tui_util::{truncate_to_width, visible_width};

use crate::tool_chain_summary::{
    chain_phrase, chain_segments, chain_stats, ChainState, SegmentTone,
};
use crate::tool_execution::ToolExecutionComponent;
use crate::tool_output_view::ToolOutputView;

pub type ToolBlock = Rc<RefCell<ToolExecutionComponent>>;

pub struct ToolChainComponent {
    blocks: Vec<ToolBlock>,
    view: ToolOutputView,
    state: ChainState,
    /// The newest run in the transcript; radar marks it.
    latest: bool,
}

impl ToolChainComponent {
    pub fn new(view: ToolOutputView) -> Self {
        Self {
            blocks: Vec::new(),
            view,
            state: ChainState::Running,
            latest: false,
        }
    }

    pub fn add(&mut self, block: ToolBlock) {
        self.blocks.push(block);
    }

    /// True until the agent speaks or the turn settles.
    pub fn is_open(&self) -> bool {
        self.state == ChainState::Running
    }

    /// Whether a summary line stands in for this chain's calls right now.
    pub fn is_summarised(&self) -> bool {
        self.is_collapsed()
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    pub fn tool_blocks(&self) -> &[ToolBlock] {
        &self.blocks
    }

    /// Settle the chain; `Interrupted` keeps the running rendering.
    pub fn close(&mut self, outcome: ChainState) {
        self.state = outcome;
    }

    pub fn set_view(&mut self, view: ToolOutputView) {
        self.view = view;
        for block in &self.blocks {
            block.borrow_mut().set_view(view);
        }
    }

    pub fn set_latest(&mut self, latest: bool) {
        self.latest = latest;
    }

    /// A chain of one is never summarised: its row says more than a phrase.
    fn is_collapsed(&self) -> bool {
        self.view == ToolOutputView::Radar && self.blocks.len() > 1
    }

    /// The blank row that holds a radar run off whatever came before it.
    fn needs_lead_in(&self) -> bool {
        if self.view != ToolOutputView::Radar {
            return false;
        }
        if self.is_collapsed() {
            return true;
        }
        self.blocks
            .first()
            .is_some_and(|b| !b.borrow().draws_leading_gap())
    }

    fn rows(&mut self, width: u16) -> Vec<String> {
        if !self.is_collapsed() {
            let mut out = Vec::new();
            for block in &self.blocks {
                out.extend(block.borrow_mut().render(width));
            }
            return out;
        }
        let width = width as usize;
        let entries: Vec<_> = self
            .blocks
            .iter()
            .map(|b| b.borrow().chain_entry())
            .collect();
        let running = self.state == ChainState::Running;
        let glyph = if running { "◐" } else { "●" };
        let glyph_tone = if entries.iter().any(|e| e.is_error) {
            "error"
        } else if running {
            "warning"
        } else {
            "success"
        };

        let t = theme();
        let (left_plain, left_styled) = if self.state == ChainState::Done {
            let phrase = chain_phrase(&entries);
            (phrase.clone(), t.fg("toolTitle", &phrase))
        } else {
            let segments = chain_segments(&entries);
            let sep = " › ";
            let ellipsis = if running { "…" } else { "" };
            let plain = format!(
                "{}{ellipsis}",
                segments
                    .iter()
                    .map(|s| s.label.as_str())
                    .collect::<Vec<_>>()
                    .join(sep)
            );
            let styled = segments
                .iter()
                .map(|s| {
                    let tone = match s.tone {
                        SegmentTone::Error => "error",
                        SegmentTone::Running => "warning",
                        SegmentTone::Ok => "toolTitle",
                    };
                    t.fg(tone, &s.label)
                })
                .collect::<Vec<_>>()
                .join(&t.fg("dim", sep))
                + &(if running {
                    t.fg("dim", "…")
                } else {
                    String::new()
                });
            (plain, styled)
        };

        let stats = chain_stats(&entries, self.state);
        let stroke = self.latest && t.has_bg("activeToolBg");
        let mark = |text: &str| {
            if stroke {
                t.bg("activeToolBg", text)
            } else {
                text.to_string()
            }
        };
        let prefix = format!("{glyph} ");
        let budget = width.saturating_sub(1 + visible_width(&prefix));
        let line = if visible_width(&left_plain) + 2 + visible_width(&stats) <= budget {
            let pad = " ".repeat(budget - visible_width(&left_plain) - visible_width(&stats));
            format!(
                " {}{}{pad}{}",
                t.fg(glyph_tone, &prefix),
                mark(&left_styled),
                t.fg("muted", &stats)
            )
        } else {
            format!(
                " {}{}",
                t.fg(glyph_tone, &prefix),
                mark(&truncate_to_width(&left_styled, budget, "...", false))
            )
        };
        let mut out = vec![line];
        out.extend(self.failure_lines(width));
        out
    }

    /// Each failed call's reason, indented under the chain line.
    fn failure_lines(&self, width: usize) -> Vec<String> {
        let t = theme();
        let mut lines = Vec::new();
        for block in &self.blocks {
            let block = block.borrow();
            let entry = block.chain_entry();
            if !entry.is_error {
                continue;
            }
            let header = format!(
                "   {} {}  {}",
                t.fg("error", "✗"),
                t.fg("toolTitle", &entry.tool),
                t.fg("toolOutput", &entry.subject)
            );
            lines.push(truncate_to_width(&header, width, "...", false));
            for raw in block.error_text().split('\n') {
                if crate::is_blank(raw) {
                    continue;
                }
                lines.push(truncate_to_width(
                    &format!("     {}", t.fg("toolOutput", raw)),
                    width,
                    "...",
                    false,
                ));
            }
        }
        lines
    }
}

impl Component for ToolChainComponent {
    fn render(&mut self, width: u16) -> Vec<String> {
        let rows = self.rows(width);
        if rows.is_empty() || !self.needs_lead_in() {
            return rows;
        }
        let mut out = Vec::with_capacity(rows.len() + 1);
        out.push(String::new());
        out.extend(rows);
        out
    }

    fn invalidate(&mut self) {
        for block in &self.blocks {
            block.borrow_mut().invalidate();
        }
    }
}
