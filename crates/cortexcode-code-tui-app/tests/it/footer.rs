//! The footer: a golden of hoocode's rendering for a range of states
//! (`fixtures/footer-gold.json`, captured from the pinned build with a stub
//! session), plus ports of `footer-width.test.ts`, the footer half of
//! `startup-progress.test.ts`, and `subagent-footer.test.ts`.

use crate::support::{lock, plain};
use cortexcode_code_tui_app::footer::*;
use cortexcode_code_tui_app::footer_data::FooterDataProvider;
use cortexcode_code_tui_app::startup_progress::{self, StartupProgress};
use cortexcode_tui_render::Component;
use cortexcode_tui_util::visible_width;
use serde_json::Value;

#[derive(Clone, Default)]
struct Stub {
    name: String,
    model: Option<FooterModel>,
    thinking: String,
    usage: (u64, u64, u64, u64, f64),
    ctx: Option<(u64, Option<f64>)>,
    cwd: String,
    reserve: u64,
    oauth: bool,
}

impl FooterSource for Stub {
    fn usage_totals(&self) -> (u64, u64, u64, u64, f64) {
        self.usage
    }
    fn context_usage(&self) -> Option<(u64, Option<f64>)> {
        self.ctx
    }
    fn model(&self) -> Option<FooterModel> {
        self.model.clone()
    }
    fn thinking_level(&self) -> String {
        self.thinking.clone()
    }
    fn cwd(&self) -> String {
        self.cwd.clone()
    }
    fn display_name(&self) -> String {
        self.name.clone()
    }
    fn reserve_tokens(&self) -> u64 {
        self.reserve
    }
    fn is_using_oauth(&self) -> bool {
        self.oauth
    }
}

fn stub_from(so: &Value) -> Stub {
    let cw = so["cw"].as_u64().unwrap_or(128_000);
    let usage = &so["usage"];
    Stub {
        name: so["name"].as_str().unwrap_or("amber-harbor").into(),
        model: (!so["noModel"].as_bool().unwrap_or(false)).then(|| FooterModel {
            id: so["modelId"].as_str().unwrap_or("mock-model").into(),
            provider: so["provider"].as_str().unwrap_or("mock").into(),
            context_window: cw,
            reasoning: so["reasoning"].as_bool().unwrap_or(false),
        }),
        thinking: so["thinking"].as_str().unwrap_or("off").into(),
        usage: (
            usage["input"].as_u64().unwrap_or(0),
            usage["output"].as_u64().unwrap_or(0),
            usage["cacheRead"].as_u64().unwrap_or(0),
            usage["cacheWrite"].as_u64().unwrap_or(0),
            usage["cost"]["total"].as_f64().unwrap_or(0.0),
        ),
        ctx: match so.get("ctx") {
            None => Some((cw, Some(0.0))),
            Some(Value::Null) => None,
            Some(ctx) => Some((
                ctx["contextWindow"].as_u64().unwrap(),
                ctx["percent"].as_f64(),
            )),
        },
        cwd: so["cwd"].as_str().unwrap_or("/tmp/project").into(),
        reserve: so["reserve"].as_u64().unwrap_or(16_384),
        oauth: so["oauth"].as_bool().unwrap_or(false),
    }
}

/// A provider whose cwd is a repo on `branch` (or no repo).
fn data_for(dir: &tempfile::TempDir, branch: Option<&str>) -> FooterDataProvider {
    if let Some(branch) = branch {
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        std::fs::write(
            dir.path().join(".git/HEAD"),
            format!("ref: refs/heads/{branch}\n"),
        )
        .unwrap();
    }
    FooterDataProvider::new(dir.path())
}

#[test]
fn renders_every_state_like_hoocode() {
    let _g = lock(Some("dark"));
    let gold: Value = serde_json::from_str(include_str!("../fixtures/footer-gold.json")).unwrap();
    for (name, case) in gold.as_object().unwrap() {
        startup_progress::clear();
        let (so, d, opts) = (&case["so"], &case["dO"], &case["opts"]);
        if opts["startup"].as_bool() == Some(true) {
            startup_progress::set(StartupProgress::Download {
                key: "fd".into(),
                label: "fd".into(),
                received_bytes: 3_670_016,
                total_bytes: Some(8_388_608),
            });
            startup_progress::set(StartupProgress::Download {
                key: "rg".into(),
                label: "ripgrep".into(),
                received_bytes: 1_048_576,
                total_bytes: None,
            });
            startup_progress::set(StartupProgress::Work {
                key: "idx".into(),
                label: "Building semantic search index".into(),
                done: 120,
                total: 480,
                unit: "files".into(),
            });
            startup_progress::set(StartupProgress::Error {
                key: "e".into(),
                label: "Semantic search index unavailable".into(),
                message: "embsearch binary not found (PATH or embsearchBinaryPath setting)".into(),
            });
        }
        let dir = tempfile::tempdir().unwrap();
        let data = data_for(&dir, d["branch"].as_str());
        data.set_available_provider_count(d["providers"].as_u64().unwrap_or(1) as usize);
        data.set_active_mode(d["mode"].as_str().unwrap_or("build"));
        if let Some(statuses) = d["statuses"].as_array() {
            for s in statuses {
                data.set_extension_status(s[0].as_str().unwrap(), s[1].as_str());
            }
        }
        let mut footer = FooterComponent::new(Box::new(stub_from(so)), data.clone());
        if let Some(chip) = opts["chip"].as_bool() {
            footer.set_session_chip_shown(chip);
        }
        if let Some(view) = opts["view"].as_str() {
            footer.set_tool_output_view(ToolOutputView::parse(view).unwrap());
        }
        if opts["auto"].as_bool() == Some(false) {
            footer.set_auto_compact_enabled(false);
        }
        if opts["density"].as_str() == Some("line") {
            footer.set_density(FooterDensity::Line);
        }
        let width = case["width"].as_u64().unwrap() as u16;
        let expected: Vec<String> = serde_json::from_value(case["lines"].clone()).unwrap();
        assert_eq!(footer.render(width), expected, "{name}");
        data.dispose();
    }
    startup_progress::clear();
}

