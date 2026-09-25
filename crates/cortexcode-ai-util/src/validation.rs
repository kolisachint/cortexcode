//! Tool-argument validation.
//!
//! Port of hoocode `utils/validation.ts` (v0.5.89) and the parts of TypeBox
//! 1.1 it relies on:
//!
//! - [`typebox_convert`]: `Value.Convert` for the schema kinds tool
//!   parameters use (hoocode's built-in tools are TypeBox schemas, so their
//!   arguments are converted this way);
//! - [`coerce_with_json_schema`]: the AJV-style coercion hoocode applies to
//!   plain JSON-schema tools (MCP), for which `Value.Convert` is a no-op;
//! - a validator that reports errors like TypeBox's compiled validator (same
//!   keyword order, instance paths and `en_US` messages), since the failure
//!   text goes back to the model.
//!
//! Known deviations: `minLength`/`maxLength` count chars, not graphemes;
//! `$ref`, `if`, `dependencies`, `format` and `unevaluated*` are not checked.

use serde_json::{Map, Value};

/// How a tool's parameter schema was built in hoocode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaOrigin {
    /// A TypeBox schema (built-in tools): `Value.Convert` only.
    TypeBox,
    /// A plain JSON schema (MCP tools): `coerceWithJsonSchema` as well.
    PlainJson,
}

// ---------------------------------------------------------------------------
// JS value helpers
// ---------------------------------------------------------------------------

fn as_f64(v: &Value) -> Option<f64> {
    v.as_f64()
}

/// A JSON number as JS would hold it: integral values as integers.
fn js_number_value(n: f64) -> Value {
    if n.fract() == 0.0 && n.abs() < 9.007_199_254_740_992e15 {
        Value::from(n as i64)
    } else {
        serde_json::Number::from_f64(n).map_or(Value::Null, Value::Number)
    }
}

/// `String(n)` for a finite number.
fn js_number_string(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e21 {
        format!("{}", n as i128)
    } else {
        format!("{n}")
    }
}

fn js_value_string(v: &Value) -> String {
    match v {
        Value::Number(n) => js_number_string(n.as_f64().unwrap_or(0.0)),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// `Number(string)` (NaN as `None`).
fn js_string_to_number(s: &str) -> Option<f64> {
    let t = s.trim();
    if t.is_empty() {
        return Some(0.0);
    }
    let (sign, body) = match t.as_bytes()[0] {
        b'-' => (-1.0, &t[1..]),
        b'+' => (1.0, &t[1..]),
        _ => (1.0, t),
    };
    if body == "Infinity" {
        return Some(sign * f64::INFINITY);
    }
    let radix = |prefix: &[&str], radix| {
        prefix
            .iter()
            .find_map(|p| body.strip_prefix(p))
            .map(|digits| (digits, radix))
    };
    if let Some((digits, r)) = radix(&["0x", "0X"], 16)
        .or_else(|| radix(&["0o", "0O"], 8))
        .or_else(|| radix(&["0b", "0B"], 2))
    {
        // Signed hex/octal/binary literals are NaN in JS.
        if sign < 0.0 || t.starts_with('+') || digits.is_empty() {
            return None;
        }
        return u64::from_str_radix(digits, r).ok().map(|n| n as f64);
    }
    let decimal = regex::Regex::new(r"^(\d+\.?\d*([eE][+-]?\d+)?|\.\d+([eE][+-]?\d+)?)$")
        .expect("valid regex");
    if !decimal.is_match(body) {
        return None;
    }
    body.parse::<f64>().ok().map(|n| sign * n)
}

/// `===` for JSON values (numbers by value); objects/arrays by content.
fn js_deep_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(l, r)| js_deep_equal(l, r))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(k, l)| y.get(k).is_some_and(|r| js_deep_equal(l, r)))
        }
        _ => a == b,
    }
}

fn is_finite_number(v: &Value) -> bool {
    as_f64(v).is_some_and(f64::is_finite)
}

fn is_integer(v: &Value) -> bool {
    as_f64(v).is_some_and(|n| n.is_finite() && n.fract() == 0.0)
}

