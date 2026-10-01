//! Port of hoocode `test/tips.test.ts` (v0.5.89). hoocode's fake timers
//! become a fake clock plus `TipsController::poll`.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;
use std::time::{Duration, Instant};

use cortexcode_code_tui_app::tips::*;

/// A rotation wired to in-memory stores, so a test can watch what it remembers.
struct Stores {
    seen: Rc<RefCell<Vec<String>>>,
    star_nudges: Rc<Cell<u64>>,
}

fn make_rotation(tips: Option<Vec<Tip>>, seen: &[&str], star_nudges: u64) -> (TipRotation, Stores) {
    let stores = Stores {
        seen: Rc::new(RefCell::new(seen.iter().map(|s| s.to_string()).collect())),
        star_nudges: Rc::new(Cell::new(star_nudges)),
    };
    let (s1, s2) = (stores.seen.clone(), stores.seen.clone());
    let (n1, n2) = (stores.star_nudges.clone(), stores.star_nudges.clone());
    let rotation = TipRotation::new(
        TipRotationOptions {
            seen: Box::new(move || s1.borrow().clone()),
            mark_seen: Box::new(move |id| {
                let mut seen = s2.borrow_mut();
                if !seen.iter().any(|s| s == id) {
                    seen.push(id.to_string());
                }
            }),
            star_nudge_count: Box::new(move || n1.get()),
            mark_star_nudge: Box::new(move || n2.set(n2.get() + 1)),
        },
        tips,
    );
    (rotation, stores)
}

fn many(n: usize) -> Vec<Tip> {
    (0..n)
        .map(|i| Tip::new(format!("t{i}"), format!("T{i}")))
        .collect()
}

// --- TIPS content ---------------------------------------------------------

#[test]
fn has_no_duplicate_ids() {
    let ids: Vec<String> = tips().into_iter().map(|t| t.id).collect();
    let unique: HashSet<&String> = ids.iter().collect();
    assert_eq!(unique.len(), ids.len());
}

#[test]
fn renders_every_tip_without_throwing() {
    for tip in tips() {
        let rendered = render_tip(&tip);
        assert!(!rendered.title.is_empty());
    }
}

#[test]
fn keeps_every_tip_within_the_bands_row_budget() {
    for tip in tips().iter().chain([star_nudge()].iter()) {
        assert!(render_tip(tip).body.len() <= 3, "{}", tip.id);
    }
}

#[test]
fn key_tips_name_the_live_binding() {
    cortexcode_code_tui_keybindings::AppKeybindingsManager::default().install();
    let modes = tips().into_iter().find(|t| t.id == "modes").unwrap();
    let body = render_tip(&modes).body;
    assert_eq!(body[0], "alt+a cycles ask → plan → build → debug.");
}

#[test]
fn the_default_rotation_leaves_out_features_cortex_does_not_ship() {
    let available: Vec<String> = available_tips().into_iter().map(|t| t.id).collect();
    for id in UNAVAILABLE_TIP_IDS {
        assert!(tips().iter().any(|t| t.id == *id), "{id} is a hoocode tip");
        assert!(!available.iter().any(|a| a == id));
    }
    assert_eq!(available.len(), tips().len() - UNAVAILABLE_TIP_IDS.len());
}

// --- TipRotation ----------------------------------------------------------

fn abc() -> Vec<Tip> {
    vec![Tip::new("a", "A"), Tip::new("b", "B"), Tip::new("c", "C")]
}

fn next_id(rotation: &mut TipRotation, moment: TipMoment) -> Option<String> {
    rotation.next(moment).map(|t| t.id)
}

#[test]
fn walks_unseen_tips_in_declaration_order() {
    let (mut rotation, _) = make_rotation(Some(abc()), &[], 0);
    assert_eq!(
        next_id(&mut rotation, TipMoment::Idle).as_deref(),
        Some("a")
    );
    assert_eq!(
        next_id(&mut rotation, TipMoment::Idle).as_deref(),
        Some("b")
    );
    assert_eq!(
        next_id(&mut rotation, TipMoment::Idle).as_deref(),
        Some("c")
    );
}

#[test]
fn skips_what_a_previous_session_already_showed() {
    let (mut rotation, _) = make_rotation(Some(abc()), &["a", "b"], 0);
    assert_eq!(
        next_id(&mut rotation, TipMoment::Idle).as_deref(),
        Some("c")
    );
}

