//! Port of the pin's `test/scroll-viewport.test.ts`: a pinned view stays put.

use crate::support::*;
use cortexcode_tui_render::{Component, ComponentHandle, Tui, TuiEvent};
use std::cell::RefCell;
use std::rc::Rc;

const WIDTH: u16 = 40;
const HEIGHT: u16 = 10;
const VIEW: usize = HEIGHT as usize - 1;

fn transcript(count: usize, label: &str) -> Rc<RefCell<Lines>> {
    Rc::new(RefCell::new(Lines(
        (1..=count).map(|i| format!("{label} {i}")).collect(),
    )))
}

fn setup_n(count: usize) -> (Tui, Handle, Rc<RefCell<Lines>>) {
    let (terminal, term) = virtual_terminal(WIDTH, HEIGHT);
    let mut tui = Tui::new(terminal, None);
    let t = transcript(count, "line");
    tui.add_child(t.clone());
    let _events = tui.start();
    (tui, term, t)
}

fn setup() -> (Tui, Handle, Rc<RefCell<Lines>>) {
    setup_n(100)
}

fn window(term: &Handle) -> Vec<String> {
    term.screen()[..VIEW].to_vec()
}

fn status_row(term: &Handle) -> String {
    term.screen()[HEIGHT as usize - 1].trim().to_string()
}

fn send(tui: &mut Tui, data: &str) {
    tui.process_event(TuiEvent::Input(data.to_string()));
}

fn wheel_up(tui: &mut Tui, times: usize) {
    for _ in 0..times {
        send(tui, "\x1b[<64;1;1M");
    }
}

fn wheel_down(tui: &mut Tui, times: usize) {
    for _ in 0..times {
        send(tui, "\x1b[<65;1;1M");
    }
}

fn top(tui: &Tui) -> i64 {
    tui.get_scroll_position().unwrap().0
}

fn append(t: &Rc<RefCell<Lines>>, text: &str) {
    t.borrow_mut().0.push(text.to_string());
}

#[test]
fn does_not_pin_while_live() {
    let (mut tui, ..) = setup();
    wheel_down(&mut tui, 5);
    assert!(!tui.scroll_pinned());
}

#[test]
fn pins_on_the_first_notch_and_moves_three_lines() {
    let (mut tui, ..) = setup();
    wheel_up(&mut tui, 1);
    assert!(tui.scroll_pinned());
    let first = top(&tui);
    wheel_up(&mut tui, 1);
    assert_eq!(first - top(&tui), 3);
}

#[test]
fn refuses_to_pin_when_the_app_says_no_but_keeps_answering_once_pinned() {
    let (mut tui, ..) = setup();
    tui.can_pin_scroll = Some(Box::new(|_| false));
    wheel_up(&mut tui, 1);
    assert!(!tui.scroll_pinned());

    let (mut tui, ..) = setup();
    wheel_up(&mut tui, 1);
    tui.can_pin_scroll = Some(Box::new(|_| false));
    let before = top(&tui);
    tui.scroll_by_lines(-1);
    assert_eq!(top(&tui), before - 1);
    assert!(tui.scroll_to_live());
}

#[test]
fn shows_the_rows_the_offset_names() {
    let (mut tui, term, _) = setup();
    tui.scroll_by_lines(-20);
    let rows = window(&term);
    let t = top(&tui) as usize;
    assert_eq!(rows[0], format!("line {}", t + 1));
    assert_eq!(rows[VIEW - 1], format!("line {}", t + VIEW));
}

#[test]
fn does_not_move_when_output_arrives() {
    let (mut tui, term, t) = setup();
    tui.scroll_by_lines(-30);
    let before = window(&term);
    for i in 0..40 {
        append(&t, &format!("streamed {i}"));
        tui.request_render(false);
    }
    assert_eq!(window(&term), before);
}

#[test]
fn reports_the_growing_transcript_while_holding_its_place() {
    let (mut tui, _term, t) = setup();
    tui.scroll_by_lines(-30);
    let at = top(&tui);
    append(&t, "one more");
    tui.request_render(false);
    let (now, total, _) = tui.get_scroll_position().unwrap();
    assert_eq!((now, total), (at, 101));
}

