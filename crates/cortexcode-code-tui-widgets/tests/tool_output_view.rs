//! Port of the pin's `test/tool-output-view.test.ts`.

mod support;

use cortexcode_ai_types::Content;
use cortexcode_code_tui_widgets::tool_output_view::{
    cycle_tool_output_view, is_tool_output_view, ToolOutputView, DEFAULT_TOOL_OUTPUT_VIEW,
    LEGACY_TOOL_OUTPUT_VIEWS, MAX_TOOL_OUTPUT_VIEW, TOOL_OUTPUT_VIEWS,
};
use cortexcode_code_tui_widgets::tool_signal::{
    render_tool_signal_line, tool_signal, tool_subject, ToolResult, ToolSignalInput,
};
use serde_json::json;
use support::{lock, strip};

use ToolOutputView::{Full, Peek, Radar};

mod tool_output_view_dial {
    use super::*;

    #[test]
    fn cycles_forward_and_backward_wrapping_at_both_ends() {
        assert_eq!(TOOL_OUTPUT_VIEWS, [Radar, Peek, Full]);
        assert_eq!(cycle_tool_output_view(Radar, true), Peek);
        assert_eq!(cycle_tool_output_view(Peek, true), Full);
        assert_eq!(cycle_tool_output_view(Full, true), Radar);
        assert_eq!(cycle_tool_output_view(Radar, false), Full);
        assert_eq!(cycle_tool_output_view(Full, false), Peek);
    }

    #[test]
    fn recognises_its_own_values_and_nothing_else() {
        assert!(is_tool_output_view(Some("peek")));
        assert!(!is_tool_output_view(Some("glance")));
        assert!(!is_tool_output_view(None));
    }

    #[test]
    fn maps_every_retired_value_and_leaves_the_live_ones_alone() {
        assert_eq!(
            LEGACY_TOOL_OUTPUT_VIEWS,
            [("collapsed", Radar), ("glance", Peek), ("standard", Full)]
        );
        assert!(!LEGACY_TOOL_OUTPUT_VIEWS.iter().any(|(k, _)| *k == "peek"));
    }

    #[test]
    fn the_jump_keys_target_is_the_top_of_the_dial() {
        assert_eq!(MAX_TOOL_OUTPUT_VIEW, *TOOL_OUTPUT_VIEWS.last().unwrap());
        assert!(TOOL_OUTPUT_VIEWS.contains(&DEFAULT_TOOL_OUTPUT_VIEW));
        assert_ne!(DEFAULT_TOOL_OUTPUT_VIEW, MAX_TOOL_OUTPUT_VIEW);
    }
}

fn cwd() -> String {
    std::env::current_dir()
        .unwrap()
        .to_string_lossy()
        .into_owned()
}

fn text_result(text: &str, is_error: bool) -> ToolResult {
    ToolResult {
        content: vec![Content::text(text)],
        details: json!({}),
        is_error,
    }
}

mod radar_signal_row {
    use super::*;

    #[test]
    fn picks_the_most_identifying_argument_as_the_subject() {
        let cwd = cwd();
        assert_eq!(
            tool_subject(&json!({"command": "npm run check"}), &cwd),
            "npm run check"
        );
        assert_eq!(
            tool_subject(&json!({"file_path": "src/a.ts", "limit": 20}), &cwd),
            "src/a.ts"
        );
        assert_eq!(
            tool_subject(&json!({"pattern": "*.test.ts", "path": "src"}), &cwd),
            "*.test.ts"
        );
        assert_eq!(
            tool_subject(&json!({"somethingCustom": "value"}), &cwd),
            "value"
        );
        assert_eq!(tool_subject(&json!({"count": 3}), &cwd), "");
        assert_eq!(tool_subject(&serde_json::Value::Null, &cwd), "");
    }

    #[test]
    fn collapses_a_multi_line_command_onto_one_line() {
        assert_eq!(
            tool_subject(&json!({"command": "cat <<EOF\nline\nEOF"}), &cwd()),
            "cat <<EOF line EOF"
        );
    }

    #[test]
    fn shortens_a_path_inside_the_working_directory() {
        let cwd = cwd();
        assert_eq!(
            tool_subject(&json!({"file_path": format!("{cwd}/src/main.ts")}), &cwd),
            "src/main.ts"
        );
    }

    #[test]
    fn reports_output_size_outcome_or_error_as_the_signal() {
        let base = ToolSignalInput {
            tool_name: "bash".into(),
            args: json!({}),
            cwd: cwd(),
            ..Default::default()
        };
        let with = |result: Option<ToolResult>, is_partial: bool| ToolSignalInput {
            result,
            is_partial,
            ..base.clone()
        };
        assert_eq!(
            tool_signal(&with(Some(text_result("a\nb\nc", false)), false)),
            ("3 lines".into(), "success")
        );
        assert_eq!(
            tool_signal(&with(Some(text_result("a", false)), false)),
            ("1 line".into(), "success")
        );
        assert_eq!(
            tool_signal(&with(
                Some(ToolResult {
                    content: vec![],
                    details: json!({}),
                    is_error: false
                }),
                false
            )),
            ("ok".into(), "success")
        );
        assert_eq!(
            tool_signal(&with(Some(text_result("boom", true)), false)),
            ("error".into(), "error")
        );
        assert_eq!(
            tool_signal(&with(None, true)),
            ("running".into(), "warning")
        );
    }

    fn row(tool_name: &str, command: &str, out: &str, width: usize) -> String {
        strip(&render_tool_signal_line(
            &ToolSignalInput {
                tool_name: tool_name.into(),
                args: json!({ "command": command }),
                cwd: cwd(),
                result: Some(text_result(out, false)),
                is_partial: false,
                show_images: false,
                is_latest: false,
            },
            width,
        ))
    }

    #[test]
    fn aligns_the_tool_column_and_keeps_the_row_within_its_width() {
        let _g = lock();
        let bash = row("bash", "npm run check", "a\nb", 80);
        let websearch = row("websearch", "npm run check", "a\nb", 80);
        assert_eq!(bash.find("npm run check"), websearch.find("npm run check"));
        assert!(bash.ends_with("2 lines"));
        assert!(bash.chars().count() <= 80);
    }

    #[test]
    fn truncates_a_long_subject_rather_than_pushing_out_the_signal() {
        let _g = lock();
        let row = row("bash", &"x".repeat(400), "a", 60);
        assert!(row.chars().count() <= 60);
        assert!(row.ends_with("1 line"));
    }
}