#[test]
fn records_each_tip_it_shows() {
    let (mut rotation, stores) = make_rotation(Some(abc()[..2].to_vec()), &[], 0);
    rotation.next(TipMoment::Idle);
    assert_eq!(*stores.seen.borrow(), vec!["a".to_string()]);
}

#[test]
fn starts_over_once_everything_has_been_seen() {
    let (mut rotation, _) = make_rotation(Some(abc()[..2].to_vec()), &["a", "b"], 0);
    assert_eq!(
        next_id(&mut rotation, TipMoment::Idle).as_deref(),
        Some("a")
    );
    assert_eq!(
        next_id(&mut rotation, TipMoment::Idle).as_deref(),
        Some("b")
    );
    assert_eq!(next_id(&mut rotation, TipMoment::Idle), None);
}

#[test]
fn honours_a_tips_moment() {
    let stream_only = Tip::new("s", "S").with_moments(&[TipMoment::Streaming]);
    let idle_only = Tip::new("i", "I").with_moments(&[TipMoment::Idle]);
    let (mut rotation, _) = make_rotation(Some(vec![stream_only, idle_only]), &[], 0);
    assert_eq!(
        next_id(&mut rotation, TipMoment::Idle).as_deref(),
        Some("i")
    );
    assert_eq!(
        next_id(&mut rotation, TipMoment::Streaming).as_deref(),
        Some("s")
    );
}

#[test]
fn does_not_ask_for_a_star_before_it_has_been_useful() {
    let (mut rotation, stores) = make_rotation(Some(abc()), &[], 0);
    for _ in 0..3 {
        rotation.next(TipMoment::Idle);
    }
    assert_eq!(stores.star_nudges.get(), 0);
}

#[test]
fn asks_for_a_star_once_the_cadence_is_reached_and_only_once_a_session() {
    let (mut rotation, stores) = make_rotation(Some(many(20)), &[], 0);
    let shown: Vec<String> = (0..20)
        .filter_map(|_| next_id(&mut rotation, TipMoment::Idle))
        .collect();
    assert_eq!(shown.iter().filter(|id| *id == "star").count(), 1);
    assert_eq!(stores.star_nudges.get(), 1);
}

#[test]
fn stops_asking_for_a_star_once_the_lifetime_cap_is_spent() {
    let (mut rotation, stores) = make_rotation(Some(many(20)), &[], STAR_NUDGE_LIMIT);
    for _ in 0..20 {
        rotation.next(TipMoment::Idle);
    }
    assert_eq!(stores.star_nudges.get(), STAR_NUDGE_LIMIT);
}

#[test]
fn never_nudges_while_streaming() {
    let (mut rotation, _) = make_rotation(Some(many(20)), &[], 0);
    let shown: Vec<String> = (0..20)
        .filter_map(|_| next_id(&mut rotation, TipMoment::Streaming))
        .collect();
    assert!(!shown.iter().any(|id| id == "star"));
}

// --- TipsController ---------------------------------------------------------

/// A controller on a fake clock, so no test waits on wall time.
struct Harness {
    controller: TipsController,
    shown: Rc<RefCell<Vec<String>>>,
    offset: Rc<Cell<Duration>>,
    enabled: Rc<Cell<bool>>,
    band_free: Rc<Cell<bool>>,
}

impl Harness {
    fn new() -> Self {
        let base = Instant::now();
        let offset = Rc::new(Cell::new(Duration::ZERO));
        let shown = Rc::new(RefCell::new(Vec::new()));
        let enabled = Rc::new(Cell::new(true));
        let band_free = Rc::new(Cell::new(true));
        let (rotation, _) = make_rotation(Some(many(30)), &[], 0);
        let (o, s, e, b) = (
            offset.clone(),
            shown.clone(),
            enabled.clone(),
            band_free.clone(),
        );
        let mut options = TipsControllerOptions::new(
            Box::new(move || e.get()),
            Box::new(move || b.get()),
            Box::new(move |tip: &Tip| s.borrow_mut().push(tip.id.clone())),
            rotation,
        );
        options.clock = Rc::new(move || base + o.get());
        Self {
            controller: TipsController::new(options),
            shown,
            offset,
            enabled,
            band_free,
        }
    }

    /// Advance the clock and fire whatever was due.
    fn advance(&mut self, by: Duration) {
        self.offset.set(self.offset.get() + by);
        self.controller.poll();
    }

    fn shown(&self) -> usize {
        self.shown.borrow().len()
    }
}

const SECOND: Duration = Duration::from_millis(1_000);

