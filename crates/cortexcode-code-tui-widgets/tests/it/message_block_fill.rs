//! Port of the pin's `test/message-block-fill.test.ts`: the fill under a
//! message block, checked cell by cell (grapheme clusters, so an emoji is one
//! wide cell), for every block, content shape and width.

use std::cell::RefCell;
use std::rc::Rc;

use crate::support::lock;
use cortexcode_agent_types::CustomMessage;
use cortexcode_ai_types::UserContent;
use cortexcode_code_tui_theme::{
    apply_block_fill, get_markdown_theme, init_theme, BlockFill, PAPER_INSET,
};
use cortexcode_code_tui_widgets::custom_message::CustomMessageComponent;
use cortexcode_code_tui_widgets::UserMessageComponent;
use cortexcode_tui_components::{BoxComponent, MarkdownTheme, Text};
use cortexcode_tui_render::Component;
use cortexcode_tui_util::visible_width;
use unicode_segmentation::UnicodeSegmentation;

#[derive(Debug, Clone)]
struct Cell {
    bg: String,
    glyph: String,
}

/// Which background is in force at each terminal cell.
fn scan(line: &str) -> Vec<Cell> {
    let mut bg = String::new();
    let mut out = Vec::new();
    let mut rest = line;
    while !rest.is_empty() {
        if rest.starts_with('\x1b') {
            if let Some(len) = sgr(rest, &mut bg) {
                rest = &rest[len..];
                continue;
            }
            if let Some(len) = osc_len(rest).or_else(|| csi_len(rest)) {
                rest = &rest[len..];
                continue;
            }
            rest = &rest[1..];
            continue;
        }
        let chunk = &rest[..rest.find('\x1b').unwrap_or(rest.len())];
        let ch = chunk.graphemes(true).next().unwrap();
        let w = visible_width(ch).max(1);
        for c in 0..w {
            out.push(Cell {
                bg: bg.clone(),
                glyph: if c == 0 {
                    ch.to_string()
                } else {
                    String::new()
                },
            });
        }
        rest = &rest[ch.len()..];
    }
    out
}

/// `\x1b[<params>m`: updates `bg`, returns the sequence length.
fn sgr(s: &str, bg: &mut String) -> Option<usize> {
    let body = s.strip_prefix("\x1b[")?;
    let end = body.find(|c: char| !(c.is_ascii_digit() || c == ';'))?;
    if body.as_bytes()[end] != b'm' {
        return None;
    }
    let raw = if body[..end].is_empty() {
        "0"
    } else {
        &body[..end]
    };
    let params: Vec<&str> = raw.split(';').collect();
    let mut k = 0;
    while k < params.len() {
        let code: u32 = params[k].parse().unwrap_or(0);
        if code == 38 || code == 48 {
            let mode: u32 = params.get(k + 1).and_then(|p| p.parse().ok()).unwrap_or(0);
            let span = match mode {
                2 => 5,
                5 => 3,
                _ => 1,
            };
            if code == 48 {
                *bg = params[k..(k + span).min(params.len())].join(";");
            }
            k += span;
            continue;
        }
        if code == 0 || code == 49 {
            bg.clear();
        } else if (40..=47).contains(&code) || (100..=107).contains(&code) {
            *bg = code.to_string();
        }
        k += 1;
    }
    Some(2 + end + 1)
}

fn osc_len(s: &str) -> Option<usize> {
    let body = s.strip_prefix("\x1b]")?;
    let end = body.find(['\x07', '\x1b'])?;
    if body.as_bytes()[end] == 0x07 {
        return Some(2 + end + 1);
    }
    body[end..].starts_with("\x1b\\").then_some(2 + end + 2)
}

fn csi_len(s: &str) -> Option<usize> {
    let body = s.strip_prefix("\x1b[")?;
    let end = body.find(|c: char| !(c.is_ascii_digit() || c == ';' || c == '?'))?;
    body.as_bytes()[end]
        .is_ascii_alphabetic()
        .then_some(2 + end + 1)
}

fn is_shadow(glyph: &str) -> bool {
    glyph == "▏" || glyph == "▔"
}

