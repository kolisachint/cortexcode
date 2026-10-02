//! Ports of hoocode `test/format-tokens.test.ts`, `test/format-duration.test.ts`
//! and the `format-list.ts` half of `test/format-list.test.ts` (the plugin
//! listing half belongs to the plugin task). The `gold_*` expectations were
//! produced by running the pinned hoocode build on the same inputs.

use cortexcode_code_agent_session::format::*;

fn plugin_groups(installed: bool) -> Vec<ListGroup> {
    // What `availablePluginGroups(availableFixture)` returns.
    let row = |name: &str, facts: [&str; 2], detail: &str| ListRow {
        name: name.into(),
        marker: None,
        facts: facts.iter().map(|f| f.to_string()).collect(),
        detail: Some(detail.into()),
        trailer: None,
    };
    let mut review = row(
        "code-review",
        ["claude/github", "git"],
        "Structured multi-pass code review over the current diff or a named PR.",
    );
    if installed {
        review.marker = Some("\u{2713}".into());
    }
    vec![
        ListGroup {
            title: Some("official".into()),
            rows: vec![
                review,
                row("pdf", ["claude", "git"], "Read and merge PDFs."),
            ],
        },
        ListGroup {
            title: Some("acme".into()),
            rows: vec![row(
                "terraform-guard",
                ["agents", "git-subdir"],
                "https://example.test/acme/tree/main/terraform-guard",
            )],
        },
    ]
}

fn opts(indent: usize, columns: Option<usize>) -> RenderListOptions<'static> {
    RenderListOptions {
        indent,
        columns,
        ..RenderListOptions::default()
    }
}

fn first_non_space(s: &str) -> usize {
    s.len() - s.trim_start().len()
}

#[test]
fn plural_does_not_print_the_hedge() {
    assert_eq!(plural(1, "plugin", None), "1 plugin");
    assert_eq!(plural(0, "plugin", None), "0 plugins");
    assert_eq!(plural(2, "marketplace", None), "2 marketplaces");
    assert_eq!(plural(3, "entry", Some("entries")), "3 entries");
}

#[test]
fn render_list_aligns_the_name_column_across_every_group() {
    let text = render_list(&plugin_groups(false), &opts(2, None));
    let rows: Vec<&str> = text.lines().filter(|l| l.contains(" \u{b7} ")).collect();
    assert_eq!(rows.len(), 3);
    let name_cols: Vec<usize> = rows.iter().map(|l| first_non_space(l)).collect();
    assert!(name_cols.windows(2).all(|w| w[0] == w[1]));
    // Facts begin at a fixed column whatever the name's own length.
    let fact_cols: Vec<usize> = rows
        .iter()
        .map(|l| {
            let rest = l.trim_start();
            let after_name = &rest[rest.find(' ').unwrap()..];
            l.len() - after_name.trim_start().len()
        })
        .collect();
    assert!(fact_cols.windows(2).all(|w| w[0] == w[1]));
}

#[test]
fn render_list_indents_wrapped_detail_under_its_row() {
    let text = render_list(&plugin_groups(false), &opts(2, Some(60)));
    let detail: Vec<&str> = text
        .lines()
        .filter(|l| l.contains("code review") || l.contains("named PR"))
        .collect();
    assert!(detail.len() > 1);
    for line in detail {
        assert!(line.starts_with("        "), "{line:?}");
    }
}

#[test]
fn render_list_emits_no_escape_codes_when_unstyled() {
    let text = render_list(&plugin_groups(false), &opts(2, Some(80)));
    assert!(!text.contains("\x1b["));
}

