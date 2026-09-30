//! `session-identity.ts`: a session's auto-assigned slug and colour slot, both
//! pure functions of the session id, plus the `/color` name vocabulary.

/// Number of colour slots a session can land in.
pub const SESSION_COLOR_SLOTS: u8 = 6;

const ADJECTIVES: [&str; 32] = [
    "amber", "ash", "azure", "brisk", "bronze", "calm", "clay", "coral", "crisp", "dusk", "ember",
    "fern", "flint", "frost", "gold", "ivory", "jade", "lunar", "mellow", "mint", "olive", "onyx",
    "plum", "quiet", "rust", "sable", "sage", "slate", "solar", "swift", "teal", "velvet",
];

const NOUNS: [&str; 32] = [
    "anchor", "arbor", "basin", "beacon", "birch", "canyon", "cedar", "cinder", "cove", "delta",
    "drift", "ferry", "forge", "harbor", "hollow", "lantern", "meadow", "mesa", "orchard", "pier",
    "prairie", "quarry", "ridge", "summit", "thicket", "tide", "trellis", "vale", "willow",
    "windmill", "yarrow", "zenith",
];

/// djb2 over the UTF-16 code units (`charCodeAt`).
fn djb2(value: &str) -> u32 {
    value.encode_utf16().fold(5381u32, |hash, c| {
        (hash << 5).wrapping_add(hash).wrapping_add(u32::from(c))
    })
}

/// sdbm over the UTF-16 code units.
fn sdbm(value: &str) -> u32 {
    value.encode_utf16().fold(0u32, |hash, c| {
        u32::from(c)
            .wrapping_add(hash << 6)
            .wrapping_add(hash << 16)
            .wrapping_sub(hash)
    })
}

/// `sessionSlugFor`: a memorable two-word slug, e.g. `amber-harbor`.
pub fn session_slug_for(session_id: &str) -> String {
    let adjective = ADJECTIVES[djb2(session_id) as usize % ADJECTIVES.len()];
    let noun = NOUNS[sdbm(session_id) as usize % NOUNS.len()];
    format!("{adjective}-{noun}")
}

/// `sessionColorSlotFor`: the auto-assigned slot in `1..=SESSION_COLOR_SLOTS`.
pub fn session_color_slot_for(session_id: &str) -> u8 {
    (djb2(session_id) % u32::from(SESSION_COLOR_SLOTS)) as u8 + 1
}

/// `isSessionColorSlot`.
pub fn is_session_color_slot(slot: f64) -> bool {
    slot.fract() == 0.0 && slot >= 1.0 && slot <= f64::from(SESSION_COLOR_SLOTS)
}

struct ColorName {
    slot: u8,
    name: &'static str,
    aliases: &'static [&'static str],
}

const SESSION_COLOR_NAMES: [ColorName; 6] = [
    ColorName {
        slot: 1,
        name: "cyan",
        aliases: &["c", "teal", "aqua"],
    },
    ColorName {
        slot: 2,
        name: "purple",
        aliases: &["p", "violet"],
    },
    ColorName {
        slot: 3,
        name: "yellow",
        aliases: &["y", "amber", "gold", "orange", "o"],
    },
    ColorName {
        slot: 4,
        name: "magenta",
        aliases: &["m", "pink", "rose", "red", "r"],
    },
    ColorName {
        slot: 5,
        name: "green",
        aliases: &["g"],
    },
    ColorName {
        slot: 6,
        name: "blue",
        aliases: &["b"],
    },
];

/// `SESSION_COLOR_NAME_LIST`: the canonical slot names, in slot order.
pub fn session_color_name_list() -> Vec<&'static str> {
    SESSION_COLOR_NAMES.iter().map(|entry| entry.name).collect()
}

/// `sessionColorName`.
pub fn session_color_name(slot: u8) -> Option<&'static str> {
    SESSION_COLOR_NAMES
        .iter()
        .find(|entry| entry.slot == slot)
        .map(|entry| entry.name)
}

/// `parseSessionColorSlot`: a number, a name, or an alias (case-insensitive).
pub fn parse_session_color_slot(input: &str) -> Option<u8> {
    let normalized = input.trim().to_lowercase();
    if normalized.is_empty() {
        return None;
    }
    if normalized.bytes().all(|b| b.is_ascii_digit()) {
        let slot: f64 = normalized.parse().ok()?;
        return is_session_color_slot(slot).then_some(slot as u8);
    }
    SESSION_COLOR_NAMES
        .iter()
        .find(|entry| entry.name == normalized || entry.aliases.contains(&normalized.as_str()))
        .map(|entry| entry.slot)
}

/// Cycle direction for [`cycle_session_color_slot`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CycleDirection {
    Forward,
    Backward,
}

/// `cycleSessionColorSlot`: one step, wrapping; a non-slot starts at 1.
pub fn cycle_session_color_slot(current: f64, direction: CycleDirection) -> u8 {
    if !is_session_color_slot(current) {
        return 1;
    }
    let step = match direction {
        CycleDirection::Forward => 1,
        CycleDirection::Backward => SESSION_COLOR_SLOTS - 1,
    };
    ((current as u8 - 1 + step) % SESSION_COLOR_SLOTS) + 1
}