fn faults_in(label: &str, lines: &[String], width: usize) -> Vec<String> {
    let mut faults = Vec::new();
    let body: Vec<Vec<Cell>> = lines
        .iter()
        .map(|l| scan(l))
        .filter(|c| !c.is_empty())
        .collect();
    let Some(shadow_run) = body.last() else {
        return faults;
    };
    // Too narrow to carry a gutter, the box draws the plain full-width band.
    let sheet = shadow_run.iter().any(|c| is_shadow(&c.glyph));
    let band = if sheet {
        width.saturating_sub(PAPER_INSET).max(1)
    } else {
        width
    };
    if !sheet {
        for (i, cells) in body.iter().enumerate() {
            if cells.len() > width {
                faults.push(format!(
                    "row {i}: {} cells overflows width {width}",
                    cells.len()
                ));
            }
            for (c, cell) in cells.iter().enumerate() {
                if cell.bg.is_empty() {
                    faults.push(format!("row {i}: hole in the plain band at cell {c}"));
                }
            }
        }
        return faults
            .into_iter()
            .map(|f| format!("{label} @{width}: {f}"))
            .collect();
    }
    for (i, cells) in body.iter().enumerate() {
        let last = i == body.len() - 1;
        if cells.len() > width {
            faults.push(format!(
                "row {i}: {} cells overflows terminal width {width}",
                cells.len()
            ));
        }
        if last {
            if cells.first().map(|c| c.glyph.as_str()) != Some(" ") {
                faults.push("bottom run: does not start on page".into());
            }
            if cells.len() != band {
                faults.push(format!(
                    "bottom run: {} cells, expected {band}",
                    cells.len()
                ));
            }
            if cells.last().map(|c| c.glyph.as_str()) != Some("▔") {
                faults.push(format!(
                    "bottom run: ends on {:?}, expected ▔",
                    cells.last().map(|c| &c.glyph)
                ));
            }
            continue;
        }
        let mut fill_end = 0;
        while fill_end < cells.len()
            && !cells[fill_end].bg.is_empty()
            && !is_shadow(&cells[fill_end].glyph)
        {
            fill_end += 1;
        }
        if fill_end != band {
            faults.push(format!("row {i}: fill ends at {fill_end}, expected {band}"));
        }
        let rest: String = cells[fill_end..].iter().map(|c| c.glyph.as_str()).collect();
        if i == 0 {
            if !rest.is_empty() {
                faults.push(format!("row 0: casts a shadow ({rest:?}) but should not"));
            }
        } else if rest != "▏" {
            faults.push(format!(
                "row {i}: shadow segment is {rest:?}, expected \"▏\""
            ));
        }
    }
    faults
        .into_iter()
        .map(|f| format!("{label} @{width}: {f}"))
        .collect()
}

const CONTENT: &[(&str, &str)] = &[
    ("plain", "hello there"),
    ("heading chip", "# Big Title\n\nbody"),
    ("inline code and bold", "run `npm test` now, **really**"),
    (
        "link",
        "see [docs](https://example.com/a/very/long/path) ok",
    ),
    ("code block", "```ts\nconst x = 1;\n```"),
    (
        "code block, long line",
        "```ts\nconst identifier = someVeryLongFunctionName(argumentOne, argumentTwo);\n```",
    ),
    ("cjk", "日本語のテキストがここにあります、もっと長くします"),
    (
        "emoji, including zwj sequences",
        "done ✅ 🎉 shipped 👍🏽 family 👩‍👩‍👧‍👦 end",
    ),
    ("table", "| a | b |\n|---|---|\n| 1 | 2 |"),
    (
        "table wider than the band",
        "| alpha | beta | gamma | delta |\n|---|---|---|---|\n| 1 | 2 | 3 | 4 |",
    ),
    (
        "blockquote",
        "> quoted line that wraps around here\n\nafter",
    ),
    ("horizontal rule", "above\n\n---\n\nbelow"),
    ("list", "- one\n- two\n- three"),
    (
        "nested list",
        "- one\n  - two\n    - three deeply nested item text",
    ),
    ("blank lines", "a\n\n\n\nb"),
    ("trailing whitespace", "text with trailing   \n\nmore"),
    (
        "a child that resets its own styling",
        "\x1b[31mred\x1b[0m then plain",
    ),
];

