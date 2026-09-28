//! The theme JSON schema (`ThemeJsonSchema`) and its validation, with the
//! error text hoocode's compiled typebox validator produces.

use serde_json::{Map, Value};

use crate::color::RawColor;

/// A color as written in theme JSON: a hex string, a `vars` reference, `""`,
/// or a 256-color index.
pub type ColorValue = RawColor;

/// Required color tokens, in schema order.
pub const REQUIRED_COLOR_TOKENS: [&str; 51] = [
    // Core UI
    "accent",
    "border",
    "borderAccent",
    "borderMuted",
    "success",
    "error",
    "warning",
    "muted",
    "dim",
    "text",
    "thinkingText",
    // Backgrounds & content text
    "selectedBg",
    "userMessageBg",
    "userMessageText",
    "customMessageBg",
    "customMessageText",
    "customMessageLabel",
    "toolPendingBg",
    "toolSuccessBg",
    "toolErrorBg",
    "toolTitle",
    "toolOutput",
    // Markdown
    "mdHeading",
    "mdLink",
    "mdLinkUrl",
    "mdCode",
    "mdCodeBlock",
    "mdCodeBlockBorder",
    "mdQuote",
    "mdQuoteBorder",
    "mdHr",
    "mdListBullet",
    // Tool diffs
    "toolDiffAdded",
    "toolDiffRemoved",
    "toolDiffContext",
    // Syntax highlighting
    "syntaxComment",
    "syntaxKeyword",
    "syntaxFunction",
    "syntaxVariable",
    "syntaxString",
    "syntaxNumber",
    "syntaxType",
    "syntaxOperator",
    "syntaxPunctuation",
    // Thinking level borders
    "thinkingOff",
    "thinkingMinimal",
    "thinkingLow",
    "thinkingMedium",
    "thinkingHigh",
    "thinkingXhigh",
    // Bash mode
    "bashMode",
];

/// Every color property the schema declares, in declaration order, with
/// whether it is optional.
const COLOR_PROPERTIES: [(&str, bool); 67] = [
    ("accent", false),
    ("border", false),
    ("borderAccent", false),
    ("borderMuted", false),
    ("success", false),
    ("error", false),
    ("warning", false),
    ("muted", false),
    ("dim", false),
    ("text", false),
    ("thinkingText", false),
    ("selectedBg", false),
    ("userMessageBg", false),
    ("userMessageText", false),
    ("customMessageBg", false),
    ("customMessageText", false),
    ("customMessageLabel", false),
    ("toolPendingBg", false),
    ("toolSuccessBg", false),
    ("toolErrorBg", false),
    ("warningBg", true),
    ("toolTitle", false),
    ("toolOutput", false),
    ("mdHeading", false),
    ("mdLink", false),
    ("mdLinkUrl", false),
    ("mdCode", false),
    ("mdCodeBlock", false),
    ("mdCodeBlockBorder", false),
    ("mdQuote", false),
    ("mdQuoteBorder", false),
    ("mdHr", false),
    ("mdListBullet", false),
    ("toolDiffAdded", false),
    ("toolDiffRemoved", false),
    ("toolDiffContext", false),
    ("syntaxComment", false),
    ("syntaxKeyword", false),
    ("syntaxFunction", false),
    ("syntaxVariable", false),
    ("syntaxString", false),
    ("syntaxNumber", false),
    ("syntaxType", false),
    ("syntaxOperator", false),
    ("syntaxPunctuation", false),
    ("thinkingOff", false),
    ("thinkingMinimal", false),
    ("thinkingLow", false),
    ("thinkingMedium", false),
    ("thinkingHigh", false),
    ("thinkingXhigh", false),
    ("bashMode", false),
    ("agent1", true),
    ("agent2", true),
    ("agent3", true),
    ("agent4", true),
    ("agent5", true),
    ("agent6", true),
    ("brandBg", true),
    ("brandText", true),
    ("paperShadow", true),
    ("halftone", true),
    ("headlineBg", true),
    ("headlineText", true),
    ("tapeBg", true),
    ("tapeText", true),
    ("activeToolBg", true),
];

/// The optional `export` section.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ThemeExportJson {
    pub page_bg: Option<ColorValue>,
    pub card_bg: Option<ColorValue>,
    pub info_bg: Option<ColorValue>,
}

