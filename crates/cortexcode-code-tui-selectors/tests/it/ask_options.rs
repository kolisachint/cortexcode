//! Port of `test/ask-options.test.ts`.

use std::cell::RefCell;
use std::rc::Rc;

use crate::support::{lock, strip};
use cortexcode_code_tools_optin::ask_options::{AskOption, AskQuestion};
use cortexcode_code_tui_selectors::ask_options::{AskOptionsComponent, AskOptionsOptions};
use cortexcode_tui_render::Component;

const UP: &str = "\x1b[A";
const DOWN: &str = "\x1b[B";
const RIGHT: &str = "\x1b[C";
const LEFT: &str = "\x1b[D";
const ENTER: &str = "\r";
const ESC: &str = "\x1b";

fn option(label: &str, description: &str) -> AskOption {
    AskOption {
        label: label.into(),
        description: Some(description.into()),
        recommended: false,
    }
}

fn questions() -> Vec<AskQuestion> {
    vec![
        AskQuestion {
            question: "where should retry logic live?".into(),
            short: Some("retry lives in".into()),
            detail: Some("the http client is shared by every provider adapter".into()),
            allow_custom: true,
            options: vec![
                option("hoocode-ai", "unified provider client"),
                option("agent-core", "runtime wrap each tool call"),
            ],
        },
        AskQuestion {
            question: "backoff strategy?".into(),
            short: Some("backoff".into()),
            detail: None,
            allow_custom: true,
            options: vec![
                option("exponential + jitter", "100ms x2"),
                option("fixed 1s", "simple"),
            ],
        },
    ]
}

type Submitted = Rc<RefCell<Option<Vec<String>>>>;

fn pane() -> (AskOptionsComponent, Submitted, Rc<RefCell<bool>>) {
    let submitted: Submitted = Rc::default();
    let cancelled = Rc::new(RefCell::new(false));
    let (s, c) = (submitted.clone(), cancelled.clone());
    let pane = AskOptionsComponent::new(
        questions(),
        Box::new(move |answers| *s.borrow_mut() = Some(answers)),
        Box::new(move || *c.borrow_mut() = true),
        AskOptionsOptions::default(),
    );
    (pane, submitted, cancelled)
}

fn out(c: &mut AskOptionsComponent) -> String {
    strip(&c.render(80))
}

fn answers(v: &[&str]) -> Option<Vec<String>> {
    Some(v.iter().map(|s| s.to_string()).collect())
}

#[test]
fn renders_header_question_options_and_custom_row() {
    let _g = lock();
    let (mut c, _, _) = pane();
    let out = out(&mut c);
    assert!(out.contains("input needed"), "{out}");
    assert!(out.contains("1/2 where should retry logic live?"), "{out}");
    assert!(out.contains("the http client is shared by every provider adapter"));
    assert!(out.contains("1 hoocode-ai"));
    assert!(out.contains("2 agent-core"));
    assert!(out.contains("custom answer"));
    assert!(out.contains("(1/3)"));
}

#[test]
fn moves_the_cursor_and_wraps() {
    let _g = lock();
    let (mut c, _, _) = pane();
    c.handle_input(DOWN);
    let o = out(&mut c);
    assert!(o.contains("› 2 agent-core"), "{o}");
    assert!(o.contains("(2/3)"));
    c.handle_input(UP);
    assert!(out(&mut c).contains("› 1 hoocode-ai"));
    c.handle_input(UP);
    assert!(out(&mut c).contains("(3/3)"));
}

#[test]
fn advances_with_right_and_submits_on_the_last_step() {
    let _g = lock();
    let (mut c, submitted, _) = pane();
    c.handle_input(RIGHT);
    let o = out(&mut c);
    assert!(o.contains("retry lives in"), "{o}");
    assert!(o.contains("hoocode-ai"));
    assert!(o.contains("2/2 backoff strategy?"));
    c.handle_input(DOWN);
    c.handle_input(RIGHT);
    assert_eq!(*submitted.borrow(), answers(&["hoocode-ai", "fixed 1s"]));
}

#[test]
fn quick_picks_with_number_keys() {
    let _g = lock();
    let (mut c, submitted, _) = pane();
    c.handle_input("2");
    c.handle_input("1");
    assert_eq!(
        *submitted.borrow(),
        answers(&["agent-core", "exponential + jitter"])
    );
}

#[test]
fn steps_back_with_left() {
    let _g = lock();
    let (mut c, submitted, _) = pane();
    c.handle_input("1");
    assert!(out(&mut c).contains("2/2 backoff strategy?"));
    c.handle_input(LEFT);
    assert!(out(&mut c).contains("1/2 where should retry logic live?"));
    c.handle_input("2");
    c.handle_input("1");
    assert_eq!(
        *submitted.borrow(),
        answers(&["agent-core", "exponential + jitter"])
    );
}

fn type_custom(c: &mut AskOptionsComponent) {
    c.handle_input(DOWN);
    c.handle_input(DOWN);
    for ch in "per-provider".chars() {
        c.handle_input(&ch.to_string());
    }
}

#[test]
fn accepts_a_typed_custom_answer() {
    let _g = lock();
    let (mut c, submitted, _) = pane();
    type_custom(&mut c);
    assert!(out(&mut c).contains("per-provider"));
    c.handle_input(RIGHT);
    assert!(out(&mut c).contains("2/2 backoff strategy?"));
    c.handle_input("2");
    assert_eq!(*submitted.borrow(), answers(&["per-provider", "fixed 1s"]));
}

#[test]
fn gives_the_arrows_to_the_text_once_typed() {
    let _g = lock();
    let (mut c, submitted, _) = pane();
    type_custom(&mut c);
    c.handle_input(LEFT);
    let o = out(&mut c);
    assert!(o.contains("1/2 where should retry logic live?"), "{o}");
    assert!(o.contains("per-provider"));
    c.handle_input(RIGHT);
    assert!(out(&mut c).contains("1/2 where should retry logic live?"));
    c.handle_input(RIGHT);
    assert!(out(&mut c).contains("2/2 backoff strategy?"));
    c.handle_input("2");
    assert_eq!(*submitted.borrow(), answers(&["per-provider", "fixed 1s"]));
}

#[test]
fn commits_a_custom_answer_with_enter_anywhere() {
    let _g = lock();
    let (mut c, submitted, _) = pane();
    type_custom(&mut c);
    c.handle_input(LEFT);
    c.handle_input(LEFT);
    c.handle_input(ENTER);
    assert!(out(&mut c).contains("2/2 backoff strategy?"));
    c.handle_input("2");
    assert_eq!(*submitted.borrow(), answers(&["per-provider", "fixed 1s"]));
}

#[test]
fn does_not_submit_an_empty_custom_answer() {
    let _g = lock();
    let (mut c, submitted, _) = pane();
    c.handle_input(DOWN);
    c.handle_input(DOWN);
    c.handle_input(RIGHT);
    assert!(out(&mut c).contains("1/2 where should retry logic live?"));
    assert_eq!(*submitted.borrow(), None);
}

#[test]
fn skips_with_escape() {
    let _g = lock();
    let (mut c, _, cancelled) = pane();
    c.handle_input(ESC);
    assert!(*cancelled.borrow());
}
