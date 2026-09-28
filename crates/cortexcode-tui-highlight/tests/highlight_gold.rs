//! cli-highlight's output against the pin's own (`fixtures/highlight-gold.json`,
//! from `migration/tools/goldens/highlight.mjs`). Cases run in the
//! generator's order on one thread: grammars compile once and share modes.

use serde_json::Value;

const MARKED: [&str; 15] = [
    "keyword",
    "built_in",
    "literal",
    "number",
    "string",
    "comment",
    "function",
    "title",
    "class",
    "type",
    "attr",
    "variable",
    "params",
    "operator",
    "punctuation",
];

fn marker(token: &str, text: &str) -> Option<String> {
    MARKED
        .contains(&token)
        .then(|| format!("<{token}>{text}</{token}>"))
}

fn none(_: &str, _: &str) -> Option<String> {
    None
}

#[test]
fn highlight_matches_the_pin() {
    let gold: Vec<Value> =
        serde_json::from_str(include_str!("fixtures/highlight-gold.json")).unwrap();
    let mut failures = Vec::new();
    let mut dump = Vec::new();
    for (i, case) in gold.iter().enumerate() {
        let lang = case["lang"].as_str().unwrap();
        let code = case["code"].as_str().unwrap();
        let ignore = case["ignoreIllegals"].as_bool().unwrap();
        let theme: &dyn Fn(&str, &str) -> Option<String> = if case["theme"] == "marker" {
            &marker
        } else {
            &none
        };
        let got = cortexcode_tui_highlight::highlight(code, lang, ignore, theme);
        let want = case["out"].as_str().map(String::from);
        dump.push(got.clone());
        if got != want {
            failures.push(format!(
                "#{i} {lang} theme={} ignoreIllegals={ignore}\n  code {code:?}\n  want {want:?}\n  got  {got:?}",
                case["theme"]
            ));
        }
    }
    // HIGHLIGHT_GOLD_DUMP=<path> writes every output, for diffing.
    if let Ok(path) = std::env::var("HIGHLIGHT_GOLD_DUMP") {
        std::fs::write(path, serde_json::to_string(&dump).unwrap()).unwrap();
    }
    assert!(
        failures.is_empty(),
        "{} of {} mismatch:\n{}",
        failures.len(),
        gold.len(),
        failures.join("\n")
    );
}