fn matches_type_name(type_name: &str, value: &Value) -> bool {
    match type_name {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "boolean" => value.is_boolean(),
        "integer" => is_integer(value),
        "number" => is_finite_number(value),
        "null" => value.is_null(),
        "string" => value.is_string(),
        _ => true,
    }
}

// ---------------------------------------------------------------------------
// Validator (TypeBox Compile(...).Check / .Errors)
// ---------------------------------------------------------------------------

/// One validation error, as TypeBox reports it.
#[derive(Debug, Clone, PartialEq)]
pub struct ValidationError {
    pub keyword: &'static str,
    /// JSON-pointer-like path of the failing value (`/a/0`).
    pub instance_path: String,
    /// The `en_US` message.
    pub message: String,
    /// For `required`: the missing properties.
    pub required_properties: Vec<String>,
}

fn add_error(
    errors: &mut Vec<ValidationError>,
    keyword: &'static str,
    instance_path: &str,
    message: String,
) -> bool {
    errors.push(ValidationError {
        keyword,
        instance_path: instance_path.to_string(),
        message,
        required_properties: Vec::new(),
    });
    false
}

fn limit(schema: &Value, key: &str) -> String {
    js_value_string(&schema[key])
}

/// `ErrorSchema`: validate `value`, appending TypeBox's errors in its order.
fn schema_errors(
    schema: &Value,
    value: &Value,
    path: &str,
    errors: &mut Vec<ValidationError>,
) -> bool {
    let obj = match schema {
        Value::Bool(true) => return true,
        Value::Bool(false) => return add_error(errors, "boolean", path, "schema is false".into()),
        Value::Object(obj) => obj,
        _ => return true,
    };
    let mut ok = true;

    if let Some(t) = obj.get("type") {
        let matches = match t {
            Value::String(name) => matches_type_name(name, value),
            Value::Array(names) => names
                .iter()
                .filter_map(Value::as_str)
                .any(|n| matches_type_name(n, value)),
            _ => true,
        };
        if !matches {
            let message = match t {
                Value::Array(names) => format!(
                    "must be either {}",
                    names
                        .iter()
                        .map(js_value_string)
                        .collect::<Vec<_>>()
                        .join(" or ")
                ),
                other => format!("must be {}", js_value_string(other)),
            };
            ok &= add_error(errors, "type", path, message);
        }
    }

    if let Value::Object(map) = value {
        if let Some(Value::Array(required)) = obj.get("required") {
            let missing: Vec<String> = required
                .iter()
                .filter_map(Value::as_str)
                .filter(|k| !map.contains_key(*k))
                .map(str::to_string)
                .collect();
            if !missing.is_empty() {
                errors.push(ValidationError {
                    keyword: "required",
                    instance_path: path.to_string(),
                    message: format!("must have required properties {}", missing.join(", ")),
                    required_properties: missing,
                });
                ok = false;
            }
        }
        if let Some(additional) = obj.get("additionalProperties") {
            let props = obj.get("properties").and_then(Value::as_object);
            let patterns: Vec<regex::Regex> = obj
                .get("patternProperties")
                .and_then(Value::as_object)
                .map(|p| p.keys().filter_map(|k| regex::Regex::new(k).ok()).collect())
                .unwrap_or_default();
            let mut extra = Vec::new();
            for (key, v) in map {
                let known = props.is_some_and(|p| p.contains_key(key))
                    || patterns.iter().any(|re| re.is_match(key));
                if !known
                    && !schema_errors(additional, v, &format!("{path}/{key}"), &mut Vec::new())
                {
                    extra.push(key.clone());
                }
            }
            if !extra.is_empty() {
                ok &= add_error(
                    errors,
                    "additionalProperties",
                    path,
                    "must not have additional properties".into(),
                );
            }
        }
        if let Some(Value::Object(patterns)) = obj.get("patternProperties") {
            for (pattern, sub) in patterns {
                let Ok(re) = regex::Regex::new(pattern) else {
                    continue;
                };
                for (key, v) in map {
                    if re.is_match(key) {
                        ok &= schema_errors(sub, v, &format!("{path}/{key}"), errors);
                    }
                }
            }
        }
        if let Some(Value::Object(props)) = obj.get("properties") {
            for (key, sub) in props {
                if let Some(v) = map.get(key) {
                    ok &= schema_errors(sub, v, &format!("{path}/{key}"), errors);
                }
            }
        }
        let count = map.len() as f64;
        if let Some(n) = obj.get("minProperties").and_then(Value::as_f64) {
            if count < n {
                ok &= add_error(
                    errors,
                    "minProperties",
                    path,
                    format!(
                        "must not have fewer than {} properties",
                        limit(schema, "minProperties")
                    ),
                );
            }
        }
        if let Some(n) = obj.get("maxProperties").and_then(Value::as_f64) {
            if count > n {
                ok &= add_error(
                    errors,
                    "maxProperties",
                    path,
                    format!(
                        "must not have more than {} properties",
                        limit(schema, "maxProperties")
                    ),
                );
            }
        }
    }

    if let Value::Array(items) = value {
        match obj.get("items") {
            Some(Value::Array(tuple)) => {
                for (i, sub) in tuple.iter().enumerate() {
                    if let Some(v) = items.get(i) {
                        ok &= schema_errors(sub, v, &format!("{path}/{i}"), errors);
                    }
                }
            }
            Some(sub) => {
                let offset = obj
                    .get("prefixItems")
                    .and_then(Value::as_array)
                    .map_or(0, Vec::len);
                for (i, v) in items.iter().enumerate().skip(offset) {
                    ok &= schema_errors(sub, v, &format!("{path}/{i}"), errors);
                }
            }
            None => {}
        }
        let count = items.len() as f64;
        if let Some(n) = obj.get("maxItems").and_then(Value::as_f64) {
            if count > n {
                ok &= add_error(
                    errors,
                    "maxItems",
                    path,
                    format!(
                        "must not have more than {} items",
                        limit(schema, "maxItems")
                    ),
                );
            }
        }
        if let Some(n) = obj.get("minItems").and_then(Value::as_f64) {
            if count < n {
                ok &= add_error(
                    errors,
                    "minItems",
                    path,
                    format!(
                        "must not have fewer than {} items",
                        limit(schema, "minItems")
                    ),
                );
            }
        }
        if let Some(Value::Array(prefix)) = obj.get("prefixItems") {
            for (i, sub) in prefix.iter().enumerate() {
                if let Some(v) = items.get(i) {
                    ok &= schema_errors(sub, v, &format!("{path}/{i}"), errors);
                }
            }
        }
        if obj.get("uniqueItems") == Some(&Value::Bool(true)) {
            let duplicate = items
                .iter()
                .enumerate()
                .any(|(i, a)| items[..i].iter().any(|b| js_deep_equal(a, b)));
            if duplicate {
                ok &= add_error(
                    errors,
                    "uniqueItems",
                    path,
                    "must not have duplicate items".into(),
                );
            }
        }
    }

    if let Value::String(s) = value {
        let len = s.chars().count() as f64;
        if let Some(n) = obj.get("maxLength").and_then(Value::as_f64) {
            if len > n {
                ok &= add_error(
                    errors,
                    "maxLength",
                    path,
                    format!(
                        "must not have more than {} characters",
                        limit(schema, "maxLength")
                    ),
                );
            }
        }
        if let Some(n) = obj.get("minLength").and_then(Value::as_f64) {
            if len < n {
                ok &= add_error(
                    errors,
                    "minLength",
                    path,
                    format!(
                        "must not have fewer than {} characters",
                        limit(schema, "minLength")
                    ),
                );
            }
        }
        if let Some(pattern) = obj.get("pattern").and_then(Value::as_str) {
            if regex::Regex::new(pattern).is_ok_and(|re| !re.is_match(s)) {
                ok &= add_error(
                    errors,
                    "pattern",
                    path,
                    format!("must match pattern \"{pattern}\""),
                );
            }
        }
    }

    if let Some(n) = value.as_f64() {
        type Bound = (
            &'static str,
            &'static str,
            &'static str,
            fn(f64, f64) -> bool,
        );
        let checks: [Bound; 4] = [
            ("exclusiveMaximum", "exclusiveMaximum", "<", |v, l| v < l),
            ("exclusiveMinimum", "exclusiveMinimum", ">", |v, l| v > l),
            ("maximum", "maximum", "<=", |v, l| v <= l),
            ("minimum", "minimum", ">=", |v, l| v >= l),
        ];
        for (key, keyword, comparison, pass) in checks {
            if let Some(l) = obj.get(key).and_then(Value::as_f64) {
                if !pass(n, l) {
                    ok &= add_error(
                        errors,
                        keyword,
                        path,
                        format!("must be {comparison} {}", limit(schema, key)),
                    );
                }
            }
        }
        if let Some(m) = obj.get("multipleOf").and_then(Value::as_f64) {
            let q = n / m;
            if m != 0.0 && (q - q.round()).abs() > 1e-9 {
                ok &= add_error(
                    errors,
                    "multipleOf",
                    path,
                    format!("must be multiple of {}", limit(schema, "multipleOf")),
                );
            }
        }
    }

    if let Some(c) = obj.get("const") {
        if !js_deep_equal(value, c) {
            ok &= add_error(errors, "const", path, "must be equal to constant".into());
        }
    }
    if let Some(Value::Array(options)) = obj.get("enum") {
        if !options.iter().any(|o| js_deep_equal(value, o)) {
            ok &= add_error(
                errors,
                "enum",
                path,
                "must be equal to one of the allowed values".into(),
            );
        }
    }
    if let Some(not) = obj.get("not") {
        if schema_errors(not, value, path, &mut Vec::new()) {
            ok &= add_error(errors, "not", path, "must not be valid".into());
        }
    }
    if let Some(Value::Array(all)) = obj.get("allOf") {
        let mut failed = Vec::new();
        for sub in all {
            let mut sub_errors = Vec::new();
            if !schema_errors(sub, value, path, &mut sub_errors) {
                failed.extend(sub_errors);
            }
        }
        if !failed.is_empty() {
            errors.extend(failed);
            ok = false;
        }
    }
    if let Some(Value::Array(any)) = obj.get("anyOf") {
        let mut failed = Vec::new();
        let mut passed = false;
        for sub in any {
            let mut sub_errors = Vec::new();
            if schema_errors(sub, value, path, &mut sub_errors) {
                passed = true;
            } else {
                failed.extend(sub_errors);
            }
        }
        if !passed {
            errors.extend(failed);
            ok &= add_error(errors, "anyOf", path, "must match a schema in anyOf".into());
        }
    }
    if let Some(Value::Array(one)) = obj.get("oneOf") {
        let mut failed = Vec::new();
        let mut passing = 0;
        for sub in one {
            let mut sub_errors = Vec::new();
            if schema_errors(sub, value, path, &mut sub_errors) {
                passing += 1;
            } else {
                failed.extend(sub_errors);
            }
        }
        if passing != 1 {
            if passing == 0 {
                errors.extend(failed);
            }
            ok &= add_error(
                errors,
                "oneOf",
                path,
                "must match exactly one schema in oneOf".into(),
            );
        }
    }
    ok
}

