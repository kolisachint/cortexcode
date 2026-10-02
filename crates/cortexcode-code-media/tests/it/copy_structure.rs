//! Port of `test/copy-structure.test.ts`: markdown as the HTML a word
//! processor takes, and the CF_HTML envelope. The transcript half
//! (`sessionToMarkdown`) is ported in code-agent-session; the `/copy` half runs
//! through the app (L2 `copy-command`).

use cortexcode_code_media::markdown_to_html::markdown_to_html;
use cortexcode_code_media::rich_clipboard::wrap_cf_html;

#[test]
fn turns_headings_into_headings() {
    assert_eq!(markdown_to_html("## Findings"), "<h2>Findings</h2>");
}

#[test]
fn turns_a_gfm_table_into_a_real_table() {
    let html =
        markdown_to_html(&["| Name | Count |", "| --- | ----: |", "| alpha | 2 |"].join("\n"));
    assert!(html.contains("<table"));
    assert!(html.contains("<th"));
    assert!(html.contains(">Name<"));
    assert!(html.contains(">alpha<"));
    assert!(!html.contains("---"));
}

#[test]
fn keeps_a_code_block_whole_and_literal() {
    let html = markdown_to_html(&["```ts", "const a = **not bold**;", "```"].join("\n"));
    assert!(html.contains("<pre"));
    assert!(html.contains("const a = **not bold**;"));
    assert!(!html.contains("<strong>"));
}

#[test]
fn does_not_read_emphasis_inside_a_code_span() {
    assert!(markdown_to_html("run `ls *.ts` first").contains("<code"));
    assert!(markdown_to_html("run `ls *.ts` first").contains("ls *.ts"));
}

