//! Port of the pin's `test/session-chip.test.ts`.

mod support;

use cortexcode_code_tui_app::session_chip::*;
use cortexcode_code_tui_theme::*;
use cortexcode_tui_util::{strip_vt_control_characters, visible_width};
use support::lock;

#[test]
fn pads_the_name_so_the_fill_reads_as_a_chip_rather_than_as_text() {
    let _g = lock(None);
    assert_eq!(
        render_session_chip("refactor-auth", 1).unwrap().plain,
        " refactor-auth "
    );
}

#[test]
fn keeps_the_styled_string_exactly_as_wide_as_the_plain_one() {
    let _g = lock(None);
    for slot in 1..=6 {
        let chip = render_session_chip("amber-harbor", slot).unwrap();
        assert_eq!(visible_width(&chip.styled), visible_width(&chip.plain));
    }
}

#[test]
fn truncates_a_name_too_long_to_glance_at() {
    let _g = lock(None);
    let chip = render_session_chip("an-extremely-long-session-name-nobody-can-scan", 1).unwrap();
    assert!(visible_width(&chip.plain) <= 22);
    assert!(strip_vt_control_characters(&chip.styled).contains('…'));
}

#[test]
fn has_nothing_to_draw_for_an_empty_name() {
    let _g = lock(None);
    assert!(render_session_chip("", 1).is_none());
    assert!(render_session_chip("   ", 1).is_none());
}

#[test]
fn fills_with_the_sessions_own_colour() {
    let _g = lock(None);
    assert_ne!(
        render_session_chip("refactor-auth", 3).unwrap().styled,
        render_session_chip("refactor-auth", 4).unwrap().styled
    );
}

#[test]
fn lifts_a_light_themes_fill_off_the_ink_it_is_drawn_from() {
    let _g = lock(None);
    let param = |styled: &str, base: &str| {
        let start = styled.find(&format!("\x1b[{base};")).unwrap() + 2;
        let end = start + styled[start..].find('m').unwrap();
        styled[start..end].to_string()
    };
    let dark_ink = ["38;2;11;11;15", "38;5;232"];

    let dark = get_theme_by_name("dark").unwrap();
    let light = get_theme_by_name("light").unwrap();

    set_theme_instance((*dark).clone());
    let on_dark = render_session_chip("refactor-auth", 1).unwrap().styled;
    let dark_token = dark
        .get_fg_ansi(session_color_token(1))
        .replace("38;", "48;");

    set_theme_instance((*light).clone());
    let on_light = render_session_chip("refactor-auth", 1).unwrap().styled;
    let light_token = light
        .get_fg_ansi(session_color_token(1))
        .replace("38;", "48;");

    assert_eq!(format!("\x1b[{}m", param(&on_dark, "48")), dark_token);
    assert_ne!(format!("\x1b[{}m", param(&on_light, "48")), light_token);
    assert!(dark_ink.contains(&param(&on_dark, "38").as_str()));
    assert!(dark_ink.contains(&param(&on_light, "38").as_str()));
}

#[test]
fn can_fill_every_slot_in_every_shipped_theme() {
    let _g = lock(None);
    for name in [
        "colorsafe-dark",
        "colorsafe-light",
        "dark",
        "light",
        "solarized-dark",
        "solarized-light",
        "vox-cutout-dark",
        "vox-cutout-light",
    ] {
        let t = get_theme_by_name(name).unwrap();
        for slot in 1..=6 {
            assert!(t.can_fill(session_color_token(slot)), "{name} slot {slot}");
        }
    }
}

#[test]
fn drops_the_chip_only_where_the_box_has_no_room_for_it() {
    assert!(session_chip_fits(SESSION_CHIP_MIN_WIDTH));
    assert!(!session_chip_fits(SESSION_CHIP_MIN_WIDTH - 1));
    assert!(session_chip_fits(200));
}
