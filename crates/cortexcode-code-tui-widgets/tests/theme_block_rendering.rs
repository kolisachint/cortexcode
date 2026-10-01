//! Port of the pin's `test/theme-block-rendering.test.ts`: every shipped
//! theme loads and every message block renders under it, including a custom
//! theme with only the schema's required colors.
//!
//! The TS block list includes `SkillInvocationMessageComponent`; skill
//! invocation blocks come with the skills extension surface (phase 12).

mod support;

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use cortexcode_agent_types::{BranchSummaryMessage, CompactionSummaryMessage, CustomMessage};
use cortexcode_ai_types::UserContent;
use cortexcode_code_tui_theme::{
    apply_block_fill, get_available_themes, get_markdown_theme, get_paper_shadow_fn, init_theme,
    load_theme_from_path, set_theme_instance, theme, BlockFill, PAPER_INSET, THEME_SCHEMA_JSON,
};
use cortexcode_code_tui_widgets::custom_message::{
    BranchSummaryMessageComponent, CompactionSummaryMessageComponent, CustomMessageComponent,
};
use cortexcode_code_tui_widgets::UserMessageComponent;
use cortexcode_tui_components::{BoxComponent, MarkdownTheme, Text};
use cortexcode_tui_render::Component;
use cortexcode_tui_util::{strip_vt_control_characters, visible_width};
use support::lock;

const WIDTHS: &[u16] = &[80, 46, 24, 12];

/// The themes the crate ships, straight off disk.
fn built_ins() -> Vec<String> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../cortexcode-code-tui-theme/themes");
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| {
            let name = e.unwrap().file_name().to_string_lossy().into_owned();
            (name.ends_with(".json") && name != "theme-schema.json")
                .then(|| name.trim_end_matches(".json").to_string())
        })
        .collect();
    names.sort();
    names
}

fn md() -> Rc<dyn Fn() -> MarkdownTheme> {
    Rc::new(get_markdown_theme)
}

fn frame(fill: BlockFill, text: &str) -> BoxComponent {
    let mut block = BoxComponent::new(1, 1, None);
    apply_block_fill(&mut block, fill);
    block.add_child(Rc::new(RefCell::new(Text::new(text, 0, 0))));
    block
}

/// Every component that paints a message block, built the way the mode builds it.
fn blocks() -> Vec<(&'static str, Box<dyn Component>)> {
    vec![
        (
            "user message",
            Box::new(UserMessageComponent::new(
                "hello **there**, a longer line to wrap",
            )),
        ),
        (
            "extension message",
            Box::new(CustomMessageComponent::new(
                CustomMessage {
                    custom_type: "skill".into(),
                    content: UserContent::Text("loaded `bq-sql-authoring`".into()),
                    display: true,
                    details: None,
                    timestamp: 0,
                },
                md(),
            )),
        ),
        (
            "compaction summary",
            Box::new(CompactionSummaryMessageComponent::new(
                CompactionSummaryMessage {
                    summary: "s".into(),
                    tokens_before: 100,
                    tokens_after: Some(10),
                    timestamp: 0,
                },
                md(),
            )),
        ),
        (
            "branch summary",
            Box::new(BranchSummaryMessageComponent::new(
                BranchSummaryMessage {
                    summary: "s".into(),
                    ..Default::default()
                },
                md(),
            )),
        ),
        (
            "warning frame",
            Box::new(frame(
                BlockFill::WarningBg,
                "Wait for the current response to finish.",
            )),
        ),
        (
            "error frame",
            Box::new(frame(BlockFill::ToolErrorBg, "Something went wrong.")),
        ),
    ]
}

fn has_shadow_ink(line: &str) -> bool {
    strip_vt_control_characters(line).contains(['▏', '▔', '█'])
}

