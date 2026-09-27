//! The footer (`components/footer.ts`): identity and location on line 1,
//! session vitals on line 2, then transient lines (extension statuses and
//! startup progress). Every line is at most `width` cells.

use cortexcode_code_agent_session::format::{format_tokens, js_to_fixed};
use cortexcode_code_task_store::{task_store, TaskSource, TaskStatus};
use cortexcode_code_tui_theme::theme;
use cortexcode_tui_render::Component;
use cortexcode_tui_util::{truncate_to_width, visible_width};

use crate::brand::{BRAND_MARK, GIT_BRANCH_GLYPH};
use crate::footer_data::FooterDataProvider;
use crate::progress_bar::{render_download_progress, render_progress_bar};
use crate::session_chip::session_chip_fits;
use crate::startup_progress::{self, StartupProgress};

/// The model as the footer shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct FooterModel {
    pub id: String,
    pub provider: String,
    pub context_window: u64,
    pub reasoning: bool,
}

/// What the footer reads from the session (an `AgentSession` in the app, a
/// stub in tests).
pub trait FooterSource {
    /// Cumulative (input, output, cache read, cache write, cost) over all
    /// session entries.
    fn usage_totals(&self) -> (u64, u64, u64, u64, f64);
    /// Context usage: (context window, percent — `None` when unknown after a
    /// compaction). `None` when there is no usage at all.
    fn context_usage(&self) -> Option<(u64, Option<f64>)>;
    fn model(&self) -> Option<FooterModel>;
    fn thinking_level(&self) -> String;
    fn cwd(&self) -> String;
    fn display_name(&self) -> String;
    /// Compaction reserve tokens.
    fn reserve_tokens(&self) -> u64;
    fn is_using_oauth(&self) -> bool;
}

/// The tool-output view dial's stops.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToolOutputView {
    Radar,
    #[default]
    Peek,
    Full,
}

impl ToolOutputView {
    pub fn as_str(self) -> &'static str {
        match self {
            ToolOutputView::Radar => "radar",
            ToolOutputView::Peek => "peek",
            ToolOutputView::Full => "full",
        }
    }

    /// The dial glyph, filling as the view widens.
    pub fn glyph(self) -> &'static str {
        match self {
            ToolOutputView::Radar => "◌",
            ToolOutputView::Peek => "◍",
            ToolOutputView::Full => "◉",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "radar" => Some(ToolOutputView::Radar),
            "peek" => Some(ToolOutputView::Peek),
            "full" => Some(ToolOutputView::Full),
            _ => None,
        }
    }
}

/// How many rows the footer takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FooterDensity {
    #[default]
    Full,
    Line,
}

/// `assembleLine`: `left` flush left, `right` flush right when it fits (≥2
/// columns between), padded to `width`; else `left` padded or truncated.
fn assemble_line(
    width: usize,
    left_plain: &str,
    left_styled: &str,
    right_plain: &str,
    right_styled: &str,
) -> String {
    let lw = visible_width(left_plain);
    if !right_plain.is_empty() && lw + 2 + visible_width(right_plain) <= width {
        return format!(
            "{left_styled}{}{right_styled}",
            " ".repeat(width - lw - visible_width(right_plain))
        );
    }
    if lw <= width {
        return format!("{left_styled}{}", " ".repeat(width - lw));
    }
    truncate_to_width(left_styled, width, &theme().fg("dim", "…"), false)
}

fn js_round(x: f64) -> f64 {
    (x + 0.5).floor()
}

/// A compact context-fill gauge, coloured by proximity to the compaction
/// trip point.
fn context_gauge(percent: f64, error_level: f64, warn_level: f64) -> (String, String) {
    const CELLS: usize = 8;
    let filled = (js_round(percent / 100.0 * CELLS as f64)).clamp(0.0, CELLS as f64) as usize;
    let fill = "▰".repeat(filled);
    let track = "▱".repeat(CELLS - filled);
    let color = if percent >= error_level {
        "error"
    } else if percent >= warn_level {
        "warning"
    } else {
        "accent"
    };
    let t = theme();
    (
        format!("{fill}{track}"),
        t.fg(color, &fill) + &t.fg("halftone", &track),
    )
}

