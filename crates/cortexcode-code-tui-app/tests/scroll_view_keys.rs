//! Port of the pin's `test/scroll-view-keys.test.ts`: which keys the pinned
//! transcript view answers to, which are live at the prompt, and that the pin
//! lets go of everything else rather than swallowing it. The prompt is the
//! plain editor here (hoocode's test uses `CustomEditor`; its scroll keys are
//! the scroll view's own, installed on the TUI).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use cortexcode_code_tui_app::scroll_view::{
    install_scroll_view, jump_to_user_message, TurnDirection,
};
use cortexcode_code_tui_keybindings::AppKeybindingsManager;
use cortexcode_code_tui_theme::{get_editor_theme, init_theme};
use cortexcode_tui_components::{Editor, EditorHost, EditorOptions};
use cortexcode_tui_render::{Component, ComponentHandle, Container, Tui, TuiEvent};
use cortexcode_tui_terminal::Terminal;

const WIDTH: u16 = 60;
const HEIGHT: u16 = 12;
/// The view is one row shorter than the screen; the last row is the indicator.
const VIEW: i64 = HEIGHT as i64 - 1;

struct SilentTerminal {
    alternate: Arc<AtomicBool>,
}

impl Terminal for SilentTerminal {
    fn start(&mut self, _: Box<dyn FnMut(&str) + Send>, _: Box<dyn FnMut() + Send>) {}
    fn stop(&mut self) {}
    fn drain_input(&mut self, _: Duration, _: Duration) {}
    fn write(&mut self, _: &str) {}
    fn columns(&self) -> u16 {
        WIDTH
    }
    fn rows(&self) -> u16 {
        HEIGHT
    }
    fn kitty_protocol_active(&self) -> bool {
        false
    }
    fn move_by(&mut self, _: i32) {}
    fn hide_cursor(&mut self) {}
    fn show_cursor(&mut self) {}
    fn clear_line(&mut self) {}
    fn clear_from_cursor(&mut self) {}
    fn clear_screen(&mut self) {}
    fn set_title(&mut self, _: &str) {}
    fn set_progress(&mut self, _: bool) {}
    fn mouse_reporting(&self) -> bool {
        true
    }
    fn set_alternate_screen(&mut self, active: bool) {
        self.alternate.store(active, Ordering::SeqCst);
    }
}

/// Rows of text, `label:i` (or `line i+1` for the plain transcript).
struct Block {
    rows: Vec<String>,
    user: bool,
}

impl Component for Block {
    fn render(&mut self, _width: u16) -> Vec<String> {
        self.rows.clone()
    }
    fn invalidate(&mut self) {}
    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
}

fn block(label: &str, rows: usize, user: bool) -> Rc<RefCell<Block>> {
    Rc::new(RefCell::new(Block {
        rows: (0..rows).map(|i| format!("{label}:{i}")).collect(),
        user,
    }))
}

fn is_user(child: &ComponentHandle) -> bool {
    child
        .borrow()
        .as_any()
        .and_then(|a| a.downcast_ref::<Block>())
        .is_some_and(|b| b.user)
}

fn new_tui() -> (Tui, Arc<AtomicBool>) {
    init_theme(Some("dark"), false);
    AppKeybindingsManager::default().install();
    let alternate = Arc::new(AtomicBool::new(false));
    let tui = Tui::new(
        Box::new(SilentTerminal {
            alternate: alternate.clone(),
        }),
        None,
    );
    (tui, alternate)
}

struct Harness {
    ui: Tui,
    alternate: Arc<AtomicBool>,
    editor: Rc<RefCell<Editor>>,
}

impl Harness {
    fn send(&mut self, data: &str) {
        self.ui.process_event(TuiEvent::Input(data.to_string()));
    }

    fn text(&self) -> String {
        self.editor.borrow().get_text()
    }

    fn top(&self) -> i64 {
        self.ui.get_scroll_position().unwrap().0
    }
}

fn setup() -> Harness {
    let (mut ui, alternate) = new_tui();
    let transcript = Rc::new(RefCell::new(Block {
        rows: (1..=120).map(|i| format!("line {i}")).collect(),
        user: false,
    }));
    let editor = Rc::new(RefCell::new(Editor::new(
        EditorHost::default(),
        get_editor_theme(),
        EditorOptions::default(),
    )));
    ui.add_child(transcript);
    ui.add_child(editor.clone());
    install_scroll_view(
        &mut ui,
        editor.clone(),
        Rc::new(RefCell::new(Container::new())),
        Rc::new(|_: &ComponentHandle| false),
    );
    ui.set_focus(Some(editor.clone()));
    // One frame, so there is a line buffer to scroll through.
    let _events = ui.start();
    Harness {
        ui,
        alternate,
        editor,
    }
}

