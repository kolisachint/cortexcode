//! Port of `tree-selector.test.ts`, plus the tree case of
//! `picker-widths.test.ts`.

mod support;

use cortexcode_code_session::{FileEntry, SessionTreeNode};
use cortexcode_code_tui_selectors::tree_selector::TreeSelectorComponent;
use cortexcode_code_tui_theme::theme;
use cortexcode_tui_keys::get_keybindings;
use cortexcode_tui_render::Component;
use cortexcode_tui_util::visible_width;
use serde_json::{json, Value};

use support::lock;

/// Raw input for a tree verb, derived from its binding (they have moved
/// before; spelling the sequence out would keep passing on the old layout).
fn alt_key(id: &str) -> String {
    // `lock()` installs the default app keybindings.
    let key = get_keybindings()
        .get_keys(id)
        .into_iter()
        .next()
        .unwrap_or_default();
    let ch = key
        .strip_prefix("alt+")
        .filter(|rest| rest.chars().count() == 1)
        .unwrap_or_else(|| panic!("expected an alt+<char> binding for {id}, got {key}"));
    format!("\x1b{ch}")
}

fn filter_default() -> String {
    alt_key("app.tree.filter.default")
}
fn filter_user_only() -> String {
    alt_key("app.tree.filter.userOnly")
}
fn filter_labeled_only() -> String {
    alt_key("app.tree.filter.labeledOnly")
}
fn alt_t() -> String {
    alt_key("app.tree.toggleLabelTimestamp")
}

const TS: &str = "2026-01-01T00:00:00.000Z";

fn entry(value: Value) -> FileEntry {
    serde_json::from_value(value).expect("a session entry")
}

fn user_message(id: &str, parent: Option<&str>, content: &str) -> FileEntry {
    entry(json!({
        "type": "message", "id": id, "parentId": parent, "timestamp": TS,
        "message": {"role": "user", "content": content, "timestamp": 0}
    }))
}

fn assistant(id: &str, parent: Option<&str>, content: Value, stop: &str) -> FileEntry {
    entry(json!({
        "type": "message", "id": id, "parentId": parent, "timestamp": TS,
        "message": {
            "role": "assistant", "content": content,
            "api": "anthropic-messages", "provider": "anthropic", "model": "claude-sonnet-4.5",
            "usage": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 0,
                      "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0}},
            "stopReason": stop, "timestamp": 0
        }
    }))
}

fn assistant_message(id: &str, parent: Option<&str>, text: &str) -> FileEntry {
    assistant(id, parent, json!([{"type": "text", "text": text}]), "stop")
}

/// Tool calls only: hidden in the default view.
fn tool_call_only_assistant(id: &str, parent: Option<&str>) -> FileEntry {
    assistant(
        id,
        parent,
        json!([{"type": "toolCall", "id": format!("tc-{id}"), "name": "read", "arguments": {"path": "test.ts"}}]),
        "toolUse",
    )
}

fn model_change(id: &str, parent: Option<&str>) -> FileEntry {
    entry(json!({
        "type": "model_change", "id": id, "parentId": parent, "timestamp": TS,
        "provider": "anthropic", "modelId": "claude-sonnet-4.5"
    }))
}

fn thinking_change(id: &str, parent: Option<&str>) -> FileEntry {
    entry(json!({
        "type": "thinking_level_change", "id": id, "parentId": parent, "timestamp": TS,
        "thinkingLevel": "high"
    }))
}

/// Parent links into a forest (`buildTree`).
fn build_tree(entries: Vec<FileEntry>) -> Vec<SessionTreeNode> {
    fn children_of(entries: &[FileEntry], parent: Option<&str>) -> Vec<SessionTreeNode> {
        entries
            .iter()
            .filter(|e| e.parent_id() == parent)
            .map(|e| SessionTreeNode {
                entry: e.clone(),
                children: children_of(entries, e.id()),
                label: None,
                label_timestamp: None,
            })
            .collect()
    }
    children_of(&entries, None)
}

fn selector(tree: &[SessionTreeNode], leaf: &str) -> TreeSelectorComponent {
    TreeSelectorComponent::new(tree, Some(leaf), 24, None, None)
}

fn selected(s: &TreeSelectorComponent) -> Option<String> {
    s.tree_list().borrow().selected_entry_id()
}

fn press(s: &mut TreeSelectorComponent, key: &str) {
    s.handle_input(key);
}

#[test]
fn focuses_nearest_visible_ancestor_when_the_leaf_is_a_model_change_with_a_sibling_branch() {
    let _g = lock();
    let tree = build_tree(vec![
        user_message("user-1", None, "hello"),
        assistant_message("asst-1", Some("user-1"), "hi"),
        user_message("user-2", Some("asst-1"), "active branch"),
        model_change("model-1", Some("user-2")),
        user_message("user-3", Some("asst-1"), "sibling branch"),
    ]);
    // user-2 (parent of model-1), not user-3 (the last row).
    assert_eq!(
        selected(&selector(&tree, "model-1")).as_deref(),
        Some("user-2")
    );
}

