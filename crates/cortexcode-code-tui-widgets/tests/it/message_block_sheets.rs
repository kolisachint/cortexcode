//! Port of the pin's `test/message-block-sheets.test.ts`: every message block
//! is a sheet under a cut-out theme and a plain full-width band otherwise, and
//! no component reaches for a block fill without `apply_block_fill`.
//!
//! The TS list includes `SkillInvocationMessageComponent`; skill invocation
//! blocks come with the skills extension surface (phase 12), so it is not
//! here yet.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use crate::support::lock;
use cortexcode_agent_types::{BranchSummaryMessage, CompactionSummaryMessage, CustomMessage};
use cortexcode_ai_types::UserContent;
use cortexcode_code_tui_theme::{
    apply_block_fill, get_markdown_theme, init_theme, BlockFill, PAPER_INSET,
};
use cortexcode_code_tui_widgets::custom_message::{
    BranchSummaryMessageComponent, CompactionSummaryMessageComponent, CustomMessageComponent,
};
use cortexcode_code_tui_widgets::UserMessageComponent;
use cortexcode_tui_components::{BoxComponent, MarkdownTheme, Text};
use cortexcode_tui_render::Component;
use cortexcode_tui_util::{strip_vt_control_characters, visible_width};

const WIDTH: u16 = 46;

fn md() -> Rc<dyn Fn() -> MarkdownTheme> {
    Rc::new(get_markdown_theme)
}

/// Every component that paints a message block, built the way the mode builds it.
fn blocks() -> Vec<(&'static str, Box<dyn Component>)> {
    let mut warning = BoxComponent::new(1, 1, None);
    // The shape InteractiveMode::show_block builds for errors and warnings.
    apply_block_fill(&mut warning, BlockFill::WarningBg);
    warning.add_child(Rc::new(RefCell::new(Text::new(
        "Wait for the current response to finish.",
        0,
        0,
    ))));
    vec![
        (
            "user message",
            Box::new(UserMessageComponent::new("hello there")),
        ),
        (
            "extension message",
            Box::new(CustomMessageComponent::new(
                CustomMessage {
                    custom_type: "skill".into(),
                    content: UserContent::Text("loaded".into()),
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
        ("warning frame", Box::new(warning)),
    ]
}

fn rendered(block: &mut Box<dyn Component>) -> Vec<String> {
    block
        .render(WIDTH)
        .into_iter()
        .filter(|line| visible_width(line) > 0)
        .collect()
}

#[test]
fn every_message_block_is_a_sheet_under_a_cut_out_theme() {
    let _g = lock();
    init_theme(Some("vox-cutout-dark"), false);
    let band = WIDTH as usize - PAPER_INSET;
    for (label, mut block) in blocks() {
        let lines = rendered(&mut block);
        // It holds back from the right margin, so it has an edge to cut.
        for line in &lines[..lines.len() - 1] {
            assert!(visible_width(line) <= band + 1, "{label}: {line:?}");
        }
        // Rows below the first carry the shadow's column; the last row is the
        // shadow's run, which stops where that column starts.
        assert!(
            lines[1..lines.len() - 1]
                .iter()
                .all(|line| strip_vt_control_characters(line).contains('▏')),
            "{label}: {lines:?}"
        );
        let run = strip_vt_control_characters(&lines[lines.len() - 1]);
        assert!(run.starts_with(' '), "{label}: {run:?}");
        assert!(run.ends_with('▔'), "{label}: {run:?}");
        assert_eq!(visible_width(&run), band, "{label}");
    }
    init_theme(Some("dark"), false);
}

#[test]
fn draws_the_plain_full_width_band_on_a_theme_with_no_paper() {
    let _g = lock();
    for (label, mut block) in blocks() {
        let lines = rendered(&mut block);
        assert!(
            lines
                .iter()
                .all(|line| visible_width(line) == WIDTH as usize),
            "{label}: {lines:?}"
        );
        assert!(
            !lines
                .iter()
                .any(|line| strip_vt_control_characters(line).contains(['▏', '▔', '█'])),
            "{label}"
        );
    }
}

/// A source scan, because the failure it guards against is a component that
/// was never wired up at all. The TS scan covers `components/*.ts` and
/// `interactive-mode.ts`; their Rust homes are this crate's top-level modules
/// and `cortexcode-code-tui-app/src` (tool renderers live in `src/tools/`, as
/// hoocode's live in `core/tools/`, outside the scan).
#[test]
fn no_component_reaches_for_a_block_fill_on_its_own() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let dirs = [
        crates.join("cortexcode-code-tui-widgets/src"),
        crates.join("cortexcode-code-tui-app/src"),
    ];
    let mut offenders = Vec::new();
    for dir in dirs {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_none_or(|e| e != "rs") {
                continue;
            }
            let source = std::fs::read_to_string(&path).unwrap();
            for fill in [
                "userMessageBg",
                "customMessageBg",
                "warningBg",
                "toolErrorBg",
            ] {
                if source.contains(&format!(".bg(\"{fill}\"")) {
                    offenders.push(format!("{}: .bg(\"{fill}\")", path.display()));
                }
            }
        }
    }
    assert_eq!(offenders, Vec::<String>::new());
}