#[test]
fn re_clamps_when_the_transcript_shrinks() {
    let (mut tui, term, t) = setup_n(200);
    tui.scroll_to_top();
    tui.scroll_by_lines(180);
    assert!(top(&tui) > 20);
    t.borrow_mut().0.truncate(30);
    tui.request_render(false);
    assert_eq!(top(&tui), (30 - VIEW) as i64);
    assert_eq!(window(&term)[0], format!("line {}", 30 - VIEW + 1));
}

#[test]
fn pages_by_a_screen_less_two() {
    let (mut tui, ..) = setup();
    tui.scroll_by_lines(-1);
    let from = top(&tui);
    tui.scroll_by_pages(-1);
    assert_eq!(from - top(&tui), VIEW as i64 - 2);
}

#[test]
fn goes_to_the_first_row() {
    let (mut tui, term, _) = setup();
    tui.scroll_to_top();
    assert_eq!(top(&tui), 0);
    assert_eq!(window(&term)[0], "line 1");
}

#[test]
fn releases_the_pin_at_the_bottom() {
    let (mut tui, ..) = setup();
    tui.scroll_by_lines(-4);
    assert!(tui.scroll_pinned());
    tui.scroll_by_lines(10);
    assert!(!tui.scroll_pinned());

    let (mut tui, ..) = setup();
    wheel_up(&mut tui, 2);
    wheel_down(&mut tui, 10);
    assert!(!tui.scroll_pinned());
}

#[test]
fn does_nothing_when_the_transcript_fits() {
    let (mut tui, ..) = setup_n(4);
    assert!(!tui.scroll_by_lines(-3));
    assert!(!tui.scroll_by_pages(-1));
    assert!(!tui.scroll_to_top());
    assert!(!tui.scroll_pinned());
}

#[test]
fn indicator_says_where_and_top_and_can_be_replaced() {
    let (mut tui, term, _) = setup();
    tui.scroll_by_lines(-20);
    let t = top(&tui) as usize;
    let status = status_row(&term);
    assert!(
        status.contains(&format!("{}\u{2013}{}/100", t + 1, t + VIEW)),
        "{status}"
    );
    tui.scroll_to_top();
    assert!(status_row(&term).contains("top"));

    let (mut tui, term, _) = setup();
    tui.set_scroll_status_formatter(Box::new(|s| format!("<<{}/{}>>", s.top, s.total)));
    tui.scroll_to_top();
    assert!(status_row(&term).contains("<<1/100>>"));
    let screen = term.screen();
    assert_eq!(screen.len(), HEIGHT as usize);
    assert_eq!(screen[VIEW - 1], format!("line {VIEW}"));
}

#[test]
fn takes_the_alternate_screen_and_gives_it_back() {
    let (mut tui, term, _) = setup();
    tui.scroll_by_lines(-10);
    assert!(term.alternate_screen());
    tui.scroll_to_live();
    assert!(!term.alternate_screen());
}

#[test]
fn shows_what_arrived_without_replaying() {
    let (mut tui, term, t) = setup_n(12);
    let before = tui.full_redraws();
    tui.scroll_to_top();
    append(&t, "arrived while reading");
    tui.request_render(false);
    tui.scroll_to_live();
    let screen = term.screen();
    assert_eq!(
        screen.last().unwrap(),
        "arrived while reading",
        "{screen:?}"
    );
    assert_eq!(tui.full_redraws(), before);
}

#[test]
fn full_redraw_after_a_resize_while_pinned() {
    let (mut tui, term, _) = setup_n(60);
    tui.scroll_to_top();
    let before = tui.full_redraws();
    term.resize(WIDTH - 8, HEIGHT);
    tui.process_event(TuiEvent::Resize);
    tui.scroll_to_live();
    assert!(tui.full_redraws() > before);
}

#[test]
fn leaves_the_alternate_screen_on_stop() {
    let (mut tui, term, _) = setup();
    tui.scroll_by_lines(-10);
    tui.stop();
    assert!(!term.alternate_screen());
}