fn active_subagent_count() -> usize {
    task_store()
        .list()
        .iter()
        .filter(|t| t.source == Some(TaskSource::Subagent) && t.status == TaskStatus::InProgress)
        .count()
}

/// Newlines, tabs and carriage returns become spaces; runs of spaces collapse.
fn sanitize_status_text(text: &str) -> String {
    let replaced: String = text
        .chars()
        .map(|c| {
            if matches!(c, '\r' | '\n' | '\t') {
                ' '
            } else {
                c
            }
        })
        .collect();
    let mut out = String::new();
    for c in replaced.chars() {
        if c == ' ' && out.ends_with(' ') {
            continue;
        }
        out.push(c);
    }
    out.trim().to_string()
}

/// One footer line for a transient progress entry.
fn render_startup_line(entry: &StartupProgress) -> String {
    let t = theme();
    match entry {
        StartupProgress::Error { label, message, .. } => {
            t.fg("dim", &format!("{label}: {message}"))
        }
        StartupProgress::Download {
            label,
            received_bytes,
            total_bytes,
            ..
        } => format!(
            "{} {}",
            t.fg("text", label),
            render_download_progress(*received_bytes, *total_bytes, None)
        ),
        StartupProgress::Work {
            label,
            done,
            total,
            unit,
            ..
        } => {
            let ratio = if *total > 0 {
                *done as f64 / *total as f64
            } else {
                0.0
            };
            format!(
                "{} {}",
                t.fg("text", label),
                render_progress_bar(ratio, &format!("{done}/{total} {unit}"), None)
            )
        }
    }
}

/// `$HOME` (or `USERPROFILE`) replaced by `~`.
fn tilde(path: &str) -> String {
    let home = std::env::var("HOME")
        .ok()
        .filter(|h| !h.is_empty())
        .or_else(|| std::env::var("USERPROFILE").ok().filter(|h| !h.is_empty()));
    match home {
        Some(home) if path.starts_with(&home) => format!("~{}", &path[home.len()..]),
        _ => path.to_string(),
    }
}

pub struct FooterComponent {
    source: Box<dyn FooterSource>,
    data: FooterDataProvider,
    auto_compact_enabled: bool,
    tool_output_view: ToolOutputView,
    session_chip_shown: bool,
    density: FooterDensity,
}

impl FooterComponent {
    pub fn new(source: Box<dyn FooterSource>, data: FooterDataProvider) -> Self {
        Self {
            source,
            data,
            auto_compact_enabled: true,
            tool_output_view: ToolOutputView::default(),
            session_chip_shown: false,
            density: FooterDensity::Full,
        }
    }

    pub fn set_source(&mut self, source: Box<dyn FooterSource>) {
        self.source = source;
    }

    pub fn set_auto_compact_enabled(&mut self, enabled: bool) {
        self.auto_compact_enabled = enabled;
    }

    /// The view dial's current stop.
    pub fn set_tool_output_view(&mut self, view: ToolOutputView) {
        self.tool_output_view = view;
    }

    /// `line` keeps the mark, mode, gauge and model on one row.
    pub fn set_density(&mut self, density: FooterDensity) {
        self.density = density;
    }

    /// Whether the input box carries the session chip (line 1 then drops the
    /// name, when the chip fits).
    pub fn set_session_chip_shown(&mut self, shown: bool) {
        self.session_chip_shown = shown;
    }

