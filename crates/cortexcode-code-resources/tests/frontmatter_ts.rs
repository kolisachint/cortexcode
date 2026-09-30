//! Case-for-case port of the pin's `coding-agent/test/frontmatter.test.ts`.

use cortexcode_code_resources::frontmatter::{parse_frontmatter, strip_frontmatter};
use serde_json::{json, Value};

#[test]
fn parses_keys_strips_quotes_and_returns_body() {
    let input =
        "---\nname: \"skill-name\"\ndescription: 'A desc'\nfoo-bar: value\n---\n\nBody text";
    let (fm, body) = parse_frontmatter(input).unwrap();
    assert_eq!(fm["name"], "skill-name");
    assert_eq!(fm["description"], "A desc");
    assert_eq!(fm["foo-bar"], "value");
    assert_eq!(body, "Body text");
}

#[test]
fn normalizes_newlines_and_handles_crlf() {
    let (_, body) = parse_frontmatter("---\r\nname: test\r\n---\r\nLine one\r\nLine two").unwrap();
    assert_eq!(body, "Line one\nLine two");
}

#[test]
fn errors_on_invalid_yaml_frontmatter() {
    let err = parse_frontmatter("---\nfoo: [bar\n---\nBody").unwrap_err();
    // js-yaml reports "at line 1, column 10"; the position is what matters.
    assert!(err.contains("line"), "{err}");
}

#[test]
fn parses_block_scalar_multiline_yaml() {
    let (fm, body) =
        parse_frontmatter("---\ndescription: |\n  Line one\n  Line two\n---\n\nBody").unwrap();
    assert_eq!(fm["description"], "Line one\nLine two\n");
    assert_eq!(body, "Body");
}

#[test]
fn returns_original_content_when_frontmatter_missing_or_unterminated() {
    let (_, body) = parse_frontmatter("Just text\nsecond line").unwrap();
    assert_eq!(body, "Just text\nsecond line");
    let (_, body) = parse_frontmatter("---\nname: test\nBody without terminator").unwrap();
    assert_eq!(body, "---\nname: test\nBody without terminator");
}

#[test]
fn returns_empty_object_for_comment_only_frontmatter() {
    let (fm, _) = parse_frontmatter("---\n# just a comment\n---\nBody").unwrap();
    assert_eq!(Value::Object(fm), json!({}));
}

#[test]
fn strip_removes_frontmatter_and_trims_body() {
    assert_eq!(
        strip_frontmatter("---\nkey: value\n---\n\nBody\n").unwrap(),
        "Body"
    );
}

#[test]
fn strip_returns_body_when_no_frontmatter_present() {
    assert_eq!(
        strip_frontmatter("\n  No frontmatter body  \n").unwrap(),
        "\n  No frontmatter body  \n"
    );
}