#[test]
fn renders_emphasis_links_and_bare_urls() {
    assert!(markdown_to_html("**bold** and *italic*").contains("<strong>bold</strong>"));
    assert!(markdown_to_html("**bold** and *italic*").contains("<em>italic</em>"));
    assert!(markdown_to_html("[docs](https://example.com)")
        .contains(r#"<a href="https://example.com">docs</a>"#));
    assert!(markdown_to_html("see https://example.com/x for more")
        .contains(r#"<a href="https://example.com/x">"#));
}

#[test]
fn nests_a_list_the_way_it_was_written() {
    let html = markdown_to_html(&["- one", "  - inner", "- two"].join("\n"));
    assert_eq!(
        html,
        "<ul>\n<li>one</li>\n<ul>\n<li>inner</li>\n</ul>\n<li>two</li>\n</ul>"
    );
}

#[test]
fn keeps_an_ordered_list_ordered() {
    assert!(markdown_to_html("1. first\n2. second").contains("<ol>"));
}

#[test]
fn renders_a_quote_and_a_rule() {
    assert!(markdown_to_html("> quoted").contains("<blockquote"));
    assert_eq!(markdown_to_html("---"), "<hr>");
}

#[test]
fn never_lets_source_markdown_become_markup() {
    let html = markdown_to_html(r#"<script>alert("x")</script> & <b>raw</b>"#);
    assert!(!html.contains("<script>"));
    assert!(!html.contains("<b>raw</b>"));
    assert!(html.contains("&lt;script&gt;"));
    assert!(html.contains("&amp;"));
}

#[test]
fn carries_its_styling_on_the_elements() {
    assert!(markdown_to_html("`x`").contains("style="));
}

fn offset(wrapped: &str, name: &str) -> usize {
    let start = wrapped.find(&format!("{name}:")).unwrap() + name.len() + 1;
    wrapped[start..start + 10].parse().unwrap()
}

#[test]
fn cf_html_points_at_the_fragment_it_actually_contains() {
    let wrapped = wrap_cf_html("<p>hello</p>");
    let bytes = wrapped.as_bytes();
    let html = &bytes[offset(&wrapped, "StartHTML")..offset(&wrapped, "EndHTML")];
    assert!(html.starts_with(b"<html>"));
    let fragment = &bytes[offset(&wrapped, "StartFragment")..offset(&wrapped, "EndFragment")];
    assert_eq!(fragment, b"<p>hello</p>");
    assert_eq!(offset(&wrapped, "EndHTML"), bytes.len());
}

#[test]
fn cf_html_counts_bytes_not_characters() {
    let wrapped = wrap_cf_html("<p>héllo — ✓</p>");
    let bytes = wrapped.as_bytes();
    let fragment = &bytes[offset(&wrapped, "StartFragment")..offset(&wrapped, "EndFragment")];
    assert_eq!(std::str::from_utf8(fragment).unwrap(), "<p>héllo — ✓</p>");
}

/// Extra coverage of the pin's behaviour: paragraphs keep single line breaks,
/// images and nested quotes.
#[test]
fn paragraphs_images_and_list_marker_changes() {
    assert_eq!(markdown_to_html("one\ntwo"), "<p>one<br>two</p>");
    assert_eq!(
        markdown_to_html("![alt](img.png)"),
        r#"<p><img src="img.png" alt="alt"></p>"#
    );
    assert_eq!(
        markdown_to_html("- a\n1. b"),
        "<ul>\n<li>a</li>\n</ul>\n<ol>\n<li>b</li>\n</ol>"
    );
    assert_eq!(
        markdown_to_html("see https://example.com."),
        r#"<p>see <a href="https://example.com">https://example.com</a>.</p>"#
    );
}

/// Outputs recorded from the pinned `markdownToHtml` (node, dist build).
#[test]
fn matches_the_pin_on_recorded_inputs() {
    let cases: &[(&str, &str)] = &[
        ("- a\n- b\n\n1. c", "<ul>\n<li>a</li>\n<li>b</li>\n</ul>\n<ol>\n<li>c</li>\n</ol>"),
        ("> a\n> - b\n> c", "<blockquote style=\"border-left:3px solid #bbb;margin:0 0 0 8px;padding-left:10px;color:#555\"><p>a</p>\n<ul>\n<li>b</li>\n</ul>\n<p>c</p></blockquote>"),
        ("# T #\n\n```\nx<y\n```\n~~s~~ _u_ ***bi***", "<h1>T</h1>\n<pre style=\"font-family:Consolas,Menlo,monospace;background:#f4f4f4;padding:10px;border-radius:4px;white-space:pre-wrap\"><code>x&lt;y</code></pre>\n<p><del>s</del> <em>u</em> <strong><em>bi</em></strong></p>"),
        ("| a \\| b | c |\n|---|---|\n| 1 | 2 |\nafter", "<table style=\"border-collapse:collapse\"><thead><tr><th style=\"border:1px solid #bbb;padding:4px 8px;text-align:left\">a | b</th><th style=\"border:1px solid #bbb;padding:4px 8px;text-align:left\">c</th></tr></thead><tbody><tr><td style=\"border:1px solid #bbb;padding:4px 8px;text-align:left\">1</td><td style=\"border:1px solid #bbb;padding:4px 8px;text-align:left\">2</td></tr></tbody></table>\n<p>after</p>"),
        ("  - x\n- y", "<ul>\n<li>x</li>\n</ul>\n<ul>\n<li>y</li>\n</ul>"),
        ("* * *", "<hr>"),
        ("snake_case_name and a*b*c", "<p>snake_case_name and a*b*c</p>"),
        ("a `` b`c `` d and `x` [l](u \"t\")", "<p>a <code style=\"font-family:Consolas,Menlo,monospace;background:#f4f4f4;padding:1px 4px;border-radius:3px\">b`c</code> d and <code style=\"font-family:Consolas,Menlo,monospace;background:#f4f4f4;padding:1px 4px;border-radius:3px\">x</code> <a href=\"u\">l</a></p>"),
    ];
    for (input, expected) in cases {
        assert_eq!(&markdown_to_html(input), expected, "input: {input:?}");
    }
}