    fn transient_lines(&self, width: usize) -> Vec<String> {
        let mut lines = Vec::new();
        let statuses = self.data.get_extension_statuses();
        if !statuses.is_empty() {
            let line = statuses
                .values()
                .map(|text| sanitize_status_text(text))
                .collect::<Vec<_>>()
                .join(" ");
            lines.push(truncate_to_width(
                &line,
                width,
                &theme().fg("dim", "..."),
                false,
            ));
        }
        for entry in startup_progress::list() {
            lines.push(truncate_to_width(
                &render_startup_line(&entry),
                width,
                &theme().fg("dim", "…"),
                false,
            ));
        }
        lines
    }

    fn render_lines(&self, width: usize) -> Vec<String> {
        let t = theme();
        let source = &self.source;
        let (total_input, total_output, total_cache_read, total_cache_write, total_cost) =
            source.usage_totals();
        let model = source.model();

        let context_usage = source.context_usage();
        let context_window = context_usage
            .map(|(w, _)| w)
            .or_else(|| model.as_ref().map(|m| m.context_window))
            .unwrap_or(0);
        let context_percent_value = context_usage.and_then(|(_, p)| p).unwrap_or(0.0);
        let context_percent = match context_usage {
            Some((_, None)) => "?".to_string(),
            _ => js_to_fixed(context_percent_value, 1),
        };

        let pwd = tilde(&source.cwd());
        let branch = self.data.get_git_branch();
        let session_name = if self.session_chip_shown && session_chip_fits(width) {
            None
        } else {
            Some(source.display_name()).filter(|n| !n.is_empty())
        };
        let mode_label = self.data.get_active_mode();

        // ── Line 1 — identity & location
        let mode_up = mode_label.to_uppercase();
        let chip = t.has_bg("brandBg") && t.has("brandText");
        let brand = if chip {
            format!(" {BRAND_MARK} {mode_up} ")
        } else {
            format!("{BRAND_MARK} {mode_up}")
        };
        let brand_styled = if chip {
            t.bg("brandBg", &t.bold(&t.fg("brandText", &brand)))
        } else {
            t.bold(&t.fg("accent", &brand))
        };
        let mut l1_plain = format!("{brand}  {pwd}");
        let mut l1_styled = format!("{brand_styled}  {}", t.fg("muted", &pwd));
        if let Some(branch) = &branch {
            l1_plain += &format!(" {GIT_BRANCH_GLYPH} {branch}");
            l1_styled += &format!(
                " {} {}",
                t.fg("dim", GIT_BRANCH_GLYPH),
                t.fg("muted", branch)
            );
        }
        if let Some(name) = &session_name {
            l1_plain += &format!(" • {name}");
            l1_styled += &t.fg("dim", &format!(" • {name}"));
        }
        let n_sub = active_subagent_count();
        let mut right: Vec<(String, String)> = Vec::new();
        if n_sub > 0 {
            right.push((
                format!("◇{n_sub} running"),
                t.fg("accent", &format!("◇{n_sub}")) + &t.fg("dim", " running"),
            ));
        }
        let view = self.tool_output_view;
        let view_text = format!("{} {}", view.glyph(), view.as_str());
        right.push((view_text.clone(), t.fg("dim", &view_text)));
        let l1_right_plain = right
            .iter()
            .map(|(p, _)| p.as_str())
            .collect::<Vec<_>>()
            .join("  ");
        let l1_right_styled = right
            .iter()
            .map(|(_, s)| s.as_str())
            .collect::<Vec<_>>()
            .join("  ");
        let line1 = assemble_line(
            width,
            &l1_plain,
            &l1_styled,
            &l1_right_plain,
            &l1_right_styled,
        );

        // ── Line 2 — session vitals
        let mut threshold_percent: Option<f64> = None;
        if self.auto_compact_enabled && context_window > 0 {
            let effective = context_window as f64 - source.reserve_tokens() as f64;
            if effective > 0.0 {
                threshold_percent = Some(effective / context_window as f64 * 100.0);
            }
        }
        let error_level = threshold_percent.map_or(90.0, |p| p - 3.0);
        let warn_level = threshold_percent.map_or(70.0, |p| p - 10.0);
        let auto_indicator = match threshold_percent {
            Some(p) => format!(" auto@{}%", js_to_fixed(p, 0)),
            None if self.auto_compact_enabled => " auto".to_string(),
            None => String::new(),
        };

        let (gauge_plain, gauge_styled) =
            context_gauge(context_percent_value, error_level, warn_level);
        let pct_text = if context_percent == "?" {
            "?".to_string()
        } else {
            format!("{context_percent}%")
        };
        let pct_color = if context_percent_value >= error_level {
            "error"
        } else if context_percent_value >= warn_level {
            "warning"
        } else {
            "muted"
        };
        let win_text = format!("{}{auto_indicator}", format_tokens(context_window));

        let mut segs: Vec<(String, String)> = vec![(
            format!("{gauge_plain} {pct_text} {win_text}"),
            format!(
                "{gauge_styled} {} {}",
                t.fg(pct_color, &pct_text),
                t.fg("dim", &win_text)
            ),
        )];
        let arrow = |a: &str, n: u64| {
            (
                format!("{a}{}", format_tokens(n)),
                t.fg("dim", a) + &t.fg("muted", &format_tokens(n)),
            )
        };
        if total_input > 0 {
            segs.push(arrow("↑", total_input));
        }
        if total_output > 0 {
            segs.push(arrow("↓", total_output));
        }
        if total_cache_read > 0 {
            segs.push(arrow("R", total_cache_read));
        }
        if total_cache_write > 0 {
            segs.push(arrow("W", total_cache_write));
        }
        let using_subscription = model.is_some() && source.is_using_oauth();
        if total_cost != 0.0 || using_subscription {
            let cost = format!(
                "${}{}",
                js_to_fixed(total_cost, 3),
                if using_subscription { " (sub)" } else { "" }
            );
            segs.push((cost.clone(), t.fg("muted", &cost)));
        }
        let l2_plain = segs
            .iter()
            .map(|(p, _)| p.as_str())
            .collect::<Vec<_>>()
            .join("  ");
        let l2_styled = segs
            .iter()
            .map(|(_, s)| s.as_str())
            .collect::<Vec<_>>()
            .join("  ");

        let model_name = model
            .as_ref()
            .map(|m| m.id.clone())
            .filter(|id| !id.is_empty())
            .unwrap_or_else(|| "no-model".into());
        let mut r2_plain = model_name.clone();
        let mut r2_styled = t.fg("muted", &model_name);
        if model.as_ref().is_some_and(|m| m.reasoning) {
            let level = source.thinking_level();
            let level = if level.is_empty() {
                "off".to_string()
            } else {
                level
            };
            let text = if level == "off" {
                "thinking off".to_string()
            } else {
                level
            };
            r2_plain += &format!(" • {text}");
            r2_styled += &t.fg("dim", &format!(" • {text}"));
        }
        if self.data.get_available_provider_count() > 1 {
            if let Some(model) = &model {
                let with_provider = format!("({}) {r2_plain}", model.provider);
                if visible_width(&l2_plain) + 2 + visible_width(&with_provider) <= width {
                    r2_plain = with_provider;
                    r2_styled = t.fg("dim", &format!("({}) ", model.provider)) + &r2_styled;
                }
            }
        }
        let line2 = assemble_line(width, &l2_plain, &l2_styled, &r2_plain, &r2_styled);

        if self.density == FooterDensity::Line {
            let compact_plain = format!("{brand}  {}", segs[0].0);
            let compact_styled = format!("{brand_styled}  {}", segs[0].1);
            let mut lines = vec![assemble_line(
                width,
                &compact_plain,
                &compact_styled,
                &r2_plain,
                &r2_styled,
            )];
            lines.extend(self.transient_lines(width));
            return lines;
        }
        let mut lines = vec![line1, line2];
        lines.extend(self.transient_lines(width));
        lines
    }
}

impl Component for FooterComponent {
    fn render(&mut self, width: u16) -> Vec<String> {
        self.render_lines(width as usize)
    }
}