/// A validated theme file (`ThemeJson`). `colors` and `vars` keep file order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeJson {
    pub schema: Option<String>,
    pub name: String,
    pub description: Option<String>,
    pub vars: Option<Vec<(String, ColorValue)>>,
    pub colors: Vec<(String, ColorValue)>,
    pub export: Option<ThemeExportJson>,
}

impl ThemeJson {
    /// The value of a color token, if the file sets it.
    pub fn color(&self, key: &str) -> Option<&ColorValue> {
        self.colors.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    /// Serialize back to JSON (file order kept).
    pub fn to_value(&self) -> Value {
        let mut root = Map::new();
        if let Some(s) = &self.schema {
            root.insert("$schema".into(), Value::String(s.clone()));
        }
        root.insert("name".into(), Value::String(self.name.clone()));
        if let Some(d) = &self.description {
            root.insert("description".into(), Value::String(d.clone()));
        }
        let to_json = |v: &ColorValue| match v {
            RawColor::Str(s) => Value::String(s.clone()),
            RawColor::Index(n) => Value::from(*n),
        };
        let record = |entries: &[(String, ColorValue)]| {
            Value::Object(
                entries
                    .iter()
                    .map(|(k, v)| (k.clone(), to_json(v)))
                    .collect(),
            )
        };
        if let Some(vars) = &self.vars {
            root.insert("vars".into(), record(vars));
        }
        root.insert("colors".into(), record(&self.colors));
        if let Some(export) = &self.export {
            let mut e = Map::new();
            for (key, value) in [
                ("pageBg", &export.page_bg),
                ("cardBg", &export.card_bg),
                ("infoBg", &export.info_bg),
            ] {
                if let Some(v) = value {
                    e.insert(key.into(), to_json(v));
                }
            }
            root.insert("export".into(), Value::Object(e));
        }
        Value::Object(root)
    }
}

/// A JSON pointer segment (`~` and `/` escaped).
fn pointer(segment: &str) -> String {
    segment.replace('~', "~0").replace('/', "~1")
}

/// A value against `ColorValueSchema`: `Ok` or its typebox errors.
fn check_color_value(value: &Value) -> Result<ColorValue, Vec<&'static str>> {
    match value {
        Value::String(s) => Ok(RawColor::Str(s.clone())),
        Value::Number(n) => {
            let integral = n
                .as_i64()
                .map(|i| i as f64)
                .or_else(|| n.as_u64().map(|u| u as f64))
                .or_else(|| n.as_f64().filter(|f| f.fract() == 0.0));
            match integral {
                None => Err(vec!["must be integer"]),
                Some(v) if v < 0.0 => Err(vec!["must be >= 0"]),
                Some(v) if v > 255.0 => Err(vec!["must be <= 255"]),
                Some(v) => Ok(RawColor::Index(v as u8)),
            }
        }
        _ => Err(vec!["must be integer"]),
    }
}

/// Collects validation errors in the order the compiled validator reports
/// them.
struct Errors {
    missing_colors: Vec<String>,
    other: Vec<String>,
}

impl Errors {
    fn push(&mut self, path: &str, message: &str) {
        let path = if path.is_empty() { "/" } else { path };
        self.other.push(format!("  - {path}: {message}"));
    }

    fn color(&mut self, path: &str, value: &Value) -> Option<ColorValue> {
        match check_color_value(value) {
            Ok(v) => Some(v),
            Err(inner) => {
                self.push(path, "must be string");
                for message in inner {
                    self.push(path, message);
                }
                self.push(path, "must match a schema in anyOf");
                None
            }
        }
    }

    fn record(&mut self, path: &str, value: &Value) -> Option<Vec<(String, ColorValue)>> {
        let Value::Object(map) = value else {
            self.push(path, "must be object");
            return None;
        };
        let mut out = Vec::new();
        let mut ok = true;
        for (key, v) in map {
            match self.color(&format!("{path}/{}", pointer(key)), v) {
                Some(c) => out.push((key.clone(), c)),
                None => ok = false,
            }
        }
        ok.then_some(out)
    }
}

