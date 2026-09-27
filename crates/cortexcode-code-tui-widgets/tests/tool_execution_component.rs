//! Port of the pin's `test/tool-execution-component.test.ts`.
//!
//! Not here: "uses built-in rendering for built-in overrides without custom
//! renderers" needs the edit renderer (task 11.2d, the diff view), and "bash
//! execute emits an initial empty partial update" tests the bash tool itself
//! (in `cortexcode-code-tool-bash/tests/bash_tool.rs`).

mod support;

use std::cell::RefCell;
use std::rc::Rc;

use cortexcode_ai_types::Content;
use cortexcode_code_tui_widgets::tool_execution::{
    RenderShell, ToolExecutionComponent, ToolExecutionOptions, ToolRenderDefinition,
};
use cortexcode_code_tui_widgets::tool_output_view::{ToolOutputView, PEEK_LINES};
use cortexcode_code_tui_widgets::tool_signal::ToolResult;
use cortexcode_code_tui_widgets::tools;
use cortexcode_tui_components::Text;
use cortexcode_tui_render::{Component, ComponentHandle};
use cortexcode_tui_util::visible_width;
use serde_json::{json, Value};
use support::{lock, strip, strip_all};

use ToolOutputView::{Full, Peek, Radar};

fn cwd() -> String {
    std::env::current_dir()
        .unwrap()
        .to_string_lossy()
        .into_owned()
}

fn text(s: impl Into<String>) -> ComponentHandle {
    Rc::new(RefCell::new(Text::new(s, 0, 0)))
}

fn call_renderer(
    s: &'static str,
) -> Option<cortexcode_code_tui_widgets::tool_execution::RenderCallFn> {
    Some(Rc::new(move |_, _| Ok(text(s))))
}

fn result_renderer(
    s: &'static str,
) -> Option<cortexcode_code_tui_widgets::tool_execution::RenderResultFn> {
    Some(Rc::new(move |_, _, _| Ok(text(s))))
}

/// `createBaseToolDefinition()`: a registered tool with no renderers.
fn base() -> ToolRenderDefinition {
    ToolRenderDefinition::default()
}

fn component(
    name: &str,
    id: &str,
    args: Value,
    view: ToolOutputView,
    definition: Option<ToolRenderDefinition>,
) -> ToolExecutionComponent {
    ToolExecutionComponent::new(
        name,
        id,
        args,
        ToolExecutionOptions {
            view,
            ..Default::default()
        },
        definition,
        &cwd(),
    )
}

fn result(text: &str, is_error: bool) -> ToolResult {
    ToolResult {
        content: vec![Content::text(text)],
        details: json!({}),
        is_error,
    }
}

fn lines_of(c: &mut ToolExecutionComponent, width: u16) -> Vec<String> {
    c.render(width).iter().map(|l| strip(l)).collect()
}

mod parity {
    use super::*;

    #[test]
    fn renders_the_status_dot_inline_with_the_first_call_line_not_on_its_own_line() {
        let _g = lock();
        let def = ToolRenderDefinition {
            render_call: call_renderer("$ npm run check"),
            ..base()
        };
        let mut c = component("custom_tool", "tool-dot", json!({}), Full, Some(def));
        let lines = lines_of(&mut c, 120);
        let dot_line = lines.iter().find(|l| l.contains('●')).expect("dot line");
        assert!(dot_line.contains("$ npm run check"));
        assert!(!lines.iter().any(|l| l.trim() == "●"));
    }

    #[test]
    fn prefixes_the_dot_on_the_first_line_and_indents_wrapped_continuation_lines() {
        let _g = lock();
        let def = ToolRenderDefinition {
            render_call: call_renderer(
                "$ cd /Users/example/very/long/path && npm run check 2>&1 | tail -6",
            ),
            ..base()
        };
        let mut c = component("custom_tool", "tool-dot-wrap", json!({}), Full, Some(def));
        let lines = lines_of(&mut c, 30);
        let dot = lines.iter().position(|l| l.contains('●')).unwrap();
        assert!(lines[dot].contains("$ cd"));
        let continuation = &lines[dot + 1];
        assert!(continuation.starts_with("  "));
        assert!(!continuation.contains('●'));
    }