/// The generated rows of `CONTENT` (built at run time in the TS table).
fn content() -> Vec<(String, String)> {
    let mut all: Vec<(String, String)> = CONTENT
        .iter()
        .map(|(l, t)| (l.to_string(), t.to_string()))
        .collect();
    all.insert(6, ("unbreakable word".into(), "a".repeat(120)));
    all.insert(
        7,
        (
            "unbreakable word in code".into(),
            format!("```\n{}\n```", "z".repeat(120)),
        ),
    );
    all.insert(
        8,
        (
            "unbreakable url".into(),
            format!("https://example.com/{}", "segment/".repeat(20)),
        ),
    );
    all
}

const WIDTHS: &[u16] = &[120, 80, 60, 40, 34, 24, 20, 16, 12, 10, 8, 6, 5, 4, 3, 2];
const THEMES: &[&str] = &["vox-cutout-dark", "vox-cutout-light"];

fn md() -> Rc<dyn Fn() -> MarkdownTheme> {
    Rc::new(get_markdown_theme)
}

fn non_empty(lines: Vec<String>) -> Vec<String> {
    lines.into_iter().filter(|l| !scan(l).is_empty()).collect()
}

#[test]
fn a_sheet_runs_to_the_terminals_last_cell() {
    let _g = lock();
    assert_eq!(PAPER_INSET, 1);
    for theme_name in THEMES {
        init_theme(Some(theme_name), false);
        for width in [120u16, 100, 80, 60, 40] {
            let lines = non_empty(UserMessageComponent::new("a message").render(width));
            let w = width as usize;
            assert_eq!(
                scan(&lines[0]).len(),
                w - 1,
                "{theme_name} @{w}: the top row"
            );
            for (i, line) in lines[1..lines.len() - 1].iter().enumerate() {
                assert_eq!(
                    scan(line).len(),
                    w,
                    "{theme_name} @{w}: row {} stops short of the margin",
                    i + 1
                );
            }
            assert_eq!(
                scan(&lines[lines.len() - 1]).len(),
                w - 1,
                "{theme_name} @{w}: the bottom run"
            );
        }
    }
    init_theme(Some("dark"), false);
}

#[test]
fn a_sheet_holds_its_geometry_at_every_width() {
    let _g = lock();
    let mut faults = Vec::new();
    for theme_name in THEMES {
        init_theme(Some(theme_name), false);
        for (label, text) in content() {
            for &width in WIDTHS {
                let w = width as usize;
                let label = format!("{theme_name} {label}");
                faults.extend(faults_in(
                    &format!("user {label:?}"),
                    &UserMessageComponent::new(&text).render(width),
                    w,
                ));
                let mut custom = CustomMessageComponent::new(
                    CustomMessage {
                        custom_type: "skill".into(),
                        content: UserContent::Text(text.clone()),
                        display: true,
                        details: None,
                        timestamp: 0,
                    },
                    md(),
                );
                faults.extend(faults_in(
                    &format!("extension {label:?}"),
                    &custom.render(width),
                    w,
                ));
                let mut warning = BoxComponent::new(1, 1, None);
                apply_block_fill(&mut warning, BlockFill::WarningBg);
                for line in text.split('\n') {
                    warning.add_child(Rc::new(RefCell::new(Text::new(line, 0, 0))));
                }
                faults.extend(faults_in(
                    &format!("warning {label:?}"),
                    &warning.render(width),
                    w,
                ));
            }
        }
    }
    init_theme(Some("dark"), false);
    assert_eq!(faults, Vec::<String>::new());
}

/// The scanner itself: backgrounds, resets, OSC/CSI skipping, wide graphemes.
#[test]
fn the_scanner_tracks_backgrounds_cell_by_cell() {
    let cells = scan("\x1b]133;A\x07\x1b[48;2;1;2;3ma\x1b[1mb\x1b[49mc👩‍👩‍👧\x1b[0K");
    let bgs: Vec<&str> = cells.iter().map(|c| c.bg.as_str()).collect();
    assert_eq!(bgs, ["48;2;1;2;3", "48;2;1;2;3", "", "", ""]);
    assert_eq!(cells[3].glyph, "👩‍👩‍👧");
    assert_eq!(cells[4].glyph, "");
    // An unfilled row in a plain band is a hole.
    assert!(!faults_in("x", &["plain".to_string()], 5).is_empty());
}