#[test]
fn focuses_nearest_visible_ancestor_when_the_leaf_is_a_thinking_level_change() {
    let _g = lock();
    let tree = build_tree(vec![
        user_message("user-1", None, "hello"),
        assistant_message("asst-1", Some("user-1"), "hi"),
        user_message("user-2", Some("asst-1"), "active branch"),
        thinking_change("thinking-1", Some("user-2")),
        user_message("user-3", Some("asst-1"), "sibling branch"),
    ]);
    assert_eq!(
        selected(&selector(&tree, "thinking-1")).as_deref(),
        Some("user-2")
    );
}

fn branching_pair() -> Vec<SessionTreeNode> {
    build_tree(vec![
        user_message("user-1", None, "hello"),
        assistant_message("asst-1", Some("user-1"), "hi"),
        user_message("user-2", Some("asst-1"), "active branch"),
        assistant_message("asst-2", Some("user-2"), "response"),
        user_message("user-3", Some("asst-1"), "sibling branch"),
    ])
}

#[test]
fn switches_to_the_nearest_visible_user_message_on_the_user_only_filter() {
    let _g = lock();
    let tree = branching_pair();
    let mut s = selector(&tree, "asst-2");
    assert_eq!(selected(&s).as_deref(), Some("asst-2"));
    press(&mut s, &filter_user_only());
    // The parent user message, not user-3.
    assert_eq!(selected(&s).as_deref(), Some("user-2"));
}

#[test]
fn returns_to_the_nearest_visible_ancestor_when_switching_back_to_default() {
    let _g = lock();
    let tree = branching_pair();
    let mut s = selector(&tree, "asst-2");
    press(&mut s, &filter_user_only());
    assert_eq!(selected(&s).as_deref(), Some("user-2"));
    press(&mut s, &filter_default());
    assert_eq!(selected(&s).as_deref(), Some("user-2"));
}

fn labeled_pair(first: &str) -> Vec<SessionTreeNode> {
    use chrono::TimeZone;
    let mut tree = build_tree(vec![
        user_message("user-1", None, first),
        assistant_message("asst-1", Some("user-1"), "hi"),
    ]);
    let at = chrono::Local
        .with_ymd_and_hms(2026, 3, 28, 14, 32, 0)
        .unwrap()
        .with_timezone(&chrono::Utc);
    tree[0].label = Some("checkpoint".into());
    tree[0].label_timestamp = Some(at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true));
    tree
}

fn list_render(s: &TreeSelectorComponent) -> String {
    s.tree_list().borrow_mut().render(200).join("\n")
}

#[test]
fn toggles_label_timestamps_for_labeled_nodes() {
    let _g = lock();
    let tree = labeled_pair("hello");
    let mut s = selector(&tree, "asst-1");
    let render = list_render(&s);
    assert!(render.contains("[checkpoint]"));
    assert!(!render.contains("3/28 14:32"));
    assert!(!render.contains("[+label time]"));

    press(&mut s, &alt_t());
    let render = list_render(&s);
    assert!(render.contains("3/28 14:32"));
    assert!(render.contains("[timestamps]"));
}

#[test]
fn types_an_uppercase_letter_into_the_search_instead_of_firing_a_verb() {
    // shift+letter verbs arrived as the bare uppercase letter on legacy
    // terminals, so searching for "TODO" toggled timestamps.
    let _g = lock();
    let tree = labeled_pair("TODO ship it");
    let mut s = selector(&tree, "asst-1");
    for ch in ["T", "L"] {
        press(&mut s, ch);
    }
    let render = list_render(&s);
    assert!(!s.is_editing_label());
    assert!(!render.contains("[timestamps]"));
    assert!(!render.contains("3/28 14:32"));
}

#[test]
fn preserves_selection_when_switching_to_an_empty_labeled_filter_and_back() {
    let _g = lock();
    let tree = build_tree(vec![
        user_message("user-1", None, "hello"),
        assistant_message("asst-1", Some("user-1"), "hi"),
        user_message("user-2", Some("asst-1"), "bye"),
        assistant_message("asst-2", Some("user-2"), "goodbye"),
    ]);
    let mut s = selector(&tree, "asst-2");
    assert_eq!(selected(&s).as_deref(), Some("asst-2"));
    press(&mut s, &filter_labeled_only());
    assert_eq!(selected(&s), None);
    press(&mut s, &filter_default());
    assert_eq!(selected(&s).as_deref(), Some("asst-2"));
}

