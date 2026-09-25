//! Helpers for constraining tool-call argument decoding.
//!
//! Port of hoocode `utils/tool-constraints.ts` (v0.5.89): OpenAI strict
//! function calling needs a closed schema (every property required,
//! `additionalProperties: false`, only a subset of keywords).

use serde_json::{Map, Value};

const STRICT_KEYWORD_ALLOWLIST: &[&str] = &[
    "$defs",
    "$ref",
    "additionalProperties",
    "anyOf",
    "const",
    "definitions",
    "description",
    "enum",
    "items",
    "properties",
    "required",
    "title",
    "type",
];

/// A formerly-optional property becomes "this type or null".
fn make_nullable(schema: Value) -> Value {
    let Value::Object(mut obj) = schema else {
        return schema;
    };
    match obj.get("type").cloned() {
        Some(Value::String(t)) => {
            if t != "null" {
                obj.insert("type".into(), serde_json::json!([t, "null"]));
            }
            return Value::Object(obj);
        }
        Some(Value::Array(mut types)) => {
            if !types.iter().any(|t| t == "null") {
                types.push(Value::String("null".into()));
                obj.insert("type".into(), Value::Array(types));
            }
            return Value::Object(obj);
        }
        _ => {}
    }
    if let Some(Value::Array(mut values)) = obj.get("enum").cloned() {
        if !values.contains(&Value::Null) {
            values.push(Value::Null);
            obj.insert("enum".into(), Value::Array(values));
        }
        return Value::Object(obj);
    }
    if let Some(Value::Array(mut variants)) = obj.get("anyOf").cloned() {
        let has_null = variants
            .iter()
            .any(|v| v.get("type").and_then(Value::as_str) == Some("null"));
        if !has_null {
            variants.push(serde_json::json!({"type": "null"}));
            obj.insert("anyOf".into(), Value::Array(variants));
        }
        return Value::Object(obj);
    }
    serde_json::json!({"anyOf": [Value::Object(obj), {"type": "null"}]})
}

fn transform_node(node: Value) -> Value {
    let Value::Object(node) = node else {
        return node;
    };
    let mut out: Map<String, Value> = node
        .into_iter()
        .filter(|(k, _)| STRICT_KEYWORD_ALLOWLIST.contains(&k.as_str()))
        .collect();

    if let Some(Value::Object(props)) = out.get("properties").cloned() {
        let originally_required: Vec<String> = match out.get("required") {
            Some(Value::Array(names)) => names
                .iter()
                .filter_map(|n| n.as_str().map(str::to_string))
                .collect(),
            _ => Vec::new(),
        };
        let mut properties = Map::new();
        for (name, schema) in props {
            let transformed = transform_node(schema);
            let value = if originally_required.contains(&name) {
                transformed
            } else {
                make_nullable(transformed)
            };
            properties.insert(name, value);
        }
        let required: Vec<Value> = properties.keys().cloned().map(Value::String).collect();
        out.insert("properties".into(), Value::Object(properties));
        out.insert("required".into(), Value::Array(required));
        out.insert("additionalProperties".into(), Value::Bool(false));
    } else if out.get("type").and_then(Value::as_str) == Some("object") {
        out.insert("properties".into(), Value::Object(Map::new()));
        out.insert("required".into(), Value::Array(Vec::new()));
        out.insert("additionalProperties".into(), Value::Bool(false));
    }

    if let Some(items) = out.remove("items") {
        let items = match items {
            Value::Array(list) => Value::Array(list.into_iter().map(transform_node).collect()),
            other => transform_node(other),
        };
        out.insert("items".into(), items);
    }
    if let Some(Value::Array(variants)) = out.get("anyOf").cloned() {
        out.insert(
            "anyOf".into(),
            Value::Array(variants.into_iter().map(transform_node).collect()),
        );
    }
    for defs_key in ["$defs", "definitions"] {
        if let Some(Value::Object(defs)) = out.get(defs_key).cloned() {
            let defs = defs
                .into_iter()
                .map(|(name, def)| (name, transform_node(def)))
                .collect();
            out.insert(defs_key.into(), Value::Object(defs));
        }
    }
    Value::Object(out)
}

