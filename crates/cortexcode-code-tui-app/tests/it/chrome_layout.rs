//! Port of the pin's `test/chrome-layout.test.ts` (the `Slot` cases are in
//! tui-render, where `Slot` lives).

use std::cell::RefCell;
use std::rc::Rc;

use cortexcode_code_tui_app::chrome_layout::*;
use cortexcode_tui_components::Text;
use cortexcode_tui_render::Slot;

fn quiet(density: ChromeDensity) -> ChromeInputs {
    ChromeInputs {
        density,
        autocomplete_open: false,
    }
}

#[test]
fn resolves_each_stop() {
    use FooterLayout as F;
    use TasksLayout as T;
    assert_eq!(
        resolve_chrome(quiet(ChromeDensity::Full)),
        ChromeLayout {
            footer: F::Full,
            tasks: T::Full
        }
    );
    assert_eq!(
        resolve_chrome(quiet(ChromeDensity::Compact)),
        ChromeLayout {
            footer: F::Line,
            tasks: T::Summary
        }
    );
    assert_eq!(
        resolve_chrome(quiet(ChromeDensity::Bare)),
        ChromeLayout {
            footer: F::Hidden,
            tasks: T::Hidden
        }
    );
}

#[test]
fn lends_the_footers_rows_to_an_open_completion_list_at_every_stop() {
    for density in ChromeDensity::ALL {
        let layout = resolve_chrome(ChromeInputs {
            density: *density,
            autocomplete_open: true,
        });
        assert_eq!(layout.footer, FooterLayout::Hidden, "{density}");
    }
}

#[test]
fn keeps_the_whole_ledger_at_full_whatever_else_is_happening() {
    for autocomplete_open in [false, true] {
        assert_eq!(
            resolve_chrome(ChromeInputs {
                density: ChromeDensity::Full,
                autocomplete_open
            })
            .tasks,
            TasksLayout::Full
        );
    }
}

#[test]
fn accepts_the_stops_and_nothing_else() {
    for density in ChromeDensity::ALL {
        assert_eq!(ChromeDensity::parse(density.as_str()), Some(*density));
    }
    for other in ["", "hidden", "FULL"] {
        assert_eq!(ChromeDensity::parse(other), None);
    }
}

struct Setup {
    controller: ChromeLayoutController,
    footer_slot: Rc<RefCell<Slot>>,
    tasks_slot: Rc<RefCell<Slot>>,
    footer_densities: Rc<RefCell<Vec<FooterLayout>>>,
}

fn setup(density: ChromeDensity) -> Setup {
    let slot = |t: &str| {
        Rc::new(RefCell::new(Slot::new(Rc::new(RefCell::new(Text::new(
            t, 0, 0,
        ))))))
    };
    let footer_slot = slot("footer");
    let tasks_slot = slot("tasks");
    let footer_densities = Rc::new(RefCell::new(Vec::new()));
    let sink = footer_densities.clone();
    let mut controller = ChromeLayoutController::new(
        ChromeSurfaces {
            footer_slot: footer_slot.clone(),
            tasks_slot: tasks_slot.clone(),
            set_footer_density: Box::new(move |d| sink.borrow_mut().push(d)),
            set_tasks_density: Box::new(|_| {}),
        },
        density,
    );
    controller.apply();
    Setup {
        controller,
        footer_slot,
        tasks_slot,
        footer_densities,
    }
}

fn visible(s: &Setup) -> [bool; 2] {
    [
        s.footer_slot.borrow().visible(),
        s.tasks_slot.borrow().visible(),
    ]
}

#[test]
fn moves_the_slots_to_match_the_stop() {
    let mut s = setup(ChromeDensity::Full);
    assert_eq!(visible(&s), [true, true]);
    s.controller.set_density(ChromeDensity::Compact);
    assert_eq!(visible(&s), [true, true]);
    s.controller.set_density(ChromeDensity::Bare);
    assert_eq!(visible(&s), [false, false]);
}

#[test]
fn steps_and_wraps_like_every_other_dial() {
    let mut s = setup(ChromeDensity::Full);
    assert_eq!(s.controller.cycle_density(true), ChromeDensity::Compact);
    assert_eq!(s.controller.cycle_density(true), ChromeDensity::Bare);
    assert_eq!(s.controller.cycle_density(true), ChromeDensity::Full);
    assert_eq!(s.controller.cycle_density(false), ChromeDensity::Bare);
}

#[test]
fn reports_nothing_to_do_when_nothing_moved() {
    let mut s = setup(ChromeDensity::Full);
    assert!(s.controller.set_autocomplete_open(true));
    assert!(!s.controller.set_autocomplete_open(true));
    assert!(!s.controller.set_density(ChromeDensity::Full));
}

#[test]
fn sets_a_slots_density_before_showing_it() {
    let mut s = setup(ChromeDensity::Full);
    s.controller.set_density(ChromeDensity::Compact);
    assert_eq!(
        s.footer_densities.borrow().last(),
        Some(&FooterLayout::Line)
    );
    s.controller.set_density(ChromeDensity::Full);
    assert_eq!(
        s.footer_densities.borrow().last(),
        Some(&FooterLayout::Full)
    );
}

#[test]
fn gives_the_footer_back_when_the_completion_list_closes() {
    let mut s = setup(ChromeDensity::Full);
    s.controller.set_autocomplete_open(true);
    assert!(!s.footer_slot.borrow().visible());
    s.controller.set_autocomplete_open(false);
    assert!(s.footer_slot.borrow().visible());
}

#[test]
fn restores_the_stop_the_dial_was_on_not_the_one_the_transient_input_implied() {
    let mut s = setup(ChromeDensity::Compact);
    s.controller.set_autocomplete_open(true);
    s.controller.set_autocomplete_open(false);
    assert!(s.footer_slot.borrow().visible());
    assert_eq!(s.controller.layout().footer, FooterLayout::Line);
}
