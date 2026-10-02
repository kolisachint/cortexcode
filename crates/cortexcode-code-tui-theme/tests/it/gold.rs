//! Every token's ANSI (fg, bg and chip fill) for every bundled theme in both
//! color modes, plus resolved/export colors and the agent/session palette
//! mapping, against `fixtures/theme-gold.json` generated from the pinned
//! hoocode build.

use std::collections::BTreeMap;

use cortexcode_code_tui_theme::*;
use serde_json::Value;

fn gold() -> Value {
    serde_json::from_str(include_str!("../fixtures/theme-gold.json")).unwrap()
}

fn themes_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("themes")
}

#[test]
fn every_token_encodes_like_hoocode_in_both_modes() {
    let gold = gold();
    let mut failures = Vec::new();
    for (name, expected) in gold["themes"].as_object().unwrap() {
        for (mode, key) in [
            (ColorMode::Truecolor, "truecolor"),
            (ColorMode::Color256, "256color"),
        ] {
            let theme =
                load_theme_from_path(&themes_dir().join(format!("{name}.json")), Some(mode))
                    .unwrap();
            let exp = &expected[key];
            for (token, ansi) in exp["fg"].as_object().unwrap() {
                if theme.get_fg_ansi(token) != ansi.as_str().unwrap() {
                    failures.push(format!(
                        "{name} {key} fg {token}: {:?} vs {ansi}",
                        theme.get_fg_ansi(token)
                    ));
                }
                let fill = if theme.can_fill(token) {
                    Value::String(theme.fill(token, ""))
                } else {
                    Value::Null
                };
                if &fill != exp["fill"].get(token).unwrap() {
                    failures.push(format!(
                        "{name} {key} fill {token}: {fill} vs {}",
                        exp["fill"][token]
                    ));
                }
            }
            for token in THEME_COLORS {
                if theme.has(token) != exp["fg"].get(token).is_some() {
                    failures.push(format!("{name} {key} has({token}) differs"));
                }
            }
            for (token, ansi) in exp["bg"].as_object().unwrap() {
                if theme.get_bg_ansi(token) != ansi.as_str().unwrap() {
                    failures.push(format!(
                        "{name} {key} bg {token}: {:?} vs {ansi}",
                        theme.get_bg_ansi(token)
                    ));
                }
            }
            for token in THEME_BGS {
                if theme.has_bg(token) != exp["bg"].get(token).is_some() {
                    failures.push(format!("{name} {key} has_bg({token}) differs"));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn resolved_and_export_colors_match_hoocode() {
    let gold = gold();
    for (name, expected) in gold["themes"].as_object().unwrap() {
        let resolved = get_resolved_theme_colors(Some(name)).unwrap();
        let exp: BTreeMap<String, String> =
            serde_json::from_value(expected["resolved"].clone()).unwrap();
        assert_eq!(resolved, exp, "{name}");
        let export = get_theme_export_colors(Some(name));
        let e = &expected["export"];
        let get = |k: &str| e.get(k).and_then(|v| v.as_str()).map(str::to_string);
        assert_eq!(export.page_bg, get("pageBg"), "{name}");
        assert_eq!(export.card_bg, get("cardBg"), "{name}");
        assert_eq!(export.info_bg, get("infoBg"), "{name}");
    }
}

#[test]
fn agent_and_session_palette_mapping_matches_hoocode() {
    let gold = gold();
    for (agent, token) in gold["agents"].as_object().unwrap() {
        assert_eq!(agent_color_for(agent), token.as_str().unwrap(), "{agent}");
    }
    for (slot, token) in gold["sessions"].as_object().unwrap() {
        // Rust slots are integers; hoocode maps a non-integer to slot 1.
        let Ok(slot) = slot.parse::<i64>() else {
            continue;
        };
        assert_eq!(session_color_token(slot), token.as_str().unwrap(), "{slot}");
    }
}