#[test]
fn preserves_selection_through_multiple_empty_filter_switches() {
    let _g = lock();
    let tree = build_tree(vec![
        user_message("user-1", None, "hello"),
        assistant_message("asst-1", Some("user-1"), "hi"),
    ]);
    let mut s = selector(&tree, "asst-1");
    assert_eq!(selected(&s).as_deref(), Some("asst-1"));
    press(&mut s, &filter_labeled_only());
    assert_eq!(selected(&s), None);
    press(&mut s, &filter_labeled_only()); // toggles back to default
    assert_eq!(selected(&s).as_deref(), Some("asst-1"));
    press(&mut s, &filter_labeled_only());
    assert_eq!(selected(&s), None);
    press(&mut s, &filter_default());
    assert_eq!(selected(&s).as_deref(), Some("asst-1"));
}

const UP: &str = "\x1b[A";
const DOWN: &str = "\x1b[B";
const CTRL_LEFT: &str = "\x1b[1;5D";
const CTRL_RIGHT: &str = "\x1b[1;5C";
const ALT_LEFT: &str = "\x1b[1;3D";
const ALT_RIGHT: &str = "\x1b[1;3C";

/// user-1 … asst-2, which branches into A (user-3a … asst-4a, active) and
/// B (user-3b … user-4b).
fn branching_tree() -> Vec<SessionTreeNode> {
    build_tree(vec![
        user_message("user-1", None, "first message"),
        assistant_message("asst-1", Some("user-1"), "response 1"),
        user_message("user-2", Some("asst-1"), "second message"),
        assistant_message("asst-2", Some("user-2"), "response 2"),
        user_message("user-3a", Some("asst-2"), "branch A start"),
        assistant_message("asst-3a", Some("user-3a"), "branch A response"),
        user_message("user-4a", Some("asst-3a"), "branch A deep"),
        assistant_message("asst-4a", Some("user-4a"), "branch A leaf"),
        user_message("user-3b", Some("asst-2"), "branch B start"),
        assistant_message("asst-3b", Some("user-3b"), "branch B response"),
        user_message("user-4b", Some("asst-3b"), "branch B deep"),
    ])
}

/// Press each key and check where the selection lands.
fn walk(s: &mut TreeSelectorComponent, steps: &[(&str, &str)]) {
    for (i, (key, expected)) in steps.iter().enumerate() {
        press(s, key);
        assert_eq!(selected(s).as_deref(), Some(*expected), "step {i}");
    }
}

#[test]
fn ctrl_right_unfolds_a_folded_node_then_jumps_segments_when_unfolded() {
    let _g = lock();
    let tree = branching_tree();
    let mut s = selector(&tree, "asst-4a");
    walk(
        &mut s,
        &[
            (CTRL_LEFT, "user-3a"),
            (CTRL_LEFT, "user-3a"), // fold
            (DOWN, "user-3b"),      // children hidden
            (UP, "user-3a"),
            (CTRL_RIGHT, "user-3a"), // unfold
            (DOWN, "asst-3a"),       // children back
            (CTRL_LEFT, "user-3a"),
            (CTRL_RIGHT, "asst-4a"), // segment jump to the leaf
        ],
    );
}

#[test]
fn alt_left_and_right_are_aliases_for_fold_and_unfold_navigation() {
    let _g = lock();
    let tree = branching_tree();
    let mut s = selector(&tree, "asst-4a");
    walk(
        &mut s,
        &[
            (ALT_LEFT, "user-3a"),
            (ALT_LEFT, "user-3a"),
            (ALT_RIGHT, "user-3a"),
            (ALT_RIGHT, "asst-4a"),
        ],
    );
}

#[test]
fn folding_the_root_hides_the_subtree_and_a_nested_fold_survives_unfolding() {
    let _g = lock();
    let tree = branching_tree();
    let mut s = selector(&tree, "asst-4a");
    walk(
        &mut s,
        &[
            (CTRL_LEFT, "user-3a"),
            (CTRL_LEFT, "user-3a"),  // fold user-3a
            (CTRL_LEFT, "user-1"),   // folded → root
            (CTRL_LEFT, "user-1"),   // fold user-1
            (DOWN, "user-1"),        // the only visible row
            (CTRL_RIGHT, "user-1"),  // unfold user-1
            (CTRL_RIGHT, "user-3a"), // segment jump; user-3a still folded
            (DOWN, "user-3b"),
        ],
    );
}

#[test]
fn folds_and_navigates_on_a_non_active_branch() {
    let _g = lock();
    let tree = branching_tree();
    let mut s = selector(&tree, "asst-4a");
    let found = (0..20).any(|_| {
        press(&mut s, DOWN);
        selected(&s).as_deref() == Some("user-3b")
    });
    assert!(found);
    walk(
        &mut s,
        &[
            (CTRL_RIGHT, "user-4b"),
            (CTRL_LEFT, "user-3b"),
            (CTRL_LEFT, "user-3b"), // fold
            (CTRL_LEFT, "user-1"),
        ],
    );
}