fn footer(stub: Stub, providers: usize) -> FooterComponent {
    let data = FooterDataProvider::new(std::env::temp_dir().join("cortex-footer-no-repo"));
    data.set_available_provider_count(providers);
    FooterComponent::new(Box::new(stub), data)
}

fn stub(name: &str) -> Stub {
    Stub {
        name: if name.is_empty() {
            "amber-harbor".into()
        } else {
            name.into()
        },
        model: Some(FooterModel {
            id: "test-model".into(),
            provider: "test".into(),
            context_window: 200_000,
            reasoning: false,
        }),
        thinking: "off".into(),
        ctx: Some((200_000, Some(12.3))),
        cwd: "/tmp/project".into(),
        reserve: 16_384,
        ..Default::default()
    }
}

#[test]
fn keeps_all_lines_within_width_for_wide_session_names() {
    let _g = lock(None);
    let mut f = footer(stub(&"한글".repeat(30)), 1);
    for line in f.render(93) {
        assert!(visible_width(&line) <= 93);
    }
}

#[test]
fn hands_the_session_name_to_the_chip_once_the_box_is_wide_enough() {
    let _g = lock(None);
    let mut f = footer(stub("refactor-auth"), 1);
    f.set_session_chip_shown(true);
    assert!(!plain(&f.render(120)[0]).contains("refactor-auth"));
    assert!(plain(&f.render(40)[0]).contains("• refactor"));
}

#[test]
fn shows_the_auto_assigned_name_when_no_chip_is_set() {
    let _g = lock(None);
    assert!(plain(&footer(stub(""), 1).render(120)[0]).contains("amber-harbor"));
}

#[test]
fn keeps_stats_line_within_width_for_wide_model_and_provider_names() {
    let _g = lock(None);
    let mut s = stub("");
    s.model = Some(FooterModel {
        id: "模".repeat(30),
        provider: "공급자".into(),
        context_window: 200_000,
        reasoning: true,
    });
    s.thinking = "high".into();
    s.usage = (12_345, 6_789, 0, 0, 1.234);
    for line in footer(s, 2).render(60) {
        assert!(visible_width(&line) <= 60);
    }
}

#[test]
fn renders_without_task_related_content() {
    let _g = lock(Some("dark"));
    let out = footer(stub(""), 1).render(120).join("\n");
    assert!(!out.contains("[subagent:"));
    assert!(!out.contains("#1 "));
}

fn last_line(f: &mut FooterComponent, width: u16) -> String {
    plain(f.render(width).last().unwrap())
}

#[test]
fn startup_progress_lines_render_after_the_footer_in_order_and_clamped() {
    let _g = lock(Some("dark"));
    let mut f = footer(stub(""), 1);
    let base = f.render(120).len();
    startup_progress::set(StartupProgress::Download {
        key: "fd".into(),
        label: "fd".into(),
        received_bytes: 3_670_016,
        total_bytes: Some(8_388_608),
    });
    assert_eq!(f.render(120).len(), base + 1);
    let line = last_line(&mut f, 120);
    assert!(line.contains("3.5 MB / 8.0 MB") && line.contains("44%"));

    startup_progress::set(StartupProgress::Download {
        key: "rg".into(),
        label: "ripgrep".into(),
        received_bytes: 1_048_576,
        total_bytes: None,
    });
    let line = last_line(&mut f, 120);
    assert!(line.contains("ripgrep") && line.contains("1.0 MB") && !line.contains('%'));

    startup_progress::set(StartupProgress::Error {
        key: "semantic-index".into(),
        label: "Semantic search index unavailable".into(),
        message: "binary not found".into(),
    });
    let lines: Vec<String> = f.render(120).iter().map(|l| plain(l)).collect();
    assert_eq!(lines.len(), base + 3);
    assert!(lines[base].contains("fd") && lines[base + 1].contains("ripgrep"));
    assert!(lines[base + 2].contains("Semantic search index unavailable: binary not found"));

    startup_progress::set(StartupProgress::Download {
        key: "long".into(),
        label: "a-very-long-tool-name-".repeat(20),
        received_bytes: 5_000_000,
        total_bytes: Some(9_000_000),
    });
    assert!(last_line(&mut f, 60).chars().count() <= 60);
    startup_progress::clear();
}