const PAGE_UP: &str = "\x1b[5~";
const PAGE_DOWN: &str = "\x1b[6~";
const UP: &str = "\x1b[A";
const DOWN: &str = "\x1b[B";
const ESCAPE: &str = "\x1b";
const WHEEL_UP: &str = "\x1b[<64;1;1M";

// --- at the prompt ----------------------------------------------------------

#[test]
fn pins_the_view_on_page_up() {
    let mut h = setup();
    h.send(PAGE_UP);
    assert!(h.ui.scroll_pinned());
}

#[test]
fn leaves_the_arrows_to_prompt_history() {
    let mut h = setup();
    h.editor.borrow_mut().add_to_history("an earlier message");
    h.send(UP);
    assert!(!h.ui.scroll_pinned());
    assert_eq!(h.text(), "an earlier message");
}

#[test]
fn does_nothing_on_page_down_having_nowhere_below_to_go() {
    let mut h = setup();
    h.send(PAGE_DOWN);
    assert!(!h.ui.scroll_pinned());
}

#[test]
fn will_not_pin_while_another_surface_holds_focus() {
    let mut h = setup();
    let other = block("picker", 1, false);
    h.ui.set_focus(Some(other));
    h.send(WHEEL_UP);
    assert!(!h.ui.scroll_pinned());
    // Nor on the prompt's page key: it belongs to the prompt.
    h.send(PAGE_UP);
    assert!(!h.ui.scroll_pinned());
}

// --- once pinned ------------------------------------------------------------

fn pinned() -> Harness {
    let mut h = setup();
    h.send(PAGE_UP);
    h
}

#[test]
fn moves_a_line_at_a_time_on_the_arrows() {
    let mut h = pinned();
    let before = h.top();
    h.send(UP);
    assert_eq!(h.top(), before - 1);
    h.send(DOWN);
    assert_eq!(h.top(), before);
}

#[test]
fn does_not_type_the_arrows_into_the_prompt() {
    let mut h = pinned();
    h.send(UP);
    h.send(DOWN);
    assert_eq!(h.text(), "");
}

#[test]
fn pages_further_back() {
    let mut h = pinned();
    let before = h.top();
    h.send(PAGE_UP);
    assert_eq!(h.top(), before - (VIEW - 2));
}

#[test]
fn goes_back_to_live_on_escape() {
    let mut h = pinned();
    h.send(ESCAPE);
    assert!(!h.ui.scroll_pinned());
}

#[test]
fn goes_back_to_live_on_paging_past_the_bottom() {
    let mut h = pinned();
    for _ in 0..20 {
        h.send(PAGE_DOWN);
    }
    assert!(!h.ui.scroll_pinned());
}

#[test]
fn returns_to_live_when_you_start_typing_and_types_the_character() {
    let mut h = pinned();
    h.send("h");
    assert!(!h.ui.scroll_pinned());
    assert_eq!(h.text(), "h");
}

#[test]
fn takes_the_alternate_screen_and_gives_it_back() {
    let mut h = pinned();
    assert!(h.alternate.load(Ordering::SeqCst));
    h.send(ESCAPE);
    assert!(!h.alternate.load(Ordering::SeqCst));
}

// --- jumping by turn ----------------------------------------------------------

/// header 4 + intro 6 = 10; ask-one at 10, reply-one 12..41, ask-two at 42.
fn transcript() -> (Tui, Rc<RefCell<Container>>) {
    let (mut ui, _) = new_tui();
    let chat = Rc::new(RefCell::new(Container::new()));
    // A header above the chat so the root offset is non-zero.
    ui.add_child(block("header", 4, false));
    {
        let mut c = chat.borrow_mut();
        c.add_child(block("intro", 6, false));
        c.add_child(block("ask-one", 2, true));
        c.add_child(block("reply-one", 30, false));
        c.add_child(block("ask-two", 2, true));
        c.add_child(block("reply-two", 30, false));
    }
    ui.add_child(chat.clone());
    let _events = ui.start();
    (ui, chat)
}

const ASK_ONE: i64 = 10;
const ASK_TWO: i64 = 42;

fn in_view(ui: &Tui, row: i64) -> bool {
    let (top, _, view_height) = ui.get_scroll_position().unwrap();
    top <= row && top + view_height > row
}

#[test]
fn lands_on_the_most_recent_message_first_coming_from_live() {
    let (mut ui, chat) = transcript();
    assert!(jump_to_user_message(
        &mut ui,
        &chat,
        TurnDirection::Previous,
        &is_user
    ));
    assert!(ui.scroll_pinned());
    assert!(in_view(&ui, ASK_TWO));
}