/// What is wrong with this block's rows, if anything.
fn faults(label: &str, lines: &[String], width: usize, sheet: bool) -> Vec<String> {
    let mut found = Vec::new();
    let rows: Vec<&String> = lines.iter().filter(|l| visible_width(l) > 0).collect();
    if rows.is_empty() {
        return vec![format!("{label}: rendered nothing")];
    }
    for (i, line) in rows.iter().enumerate() {
        if visible_width(line) > width {
            found.push(format!(
                "{label} row {i}: {} cells overflows {width}",
                visible_width(line)
            ));
        }
    }
    if !sheet {
        for (i, line) in rows.iter().enumerate() {
            if visible_width(line) != width {
                found.push(format!(
                    "{label} row {i}: {} cells, expected {width}",
                    visible_width(line)
                ));
            }
            if has_shadow_ink(line) {
                found.push(format!(
                    "{label} row {i}: shadow ink on a theme with no paper"
                ));
            }
        }
        return found;
    }
    let band = width - PAPER_INSET;
    let run = strip_vt_control_characters(rows[rows.len() - 1]);
    if !run.starts_with(' ') {
        found.push(format!("{label}: bottom run does not start on page"));
    }
    if !run.ends_with('▔') {
        found.push(format!(
            "{label}: bottom run ends on {:?}, expected ▔",
            run.chars().last()
        ));
    }
    if visible_width(&run) != band {
        found.push(format!(
            "{label}: bottom run is {} cells, expected {band}",
            visible_width(&run)
        ));
    }
    for (i, line) in rows[1..rows.len() - 1].iter().enumerate() {
        if !strip_vt_control_characters(line).contains('▏') {
            found.push(format!("{label} row {}: no shadow column", i + 1));
        }
    }
    found
}

#[test]
fn has_at_least_the_themes_this_suite_expects() {
    let built_ins = built_ins();
    assert!(built_ins.len() >= 8, "{built_ins:?}");
    assert!(built_ins.contains(&"dark".to_string()));
    assert!(built_ins.contains(&"vox-cutout-dark".to_string()));
    let available = get_available_themes();
    for name in &built_ins {
        assert!(available.contains(name), "{name}");
    }
}

#[test]
fn every_shipped_theme_loads_and_renders_every_message_block() {
    let _g = lock();
    let mut found = Vec::new();
    for name in built_ins() {
        init_theme(Some(&name), false);
        assert_eq!(theme().name.as_deref(), Some(name.as_str()));
        // Which shape this theme asks for, read the way the components read it.
        let sheet = get_paper_shadow_fn().is_some();
        for &width in WIDTHS {
            for (label, mut block) in blocks() {
                let lines = block.render(width);
                found.extend(faults(
                    &format!("{name}: {label} @{width}"),
                    &lines,
                    width as usize,
                    sheet,
                ));
            }
        }
    }
    init_theme(Some("dark"), false);
    assert_eq!(found, Vec::<String>::new());
}

#[test]
fn follows_a_theme_switch_under_blocks_already_on_screen_both_ways() {
    let _g = lock();
    let mut built = blocks();
    let mut found = Vec::new();
    for (theme_name, sheet, stage) in [
        ("dark", false, "built plain"),
        ("vox-cutout-dark", true, "switched to paper"),
        ("light", false, "switched back"),
    ] {
        init_theme(Some(theme_name), false);
        for (label, block) in built.iter_mut() {
            found.extend(faults(
                &format!("{label} {stage}"),
                &block.render(46),
                46,
                sheet,
            ));
        }
    }
    init_theme(Some("dark"), false);
    assert_eq!(found, Vec::<String>::new());
}

#[test]
fn gives_exactly_the_two_cut_out_themes_the_paper_treatment() {
    let _g = lock();
    let with_paper: Vec<String> = built_ins()
        .into_iter()
        .filter(|name| {
            init_theme(Some(name), false);
            get_paper_shadow_fn().is_some()
        })
        .collect();
    init_theme(Some("dark"), false);
    assert_eq!(with_paper, ["vox-cutout-dark", "vox-cutout-light"]);
}

#[test]
fn a_custom_theme_with_only_the_required_colors_renders_plain_bands() {
    let _g = lock();
    // Built from the schema's own required list: no `warningBg` (it falls back
    // to `customMessageBg`), no `paperShadow`, none of the cut-out pairs.
    let schema: serde_json::Value = serde_json::from_str(THEME_SCHEMA_JSON).unwrap();
    let required = schema["properties"]["colors"]["required"]
        .as_array()
        .unwrap();
    let colors: serde_json::Map<String, serde_json::Value> = required
        .iter()
        .map(|t| (t.as_str().unwrap().to_string(), "#808080".into()))
        .collect();
    assert!(!colors.contains_key("warningBg"));
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("minimal.json");
    std::fs::write(
        &path,
        serde_json::json!({ "name": "minimal", "colors": colors }).to_string(),
    )
    .unwrap();

    set_theme_instance(load_theme_from_path(&path, None).unwrap());
    assert!(get_paper_shadow_fn().is_none());
    let mut found = Vec::new();
    for &width in WIDTHS {
        for (label, mut block) in blocks() {
            found.extend(faults(
                &format!("{label} @{width}"),
                &block.render(width),
                width as usize,
                false,
            ));
        }
    }
    init_theme(Some("dark"), false);
    assert_eq!(found, Vec::<String>::new());
}