    #[test]
    fn stacks_custom_call_and_result_renderers_like_the_old_implementation() {
        let _g = lock();
        let def = ToolRenderDefinition {
            render_call: call_renderer("custom call"),
            render_result: result_renderer("custom result"),
            ..base()
        };
        let mut c = component("custom_tool", "tool-1", json!({}), Full, Some(def));
        assert!(strip_all(&c.render(120)).contains("custom call"));
        c.update_result(result("done", false), false);
        let rendered = strip_all(&c.render(120));
        assert!(rendered.contains("custom call"));
        assert!(rendered.contains("custom result"));
    }

    #[test]
    fn preserves_legacy_file_path_rendering_compatibility_for_built_in_tools() {
        let _g = lock();
        let mut c = component(
            "read",
            "tool-3",
            json!({"file_path": "README.md"}),
            Full,
            None,
        );
        let rendered = strip_all(&c.render(120));
        assert!(rendered.contains("read"));
        assert!(rendered.contains("README.md"));
    }

    #[test]
    fn does_not_duplicate_built_in_headers_when_passed_the_active_built_in_definition() {
        let _g = lock();
        let mut c = component(
            "read",
            "tool-4",
            json!({"path": "README.md"}),
            Full,
            Some(tools::read::definition()),
        );
        c.update_result(result("hello", false), false);
        let rendered = strip_all(&c.render(120));
        let reads = rendered
            .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
            .filter(|w| *w == "read")
            .count();
        assert_eq!(reads, 1, "{rendered}");
    }

    #[test]
    fn inherits_missing_built_in_result_renderer_slot_from_the_built_in_tool() {
        let _g = lock();
        let def = ToolRenderDefinition {
            render_call: call_renderer("override call"),
            ..base()
        };
        let mut c = component(
            "read",
            "tool-4b",
            json!({"path": "notes.txt"}),
            Full,
            Some(def),
        );
        c.update_result(result("hello", false), false);
        let rendered = strip_all(&c.render(120));
        assert!(rendered.contains("override call"));
        assert!(rendered.contains("hello"));
    }

    #[test]
    fn inherits_missing_built_in_call_renderer_slot_from_the_built_in_tool() {
        let _g = lock();
        let def = ToolRenderDefinition {
            render_result: result_renderer("override result"),
            ..base()
        };
        let mut c = component(
            "read",
            "tool-4c",
            json!({"path": "README.md"}),
            Full,
            Some(def),
        );
        c.update_result(result("hello", false), false);
        let rendered = strip_all(&c.render(120));
        assert!(rendered.contains("read"));
        assert!(rendered.contains("README.md"));
        assert!(rendered.contains("override result"));
    }

    #[test]
    fn uses_custom_renderers_for_built_in_overrides_that_reuse_built_in_definition_parameters() {
        let _g = lock();
        let def = ToolRenderDefinition {
            render_call: call_renderer("override call"),
            render_result: result_renderer("override result"),
            ..tools::read::definition()
        };
        let mut c = component(
            "read",
            "tool-4d",
            json!({"path": "README.md"}),
            Full,
            Some(def),
        );
        c.update_result(result("hello", false), false);
        let rendered = strip_all(&c.render(120));
        assert!(rendered.contains("override call"));
        assert!(rendered.contains("override result"));
        assert!(!rendered.contains("read README.md"));
    }

    #[test]
    fn uses_custom_renderers_for_built_in_overrides_that_reuse_wrapped_built_in_tool_parameters() {
        // Parameters are not part of the rendering half; a definition that
        // only brings its own renderers is the same case.
        let _g = lock();
        let def = ToolRenderDefinition {
            render_call: call_renderer("wrapped override call"),
            render_result: result_renderer("wrapped override result"),
            ..base()
        };
        let mut c = component(
            "read",
            "tool-4e",
            json!({"path": "README.md"}),
            Full,
            Some(def),
        );
        c.update_result(result("hello", false), false);
        let rendered = strip_all(&c.render(120));
        assert!(rendered.contains("wrapped override call"));
        assert!(rendered.contains("wrapped override result"));
    }

    #[test]
    fn shares_renderer_state_across_custom_call_and_result_slots() {
        let _g = lock();
        let def = ToolRenderDefinition {
            render_call: Some(Rc::new(|_, ctx| {
                let token = ctx
                    .state
                    .entry("token")
                    .or_insert_with(|| json!("shared-token"))
                    .as_str()
                    .unwrap()
                    .to_string();
                Ok(text(format!("custom call {token}")))
            })),
            render_result: Some(Rc::new(|_, _, ctx| {
                let token = ctx.state.get("token").and_then(Value::as_str).unwrap_or("");
                Ok(text(format!("custom result {token}")))
            })),
            ..base()
        };
        let mut c = component("custom_tool", "tool-5", json!({}), Full, Some(def));
        c.update_result(result("done", false), false);
        let rendered = strip_all(&c.render(120));
        assert!(rendered.contains("custom call shared-token"));
        assert!(rendered.contains("custom result shared-token"));
    }

