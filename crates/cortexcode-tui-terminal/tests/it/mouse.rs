//! Port of the pin's `test/mouse.test.ts`.

use cortexcode_tui_terminal::{
    is_mouse_sequence, mouse_sequence_length, parse_mouse_event, MouseEventKind as K,
};

fn kind(data: &str) -> Option<K> {
    parse_mouse_event(data).map(|e| e.kind)
}

#[test]
fn sgr_wheel_both_directions_and_no_button() {
    assert_eq!(kind("\x1b[<64;10;5M"), Some(K::WheelUp));
    assert_eq!(kind("\x1b[<65;10;5M"), Some(K::WheelDown));
    assert_eq!(parse_mouse_event("\x1b[<64;1;1M").unwrap().button, -1);
    assert_eq!(kind("\x1b[<66;1;1M"), Some(K::WheelLeft));
    assert_eq!(kind("\x1b[<67;1;1M"), Some(K::WheelRight));
}

#[test]
fn sgr_coordinates_press_release_modifiers_buttons() {
    let e = parse_mouse_event("\x1b[<0;42;7M").unwrap();
    assert_eq!((e.column, e.row), (42, 7));
    assert_eq!(parse_mouse_event("\x1b[<0;400;9M").unwrap().column, 400);
    assert_eq!(kind("\x1b[<0;1;1M"), Some(K::Press));
    assert_eq!(kind("\x1b[<0;1;1m"), Some(K::Release));
    assert!(parse_mouse_event("\x1b[<4;1;1M").unwrap().shift);
    assert!(parse_mouse_event("\x1b[<8;1;1M").unwrap().alt);
    assert!(parse_mouse_event("\x1b[<16;1;1M").unwrap().ctrl);
    let all = parse_mouse_event("\x1b[<92;1;1M").unwrap();
    assert_eq!(
        (all.kind, all.shift, all.alt, all.ctrl),
        (K::WheelUp, true, true, true)
    );
    for b in 0..3 {
        assert_eq!(
            parse_mouse_event(&format!("\x1b[<{b};1;1M"))
                .unwrap()
                .button,
            b
        );
    }
}

#[test]
fn x10_wheel_and_release() {
    let e = parse_mouse_event(&format!(
        "\x1b[M\x60{}{}",
        char::from(32 + 10),
        char::from(32 + 5)
    ))
    .unwrap();
    assert_eq!((e.kind, e.column, e.row), (K::WheelUp, 10, 5));
    assert_eq!(
        kind(&format!("\x1b[M{}!!", char::from(32 + 3))),
        Some(K::Release)
    );
}

#[test]
fn rejects_non_reports() {
    for data in [
        "a",
        "\x1b[A",
        "\x1b[200~",
        "\x1b[<",
        "\x1b[M",
        "\x1b[Mab",
        "",
    ] {
        assert_eq!(parse_mouse_event(data), None, "{data:?}");
    }
}

#[test]
fn sequence_length() {
    assert_eq!(mouse_sequence_length("\x1b[<64;10;5M"), 11);
    assert_eq!(mouse_sequence_length("\x1b[<64;10;5Mx"), 11);
    assert_eq!(mouse_sequence_length("\x1b[Mabc"), 6);
    assert_eq!(mouse_sequence_length("x\x1b[<64;10;5M"), 0);
    assert_eq!(mouse_sequence_length("\x1b[A"), 0);
    let mut rest = "\x1b[<64;1;1M\x1b[<64;1;2M\x1b[<64;1;3M\x1b[A";
    let mut events = 0;
    while mouse_sequence_length(rest) > 0 {
        let n = mouse_sequence_length(rest);
        assert_eq!(kind(&rest[..n]), Some(K::WheelUp));
        rest = &rest[n..];
        events += 1;
    }
    assert_eq!((events, rest), (3, "\x1b[A"));
}

#[test]
fn is_mouse_sequence_both_encodings_only() {
    assert!(is_mouse_sequence("\x1b[<0;1;1M"));
    assert!(is_mouse_sequence("\x1b[Mabc"));
    assert!(!is_mouse_sequence("\x1b[Ma"));
    assert!(!is_mouse_sequence("\x1b[A"));
}