/// `validator.Check(value)`.
pub fn check_schema(schema: &Value, value: &Value) -> bool {
    schema_errors(schema, value, "", &mut Vec::new())
}

/// `validator.Errors(value)`.
pub fn schema_validation_errors(schema: &Value, value: &Value) -> Vec<ValidationError> {
    let mut errors = Vec::new();
    schema_errors(schema, value, "", &mut errors);
    errors
}

// ---------------------------------------------------------------------------
// TypeBox Value.Convert
// ---------------------------------------------------------------------------

fn try_number(value: &Value) -> Option<f64> {
    match value {
        Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        Value::Number(n) => n.as_f64(),
        Value::Null => Some(0.0),
        Value::String(s) => {
            if let Some(n) = js_string_to_number(s).filter(|n| n.is_finite()) {
                return Some(n);
            }
            match s.to_lowercase().as_str() {
                "false" => Some(0.0),
                "true" => Some(1.0),
                _ => None,
            }
        }
        _ => None,
    }
}

fn try_boolean(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(b) => Some(*b),
        Value::Number(n) => match n.as_f64() {
            Some(0.0) => Some(false),
            Some(1.0) => Some(true),
            _ => None,
        },
        Value::Null => Some(false),
        Value::String(s) => match s.to_lowercase().as_str() {
            "false" => Some(false),
            "true" => Some(true),
            _ if s == "0" => Some(false),
            _ if s == "1" => Some(true),
            _ => None,
        },
        _ => None,
    }
}

