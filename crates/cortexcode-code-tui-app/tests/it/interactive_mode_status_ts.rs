//! Port of the `showLoadedResources` cases of the pin's
//! `test/interactive-mode-status.test.ts`, against `show_loaded_resources`.
//! The file's `showRecord` cases are in `record_row.rs`'s tests, its
//! `jumpToFullView` / `setToolsExpanded` cases drive the real interactive
//! mode (`cortexcode-code-cli` runtime tests). Its canvas case (real canvas
//! discovery) and the extension UI context cases (`setTheme`,
//! `addAutocompleteProvider`, autocomplete wrappers) belong to phase 12.

use crate::support::lock;
use cortexcode_code_resources::context_files::{ContextFile, ContextFileSize};
use cortexcode_code_resources::diagnostics::{DiagnosticType, ResourceDiagnostic};
use cortexcode_code_resources::source_info::{SourceInfo, SourceOrigin, SourceScope};
use cortexcode_code_tui_app::resource_display::*;

/// Every child rendered at 220, SGR stripped, trailing spaces gone, trimmed.
fn normalized(listing: &ResourceListing, force: bool, diagnostics_when_quiet: bool) -> String {
    show_loaded_resources(listing, force, diagnostics_when_quiet)
        .into_iter()
        .flat_map(|h| h.borrow_mut().render(220))
        .map(|l| {
            cortexcode_tui_util::strip_vt_control_characters(&l)
                .trim_end()
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

fn render(listing: &ResourceListing) -> String {
    normalized(listing, false, false)
}

fn base(cwd: &str) -> ResourceListing {
    ResourceListing {
        cwd: cwd.into(),
        columns: Some(100),
        ..Default::default()
    }
}

fn skill(name: &str, path: &str) -> ListedItem {
    ListedItem {
        name: name.into(),
        path: path.into(),
        source_info: None,
        display_name: None,
    }
}

fn context(path: &str) -> ContextFile {
    ContextFile {
        path: path.into(),
        content: String::new(),
        tokens: None,
        size: None,
    }
}

fn source(
    path: &str,
    source: &str,
    scope: SourceScope,
    package: bool,
    base_dir: &str,
) -> SourceInfo {
    SourceInfo {
        path: path.into(),
        source: source.into(),
        scope,
        origin: if package {
            SourceOrigin::Package
        } else {
            SourceOrigin::TopLevel
        },
        base_dir: Some(base_dir.into()),
    }
}

fn ext(path: &str, info: Option<SourceInfo>) -> ListedItem {
    ListedItem {
        name: path.into(),
        path: path.into(),
        source_info: info,
        display_name: None,
    }
}

fn local(path: &str, scope: SourceScope, base_dir: &str) -> ListedItem {
    let source_name = if scope == SourceScope::Temporary {
        "cli"
    } else {
        "local"
    };
    ext(
        path,
        Some(source(path, source_name, scope, false, base_dir)),
    )
}

fn package(path: &str, source_name: &str, base_dir: &str) -> ListedItem {
    ext(
        path,
        Some(source(
            path,
            source_name,
            SourceScope::Project,
            true,
            base_dir,
        )),
    )
}

fn extension_fixtures() -> Vec<ListedItem> {
    vec![
        local(
            "/tmp/project/.hoocode/extensions/answer.ts",
            SourceScope::Project,
            "/tmp/project/.hoocode/extensions",
        ),
        local(
            "/tmp/project/.hoocode/extensions/local-index/index.ts",
            SourceScope::Project,
            "/tmp/project/.hoocode/extensions",
        ),
        local(
            "/tmp/agent/extensions/user-index/index.ts",
            SourceScope::User,
            "/tmp/agent/extensions",
        ),
        package(
            "/tmp/project/.hoocode/npm/node_modules/pi-markdown-preview/extensions/index.ts",
            "npm:pi-markdown-preview",
            "/tmp/project/.hoocode/npm/node_modules/pi-markdown-preview",
        ),
        package(
            "/tmp/project/.hoocode/npm/node_modules/@scope/pi-scoped/extensions/index.ts",
            "npm:@scope/pi-scoped",
            "/tmp/project/.hoocode/npm/node_modules/@scope/pi-scoped",
        ),
        package(
            "/tmp/project/.hoocode/git/github.com/HazAT/pi-interactive-subagents/extensions/index.ts",
            "git:github.com/HazAT/pi-interactive-subagents",
            "/tmp/project/.hoocode/git/github.com/HazAT/pi-interactive-subagents",
        ),
        package(
            "/tmp/project/.hoocode/git/github.com/HazAT/pi-interactive-subagents/extensions/subagents/index.ts",
            "git:github.com/HazAT/pi-interactive-subagents",
            "/tmp/project/.hoocode/git/github.com/HazAT/pi-interactive-subagents",
        ),
        local(
            "/tmp/temp/cli-extension.ts",
            SourceScope::Temporary,
            "/tmp/temp",
        ),
    ]
}

fn expanded_with(extensions: Vec<ListedItem>) -> String {
    render(&ResourceListing {
        expanded: true,
        extensions,
        ..base("/tmp/project")
    })
}

#[test]
fn shows_the_counted_summary_no_names_by_default() {
    let _g = lock(Some("dark"));
    let out = render(&ResourceListing {
        skills: vec![skill("commit", "/tmp/skill/SKILL.md")],
        ..base("/tmp/project")
    });
    assert!(out.contains("1 skill"), "{out}");
    assert!(!out.contains("commit"));
    assert!(!out.contains("details"));
}

#[test]
fn shows_the_detailed_listing_when_expanded() {
    let _g = lock(Some("dark"));
    let out = render(&ResourceListing {
        expanded: true,
        skills: vec![skill("commit", "/tmp/skill/SKILL.md")],
        ..base("/tmp/project")
    });
    assert!(out.contains("[Skills]"));
    assert!(out.contains("commit"));
}

#[test]
fn shows_the_detailed_listing_on_verbose_startup_even_when_collapsed() {
    let _g = lock(Some("dark"));
    let out = render(&ResourceListing {
        quiet_startup: true,
        verbose: true,
        skills: vec![skill("commit", "/tmp/skill/SKILL.md")],
        ..base("/tmp/project")
    });
    assert!(out.contains("[Skills]"), "{out}");
    assert!(out.contains("commit"));
}

#[test]
fn summarizes_extensions_as_a_count_labels_live_in_the_expanded_details() {
    let _g = lock(Some("dark"));
    let extensions = vec![
        ext("/tmp/extensions/answer.ts", None),
        ext("/tmp/extensions/btw.ts", None),
    ];
    let out = render(&ResourceListing {
        extensions: extensions.clone(),
        ..base("/tmp/project")
    });
    assert!(out.contains("2 extensions"), "{out}");
    assert!(!out.contains("answer.ts"));
    assert!(expanded_with(extensions).contains("answer.ts, btw.ts"));
}

#[test]
fn summarizes_mixed_extension_layouts_as_a_count_when_collapsed() {
    let _g = lock(Some("dark"));
    let out = render(&ResourceListing {
        extensions: extension_fixtures(),
        ..base("/tmp/project")
    });
    assert!(out.contains("8 extensions"), "{out}");
    assert!(!out.contains("[Extensions]"));
}

#[test]
fn adds_more_parent_folders_until_local_extension_labels_are_unique() {
    let _g = lock(Some("dark"));
    let out = expanded_with(vec![
        local(
            "/tmp/alpha/one/index.ts",
            SourceScope::Temporary,
            "/tmp/alpha",
        ),
        local(
            "/tmp/beta/one/index.ts",
            SourceScope::Temporary,
            "/tmp/beta",
        ),
        local(
            "/tmp/gamma/one/index.ts",
            SourceScope::Temporary,
            "/tmp/gamma",
        ),
    ]);
    assert_eq!(
        out,
        "⊹ 3 extensions\n[Extensions]\n  alpha/one, beta/one, gamma/one\n  path\n    /tmp/alpha/one\n    /tmp/beta/one\n    /tmp/gamma/one"
    );
}

#[test]
fn strips_index_ts_and_index_js_from_local_extension_labels() {
    let _g = lock(Some("dark"));
    for file in ["index.ts", "index.js"] {
        let out = expanded_with(vec![local(
            &format!("/tmp/extensions/plan-mode/{file}"),
            SourceScope::Project,
            "/tmp/extensions",
        )]);
        assert_eq!(
            out,
            "⊹ 1 extension\n[Extensions]\n  plan-mode\n  project\n    /tmp/extensions/plan-mode",
            "{file}"
        );
    }
}

#[test]
fn mixed_single_file_and_subdirectory_index_ts_extensions_strip_index_ts() {
    let _g = lock(Some("dark"));
    let out = expanded_with(vec![
        local(
            "/tmp/extensions/webfetch.ts",
            SourceScope::Project,
            "/tmp/extensions",
        ),
        local(
            "/tmp/extensions/plan-mode/index.ts",
            SourceScope::Project,
            "/tmp/extensions",
        ),
    ]);
    assert_eq!(
        out,
        "⊹ 2 extensions\n[Extensions]\n  plan-mode, webfetch.ts\n  project\n    /tmp/extensions/plan-mode\n    /tmp/extensions/webfetch.ts"
    );
}

#[test]
fn multiple_index_ts_with_unique_parent_dirs_need_no_disambiguation() {
    let _g = lock(Some("dark"));
    let out = expanded_with(vec![
        local(
            "/tmp/extensions/foo/index.ts",
            SourceScope::Project,
            "/tmp/extensions",
        ),
        local(
            "/tmp/extensions/bar/index.ts",
            SourceScope::Project,
            "/tmp/extensions",
        ),
    ]);
    assert_eq!(
        out,
        "⊹ 2 extensions\n[Extensions]\n  bar, foo\n  project\n    /tmp/extensions/bar\n    /tmp/extensions/foo"
    );
}

#[test]
fn multiple_index_ts_with_the_same_parent_dir_name_disambiguated_with_grandparent() {
    let _g = lock(Some("dark"));
    let out = expanded_with(vec![
        local(
            "/tmp/alpha/tools/index.ts",
            SourceScope::Temporary,
            "/tmp/alpha",
        ),
        local(
            "/tmp/beta/tools/index.ts",
            SourceScope::Temporary,
            "/tmp/beta",
        ),
    ]);
    assert_eq!(
        out,
        "⊹ 2 extensions\n[Extensions]\n  alpha/tools, beta/tools\n  path\n    /tmp/alpha/tools\n    /tmp/beta/tools"
    );
}

#[test]
fn non_index_file_in_subdirectory_stays_as_filename() {
    let _g = lock(Some("dark"));
    let out = expanded_with(vec![local(
        "/tmp/extensions/my-ext/main.ts",
        SourceScope::Project,
        "/tmp/extensions",
    )]);
    assert_eq!(
        out,
        "⊹ 1 extension\n[Extensions]\n  main.ts\n  project\n    /tmp/extensions/my-ext/main.ts"
    );
}

#[test]
fn package_extensions_still_strip_index_ts() {
    let _g = lock(Some("dark"));
    let out = expanded_with(vec![package(
        "/tmp/project/.hoocode/npm/node_modules/pi-markdown-preview/extensions/index.ts",
        "npm:pi-markdown-preview",
        "/tmp/project/.hoocode/npm/node_modules/pi-markdown-preview",
    )]);
    assert_eq!(
        out,
        "⊹ 1 extension\n[Extensions]\n  pi-markdown-preview\n  project\n    npm:pi-markdown-preview\n      extensions"
    );
}

#[test]
fn captures_mixed_extension_layouts_in_expanded_output() {
    let _g = lock(Some("dark"));
    let out = expanded_with(extension_fixtures());
    assert_eq!(
        out,
        [
            "⊹ 8 extensions",
            "[Extensions]",
            "  @scope/pi-scoped, answer.ts, cli-extension.ts, HazAT/pi-interactive-subagents, HazAT/pi-interactive-subagents:subagents, local-index, pi-markdown-preview, user-index",
            "  project",
            "    /tmp/project/.hoocode/extensions/answer.ts",
            "    /tmp/project/.hoocode/extensions/local-index",
            "    git:github.com/HazAT/pi-interactive-subagents",
            "      extensions",
            "      extensions/subagents",
            "    npm:@scope/pi-scoped",
            "      extensions",
            "    npm:pi-markdown-preview",
            "      extensions",
            "  user",
            "    /tmp/agent/extensions/user-index",
            "  path",
            "    /tmp/temp/cli-extension.ts",
        ]
        .join("\n")
    );
}

#[test]
fn shows_context_paths_relative_to_cwd_while_preserving_full_external_paths() {
    let _g = lock(Some("dark"));
    let home = std::env::var("HOME").unwrap();
    let cwd = format!("{home}/Development/hoocode");
    for expanded in [false, true] {
        let out = render(&ResourceListing {
            expanded,
            context_files: vec![
                context(&format!("{home}/.hoocode/agent/AGENTS.md")),
                context(&format!("{cwd}/AGENTS.md")),
            ],
            ..base(&cwd)
        });
        assert!(out.contains("context"), "{out}");
        assert!(
            out.contains("~/.hoocode/agent/AGENTS.md, AGENTS.md"),
            "{out}"
        );
        assert!(!out.contains(&format!("{cwd}/AGENTS.md")));
    }
}

#[test]
fn does_not_show_the_listing_on_quiet_startup_during_reload() {
    let _g = lock(Some("dark"));
    let listing = ResourceListing {
        quiet_startup: true,
        skills: vec![skill("commit", "/tmp/skill/SKILL.md")],
        extensions: vec![ext("/tmp/ext/index.ts", None)],
        ..base("/tmp/project")
    };
    assert!(show_loaded_resources(&listing, false, true).is_empty());
}

#[test]
fn still_shows_diagnostics_on_quiet_startup_when_requested() {
    let _g = lock(Some("dark"));
    let out = normalized(
        &ResourceListing {
            quiet_startup: true,
            skills: vec![skill("commit", "/tmp/skill/SKILL.md")],
            skill_diagnostics: vec![ResourceDiagnostic {
                kind: DiagnosticType::Warning,
                message: "duplicate skill name".into(),
                path: None,
                collision: None,
            }],
            ..base("/tmp/project")
        },
        false,
        true,
    );
    assert!(out.contains("[Skill conflicts]"), "{out}");
    assert!(!out.contains("[Skills]"));
}

#[test]
fn annotates_an_oversized_context_file_on_the_context_row() {
    let _g = lock(Some("dark"));
    let out = render(&ResourceListing {
        context_files: vec![ContextFile {
            path: "/tmp/AGENTS.md".into(),
            content: "large".into(),
            tokens: Some(3624),
            size: Some(ContextFileSize::Large),
        }],
        ..base("/tmp/project")
    });
    assert!(out.contains("~3.6k tokens"), "{out}");
    assert!(out.contains("consider trimming"));
}

#[test]
fn shows_context_file_read_failures_as_warnings() {
    let _g = lock(Some("dark"));
    let out = render(&ResourceListing {
        context_warnings: vec!["Could not read /tmp/AGENTS.md: EACCES".into()],
        ..base("/tmp/project")
    });
    assert!(out.contains("Could not read"), "{out}");
}

#[test]
fn summary_shows_counts_and_context_names_sections_stay_collapsed() {
    let _g = lock(Some("dark"));
    let out = render(&ResourceListing {
        context_files: vec![context("/tmp/AGENTS.md")],
        skills: vec![skill("commit", "/tmp/skill/SKILL.md")],
        ..base("/tmp/project")
    });
    assert!(out.contains("1 skill"), "{out}");
    assert!(out.contains("AGENTS.md"));
    assert!(!out.contains("[Context]"));
    assert!(!out.contains("[Skills]"));
}

#[test]
fn expanded_details_carry_every_section_regardless_of_resource_count() {
    let _g = lock(Some("dark"));
    let out = render(&ResourceListing {
        expanded: true,
        skills: ["alpha", "beta", "gamma", "delta", "epsilon"]
            .iter()
            .map(|n| skill(n, &format!("/tmp/skill/{n}/SKILL.md")))
            .collect(),
        context_files: vec![context("/tmp/AGENTS.md")],
        ..base("/tmp/project")
    });
    assert!(out.contains("5 skills"), "{out}");
    assert!(out.contains("[Skills]"));
}