struct Sink(Rc<RefCell<Vec<String>>>);
impl Component for Sink {
    fn render(&mut self, _width: u16) -> Vec<String> {
        vec!["sink".into()]
    }
    fn handle_input(&mut self, data: &str) {
        self.0.borrow_mut().push(data.to_string());
    }
}

#[test]
fn a_component_still_gets_its_keys_but_never_a_mouse_report() {
    let (mut tui, ..) = setup();
    let seen = Rc::new(RefCell::new(Vec::new()));
    let sink: ComponentHandle = Rc::new(RefCell::new(Sink(seen.clone())));
    tui.set_focus(Some(sink));
    send(&mut tui, "\x1b[<64;1;1M\x1b[A");
    assert_eq!(*seen.borrow(), ["\x1b[A"]);
    seen.borrow_mut().clear();
    send(&mut tui, "\x1b[<0;12;4M");
    send(&mut tui, "\x1b[<0;12;4m");
    assert!(seen.borrow().is_empty());
}

fn searchable() -> (Tui, Handle) {
    let (terminal, term) = virtual_terminal(WIDTH, HEIGHT);
    let mut tui = Tui::new(terminal, None);
    let lines = (0..60)
        .map(|i| {
            if i == 5 || i == 25 || i == 45 {
                "has the Needle here".to_string()
            } else {
                format!("filler {i}")
            }
        })
        .collect();
    tui.add_child(Rc::new(RefCell::new(Lines(lines))));
    let _events = tui.start();
    (tui, term)
}

fn in_view(tui: &Tui, row: i64) -> bool {
    let (top, _, view) = tui.get_scroll_position().unwrap();
    top <= row && row < top + view
}

#[test]
fn search_finds_rows_case_insensitively_and_starts_from_the_newest() {
    let (mut tui, _) = searchable();
    assert_eq!(tui.set_scroll_search("needle", true), 3);
    assert!(in_view(&tui, 45));
}

#[test]
fn search_steps_back_and_wraps() {
    let (mut tui, _) = searchable();
    tui.set_scroll_search("needle", true);
    assert!(tui.scroll_search_step(-1));
    assert!(in_view(&tui, 25));
    tui.scroll_search_step(-1);
    assert!(in_view(&tui, 5));
    tui.scroll_search_step(-1);
    assert!(in_view(&tui, 45));
}

#[test]
fn search_with_no_match_does_not_move() {
    let (mut tui, _) = searchable();
    tui.scroll_to_top();
    let before = top(&tui);
    assert_eq!(tui.set_scroll_search("nothingmatchesthis", true), 0);
    assert_eq!(top(&tui), before);
}

#[test]
fn search_marks_matches_and_feeds_the_indicator() {
    let (mut tui, term) = searchable();
    tui.set_scroll_status_formatter(Box::new(|s| match &s.search {
        Some(q) => format!("q={} {}/{}", q.query, q.index, q.count),
        None => "no search".into(),
    }));
    tui.set_scroll_search("needle", true);
    assert!(term.last_write().contains("\x1b[7m"));
    assert!(status_row(&term).contains("q=needle 3/3"));
    tui.scroll_search_step(-1);
    assert!(status_row(&term).contains("q=needle 2/3"));
}

#[test]
fn search_re_runs_when_the_transcript_grows() {
    let (terminal, _term) = virtual_terminal(WIDTH, HEIGHT);
    let mut tui = Tui::new(terminal, None);
    let t = transcript(60, "filler");
    tui.add_child(t.clone());
    let _events = tui.start();
    tui.set_scroll_search("arrived", true);
    assert_eq!(tui.get_scroll_position(), None);
    tui.scroll_to_top();
    append(&t, "arrived later");
    tui.request_render(false);
    assert!(tui.scroll_search_step(1));
}

#[test]
fn search_ends_with_its_pinned_view() {
    let (mut tui, _) = searchable();
    tui.set_scroll_search("needle", true);
    assert!(tui.scroll_search_active());
    tui.scroll_to_live();
    assert!(!tui.scroll_search_active());
}