fn try_string(value: &Value) -> Option<String> {
    match value {
        Value::Bool(b) => Some(b.to_string()),
        Value::Number(_) => Some(js_value_string(value)),
        Value::Null => Some("null".into()),
        Value::String(s) => Some(s.clone()),
        _ => None,
    }
}

fn try_null(value: &Value) -> bool {
    match value {
        Value::Bool(b) => !*b,
        Value::Number(n) => n.as_f64() == Some(0.0),
        Value::Null => true,
        Value::String(s) => {
            let lower = s.to_lowercase();
            lower == "undefined" || lower == "null" || s.is_empty() || s == "0"
        }
        _ => false,
    }
}

/// `Value.Convert(schema, value)` over a serialized TypeBox schema.
pub fn typebox_convert(schema: &Value, value: Value) -> Value {
    let Value::Object(obj) = schema else {
        return value;
    };
    if let Some(Value::Array(any)) = obj.get("anyOf") {
        return convert_union(schema, any, value);
    }
    if let Some(c) = obj.get("const") {
        return convert_literal(c, value);
    }
    if let Some(Value::Array(options)) = obj.get("enum") {
        // `Type.Enum` (no type); a typed enum is `Type.Unsafe` (StringEnum),
        // which Convert leaves alone.
        if obj.get("type").is_some() {
            return value;
        }
        let union: Vec<Value> = options
            .iter()
            .map(|o| serde_json::json!({"const": o}))
            .collect();
        let union_schema = serde_json::json!({"anyOf": union});
        return convert_union(&union_schema, &union, value);
    }
    match obj.get("type").and_then(Value::as_str) {
        Some("number") => try_number(&value).map_or(value, js_number_value),
        Some("integer") => try_number(&value).map_or(value, |n| js_number_value(n.trunc())),
        Some("boolean") => try_boolean(&value).map_or(value, Value::Bool),
        Some("string") => try_string(&value).map_or(value, Value::String),
        Some("null") => {
            if try_null(&value) {
                Value::Null
            } else {
                value
            }
        }
        Some("array") => {
            let items = match value {
                Value::Array(items) => items,
                other => vec![other],
            };
            match obj.get("items") {
                Some(item_schema) => Value::Array(
                    items
                        .into_iter()
                        .map(|v| typebox_convert(item_schema, v))
                        .collect(),
                ),
                None => Value::Array(items),
            }
        }
        Some("object") => match value {
            Value::Object(mut map) => {
                let props = obj.get("properties").and_then(Value::as_object);
                if let Some(props) = props {
                    for (key, sub) in props {
                        if let Some(v) = map.remove(key) {
                            map.insert(key.clone(), typebox_convert(sub, v));
                        }
                    }
                }
                if let Some(additional) = obj.get("additionalProperties").filter(|a| a.is_object())
                {
                    let keys: Vec<String> = map
                        .keys()
                        .filter(|k| !props.is_some_and(|p| p.contains_key(*k)))
                        .cloned()
                        .collect();
                    for key in keys {
                        if let Some(v) = map.remove(&key) {
                            map.insert(key, typebox_convert(additional, v));
                        }
                    }
                }
                Value::Object(map)
            }
            other => other,
        },
        _ => value,
    }
}

