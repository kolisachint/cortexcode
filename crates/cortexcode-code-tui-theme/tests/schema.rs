//! Theme file validation: the error text matches hoocode's compiled
//! validator (captured from the pinned build).

use cortexcode_code_tui_theme::*;
use serde_json::Value;

fn dark() -> Value {
    serde_json::from_str(builtin_theme_source("dark").unwrap()).unwrap()
}

fn error_for(edit: impl FnOnce(&mut Value)) -> String {
    let mut json = dark();
    edit(&mut json);
    parse_theme_json("t.json", &json).unwrap_err()
}

const HEADER: &str = "Invalid theme \"t.json\":\n";

fn other(lines: &[&str]) -> String {
    format!("{HEADER}\n\nOther errors:\n{}", lines.join("\n"))
}

fn anyof(path: &str, middle: &str) -> Vec<String> {
    vec![
        format!("  - {path}: must be string"),
        format!("  - {path}: {middle}"),
        format!("  - {path}: must match a schema in anyOf"),
    ]
}

#[test]
fn lists_missing_required_colors_sorted() {
    let e = error_for(|j| {
        let c = j["colors"].as_object_mut().unwrap();
        c.remove("mdHr");
        c.remove("accent");
    });
    assert_eq!(
        e,
        format!(
            "{HEADER}\nMissing required color tokens:\n  - accent\n  - mdHr\n\nPlease add these colors to your theme's \"colors\" object.\nSee the built-in themes (dark.json, light.json) for reference values."
        )
    );
}

#[test]
fn reports_bad_color_values_like_the_union_validator() {
    let cases: [(Value, &str); 5] = [
        (Value::Bool(true), "must be integer"),
        (Value::Null, "must be integer"),
        (serde_json::json!(300), "must be <= 255"),
        (serde_json::json!(-1), "must be >= 0"),
        (serde_json::json!(1.5), "must be integer"),
    ];
    for (value, middle) in cases {
        let e = error_for(|j| j["colors"]["accent"] = value.clone());
        let lines = anyof("/colors/accent", middle);
        assert_eq!(
            e,
            other(&lines.iter().map(String::as_str).collect::<Vec<_>>()),
            "{value}"
        );
    }
    // 255.0 is an integer.
    let mut json = dark();
    json["colors"]["accent"] = serde_json::json!(255.0);
    assert!(parse_theme_json("t.json", &json).is_ok());
}

#[test]
fn reports_root_and_section_shape_errors() {
    assert_eq!(
        error_for(|j| {
            let o = j.as_object_mut().unwrap();
            o.remove("name");
            o.remove("colors");
        }),
        other(&["  - /: must have required properties name, colors"])
    );
    assert_eq!(
        parse_theme_json("t.json", &serde_json::json!([])).unwrap_err(),
        other(&["  - /: must be object"])
    );
    assert_eq!(
        error_for(|j| j["name"] = serde_json::json!(5)),
        other(&["  - /name: must be string"])
    );
    assert_eq!(
        error_for(|j| j["$schema"] = serde_json::json!(1)),
        other(&["  - /$schema: must be string"])
    );
    assert_eq!(
        error_for(|j| j["description"] = serde_json::json!(3)),
        other(&["  - /description: must be string"])
    );
    assert_eq!(
        error_for(|j| j["vars"] = serde_json::json!([])),
        other(&["  - /vars: must be object"])
    );
    assert_eq!(
        error_for(|j| j["colors"] = serde_json::json!(4)),
        other(&["  - /colors: must be object"])
    );
    assert_eq!(
        error_for(|j| j["export"] = serde_json::json!(5)),
        other(&["  - /export: must be object"])
    );
    let lines = anyof("/vars/x", "must be integer");
    assert_eq!(
        error_for(|j| j["vars"] = serde_json::json!({"x": {}})),
        other(&lines.iter().map(String::as_str).collect::<Vec<_>>())
    );
    let lines = anyof("/export/pageBg", "must be integer");
    assert_eq!(
        error_for(|j| j["export"] = serde_json::json!({"pageBg": true})),
        other(&lines.iter().map(String::as_str).collect::<Vec<_>>())
    );
}

#[test]
fn orders_mixed_errors_like_the_validator() {
    let e = error_for(|j| {
        j["colors"].as_object_mut().unwrap().remove("accent");
        j["colors"]["border"] = Value::Bool(true);
        j["name"] = serde_json::json!(1);
        j["vars"] = serde_json::json!(3);
    });
    assert_eq!(
        e,
        format!(
            "{HEADER}\nMissing required color tokens:\n  - accent\n\nPlease add these colors to your theme's \"colors\" object.\nSee the built-in themes (dark.json, light.json) for reference values.\n\nOther errors:\n  - /name: must be string\n  - /vars: must be object\n  - /colors/border: must be string\n  - /colors/border: must be integer\n  - /colors/border: must match a schema in anyOf"
        )
    );
}

#[test]
fn reports_resolution_errors_when_the_theme_is_built() {
    let build = |edit: &dyn Fn(&mut Value)| {
        let mut json = dark();
        edit(&mut json);
        let parsed = parse_theme_json("t.json", &json).unwrap();
        create_theme(&parsed, Some(ColorMode::Truecolor), None).unwrap_err()
    };
    assert_eq!(
        build(&|j| j["colors"]["accent"] = "#12".into()),
        "Invalid hex color: #12"
    );
    assert_eq!(
        build(&|j| j["colors"]["accent"] = "nope".into()),
        "Variable reference not found: nope"
    );
    assert_eq!(
        build(&|j| {
            j["vars"] = serde_json::json!({"a": "b", "b": "a"});
            j["colors"]["accent"] = "a".into();
        }),
        "Circular variable reference detected: a"
    );
}

#[test]
fn every_bundled_theme_validates() {
    for name in [
        "colorsafe-dark",
        "colorsafe-light",
        "dark",
        "light",
        "solarized-dark",
        "solarized-light",
        "vox-cutout-dark",
        "vox-cutout-light",
    ] {
        let parsed = parse_theme_json_content(name, builtin_theme_source(name).unwrap()).unwrap();
        assert_eq!(parsed.name, name);
    }
}
