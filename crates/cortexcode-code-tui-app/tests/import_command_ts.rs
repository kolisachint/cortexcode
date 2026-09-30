//! Port of the pin's `test/interactive-mode-import-command.test.ts`:
//! `/import` and `/export` path arguments (`getPathArgument`) and the missing
//! file message. The confirm-then-import flow (the TS cases driving
//! `handleImport` with a mocked runtime host) runs for real in the L2
//! scenario `export-import`, including the missing-file error.

use cortexcode_code_agent_session::RuntimeError;
use cortexcode_code_tui_app::interactive_mode::command_path_argument;

#[test]
fn strips_quotes_from_import_path_arguments() {
    assert_eq!(
        command_path_argument("/import \"path/to/session.jsonl\"", "/import").as_deref(),
        Some("path/to/session.jsonl")
    );
    assert_eq!(
        command_path_argument("/import \"path with spaces/session.jsonl\"", "/import").as_deref(),
        Some("path with spaces/session.jsonl")
    );
}

#[test]
fn preserves_apostrophes_in_unquoted_import_path_arguments() {
    assert_eq!(
        command_path_argument("/import john's/session.jsonl", "/import").as_deref(),
        Some("john's/session.jsonl")
    );
}

#[test]
fn enforces_command_token_boundaries() {
    assert_eq!(
        command_path_argument("/important /tmp/session.jsonl", "/import"),
        None
    );
    assert_eq!(command_path_argument("/exporter out.html", "/export"), None);
    assert_eq!(
        command_path_argument("/import /tmp/session.jsonl", "/import").as_deref(),
        Some("/tmp/session.jsonl")
    );
}

#[test]
fn a_bare_command_or_an_unclosed_quote_has_no_path() {
    assert_eq!(command_path_argument("/import", "/import"), None);
    assert_eq!(command_path_argument("/import   ", "/import"), None);
    assert_eq!(command_path_argument("/import \"open", "/import"), None);
}

#[test]
fn a_missing_import_file_reads_as_a_non_fatal_error() {
    let error = RuntimeError::ImportFileNotFound("/tmp/missing-session.jsonl".into());
    assert_eq!(
        format!("Failed to import session: {error}"),
        "Failed to import session: File not found: /tmp/missing-session.jsonl"
    );
}