    #[test]
    fn exposes_args_in_render_result_context() {
        let _g = lock();
        let def = ToolRenderDefinition {
            render_call: call_renderer("call"),
            render_result: Some(Rc::new(|_, _, ctx| {
                Ok(text(format!(
                    "arg:{}",
                    ctx.args.get("foo").and_then(Value::as_str).unwrap_or("")
                )))
            })),
            ..base()
        };
        let mut c = component(
            "custom_tool",
            "tool-5b",
            json!({"foo": "bar"}),
            Full,
            Some(def),
        );
        c.update_result(result("done", false), false);
        assert!(strip_all(&c.render(120)).contains("arg:bar"));
    }

    #[test]
    fn falls_back_when_custom_renderers_are_absent() {
        let _g = lock();
        let mut c = component(
            "custom_tool",
            "tool-6",
            json!({"foo": "bar"}),
            Full,
            Some(base()),
        );
        c.update_result(result("done", false), false);
        let rendered = strip_all(&c.render(120));
        assert!(rendered.contains("custom_tool"));
        assert!(rendered.contains("done"));
    }

    #[test]
    fn trims_trailing_blank_display_lines_from_write_previews() {
        let _g = lock();
        let mut c = component(
            "write",
            "tool-7",
            json!({"path": "README.md", "content": "one\ntwo\n"}),
            Full,
            Some(tools::write::definition()),
        );
        let rendered = strip_all(&c.render(120));
        assert!(rendered.contains("one"));
        assert!(rendered.contains("two"));
        assert!(!rendered.contains("two\n\n"));
    }

    #[test]
    fn trims_trailing_blank_display_lines_from_read_results() {
        let _g = lock();
        let mut c = component(
            "read",
            "tool-8",
            json!({"path": "notes.txt"}),
            Full,
            Some(tools::read::definition()),
        );
        c.update_result(result("one\ntwo\n", false), false);
        let rendered = strip_all(&c.render(120));
        assert!(rendered.contains("one"));
        assert!(rendered.contains("two"));
        assert!(!rendered.contains("two\n\n"));
    }

    struct Compact {
        title: &'static str,
        path: String,
        content: &'static str,
        compact: String,
        hidden: &'static str,
        absent: Option<&'static str>,
    }

    fn compact_scenarios() -> Vec<Compact> {
        let cwd = cwd();
        let outside =
            std::path::absolute(std::path::Path::new(&cwd).join("..").join("AGENTS.md")).unwrap();
        let outside = cortexcode_code_paths::format_path_relative_to_cwd_or_absolute(
            &outside,
            std::path::Path::new(&cwd),
        );
        vec![
            Compact {
                title: "SKILL.md",
                path: format!("{cwd}/attio/SKILL.md"),
                content:
                    "---\nname: attio\ndescription: CRM helper\n---\n\n# Hidden skill instructions",
                compact: "[skill] attio".into(),
                hidden: "Hidden skill instructions",
                absent: Some("read skill attio"),
            },
            Compact {
                title: "AGENTS.md",
                path: format!("{cwd}/.hoocode/AGENTS.md"),
                content: "Hidden resource instructions",
                compact: "read resource .hoocode/AGENTS.md".into(),
                hidden: "Hidden resource instructions",
                absent: None,
            },
            Compact {
                title: "outside AGENTS.md",
                path: format!("{cwd}/../AGENTS.md"),
                content: "Hidden outside resource instructions",
                compact: format!("read resource {outside}"),
                hidden: "Hidden outside resource instructions",
                absent: None,
            },
            Compact {
                title: "documentation",
                path: tools::read::readme_path().to_string_lossy().into_owned(),
                content: "Hidden docs content",
                compact: "read docs README.md".into(),
                hidden: "Hidden docs content",
                absent: None,
            },
        ]
    }