fn convert_union(union_schema: &Value, variants: &[Value], value: Value) -> Value {
    if variants.iter().any(|v| check_schema(v, &value)) {
        return value;
    }
    variants
        .iter()
        .map(|v| typebox_convert(v, value.clone()))
        .find(|candidate| check_schema(union_schema, candidate))
        .unwrap_or(value)
}

fn convert_literal(constant: &Value, value: Value) -> Value {
    if js_deep_equal(constant, &value) {
        return value;
    }
    let converted = match constant {
        Value::Number(_) => try_number(&value).map(js_number_value),
        Value::Bool(_) => try_boolean(&value).map(Value::Bool),
        Value::String(_) => try_string(&value).map(Value::String),
        _ => None,
    };
    match converted {
        Some(c) if js_deep_equal(constant, &c) => c,
        _ => value,
    }
}

// ---------------------------------------------------------------------------
// coerceWithJsonSchema (plain JSON schemas)
// ---------------------------------------------------------------------------

fn schema_types(schema: &Value) -> Vec<String> {
    match schema.get("type") {
        Some(Value::String(t)) => vec![t.clone()],
        Some(Value::Array(ts)) => ts
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

/// `coercePrimitiveByType`: `None` when the value stays as it is.
fn coerce_primitive_by_type(value: &Value, type_name: &str) -> Option<Value> {
    match type_name {
        "number" | "integer" => match value {
            Value::Null => Some(Value::from(0)),
            Value::String(s) if !s.trim().is_empty() => {
                let parsed = js_string_to_number(s)?;
                let ok = if type_name == "number" {
                    parsed.is_finite()
                } else {
                    parsed.is_finite() && parsed.fract() == 0.0
                };
                ok.then(|| js_number_value(parsed))
            }
            Value::Bool(b) => Some(Value::from(i64::from(*b))),
            _ => None,
        },
        "boolean" => match value {
            Value::Null => Some(Value::Bool(false)),
            Value::String(s) if s == "true" => Some(Value::Bool(true)),
            Value::String(s) if s == "false" => Some(Value::Bool(false)),
            Value::Number(n) if n.as_f64() == Some(1.0) => Some(Value::Bool(true)),
            Value::Number(n) if n.as_f64() == Some(0.0) => Some(Value::Bool(false)),
            _ => None,
        },
        "string" => match value {
            Value::Null => Some(Value::String(String::new())),
            Value::Number(_) | Value::Bool(_) => Some(Value::String(js_value_string(value))),
            _ => None,
        },
        "null" => match value {
            Value::String(s) if s.is_empty() => Some(Value::Null),
            Value::Number(n) if n.as_f64() == Some(0.0) => Some(Value::Null),
            Value::Bool(false) => Some(Value::Null),
            _ => None,
        },
        _ => None,
    }
}

fn coerce_with_union_schema(value: Value, schemas: &[Value]) -> Value {
    // A value some branch already accepts is left alone (as TypeBox Convert).
    if schemas
        .iter()
        .any(|s| s.is_object() && check_schema(s, &value))
    {
        return value;
    }
    for schema in schemas.iter().filter(|s| s.is_object()) {
        let coerced = coerce_with_json_schema(value.clone(), schema);
        if check_schema(schema, &coerced) {
            return coerced;
        }
    }
    value
}

/// `coerceWithJsonSchema`: AJV-compatible primitive coercion.
pub fn coerce_with_json_schema(value: Value, schema: &Value) -> Value {
    let mut next = value;
    if let Some(Value::Array(all)) = schema.get("allOf") {
        for nested in all {
            next = coerce_with_json_schema(next, nested);
        }
    }
    if let Some(Value::Array(any)) = schema.get("anyOf") {
        next = coerce_with_union_schema(next, any);
    }
    if let Some(Value::Array(one)) = schema.get("oneOf") {
        next = coerce_with_union_schema(next, one);
    }

    let types = schema_types(schema);
    let matches_union_member = types.len() > 1 && types.iter().any(|t| matches_type_name(t, &next));
    if !types.is_empty() && !matches_union_member {
        if let Some(candidate) = types
            .iter()
            .find_map(|t| coerce_primitive_by_type(&next, t))
        {
            next = candidate;
        }
    }

    if types.iter().any(|t| t == "object") {
        if let Value::Object(map) = &mut next {
            let props = schema.get("properties").and_then(Value::as_object);
            if let Some(props) = props {
                for (key, sub) in props {
                    if let Some(v) = map.remove(key) {
                        map.insert(key.clone(), coerce_with_json_schema(v, sub));
                    }
                }
            }
            if let Some(additional) = schema.get("additionalProperties").filter(|a| a.is_object()) {
                let keys: Vec<String> = map
                    .keys()
                    .filter(|k| !props.is_some_and(|p| p.contains_key(*k)))
                    .cloned()
                    .collect();
                for key in keys {
                    if let Some(v) = map.remove(&key) {
                        map.insert(key, coerce_with_json_schema(v, additional));
                    }
                }
            }
        }
    }
    if types.iter().any(|t| t == "array") {
        if let Value::Array(items) = &mut next {
            match schema.get("items") {
                Some(Value::Array(tuple)) => {
                    for (i, item) in items.iter_mut().enumerate() {
                        if let Some(sub) = tuple.get(i) {
                            *item = coerce_with_json_schema(item.take(), sub);
                        }
                    }
                }
                Some(sub) if sub.is_object() => {
                    for item in items.iter_mut() {
                        *item = coerce_with_json_schema(item.take(), sub);
                    }
                }
                _ => {}
            }
        }
    }
    next
}

// ---------------------------------------------------------------------------
// validateToolArguments
// ---------------------------------------------------------------------------

/// `formatValidationPath`.
fn format_validation_path(error: &ValidationError) -> String {
    let base = error
        .instance_path
        .strip_prefix('/')
        .unwrap_or(&error.instance_path)
        .replace('/', ".");
    if error.keyword == "required" {
        if let Some(first) = error.required_properties.first() {
            return if base.is_empty() {
                first.clone()
            } else {
                format!("{base}.{first}")
            };
        }
    }
    if base.is_empty() {
        "root".to_string()
    } else {
        base
    }
}

/// Integral floats as integers, as `JSON.stringify` prints them.
fn js_normalize(value: &Value) -> Value {
    match value {
        Value::Number(n) => n.as_f64().map_or(value.clone(), js_number_value),
        Value::Array(items) => Value::Array(items.iter().map(js_normalize).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), js_normalize(v)))
                .collect::<Map<String, Value>>(),
        ),
        other => other.clone(),
    }
}