fn strip_sgr(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' && chars.peek() == Some(&'[') {
            for c in chars.by_ref() {
                if c == 'm' {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[test]
fn render_list_keeps_padding_correct_when_styling_wraps_the_name() {
    let red = |t: &str| format!("\x1b[31m{t}\x1b[39m");
    let groups = vec![ListGroup {
        title: None,
        rows: vec![
            ListRow {
                name: "a".into(),
                facts: vec!["x".into()],
                ..ListRow::default()
            },
            ListRow {
                name: "bbbb".into(),
                facts: vec!["y".into()],
                ..ListRow::default()
            },
        ],
    }];
    let options = RenderListOptions {
        style: ListStyle {
            name: Some(&red),
            ..ListStyle::default()
        },
        ..RenderListOptions::default()
    };
    let styled = render_list(&groups, &options);
    let lines: Vec<String> = styled.lines().map(strip_sgr).collect();
    assert!(styled.contains("\x1b[31m"));
    assert_eq!(lines[0].find('x'), lines[1].find('y'));
}

#[test]
fn render_list_marks_installed_rows_and_keeps_unmarked_rows_aligned() {
    let text = render_list(&plugin_groups(true), &opts(2, None));
    let marked = text.lines().find(|l| l.contains("code-review")).unwrap();
    let unmarked = text.lines().find(|l| l.contains("pdf")).unwrap();
    assert!(marked.contains('\u{2713}'));
    // Columns, not bytes: the tick is three bytes wide in UTF-8.
    let col = |l: &str, needle: &str| l[..l.find(needle).unwrap()].chars().count();
    assert_eq!(col(marked, "code-review"), col(unmarked, "pdf"));
}

#[test]
fn render_list_shows_the_source_detail() {
    let text = render_list(&plugin_groups(false), &opts(2, None));
    assert!(text.contains("https://example.test/acme/tree/main/terraform-guard"));
}

#[test]
fn render_list_of_nothing_is_empty() {
    assert_eq!(render_list(&[], &opts(2, None)), "");
    assert_eq!(
        render_list(
            &[ListGroup {
                title: Some("t".into()),
                rows: vec![]
            }],
            &opts(0, None)
        ),
        ""
    );
}

fn gold_groups() -> Vec<ListGroup> {
    let mut groups = plugin_groups(true);
    groups[1].rows[0].trailer =
        Some("/home/someone/.cortex/plugins/a/very/long/path/that/needs/truncation/here".into());
    groups
}

#[test]
fn gold_render_list_matches_hoocode() {
    assert_eq!(
        render_list(&gold_groups(), &opts(2, None)),
        "  official\n    ✓ code-review      claude/github · git\n        Structured multi-pass code review over the current diff or a named PR.\n      pdf              claude · git\n        Read and merge PDFs.\n  acme\n      terraform-guard  agents · git-subdir\n        https://example.test/acme/tree/main/terraform-guard\n        /home/someone/.cortex/plugins/a/very/long/path/that/needs/truncation/here"
    );
    assert_eq!(
        render_list(&gold_groups(), &opts(2, Some(60))),
        "  official\n    ✓ code-review      claude/github · git\n        Structured multi-pass code review over the current\n        diff or a named PR.\n      pdf              claude · git\n        Read and merge PDFs.\n  acme\n      terraform-guard  agents · git-subdir\n        https://example.test/acme/tree/main/terraform-guard\n        /home/someone/.cortex/plugins/a/very/long/path/that…"
    );
    let groups = vec![ListGroup {
        title: None,
        rows: vec![
            ListRow {
                name: "a".into(),
                facts: vec!["x".into(), String::new(), "y".into()],
                ..ListRow::default()
            },
            ListRow {
                name: "bbbb".into(),
                ..ListRow::default()
            },
        ],
    }];
    let options = RenderListOptions {
        fact_separator: Some(", "),
        ..RenderListOptions::default()
    };
    assert_eq!(render_list(&groups, &options), "a     x, y\nbbbb");
}

#[test]
fn compact_rows_truncate_to_the_terminal_instead_of_wrapping() {
    let long = "a".repeat(300);
    let text = render_compact_rows(
        &[("code-reviewer", long.as_str()), ("Explore", "short")],
        &CompactRowsOptions {
            columns: Some(60),
            ..CompactRowsOptions::default()
        },
    );
    let lines: Vec<&str> = text.split('\n').collect();
    assert_eq!(lines.len(), 2);
    for line in &lines {
        assert!(!line.contains("\x1b["));
        // JS `line.length` counts UTF-16 units; the ellipsis is one.
        assert!(line.encode_utf16().count() <= 60);
    }
    assert!(lines[0].contains('\u{2026}'));
}

#[test]
fn compact_rows_align_detail_into_one_column() {
    let text = render_compact_rows(
        &[("code-reviewer", "alpha"), ("Explore", "beta")],
        &CompactRowsOptions::default(),
    );
    let lines: Vec<&str> = text.split('\n').collect();
    assert_eq!(lines[0].find("alpha"), lines[1].find("beta"));
}

#[test]
fn compact_rows_emit_nothing_for_an_empty_list() {
    assert_eq!(render_compact_rows(&[], &CompactRowsOptions::default()), "");
}

#[test]
fn gold_compact_rows_match_hoocode() {
    let long = "a".repeat(300);
    let wide = "short 日本語テキスト日本語テキスト日本語テキスト日本語テキスト";
    assert_eq!(
        render_compact_rows(
            &[("code-reviewer", long.as_str()), ("Explore", wide)],
            &CompactRowsOptions {
                columns: Some(60),
                ..CompactRowsOptions::default()
            },
        ),
        "  code-reviewer  aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa…\n  Explore        short 日本語テキスト日本語テキスト日本語テ…"
    );
    assert_eq!(
        render_compact_rows(
            &[("code-reviewer", "alpha"), ("Explore", "")],
            &CompactRowsOptions::default()
        ),
        "  code-reviewer  alpha\n  Explore"
    );
}

#[test]
fn gold_truncate_and_wrap_match_hoocode() {
    assert_eq!(truncate_visible("hello world", 5, "…"), "hell…");
    assert_eq!(truncate_visible("hello", 1, "…"), "…");
    assert_eq!(truncate_visible("hi", 0, "…"), "");
    assert_eq!(truncate_visible("日本語テキスト", 5, "…"), "日本…");
    assert_eq!(
        wrap_indented("  one two\u{3000}three  four five six seven  ", 3, Some(12)),
        vec!["   one two", "   three", "   four five", "   six seven"]
    );
    assert_eq!(wrap_indented("x y", 2, None), vec!["  x y"]);
    assert!(wrap_indented("   ", 2, Some(40)).is_empty());
    assert_eq!(pad_cell("\x1b[1mab\x1b[0m", 4), "\x1b[1mab\x1b[0m  ");
}

// format-tokens.test.ts

#[test]
fn format_tokens_bands() {
    for (n, s) in [(0, "0"), (99, "99"), (999, "999")] {
        assert_eq!(format_tokens(n), s);
    }
    for (n, s) in [(1000, "1.0k"), (4100, "4.1k"), (9900, "9.9k")] {
        assert_eq!(format_tokens(n), s);
    }
    for (n, s) in [(10_000, "10k"), (41_000, "41k"), (999_000, "999k")] {
        assert_eq!(format_tokens(n), s);
    }
    for (n, s) in [
        (1_000_000, "1.0M"),
        (4_100_000, "4.1M"),
        (9_900_000, "9.9M"),
    ] {
        assert_eq!(format_tokens(n), s);
    }
    for (n, s) in [(10_000_000, "10M"), (41_000_000, "41M")] {
        assert_eq!(format_tokens(n), s);
    }
}

#[test]
fn gold_format_tokens_rounding_matches_hoocode() {
    let inputs = [
        0u64, 999, 1000, 1049, 1050, 1150, 1250, 1350, 4100, 9949, 9950, 9999, 10000, 10499, 10500,
        41000, 999_499, 999_500, 999_999, 1_000_000, 1_250_000, 4_100_000, 9_950_000, 9_999_999,
        10_000_000, 41_000_000, 10_500_000,
    ];
    let expected = [
        "0", "999", "1.0k", "1.0k", "1.1k", "1.1k", "1.3k", "1.4k", "4.1k", "9.9k", "9.9k",
        "10.0k", "10k", "10k", "11k", "41k", "999k", "1000k", "1000k", "1.0M", "1.3M", "4.1M",
        "9.9M", "10.0M", "10M", "41M", "11M",
    ];
    let got: Vec<String> = inputs.iter().map(|n| format_tokens(*n)).collect();
    assert_eq!(got, expected);
}

// format-duration.test.ts

#[test]
fn format_duration_secs_bands() {
    for (s, e) in [(0.0, "0.0s"), (4.1, "4.1s"), (9.9, "9.9s")] {
        assert_eq!(format_duration_secs(s), e);
    }
    for (s, e) in [(10.0, "10s"), (30.0, "30s"), (59.0, "59s")] {
        assert_eq!(format_duration_secs(s), e);
    }
    for (s, e) in [(60.0, "1m00s"), (90.0, "1m30s"), (3599.0, "59m59s")] {
        assert_eq!(format_duration_secs(s), e);
    }
    for (s, e) in [
        (3600.0, "1h00m"),
        (3660.0, "1h01m"),
        (7200.0, "2h00m"),
        (7260.0, "2h01m"),
    ] {
        assert_eq!(format_duration_secs(s), e);
    }
    assert_eq!(format_duration_secs(-10.0), "0.0s");
    assert_eq!(format_duration_secs(5400.0), "1h30m");
    assert_eq!(format_duration_secs(9000.0), "2h30m");
}

#[test]
fn gold_format_duration_rounding_matches_hoocode() {
    let inputs = [
        0.0, 0.05, 0.15, 0.25, 4.1, 9.94, 9.95, 9.99, 10.0, 10.5, 59.4, 59.5, 60.0, 90.0, 119.6,
        3599.0, 3600.0, 3629.9, 3630.0, 5400.0, 7260.0, -10.0,
    ];
    let expected = [
        "0.0s", "0.1s", "0.1s", "0.3s", "4.1s", "9.9s", "9.9s", "10.0s", "10s", "11s", "59s",
        "60s", "1m00s", "1m30s", "1m60s", "59m59s", "1h00m", "1h00m", "1h01m", "1h30m", "2h01m",
        "0.0s",
    ];
    let got: Vec<String> = inputs.iter().map(|s| format_duration_secs(*s)).collect();
    assert_eq!(got, expected);
}

#[test]
fn js_to_fixed_rounds_exact_ties_up() {
    assert_eq!(js_to_fixed(1.25, 1), "1.3");
    assert_eq!(js_to_fixed(0.125, 2), "0.13");
    assert_eq!(js_to_fixed(2.5, 0), "3");
    assert_eq!(js_to_fixed(1.15, 1), "1.1");
    assert_eq!(js_to_fixed(-1.25, 1), "-1.3");
    assert_eq!(js_to_fixed(-0.01, 1), "-0.0");
    assert_eq!(js_to_fixed(-0.0, 1), "0.0");
}
