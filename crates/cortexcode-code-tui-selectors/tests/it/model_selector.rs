//! Port of `suite/regressions/3217-scoped-model-order.test.ts`, plus the
//! pickers' own behaviour (no TS tests of their own).

use cortexcode_ai_types::Model;
use cortexcode_code_tui_selectors::model_selector::{ModelSelectorComponent, ModelSelectorEvent};
use cortexcode_code_tui_selectors::scoped_models_selector::{
    clear_all, enable_all, move_id, toggle, ScopedModelsEvent, ScopedModelsSelectorComponent,
};
use cortexcode_code_tui_theme::SELECT_CURSOR;
use cortexcode_tui_render::Component;
use serde_json::json;

use crate::support::{lock, strip};

fn model(provider: &str, id: &str, name: &str) -> Model {
    serde_json::from_value(json!({
        "id": id, "name": name, "api": "openai-completions", "provider": provider,
        "baseUrl": "https://example.test", "reasoning": true, "input": ["text"],
        "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0},
        "contextWindow": 128000, "maxTokens": 8192,
    }))
    .unwrap()
}

fn faux() -> Vec<Model> {
    vec![
        model("faux", "faux-1", "One"),
        model("faux", "faux-2", "Two"),
        model("faux", "faux-3", "Three"),
    ]
}

fn full_ids(models: &[Model]) -> Vec<String> {
    models
        .iter()
        .map(|m| format!("{}/{}", m.provider, m.id))
        .collect()
}

/// The model id on each list row, in order.
fn listed_ids(rendered: &str, provider: &str) -> Vec<String> {
    rendered
        .lines()
        .filter(|line| line.contains(&format!("[{provider}]")))
        .map(|line| {
            let trimmed = line.trim();
            let trimmed = trimmed.strip_prefix('│').unwrap_or(trimmed);
            let trimmed = trimmed.strip_suffix('│').unwrap_or(trimmed).trim();
            let trimmed = trimmed.strip_prefix(SELECT_CURSOR).unwrap_or(trimmed);
            trimmed.split(" [").next().unwrap().trim().to_string()
        })
        .collect()
}

#[test]
fn propagates_reordered_scoped_models_back_to_the_session_state() {
    let _g = lock();
    let models = faux();
    let ordered = full_ids(&models);
    let mut selector = ScopedModelsSelectorComponent::new(models, Some(ordered.clone()));
    selector.handle_input("\x1b[1;3B");
    assert_eq!(
        selector.take_events(),
        vec![ScopedModelsEvent::Change(Some(vec![
            ordered[1].clone(),
            ordered[0].clone(),
            ordered[2].clone(),
        ]))]
    );
}

#[test]
fn preserves_scoped_model_order_in_the_model_scoped_tab() {
    let _g = lock();
    let models = faux();
    let (one, two, three) = (models[0].clone(), models[1].clone(), models[2].clone());
    let mut selector = ModelSelectorComponent::new(
        Some(one.clone()),
        Ok(models),
        None,
        vec![two.clone(), one.clone(), three.clone()],
        None,
    );
    let rendered = strip(&selector.render(120));
    let ids = listed_ids(&rendered, "faux");
    assert_eq!(ids[..3], [two.id, one.id, three.id]);
    assert!(rendered.contains("Scope: all | scoped"));
}

#[test]
fn lists_the_current_model_first_ticked_then_by_provider() {
    let _g = lock();
    let models = vec![
        model("zeta", "z-1", "Z"),
        model("alpha", "a-1", "A"),
        model("mid", "m-1", "M"),
    ];
    let current = models[2].clone();
    let mut selector = ModelSelectorComponent::new(Some(current), Ok(models), None, vec![], None);
    let rendered = strip(&selector.render(100));
    assert!(rendered.contains("Only showing models from configured providers"));
    assert!(rendered.contains("m-1 [mid] ✓"));
    assert!(rendered.contains("Model Name: M"));
    let order: Vec<_> = rendered
        .lines()
        .filter_map(|l| ["z-1", "a-1", "m-1"].into_iter().find(|id| l.contains(id)))
        .collect();
    assert_eq!(order, ["m-1", "a-1", "z-1"]);
}

#[test]
fn typing_narrows_and_enter_selects_the_highlighted_model() {
    let _g = lock();
    let models = faux();
    let mut selector =
        ModelSelectorComponent::new(Some(models[0].clone()), Ok(models), None, vec![], None);
    selector.handle_input("2");
    let rendered = strip(&selector.render(100));
    assert_eq!(listed_ids(&rendered, "faux"), ["faux-2"]);
    selector.handle_input("\r");
    match selector.take_events().as_slice() {
        [ModelSelectorEvent::Select(m)] => assert_eq!(m.id, "faux-2"),
        other => panic!("unexpected {other:?}"),
    }
    selector.handle_input("\x1b");
    assert_eq!(selector.take_events(), vec![ModelSelectorEvent::Cancel]);
}

