//! Port of `test/session-selector-search.test.ts`.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use cortexcode_code_session::SessionInfo;
use cortexcode_code_tui_selectors::session_selector_search::{
    filter_and_sort_sessions, NameFilter, SortMode,
};

fn date(s: &str) -> DateTime<Utc> {
    s.parse().unwrap()
}

fn make(id: &str, modified: &str, text: &str, name: Option<&str>) -> SessionInfo {
    SessionInfo {
        path: PathBuf::from(format!("/tmp/{id}.jsonl")),
        id: id.to_string(),
        cwd: String::new(),
        name: name.map(str::to_string),
        color: None,
        branch: None,
        parent_session_path: None,
        created: DateTime::<Utc>::UNIX_EPOCH,
        modified: date(modified),
        message_count: 1,
        first_message: "(no messages)".to_string(),
        all_messages_text: text.to_string(),
    }
}

fn ids(sessions: &[SessionInfo]) -> Vec<&str> {
    sessions.iter().map(|s| s.id.as_str()).collect()
}

#[test]
fn filters_by_quoted_phrase_with_whitespace_normalization() {
    let sessions = vec![
        make(
            "a",
            "2026-01-01T00:00:00Z",
            "node\n\n   cve was discussed",
            None,
        ),
        make("b", "2026-01-02T00:00:00Z", "node something else", None),
    ];
    let result =
        filter_and_sort_sessions(&sessions, "\"node cve\"", SortMode::Recent, NameFilter::All);
    assert_eq!(ids(&result), ["a"]);
}

#[test]
fn filters_by_regex_case_insensitive() {
    let sessions = vec![
        make("a", "2026-01-02T00:00:00Z", "Brave is great", None),
        make("b", "2026-01-03T00:00:00Z", "bravery is not the same", None),
    ];
    let result = filter_and_sort_sessions(
        &sessions,
        "re:\\bbrave\\b",
        SortMode::Recent,
        NameFilter::All,
    );
    assert_eq!(ids(&result), ["a"]);
}

#[test]
fn recent_sort_preserves_input_order() {
    let sessions = vec![
        make("newer", "2026-01-03T00:00:00Z", "brave", None),
        make("older", "2026-01-01T00:00:00Z", "brave", None),
        make("nomatch", "2026-01-04T00:00:00Z", "something else", None),
    ];
    let result =
        filter_and_sort_sessions(&sessions, "\"brave\"", SortMode::Recent, NameFilter::All);
    assert_eq!(ids(&result), ["newer", "older"]);
}

#[test]
fn relevance_sort_orders_by_score_then_modified_desc() {
    let sessions = vec![
        make("late", "2026-01-03T00:00:00Z", "xxxx brave", None),
        make("early", "2026-01-01T00:00:00Z", "brave xxxx", None),
    ];
    let result =
        filter_and_sort_sessions(&sessions, "\"brave\"", SortMode::Relevance, NameFilter::All);
    assert_eq!(ids(&result), ["early", "late"]);

    let tie = vec![
        make("newer", "2026-01-03T00:00:00Z", "brave", None),
        make("older", "2026-01-01T00:00:00Z", "brave", None),
    ];
    let result = filter_and_sort_sessions(&tie, "\"brave\"", SortMode::Relevance, NameFilter::All);
    assert_eq!(ids(&result), ["newer", "older"]);
}

#[test]
fn returns_empty_for_invalid_regex() {
    let sessions = vec![make("a", "2026-01-01T00:00:00Z", "brave", None)];
    let result = filter_and_sort_sessions(&sessions, "re:(", SortMode::Recent, NameFilter::All);
    assert!(result.is_empty());
}

fn named_sessions() -> Vec<SessionInfo> {
    vec![
        make(
            "named1",
            "2026-01-03T00:00:00Z",
            "blueberry",
            Some("My Project"),
        ),
        make(
            "named2",
            "2026-01-02T00:00:00Z",
            "blueberry",
            Some("Another Named"),
        ),
        make("other1", "2026-01-04T00:00:00Z", "blueberry", None),
        make("other2", "2026-01-01T00:00:00Z", "blueberry", None),
    ]
}

#[test]
fn name_filter_all_returns_everything() {
    let result = filter_and_sort_sessions(&named_sessions(), "", SortMode::Recent, NameFilter::All);
    assert_eq!(ids(&result), ["named1", "named2", "other1", "other2"]);
}

#[test]
fn name_filter_named_returns_named_only() {
    let result =
        filter_and_sort_sessions(&named_sessions(), "", SortMode::Recent, NameFilter::Named);
    assert_eq!(ids(&result), ["named1", "named2"]);
}

#[test]
fn name_filter_applies_before_query() {
    let result = filter_and_sort_sessions(
        &named_sessions(),
        "blueberry",
        SortMode::Recent,
        NameFilter::Named,
    );
    assert_eq!(ids(&result), ["named1", "named2"]);
}

#[test]
fn named_filter_excludes_whitespace_only_names() {
    let sessions = vec![
        make("whitespace", "2026-01-01T00:00:00Z", "test", Some("   ")),
        make("empty", "2026-01-02T00:00:00Z", "test", Some("")),
        make("named", "2026-01-03T00:00:00Z", "test", Some("Real Name")),
    ];
    let result = filter_and_sort_sessions(&sessions, "", SortMode::Recent, NameFilter::Named);
    assert_eq!(ids(&result), ["named"]);
}