/// `parseThemeJson`: validate a parsed theme against the schema.
pub fn parse_theme_json(label: &str, json: &Value) -> Result<ThemeJson, String> {
    let mut errors = Errors {
        missing_colors: Vec::new(),
        other: Vec::new(),
    };
    let result = validate(json, &mut errors);
    if errors.missing_colors.is_empty() && errors.other.is_empty() {
        if let Some(theme) = result {
            return Ok(theme);
        }
    }

    let mut message = format!("Invalid theme \"{label}\":\n");
    if !errors.missing_colors.is_empty() {
        let mut missing = errors.missing_colors.clone();
        missing.sort();
        missing.dedup();
        message.push_str("\nMissing required color tokens:\n");
        message.push_str(
            &missing
                .iter()
                .map(|c| format!("  - {c}"))
                .collect::<Vec<_>>()
                .join("\n"),
        );
        message.push_str("\n\nPlease add these colors to your theme's \"colors\" object.");
        message.push_str("\nSee the built-in themes (dark.json, light.json) for reference values.");
    }
    if !errors.other.is_empty() {
        message.push_str(&format!("\n\nOther errors:\n{}", errors.other.join("\n")));
    }
    Err(message)
}

fn validate(json: &Value, errors: &mut Errors) -> Option<ThemeJson> {
    let Value::Object(root) = json else {
        errors.push("", "must be object");
        return None;
    };
    let missing: Vec<&str> = ["name", "colors"]
        .into_iter()
        .filter(|k| !root.contains_key(*k))
        .collect();
    if !missing.is_empty() {
        errors.push(
            "",
            &format!("must have required properties {}", missing.join(", ")),
        );
    }

    let mut ok = missing.is_empty();
    let string_prop = |errors: &mut Errors, key: &str| -> (bool, Option<String>) {
        match root.get(key) {
            None => (true, None),
            Some(Value::String(s)) => (true, Some(s.clone())),
            Some(_) => {
                errors.push(&format!("/{}", pointer(key)), "must be string");
                (false, None)
            }
        }
    };
    let (schema_ok, schema) = string_prop(errors, "$schema");
    let (name_ok, name) = string_prop(errors, "name");
    let (desc_ok, description) = string_prop(errors, "description");
    ok &= schema_ok && name_ok && desc_ok;

    let vars = match root.get("vars") {
        None => None,
        Some(v) => {
            let r = errors.record("/vars", v);
            ok &= r.is_some();
            r
        }
    };

    let colors = match root.get("colors") {
        None => None,
        Some(Value::Object(map)) => {
            for (key, optional) in COLOR_PROPERTIES {
                if !optional && !map.contains_key(key) {
                    errors.missing_colors.push(key.to_string());
                    ok = false;
                }
            }
            let mut valid = true;
            for (key, _) in COLOR_PROPERTIES {
                if let Some(v) = map.get(key) {
                    if errors.color(&format!("/colors/{key}"), v).is_none() {
                        valid = false;
                    }
                }
            }
            ok &= valid;
            // Undeclared tokens (e.g. `mcp`) pass the schema; keep the ones
            // that are usable color values.
            let mut out = Vec::new();
            for (key, v) in map {
                if let Ok(c) = check_color_value(v) {
                    out.push((key.clone(), c));
                }
            }
            Some(out)
        }
        Some(_) => {
            errors.push("/colors", "must be object");
            ok = false;
            None
        }
    };

    let export = match root.get("export") {
        None => None,
        Some(Value::Object(map)) => {
            let mut export = ThemeExportJson::default();
            for (key, slot) in [
                ("pageBg", &mut export.page_bg),
                ("cardBg", &mut export.card_bg),
                ("infoBg", &mut export.info_bg),
            ] {
                if let Some(v) = map.get(key) {
                    match errors.color(&format!("/export/{key}"), v) {
                        Some(c) => *slot = Some(c),
                        None => ok = false,
                    }
                }
            }
            Some(export)
        }
        Some(_) => {
            errors.push("/export", "must be object");
            ok = false;
            None
        }
    };

    if !ok {
        return None;
    }
    Some(ThemeJson {
        schema,
        name: name?,
        description,
        vars,
        colors: colors?,
        export,
    })
}

/// `parseThemeJsonContent`: parse and validate a theme file's text.
pub fn parse_theme_json_content(label: &str, content: &str) -> Result<ThemeJson, String> {
    let json: Value = serde_json::from_str(content)
        .map_err(|error| format!("Failed to parse theme {label}: SyntaxError: {error}"))?;
    parse_theme_json(label, &json)
}