    #[test]
    fn renders_compact_read_results_until_expanded() {
        let _g = lock();
        for s in compact_scenarios() {
            let mut c = component(
                "read",
                &format!("tool-compact-{}", s.title),
                json!({"path": s.path}),
                Peek,
                Some(tools::read::definition()),
            );
            c.update_result(result(s.content, false), false);
            let collapsed = strip_all(&c.render(120));
            assert!(collapsed.contains(&s.compact), "{}: {collapsed}", s.title);
            assert!(!collapsed.contains(s.hidden), "{}", s.title);
            if let Some(absent) = s.absent {
                assert!(!collapsed.contains(absent), "{}", s.title);
            }
            c.set_view(Full);
            assert!(strip_all(&c.render(120)).contains(s.hidden), "{}", s.title);
        }
    }

    #[test]
    fn shows_the_read_line_range_in_compact_reads_before_the_expand_hint() {
        let _g = lock();
        let cwd = cwd();
        for (path, compact) in [
            (format!("{cwd}/attio/SKILL.md"), "[skill] attio:120-329"),
            (
                tools::read::readme_path().to_string_lossy().into_owned(),
                "read docs README.md:120-329",
            ),
        ] {
            let mut c = component(
                "read",
                "tool-compact-range",
                json!({"path": path, "offset": 120, "limit": 210}),
                Peek,
                Some(tools::read::definition()),
            );
            let collapsed = strip_all(&c.render(120));
            assert!(collapsed.contains(compact), "{collapsed}");
            assert!(collapsed.find(":120-329").unwrap() < collapsed.find("to expand").unwrap());
        }
    }
}

mod view_dial {
    use super::*;

    fn build(view: ToolOutputView, args: Value) -> ToolExecutionComponent {
        let def = ToolRenderDefinition {
            render_call: call_renderer("custom call"),
            render_result: Some(Rc::new(|_, options, _| {
                Ok(text(if options.expanded {
                    "RESULT BODY"
                } else {
                    "TRIMMED BODY"
                }))
            })),
            ..base()
        };
        let mut c = component("custom_tool", "view", args, view, Some(def));
        c.update_result(result("one\ntwo\nthree", false), false);
        c
    }

    #[test]
    fn full_shows_the_whole_result_body() {
        let _g = lock();
        let rendered = strip_all(&build(Full, json!({})).render(120));
        assert!(rendered.contains("custom call"));
        assert!(rendered.contains("RESULT BODY"));
    }

    #[test]
    fn peek_shows_the_call_line_and_a_trimmed_body_with_no_disclosure_caret() {
        let _g = lock();
        let mut c = build(Peek, json!({}));
        let trimmed = strip_all(&c.render(120));
        assert!(trimmed.contains("custom call"));
        assert!(trimmed.contains("TRIMMED BODY"));
        assert!(!trimmed.contains("RESULT BODY"));
        assert!(!trimmed.contains('▸'));
        assert!(!trimmed.contains('▾'));
        c.set_view(Full);
        assert!(strip_all(&c.render(120)).contains("RESULT BODY"));
    }

    #[test]
    fn a_failure_shows_why_it_failed_in_every_view_unasked() {
        let _g = lock();
        for view in [Radar, Peek, Full] {
            let def = ToolRenderDefinition {
                render_call: call_renderer("custom call"),
                render_result: result_renderer("WHY IT BROKE"),
                ..base()
            };
            let mut c = component(
                "custom_tool",
                "err",
                json!({"command": "bun test"}),
                view,
                Some(def),
            );
            c.update_result(result("boom", true), false);
            assert!(
                strip_all(&c.render(120)).contains("WHY IT BROKE"),
                "{view:?}"
            );
        }
    }

    #[test]
    fn radar_replaces_the_call_renderer_with_a_signal_row() {
        let _g = lock();
        let rendered = strip_all(&build(Radar, json!({"command": "npm run check"})).render(120));
        assert!(!rendered.contains("custom call"));
        assert!(!rendered.contains("RESULT BODY"));
        assert!(rendered.contains("custom_to"));
        assert!(rendered.contains("npm run check"));
        assert!(rendered.contains("3 lines"));
    }

    #[test]
    fn a_radar_row_stays_a_row_until_the_dial_itself_moves() {
        let _g = lock();
        let mut c = build(Radar, json!({"command": "npm run check"}));
        assert!(strip_all(&c.render(120)).contains("npm run check"));
        c.set_view(Full);
        let opened = strip_all(&c.render(120));
        assert!(opened.contains("custom call"));
        assert!(opened.contains("RESULT BODY"));
        c.set_view(Radar);
        assert!(strip_all(&c.render(120)).contains("npm run check"));
    }