/// `toStrictJsonSchema`: the closed form OpenAI strict function calling
/// accepts. Non-object input yields `{}`.
pub fn to_strict_json_schema(schema: &Value) -> Value {
    match transform_node(schema.clone()) {
        Value::Object(obj) => Value::Object(obj),
        _ => Value::Object(Map::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn closes_objects_and_makes_optionals_nullable() {
        let schema = json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "minLength": 1},
                "limit": {"type": "number", "default": 5},
                "mode": {"enum": ["a", "b"]},
                "opts": {"type": "object"},
                "tags": {"type": "array", "items": {"type": "string", "pattern": "x"}}
            },
            "required": ["path"]
        });
        assert_eq!(
            to_strict_json_schema(&schema),
            json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "limit": {"type": ["number", "null"]},
                    "mode": {"enum": ["a", "b", null]},
                    "opts": {"type": ["object", "null"], "properties": {}, "required": [], "additionalProperties": false},
                    "tags": {"type": ["array", "null"], "items": {"type": "string"}}
                },
                "required": ["path", "limit", "mode", "opts", "tags"],
                "additionalProperties": false
            })
        );
    }

    #[test]
    fn wraps_untyped_optionals_in_any_of() {
        let schema = json!({"properties": {"x": {"description": "d"}}});
        assert_eq!(
            to_strict_json_schema(&schema)["properties"]["x"],
            json!({"anyOf": [{"description": "d"}, {"type": "null"}]})
        );
        assert_eq!(to_strict_json_schema(&json!(null)), json!({}));
    }

    // --- constrain-tool-calls.test.ts: toStrictJsonSchema ---

    /// The TypeBox `editSchema` of the TS test, as JSON.
    fn edit_schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"description": "File path", "minLength": 1, "type": "string"},
                "oldText": {"type": "string"},
                "newText": {"type": "string"},
                "replaceAll": {"default": false, "type": "boolean"}
            },
            "required": ["path", "oldText", "newText"]
        })
    }

    #[test]
    fn edit_schema_is_closed_with_nullable_optionals() {
        let strict = to_strict_json_schema(&edit_schema());
        assert_eq!(strict["additionalProperties"], false);
        assert_eq!(
            strict["required"],
            json!(["path", "oldText", "newText", "replaceAll"])
        );
        let properties = &strict["properties"];
        assert_eq!(properties["replaceAll"]["type"], json!(["boolean", "null"]));
        assert_eq!(properties["path"]["type"], "string");
        assert!(properties["path"].get("minLength").is_none());
        assert!(properties["replaceAll"].get("default").is_none());
        assert_eq!(properties["path"]["description"], "File path");
    }

    #[test]
    fn recurses_into_arrays_and_nested_objects() {
        let nested = json!({
            "type": "object",
            "properties": {"edits": {"type": "array", "items": {
                "type": "object",
                "properties": {
                    "oldText": {"type": "string"},
                    "newText": {"type": "string"},
                    "replaceAll": {"type": "boolean"}
                },
                "required": ["oldText", "newText"]
            }}},
            "required": ["edits"]
        });
        let items = &to_strict_json_schema(&nested)["properties"]["edits"]["items"];
        assert_eq!(items["additionalProperties"], false);
        assert_eq!(
            items["required"],
            json!(["oldText", "newText", "replaceAll"])
        );
        assert_eq!(
            items["properties"]["replaceAll"]["type"],
            json!(["boolean", "null"])
        );
    }

    #[test]
    fn enum_and_any_of_become_nullable_without_widening_type() {
        let schema = json!({
            "type": "object",
            "properties": {
                "mode": {"enum": ["a", "b"]},
                "value": {"anyOf": [{"type": "string"}, {"type": "number"}]}
            }
        });
        let properties = &to_strict_json_schema(&schema)["properties"];
        assert_eq!(properties["mode"]["enum"], json!(["a", "b", null]));
        assert_eq!(
            properties["value"]["anyOf"],
            json!([{"type": "string"}, {"type": "number"}, {"type": "null"}])
        );
    }
}