/// `validateToolArguments`: the converted/coerced arguments, or the TS error
/// text (`Validation failed for tool "…":` with one `  - path: message` line
/// per error and the received arguments).
pub fn validate_tool_arguments(
    tool_name: &str,
    schema: &Value,
    arguments: &Value,
    origin: SchemaOrigin,
) -> Result<Value, String> {
    let mut args = arguments.clone();
    if origin == SchemaOrigin::TypeBox {
        args = typebox_convert(schema, args);
    } else if schema.is_object() {
        let coerced = coerce_with_json_schema(args.clone(), schema);
        if args.is_object() && coerced.is_object() {
            args = coerced;
        } else if coerced != args {
            return Ok(if check_schema(schema, &coerced) {
                coerced
            } else {
                args
            });
        }
    }

    let errors = schema_validation_errors(schema, &args);
    if errors.is_empty() {
        return Ok(args);
    }
    let lines = errors
        .iter()
        .map(|e| format!("  - {}: {}", format_validation_path(e), e.message))
        .collect::<Vec<_>>()
        .join("\n");
    let received = serde_json::to_string_pretty(&js_normalize(arguments)).unwrap_or_default();
    Err(format!(
        "Validation failed for tool \"{tool_name}\":\n{lines}\n\nReceived arguments:\n{received}"
    ))
}

#[cfg(test)]
#[path = "validation_tests.rs"]
mod tests;