#[test]
fn says_nothing_during_the_startup_grace_period() {
    let mut h = Harness::new();
    h.controller.on_activity();
    h.advance(DEFAULT_IDLE_DELAY);
    assert_eq!(h.shown(), 0);
}

#[test]
fn shows_a_tip_once_the_prompt_has_been_quiet_for_the_idle_delay() {
    let mut h = Harness::new();
    h.advance(DEFAULT_GRACE);
    h.controller.on_activity();
    h.advance(DEFAULT_IDLE_DELAY);
    assert_eq!(h.shown(), 1);
}

#[test]
fn restarts_the_idle_clock_on_every_keystroke() {
    let mut h = Harness::new();
    h.advance(DEFAULT_GRACE);
    h.controller.on_activity();
    h.advance(DEFAULT_IDLE_DELAY - SECOND);
    h.controller.on_activity();
    h.advance(DEFAULT_IDLE_DELAY - SECOND);
    assert_eq!(h.shown(), 0);
}

#[test]
fn shows_a_tip_during_a_long_turn() {
    let mut h = Harness::new();
    h.advance(DEFAULT_GRACE);
    h.controller.on_turn_start();
    h.advance(DEFAULT_STREAMING_DELAY);
    assert_eq!(h.shown(), 1);
}

#[test]
fn says_nothing_during_a_short_turn() {
    let mut h = Harness::new();
    h.advance(DEFAULT_GRACE);
    h.controller.on_turn_start();
    h.advance(DEFAULT_STREAMING_DELAY - SECOND);
    h.controller.on_turn_end();
    h.advance(DEFAULT_STREAMING_DELAY);
    assert_eq!(h.shown(), 0);
}

#[test]
fn never_takes_the_band_from_a_notification_the_user_caused() {
    let mut h = Harness::new();
    h.advance(DEFAULT_GRACE);
    h.band_free.set(false);
    h.controller.on_activity();
    h.advance(DEFAULT_IDLE_DELAY);
    assert_eq!(h.shown(), 0);
}

#[test]
fn does_not_pounce_the_moment_the_band_frees_up() {
    let mut h = Harness::new();
    h.advance(DEFAULT_GRACE);
    h.band_free.set(false);
    h.controller.on_activity();
    h.advance(DEFAULT_IDLE_DELAY);
    h.band_free.set(true);
    h.advance(SECOND);
    assert_eq!(h.shown(), 0);
    assert_eq!(h.controller.deadline(), None);
}

#[test]
fn respects_the_cooldown_between_tips() {
    let mut h = Harness::new();
    h.advance(DEFAULT_GRACE);
    h.controller.on_activity();
    h.advance(DEFAULT_IDLE_DELAY);
    assert_eq!(h.shown(), 1);

    h.controller.on_activity();
    h.advance(DEFAULT_IDLE_DELAY);
    assert_eq!(h.shown(), 1);

    h.controller.on_activity();
    h.advance(DEFAULT_COOLDOWN);
    assert_eq!(h.shown(), 2);
}

#[test]
fn goes_quiet_as_soon_as_the_setting_is_turned_off() {
    let mut h = Harness::new();
    h.advance(DEFAULT_GRACE);
    h.enabled.set(false);
    h.controller.on_activity();
    h.advance(DEFAULT_IDLE_DELAY);
    assert_eq!(h.shown(), 0);
}

#[test]
fn shows_nothing_after_stop() {
    let mut h = Harness::new();
    h.advance(DEFAULT_GRACE);
    h.controller.on_activity();
    h.controller.stop();
    h.advance(DEFAULT_IDLE_DELAY);
    assert_eq!(h.shown(), 0);
}

#[test]
fn deadline_is_the_earliest_armed_timer() {
    let mut h = Harness::new();
    assert_eq!(h.controller.deadline(), None);
    h.controller.on_activity();
    let idle = h.controller.deadline().unwrap();
    h.controller.on_turn_start();
    let stream = h.controller.deadline().unwrap();
    assert!(stream < idle);
    h.controller.on_turn_end();
    assert!(h.controller.deadline().unwrap() > stream);
}

#[test]
fn tip_ttl_grows_with_the_body_and_caps_at_twelve_seconds() {
    assert_eq!(tip_ttl(0), Duration::from_millis(6_000));
    assert_eq!(tip_ttl(2), Duration::from_millis(9_000));
    assert_eq!(tip_ttl(5), Duration::from_millis(12_000));
}
