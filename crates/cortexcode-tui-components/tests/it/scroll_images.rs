//! Port of the pin's `test/scroll-images.test.ts`: pictures in the pinned
//! window are drawn when they fit, as copies the window can delete alone.

use crate::render_support::*;
use cortexcode_tui_components::{Image, ImageOptions, ImageTheme};
use cortexcode_tui_images::{
    set_capabilities, set_cell_dimensions, CellDimensions, ImageDimensions, ImageProtocol,
    TerminalCapabilities,
};
use cortexcode_tui_render::{Component, Tui};
use std::cell::RefCell;
use std::rc::Rc;

const WIDTH: u16 = 40;
const HEIGHT: u16 = 10;
const VIEW: i64 = HEIGHT as i64 - 1;
const BEFORE: usize = 20;
const AFTER: usize = 20;
const IMAGE_ROWS: i64 = 3;
const IMAGE_LINE: i64 = BEFORE as i64 + IMAGE_ROWS - 1;
const KITTY: &str = "\x1b_G";
const PLACEHOLDER: &str = "[image]";

struct ImageTranscript {
    image: Image,
}

impl Component for ImageTranscript {
    fn render(&mut self, width: u16) -> Vec<String> {
        let mut lines: Vec<String> = (1..=BEFORE).map(|i| format!("before {i}")).collect();
        lines.extend(self.image.render(width));
        lines.extend((1..=AFTER).map(|i| format!("after {i}")));
        lines
    }
    fn invalidate(&mut self) {
        self.image.invalidate();
    }
}

fn transmitted_id(buffer: &str) -> Option<u32> {
    let start = buffer.find(KITTY)?;
    let params_start = start + KITTY.len();
    let params = &buffer[params_start..params_start + buffer[params_start..].find(';')?];
    params
        .split(',')
        .find_map(|p| p.strip_prefix("i="))
        .and_then(|v| v.parse().ok())
}

struct Harness {
    tui: Tui,
    term: Handle,
    transcript: Rc<RefCell<ImageTranscript>>,
}

fn setup() -> Harness {
    set_capabilities(TerminalCapabilities {
        images: Some(ImageProtocol::Kitty),
        true_color: true,
        hyperlinks: true,
    });
    set_cell_dimensions(CellDimensions {
        width_px: 9,
        height_px: 18,
    });
    let (terminal, term) = virtual_terminal(WIDTH, HEIGHT);
    let mut tui = Tui::new(terminal, None);
    let transcript = Rc::new(RefCell::new(ImageTranscript {
        image: Image::new(
            "bm90LWEtcmVhbC1wbmc=",
            "image/png",
            ImageTheme {
                fallback_color: Box::new(|t: &str| t.to_string()),
            },
            ImageOptions {
                max_width_cells: Some(10),
                ..Default::default()
            },
            Some(ImageDimensions {
                width_px: 90,
                height_px: 54,
            }),
        ),
    }));
    tui.add_child(transcript.clone());
    let _events = tui.start();
    Harness {
        tui,
        term,
        transcript,
    }
}

impl Harness {
    fn pin(&mut self, top: i64) {
        self.tui.scroll_to_top();
        if top > 0 {
            self.tui.scroll_by_lines(top);
        }
        assert_eq!(self.tui.get_scroll_position().map(|p| p.0), Some(top));
        self.term.clear_writes();
        self.tui.request_render(false);
    }
}

#[test]
fn draws_the_picture_when_all_of_it_is_in_the_window() {
    let mut h = setup();
    h.pin(IMAGE_LINE - (IMAGE_ROWS - 1));
    let frame = h.term.joined_writes();
    assert!(frame.contains(KITTY));
    assert!(!frame.contains(PLACEHOLDER));
}

#[test]
fn draws_it_with_its_bottom_row_against_the_bottom() {
    let mut h = setup();
    h.pin(IMAGE_LINE - (VIEW - 1));
    let frame = h.term.joined_writes();
    assert!(frame.contains(KITTY));
    assert!(!frame.contains(PLACEHOLDER));
}

#[test]
fn names_it_rather_than_draw_above_the_first_row() {
    let mut h = setup();
    h.pin(IMAGE_LINE - (IMAGE_ROWS - 2));
    let frame = h.term.joined_writes();
    assert!(frame.contains(PLACEHOLDER));
    assert!(!frame.contains(KITTY));
}

#[test]
fn transmits_its_own_copy_under_a_new_id() {
    let mut h = setup();
    h.pin(IMAGE_LINE - (IMAGE_ROWS - 1));
    let pinned = transmitted_id(&h.term.joined_writes()).expect("id");
    assert_ne!(Some(pinned), h.transcript.borrow().image.image_id());
}

#[test]
fn takes_the_last_copy_off_before_painting_the_next() {
    let mut h = setup();
    h.pin(IMAGE_LINE - (IMAGE_ROWS - 1));
    let pinned = transmitted_id(&h.term.joined_writes()).expect("id");
    h.term.clear_writes();
    h.tui.scroll_by_lines(-1);
    let frame = h.term.joined_writes();
    let removed = frame
        .find(&format!("a=d,d=I,i={pinned},"))
        .expect("previous copy removed");
    assert!(frame[removed + 1..].contains(KITTY));
}

#[test]
fn frees_its_copies_on_the_way_out_and_leaves_the_live_one() {
    let mut h = setup();
    h.pin(IMAGE_LINE - (IMAGE_ROWS - 1));
    let pinned = transmitted_id(&h.term.joined_writes()).expect("id");
    h.term.clear_writes();
    h.tui.scroll_to_live();
    let frame = h.term.joined_writes();
    assert!(frame.contains(&format!("a=d,d=I,i={pinned},")));
    let live = h.transcript.borrow().image.image_id().unwrap();
    assert!(!frame.contains(&format!("a=d,d=I,i={live},")));
}