/// Port of `coding-agent/test/session-identity.test.ts`.
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};

    fn same_instant_ids(count: usize) -> Vec<String> {
        (0..count)
            .map(|i| format!("0195b3c1-8a40-7{:03x}-8f2a-{:012x}", i + 1, i))
            .collect()
    }

    #[test]
    fn stable_and_well_formed() {
        let id = "0195b3c1-8a40-7001-8f2a-2d9c4e6b1a77";
        assert_eq!(session_slug_for(id), session_slug_for(id));
        assert_eq!(session_color_slot_for(id), session_color_slot_for(id));
        for id in same_instant_ids(200) {
            assert!(is_session_color_slot(f64::from(session_color_slot_for(
                &id
            ))));
            let slug = session_slug_for(&id);
            assert!(slug.len() <= 20);
            let (a, n) = slug.split_once('-').unwrap();
            assert!(!a.is_empty() && a.bytes().all(|b| b.is_ascii_lowercase()));
            assert!(!n.is_empty() && n.bytes().all(|b| b.is_ascii_lowercase()));
        }
    }

    #[test]
    fn spreads_same_instant_sessions() {
        let ids = same_instant_ids(60);
        let slugs: HashSet<String> = ids.iter().map(|id| session_slug_for(id)).collect();
        let slots: HashSet<u8> = ids.iter().map(|id| session_color_slot_for(id)).collect();
        assert!(slugs.len() > 30);
        assert_eq!(slots.len(), SESSION_COLOR_SLOTS as usize);

        let mut by_adjective: HashMap<String, HashSet<String>> = HashMap::new();
        for slug in same_instant_ids(200).iter().map(|id| session_slug_for(id)) {
            let (a, n) = slug.split_once('-').unwrap();
            by_adjective.entry(a.into()).or_default().insert(n.into());
        }
        assert!(by_adjective.values().map(HashSet::len).max().unwrap() > 1);
    }

    #[test]
    fn known_values_match_hoocode() {
        // node: sessionSlugFor / sessionColorSlotFor at the pin.
        assert_eq!(
            session_slug_for("0195b3c1-8a40-7001-8f2a-2d9c4e6b1a77"),
            "ember-windmill"
        );
        assert_eq!(
            session_color_slot_for("0195b3c1-8a40-7001-8f2a-2d9c4e6b1a77"),
            1
        );
    }

    #[test]
    fn rejects_slots_outside_the_palette() {
        assert!(!is_session_color_slot(0.0));
        assert!(!is_session_color_slot(7.0));
        assert!(!is_session_color_slot(2.5));
        assert!(!is_session_color_slot(f64::NAN));
    }

    #[test]
    fn color_names() {
        let names = session_color_name_list();
        assert_eq!(names.len(), 6);
        for slot in 1..=6u8 {
            assert_eq!(session_color_name(slot), Some(names[slot as usize - 1]));
            assert_eq!(parse_session_color_slot(&slot.to_string()), Some(slot));
        }
        assert_eq!(session_color_name(0), None);
        assert_eq!(session_color_name(7), None);
        assert_eq!(names.iter().collect::<HashSet<_>>().len(), 6);
        assert_eq!(parse_session_color_slot(" 3 "), Some(3));
        for name in &names {
            let slot = parse_session_color_slot(name).unwrap();
            assert_eq!(session_color_name(slot), Some(*name));
            assert_eq!(
                parse_session_color_slot(&name[..1]),
                parse_session_color_slot(name)
            );
        }
        assert_eq!(
            parse_session_color_slot("GREEN"),
            parse_session_color_slot("green")
        );
        assert_eq!(
            parse_session_color_slot("  Blue "),
            parse_session_color_slot("blue")
        );
        for (a, b) in [
            ("red", "magenta"),
            ("r", "magenta"),
            ("orange", "yellow"),
            ("teal", "cyan"),
            ("violet", "purple"),
        ] {
            assert_eq!(parse_session_color_slot(a), parse_session_color_slot(b));
        }
        for (initial, name) in [("g", "green"), ("b", "blue"), ("y", "yellow")] {
            assert_eq!(
                parse_session_color_slot(initial),
                parse_session_color_slot(name)
            );
        }
        let extra = ["red", "r", "g", "b", "y", "m", "orange", "pink", "1", "6"];
        for spelling in names.iter().copied().chain(extra) {
            let slot = parse_session_color_slot(spelling).unwrap_or_else(|| panic!("{spelling}"));
            assert!(is_session_color_slot(f64::from(slot)), "{spelling}");
        }
        for junk in ["", "   ", "0", "7", "2.5", "-1", "chartreuse", "z"] {
            assert_eq!(parse_session_color_slot(junk), None, "{junk}");
        }
    }

    #[test]
    fn cycles() {
        let mut slot = 1u8;
        let mut seen = HashSet::from([slot]);
        for _ in 0..5 {
            slot = cycle_session_color_slot(f64::from(slot), CycleDirection::Forward);
            seen.insert(slot);
        }
        assert_eq!(seen.len(), 6);
        assert_eq!(
            cycle_session_color_slot(f64::from(slot), CycleDirection::Forward),
            1
        );
        for slot in 1..=6u8 {
            let f = cycle_session_color_slot(f64::from(slot), CycleDirection::Forward);
            assert_eq!(
                cycle_session_color_slot(f64::from(f), CycleDirection::Backward),
                slot
            );
            let b = cycle_session_color_slot(f64::from(slot), CycleDirection::Backward);
            assert_eq!(
                cycle_session_color_slot(f64::from(b), CycleDirection::Forward),
                slot
            );
        }
        assert_eq!(cycle_session_color_slot(6.0, CycleDirection::Forward), 1);
        assert_eq!(cycle_session_color_slot(1.0, CycleDirection::Backward), 6);
        for junk in [0.0, -1.0, 2.5, 7.0, f64::NAN] {
            assert_eq!(cycle_session_color_slot(junk, CycleDirection::Forward), 1);
            assert_eq!(cycle_session_color_slot(junk, CycleDirection::Backward), 1);
        }
    }
}