    #[test]
    fn radar_rows_stack_without_the_separator_the_other_views_keep() {
        let _g = lock();
        let radar = build(Radar, json!({"command": "npm run check"})).render(120);
        assert!(radar.iter().all(|l| !strip(l).trim().is_empty()));
        let peek = build(Peek, json!({"command": "npm run check"})).render(120);
        assert_eq!(strip(&peek[0]).trim(), "");
    }

    #[test]
    fn radar_gives_a_self_rendering_tool_the_same_row_framing_as_every_other() {
        let _g = lock();
        let def = ToolRenderDefinition {
            render_shell: Some(RenderShell::SelfRendered),
            render_call: call_renderer("custom call"),
            render_result: result_renderer("RESULT BODY"),
        };
        let mut c = component(
            "self_tool",
            "view-radar-self",
            json!({"command": "npm run check"}),
            Radar,
            Some(def),
        );
        c.update_result(result("a\nb", false), false);
        let self_row = strip_all(&c.render(120));
        let plain_row = strip_all(&build(Radar, json!({"command": "npm run check"})).render(120));
        assert_eq!(
            self_row.find("npm run check"),
            plain_row.find("npm run check")
        );
        assert_eq!(self_row.chars().count(), plain_row.chars().count());
        c.set_view(Full);
        assert!(strip_all(&c.render(120)).contains("RESULT BODY"));
    }

    #[test]
    fn a_tool_with_no_renderer_of_its_own_is_trimmed_to_the_same_peek_budget() {
        let _g = lock();
        let lines: Vec<String> = (1..=PEEK_LINES + 4).map(|i| format!("line {i}")).collect();
        let def = ToolRenderDefinition {
            render_call: call_renderer("custom call"),
            ..base()
        };
        let mut c = component(
            "renderless_tool",
            "view-fallback",
            json!({"command": "npm run check"}),
            Peek,
            Some(def),
        );
        c.update_result(result(&lines.join("\n"), false), false);
        let trimmed = strip_all(&c.render(120));
        assert!(trimmed.contains(&format!("line {PEEK_LINES}")));
        assert!(!trimmed.contains(&format!("line {}", PEEK_LINES + 1)));
        assert!(trimmed.contains("4 more lines"));
        c.set_view(Full);
        assert!(strip_all(&c.render(120)).contains(&format!("line {}", PEEK_LINES + 4)));
    }

    #[test]
    fn set_view_switches_an_existing_block_live() {
        let _g = lock();
        let mut c = build(Full, json!({}));
        assert!(strip_all(&c.render(120)).contains("RESULT BODY"));
        c.set_view(Peek);
        assert!(!strip_all(&c.render(120)).contains("RESULT BODY"));
        c.set_view(Full);
        assert!(strip_all(&c.render(120)).contains("RESULT BODY"));
    }
}

mod freeze {
    use super::*;

    fn finished(id: &str) -> ToolExecutionComponent {
        let mut c = component(
            "read",
            id,
            json!({"path": "README.md"}),
            Full,
            Some(tools::read::definition()),
        );
        c.update_result(result("hello world output", false), false);
        c
    }

    #[test]
    fn a_finished_block_is_freezable_a_partial_one_is_not() {
        let _g = lock();
        let mut c = component(
            "read",
            "freeze-partial",
            json!({"path": "README.md"}),
            Full,
            Some(tools::read::definition()),
        );
        c.update_result(result("partial", false), true);
        assert!(!c.is_freezable());
        c.update_result(result("final", false), false);
        assert!(c.is_freezable());
    }

    #[test]
    fn freezing_preserves_the_rendered_output_and_makes_the_block_immutable() {
        let _g = lock();
        let mut c = finished("freeze-preserve");
        let before = c.render(120);
        assert!(strip_all(&before).contains("README.md"));
        c.freeze();
        assert_eq!(c.render(120), before);
        assert!(!c.is_freezable());
        c.update_result(result("SHOULD NOT APPEAR", false), false);
        c.invalidate();
        let after = strip_all(&c.render(120));
        assert!(!after.contains("SHOULD NOT APPEAR"));
        assert_eq!(after, strip_all(&before));
    }

    #[test]
    fn a_frozen_block_never_emits_a_line_wider_than_the_terminal() {
        let _g = lock();
        let mut c = finished("freeze-narrow");
        c.freeze();
        let snapshot = strip_all(&c.render(120));
        for width in [80u16, 40, 20, 10] {
            for line in c.render(width) {
                assert!(visible_width(&line) <= width as usize);
            }
        }
        assert_eq!(strip_all(&c.render(120)), snapshot);
    }
}