#[test]
fn folds_and_navigates_with_multiple_roots() {
    let _g = lock();
    let tree = build_tree(vec![
        user_message("user-1", None, "first root"),
        assistant_message("asst-1", Some("user-1"), "response 1"),
        user_message("user-2", None, "second root"),
        assistant_message("asst-2", Some("user-2"), "response 2"),
    ]);
    let mut s = selector(&tree, "asst-1");
    assert_eq!(selected(&s).as_deref(), Some("asst-1"));
    walk(
        &mut s,
        &[
            (CTRL_LEFT, "user-1"),
            (CTRL_LEFT, "user-1"), // fold
            (DOWN, "user-2"),
            (CTRL_RIGHT, "asst-2"),
            (CTRL_LEFT, "user-2"),
            (CTRL_LEFT, "user-2"), // fold
            (CTRL_LEFT, "user-2"), // a folded root stays
        ],
    );
}

#[test]
fn folding_the_root_hides_descendants_behind_filtered_out_entries() {
    let _g = lock();
    let tree = build_tree(vec![
        user_message("user-1", None, "hello"),
        tool_call_only_assistant("tool-asst-1", Some("user-1")),
        user_message("user-2", Some("tool-asst-1"), "follow up"),
        assistant_message("asst-2", Some("user-2"), "response"),
    ]);
    let mut s = selector(&tree, "asst-2");
    walk(
        &mut s,
        &[
            (CTRL_LEFT, "user-1"),
            (CTRL_LEFT, "user-1"),
            (DOWN, "user-1"),
        ],
    );
}

fn navigate_to(s: &mut TreeSelectorComponent, id: &str) -> Option<String> {
    let mut current = None;
    for _ in 0..20 {
        press(s, DOWN);
        current = selected(s);
        if current.as_deref() == Some(id) {
            break;
        }
    }
    current
}

#[test]
fn search_resets_fold_state() {
    let _g = lock();
    let tree = branching_tree();
    let mut s = selector(&tree, "asst-4a");
    press(&mut s, CTRL_LEFT);
    press(&mut s, CTRL_LEFT); // fold user-3a
    press(&mut s, DOWN);
    assert_eq!(selected(&s).as_deref(), Some("user-3b"));

    press(&mut s, "b"); // search resets folds
    press(&mut s, "\x1b"); // clear the search

    assert_eq!(navigate_to(&mut s, "user-3a").as_deref(), Some("user-3a"));
    press(&mut s, DOWN);
    assert_eq!(selected(&s).as_deref(), Some("asst-3a"));
}

#[test]
fn filter_mode_change_resets_fold_state() {
    let _g = lock();
    let tree = branching_tree();
    let mut s = selector(&tree, "asst-4a");
    press(&mut s, CTRL_LEFT);
    press(&mut s, CTRL_LEFT); // fold user-3a
    press(&mut s, &filter_user_only());
    press(&mut s, &filter_default());

    assert_eq!(navigate_to(&mut s, "user-3a").as_deref(), Some("user-3a"));
    press(&mut s, DOWN);
    assert_eq!(selected(&s).as_deref(), Some("asst-3a"));
}

#[test]
fn fills_the_selected_row_to_the_full_width() {
    let _g = lock();
    let tree = build_tree(vec![
        user_message("user-1", None, "hello"),
        assistant_message("asst-1", Some("user-1"), "hi"),
    ]);
    let s = selector(&tree, "asst-1");
    let width = 120;
    let rendered = s.tree_list().borrow_mut().render(width);
    let band = theme().get_bg_ansi("selectedBg").to_string();
    let banded: Vec<&String> = rendered.iter().filter(|l| l.contains(&band)).collect();
    assert_eq!(banded.len(), 1);
    assert_eq!(visible_width(banded[0]), width as usize);
}

/// picker-widths.test.ts: "session tree fits within %i columns and fills
/// its selected row".
#[test]
fn session_tree_fits_within_the_width_and_fills_its_selected_row() {
    let _g = lock();
    const LONG: &str = "lorem ipsum dolor sit amet consectetur adipiscing elit sed do eiusmod tempor incididunt ut labore";
    let tree = build_tree(vec![
        user_message("user-1", None, LONG),
        assistant_message("asst-1", Some("user-1"), LONG),
    ]);
    let band = theme().get_bg_ansi("selectedBg").to_string();
    for width in [40u16, 80, 160] {
        let mut s = selector(&tree, "asst-1");
        let lines = s.render(width);
        for line in &lines {
            assert!(
                visible_width(line) <= width as usize,
                "{} > {width}: {line}",
                visible_width(line)
            );
        }
        for line in lines.iter().filter(|l| l.contains(&band)) {
            assert_eq!(visible_width(line), width as usize);
        }
    }
}