#[test]
fn keeps_stepping_back_through_earlier_messages() {
    let (mut ui, chat) = transcript();
    jump_to_user_message(&mut ui, &chat, TurnDirection::Previous, &is_user);
    assert!(jump_to_user_message(
        &mut ui,
        &chat,
        TurnDirection::Previous,
        &is_user
    ));
    assert!(in_view(&ui, ASK_ONE));
}

#[test]
fn stops_rather_than_wrapping_at_the_first_message() {
    let (mut ui, chat) = transcript();
    jump_to_user_message(&mut ui, &chat, TurnDirection::Previous, &is_user);
    jump_to_user_message(&mut ui, &chat, TurnDirection::Previous, &is_user);
    assert!(!jump_to_user_message(
        &mut ui,
        &chat,
        TurnDirection::Previous,
        &is_user
    ));
}

#[test]
fn comes_forward_again() {
    let (mut ui, chat) = transcript();
    jump_to_user_message(&mut ui, &chat, TurnDirection::Previous, &is_user);
    jump_to_user_message(&mut ui, &chat, TurnDirection::Previous, &is_user);
    assert!(jump_to_user_message(
        &mut ui,
        &chat,
        TurnDirection::Next,
        &is_user
    ));
    assert!(ui.get_scroll_position().unwrap().0 <= ASK_TWO);
}

#[test]
fn does_nothing_when_nothing_in_the_transcript_is_yours() {
    let (mut ui, _) = new_tui();
    let chat = Rc::new(RefCell::new(Container::new()));
    chat.borrow_mut().add_child(block("reply", 60, false));
    ui.add_child(chat.clone());
    let _events = ui.start();
    assert!(!jump_to_user_message(
        &mut ui,
        &chat,
        TurnDirection::Previous,
        &is_user
    ));
}

#[test]
fn declines_before_anything_has_been_rendered() {
    let (mut ui, _) = new_tui();
    let chat = Rc::new(RefCell::new(Container::new()));
    chat.borrow_mut().add_child(block("ask", 2, true));
    ui.add_child(chat.clone());
    assert!(!jump_to_user_message(
        &mut ui,
        &chat,
        TurnDirection::Previous,
        &is_user
    ));
}

// --- searching from the pinned view -------------------------------------------

const CTRL_R: &str = "\x12";
const SLASH: &str = "/";
const ENTER: &str = "\r";

#[test]
fn opens_on_slash_once_pinned_and_takes_typed_characters() {
    let mut h = pinned();
    h.send(SLASH);
    assert!(h.ui.scroll_search_active());
    h.send("l");
    h.send("i");
    assert_eq!(h.ui.scroll_search_query(), "li");
    // None of it leaked into the prompt behind the query line.
    assert_eq!(h.text(), "");
}

#[test]
fn backspaces_the_query() {
    let mut h = pinned();
    h.send(SLASH);
    h.send("l");
    h.send("i");
    h.send("\x7f");
    assert_eq!(h.ui.scroll_search_query(), "l");
}

#[test]
fn treats_an_empty_query_committed_with_enter_as_cancelled() {
    let mut h = pinned();
    h.send(SLASH);
    h.send(ENTER);
    assert!(!h.ui.scroll_search_active());
    assert!(h.ui.scroll_pinned());
}

#[test]
fn drops_the_search_on_the_first_escape_and_the_view_on_the_second() {
    let mut h = pinned();
    h.send(SLASH);
    h.send("l");
    h.send(ENTER);
    assert!(h.ui.scroll_search_active());
    h.send(ESCAPE);
    assert!(!h.ui.scroll_search_active());
    assert!(h.ui.scroll_pinned());
    h.send(ESCAPE);
    assert!(!h.ui.scroll_pinned());
}

#[test]
fn forgets_a_half_typed_query_when_the_view_unpins_without_it() {
    let mut h = pinned();
    h.send(SLASH);
    h.send("l");
    assert_eq!(h.ui.scroll_search_query(), "l");
    h.ui.scroll_to_live();
    assert!(!h.ui.scroll_pinned());
    h.send(PAGE_UP);
    h.send("x");
    assert!(
        !h.ui.scroll_pinned(),
        "x un-pinned instead of typing into a stale query"
    );
    assert_eq!(h.text(), "x");
}

#[test]
fn opens_from_the_prompt_on_ctrl_r_pinning_first() {
    let mut h = setup();
    assert!(!h.ui.scroll_pinned());
    h.send(CTRL_R);
    assert!(h.ui.scroll_pinned());
    assert!(h.ui.scroll_search_active());
}

#[test]
fn leaves_slash_alone_at_the_prompt_where_it_starts_a_slash_command() {
    let mut h = setup();
    h.send(SLASH);
    assert!(!h.ui.scroll_search_active());
    assert_eq!(h.text(), "/");
}