#[test]
fn up_and_down_wrap_and_tab_switches_scope() {
    let _g = lock();
    let models = faux();
    let mut selector = ModelSelectorComponent::new(
        Some(models[0].clone()),
        Ok(models.clone()),
        None,
        vec![models[2].clone()],
        None,
    );
    // Scoped first: only faux-3.
    assert_eq!(
        listed_ids(&strip(&selector.render(100)), "faux"),
        ["faux-3"]
    );
    selector.handle_input("\t");
    let rendered = strip(&selector.render(100));
    assert_eq!(listed_ids(&rendered, "faux").len(), 3);
    selector.handle_input("\x1b[A");
    assert!(strip(&selector.render(100)).contains("Model Name: Three"));
    selector.handle_input("\x1b[B");
    assert!(strip(&selector.render(100)).contains("Model Name: One"));
}

#[test]
fn an_unlistable_registry_shows_its_error() {
    let _g = lock();
    let mut selector =
        ModelSelectorComponent::new(None, Err("boom\nsecond".into()), None, vec![], None);
    let rendered = strip(&selector.render(100));
    assert!(rendered.contains("boom"));
    assert!(rendered.contains("second"));
    assert!(!rendered.contains("No matching models"));
}

#[test]
fn toggle_from_all_enabled_keeps_only_that_model_and_marks_unsaved() {
    let _g = lock();
    let models = faux();
    let ids = full_ids(&models);
    let mut selector = ScopedModelsSelectorComponent::new(models, None);
    let rendered = strip(&selector.render(120));
    // "option+s" on macOS, like the pin's formatKeyText.
    let save = cortexcode_code_tui_keybindings::format_key_text("alt+s", false);
    assert!(rendered.contains(&format!("Session-only. {save} to save to settings.")));
    assert!(rendered.contains("all enabled"));
    selector.handle_input("\r");
    assert_eq!(
        selector.take_events(),
        vec![ScopedModelsEvent::Change(Some(vec![ids[0].clone()]))]
    );
    let rendered = strip(&selector.render(200));
    assert!(rendered.contains("1/3 enabled (unsaved)"));
    assert!(rendered.contains("faux-2 [faux] ✗"));
    // alt+s saves and clears the mark.
    selector.handle_input("\x1bs");
    assert_eq!(
        selector.take_events(),
        vec![ScopedModelsEvent::Persist(Some(vec![ids[0].clone()]))]
    );
    assert!(!strip(&selector.render(200)).contains("(unsaved)"));
}

#[test]
fn ctrl_c_clears_the_search_before_cancelling() {
    let _g = lock();
    let mut selector = ScopedModelsSelectorComponent::new(faux(), None);
    selector.handle_input("x");
    selector.handle_input("y");
    assert!(strip(&selector.render(100)).contains("No matching models"));
    selector.handle_input("\x03");
    assert!(selector.take_events().is_empty());
    assert!(!strip(&selector.render(100)).contains("No matching models"));
    selector.handle_input("\x03");
    assert_eq!(selector.take_events(), vec![ScopedModelsEvent::Cancel]);
}

#[test]
fn enabled_set_helpers_follow_the_original() {
    let all: Vec<String> = ["a", "b", "c"].iter().map(|s| s.to_string()).collect();
    let s = |v: &[&str]| Some(v.iter().map(|x| x.to_string()).collect::<Vec<_>>());
    assert_eq!(toggle(&None, "b"), s(&["b"]));
    assert_eq!(toggle(&s(&["a", "b"]), "a"), s(&["b"]));
    assert_eq!(toggle(&s(&["a"]), "c"), s(&["a", "c"]));
    assert_eq!(enable_all(&None, &all, None), None);
    assert_eq!(enable_all(&s(&["a"]), &all, None), None);
    assert_eq!(
        enable_all(&s(&["a"]), &all, Some(&["c".to_string()])),
        s(&["a", "c"])
    );
    assert_eq!(clear_all(&None, &all, None), s(&[]));
    assert_eq!(
        clear_all(&None, &all, Some(&["b".to_string()])),
        s(&["a", "c"])
    );
    assert_eq!(clear_all(&s(&["a", "b"]), &all, None), s(&[]));
    assert_eq!(move_id(&s(&["a", "b"]), "b", -1), s(&["b", "a"]));
    assert_eq!(move_id(&s(&["a", "b"]), "a", -1), s(&["a", "b"]));
    assert_eq!(move_id(&None, "a", 1), None);
}
