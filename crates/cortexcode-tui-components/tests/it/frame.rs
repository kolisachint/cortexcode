//! Port of the pin's `test/frame.test.ts`.

use cortexcode_tui_components::*;
use cortexcode_tui_render::{Component, ComponentHandle};
use cortexcode_tui_util::visible_width;
use std::cell::RefCell;
use std::rc::Rc;

struct WidthProbe {
    seen: Rc<RefCell<Vec<u16>>>,
    rows: usize,
}

impl Component for WidthProbe {
    fn render(&mut self, width: u16) -> Vec<String> {
        self.seen.borrow_mut().push(width);
        (0..self.rows).map(|i| format!("row{i}")).collect()
    }
}

fn probe(rows: usize) -> (ComponentHandle, Rc<RefCell<Vec<u16>>>) {
    let seen = Rc::new(RefCell::new(Vec::new()));
    let c: ComponentHandle = Rc::new(RefCell::new(WidthProbe {
        seen: seen.clone(),
        rows,
    }));
    (c, seen)
}

struct Overrunner;
impl Component for Overrunner {
    fn render(&mut self, width: u16) -> Vec<String> {
        vec!["x".repeat(width as usize + 12)]
    }
}

const WIDTHS: [u16; 15] = [200, 120, 100, 80, 60, 40, 20, 12, 8, 6, 5, 4, 3, 2, 1];

fn frame(border: FrameBorderStyle) -> Frame {
    Frame::new(FrameOptions {
        border: Some(border),
        ..FrameOptions::default()
    })
}

fn label(text: &str) -> Option<FrameLabel> {
    Some(FrameLabel {
        plain: text.into(),
        styled: text.into(),
    })
}

#[test]
fn fills_exactly_the_width_at_every_width() {
    for border in [FrameBorderStyle::Box, FrameBorderStyle::Rule] {
        for width in WIDTHS {
            let mut f = frame(border);
            f.add_child(probe(3).0);
            for (i, line) in f.render(width).iter().enumerate() {
                assert_eq!(
                    visible_width(line),
                    width as usize,
                    "{border:?} @{width}: row {i} {line:?}"
                );
            }
        }
    }
}

#[test]
fn keeps_its_geometry_when_a_child_overruns() {
    for width in WIDTHS {
        let mut f = frame(FrameBorderStyle::Box);
        f.add_child(Rc::new(RefCell::new(Overrunner)));
        for line in f.render(width) {
            assert_eq!(visible_width(&line), width as usize, "@{width}");
        }
    }
}

#[test]
fn spends_two_columns_on_border_and_two_on_gutter() {
    let (p, seen) = probe(1);
    let mut f = Frame::new(FrameOptions {
        border: Some(FrameBorderStyle::Box),
        padding_x: Some(1),
        ..FrameOptions::default()
    });
    f.add_child(p);
    f.render(100);
    assert_eq!(seen.borrow().last(), Some(&96));
    assert_eq!(f.content_width(100), 96);
}

#[test]
fn draws_nothing_when_empty() {
    let mut f = frame(FrameBorderStyle::Box);
    assert!(f.render(100).is_empty());
    f.add_child(Rc::new(RefCell::new(Text::new("", 0, 0))));
    assert!(f.render(100).is_empty());
}

#[test]
fn degrades_to_rules_when_a_box_will_not_fit() {
    let mut f = frame(FrameBorderStyle::Box);
    f.add_child(probe(1).0);
    let lines = f.render(3);
    assert!(!lines[0].contains('┌'));
    for line in lines {
        assert_eq!(visible_width(&line), 3);
    }
}

#[test]
fn passes_the_whole_width_through_with_no_border() {
    let (p, seen) = probe(1);
    let mut f = frame(FrameBorderStyle::None);
    f.add_child(p);
    assert_eq!(f.render(100), vec!["row0".to_string()]);
    assert_eq!(seen.borrow().last(), Some(&100));
}

#[test]
fn lays_the_label_into_the_top_border_only() {
    let mut f = frame(FrameBorderStyle::Box);
    f.set_label(label(" models "));
    f.add_child(probe(1).0);
    let lines = f.render(60);
    assert!(lines[0].contains(" models "));
    assert!(!lines.last().unwrap().contains("models"));
    assert_eq!(visible_width(&lines[0]), 60);
}

#[test]
fn drops_a_label_with_no_room() {
    let mut f = frame(FrameBorderStyle::Box);
    f.set_label(label(" a very long surface name "));
    f.add_child(probe(1).0);
    let lines = f.render(20);
    assert!(!lines[0].contains("surface name"));
    assert_eq!(visible_width(&lines[0]), 20);
}

#[test]
fn label_fits_agrees_with_what_is_drawn() {
    let mut f = frame(FrameBorderStyle::Box);
    f.add_child(probe(1).0);
    assert!(f.label_fits(" models ", 60));
    assert!(!f.label_fits(" a very long surface name ", 20));
    assert!(!f.label_fits("", 60));
    for (plain, width) in [
        (" models ", 60u16),
        (" a very long surface name ", 20),
        (" x ", 8),
        (" x ", 7),
    ] {
        f.set_label(label(plain));
        let drawn = f.render(width)[0].contains(plain.trim());
        assert_eq!(
            drawn,
            f.label_fits(plain, width as usize),
            "{plain:?} @{width}"
        );
    }
}

fn edge(side: FrameEdge, hidden: usize) -> String {
    let chars = FrameBorderChars::default();
    render_frame_edge(&FrameEdgeOptions {
        edge: side,
        bar_width: 58,
        is_box: true,
        chars: &chars,
        color: &|s: &str| s.to_string(),
        label: None,
        hidden,
    })
}

#[test]
fn edge_announces_hidden_rows() {
    assert!(edge(FrameEdge::Top, 3).contains("↑ 3 more"));
    assert!(edge(FrameEdge::Bottom, 7).contains("↓ 7 more"));
    assert_eq!(visible_width(&edge(FrameEdge::Top, 3)), 60);
    assert_eq!(visible_width(&edge(FrameEdge::Bottom, 7)), 60);
}
