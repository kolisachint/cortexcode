//! Port of `test/picker-widths.test.ts` for the pickers in this crate. The
//! extension picker's case lives with it in code-tui-app's tests; the session
//! tree's goes with the tree selector (11.3e).

mod support;

use std::path::PathBuf;
use std::rc::Rc;

use chrono::{DateTime, Utc};
use cortexcode_code_session::SessionInfo;
use cortexcode_code_tools_optin::ask_options::{AskOption, AskQuestion};
use cortexcode_code_tui_keybindings::AppKeybindingsManager;
use cortexcode_code_tui_selectors::ask_options::{AskOptionsComponent, AskOptionsOptions};
use cortexcode_code_tui_selectors::session_selector::{
    LoadSink, SessionSelectorComponent, SessionSelectorOptions,
};
use cortexcode_code_tui_selectors::user_message_selector::{
    UserMessageItem, UserMessageSelectorComponent,
};
use cortexcode_code_tui_theme::{select_gutter, theme, SELECT_CURSOR};
use cortexcode_tui_render::Component;
use cortexcode_tui_util::visible_width;
use support::{agent_dir, lock};

/// Long enough that a picker which forgot to truncate overruns the width.
const LONG: &str = "lorem ipsum dolor sit amet consectetur adipiscing elit sed do eiusmod tempor incididunt ut labore";

/// Cramped, ordinary and wide.
const WIDTHS: [u16; 3] = [40, 80, 160];

fn overruns(lines: &[String], width: u16) -> Vec<String> {
    lines
        .iter()
        .filter(|l| visible_width(l) > width as usize)
        .map(|l| format!("{} > {width}: {l}", visible_width(l)))
        .collect()
}

fn banded_rows(lines: &[String]) -> Vec<String> {
    let bg = theme().get_bg_ansi("selectedBg").to_string();
    lines.iter().filter(|l| l.contains(&bg)).cloned().collect()
}

fn assert_fits_and_fills(lines: &[String], width: u16) {
    assert_eq!(overruns(lines, width), Vec::<String>::new());
    for row in banded_rows(lines) {
        assert_eq!(visible_width(&row), width as usize, "{row}");
    }
}

fn make_session(id: &str, first_message: &str) -> SessionInfo {
    SessionInfo {
        path: PathBuf::from(format!("/tmp/{id}.jsonl")),
        id: id.into(),
        cwd: "/some/quite/deeply/nested/working/directory/for/the/project".into(),
        name: None,
        color: None,
        branch: None,
        parent_session_path: None,
        created: DateTime::<Utc>::UNIX_EPOCH,
        modified: DateTime::<Utc>::UNIX_EPOCH,
        message_count: 1,
        first_message: first_message.into(),
        all_messages_text: first_message.into(),
    }
}

#[test]
fn cursor_and_gutter_are_the_same_width() {
    assert_eq!(
        visible_width(&select_gutter()),
        visible_width(SELECT_CURSOR)
    );
}

#[test]
fn session_picker_fits_and_fills_its_selected_row() {
    let _g = lock();
    for width in WIDTHS {
        let sessions = vec![make_session("a", LONG), make_session("b", "short")];
        let mut selector = SessionSelectorComponent::new(
            Box::new(move |sink: LoadSink| sink.finish(Ok(sessions.clone()))),
            Box::new(|sink: LoadSink| sink.finish(Ok(Vec::new()))),
            Box::new(|_| {}),
            Box::new(|| {}),
            SessionSelectorOptions {
                show_rename_hint: Some(true),
                keybindings: Some(Rc::new(
                    AppKeybindingsManager::create(Some(agent_dir())).into_manager(),
                )),
                ..Default::default()
            },
            None,
        );
        selector.poll();
        assert_fits_and_fills(&selector.render(width), width);
    }
}

#[test]
fn fork_from_message_picker_fits() {
    let _g = lock();
    for width in WIDTHS {
        let mut selector = UserMessageSelectorComponent::new(
            vec![
                UserMessageItem {
                    id: "1".into(),
                    text: LONG.into(),
                    timestamp: None,
                },
                UserMessageItem {
                    id: "2".into(),
                    text: "short".into(),
                    timestamp: None,
                },
            ],
            None,
        );
        assert_fits_and_fills(&selector.render(width), width);
    }
}

fn plain(label: &str) -> AskOption {
    AskOption {
        label: label.into(),
        description: None,
        recommended: false,
    }
}

fn ask(questions: Vec<AskQuestion>) -> AskOptionsComponent {
    AskOptionsComponent::new(
        questions,
        Box::new(|_| {}),
        Box::new(|| {}),
        AskOptionsOptions::default(),
    )
}

#[test]
fn ask_for_input_pane_fits() {
    let _g = lock();
    for width in WIDTHS {
        let mut c = ask(vec![AskQuestion {
            question: LONG.into(),
            short: None,
            detail: None,
            options: vec![
                AskOption {
                    label: LONG.into(),
                    description: Some(LONG.into()),
                    recommended: false,
                },
                plain("short"),
            ],
            allow_custom: true,
        }]);
        assert_eq!(overruns(&c.render(width), width), Vec::<String>::new());
    }
}

#[test]
fn ask_for_input_keeps_numbering_aligned() {
    let _g = lock();
    let mut c = ask(vec![AskQuestion {
        question: "q".into(),
        short: None,
        detail: None,
        options: vec![plain("first"), plain("second")],
        allow_custom: false,
    }]);
    let lines = c.render(80);
    let first = lines.iter().find(|l| l.contains("first")).unwrap();
    let second = lines.iter().find(|l| l.contains("second")).unwrap();
    assert_eq!(
        visible_width(&first[..first.find("first").unwrap()]),
        visible_width(&second[..second.find("second").unwrap()])
    );
}
