//! Ported from `validation.test.ts` (hoocode v0.5.89), plus cases recorded
//! from the pinned hoocode's `validateToolArguments` with node
//! (`validation_fixture.json`: a TypeBox read-like schema, run once as TypeBox
//! and once as its serialized plain JSON form).

use super::*;
use serde_json::json;

fn echo_schema(value_schema: Value) -> Value {
    json!({"type": "object", "properties": {"value": value_schema}, "required": ["value"]})
}

fn echo(value_schema: Value, input: Value, origin: SchemaOrigin) -> Result<Value, String> {
    validate_tool_arguments(
        "echo",
        &echo_schema(value_schema),
        &json!({"value": input}),
        origin,
    )
}

#[test]
fn typebox_schemas_convert_like_value_convert() {
    // "still validates when Function constructor is unavailable": Type.Object({count: Type.Number()}).
    let schema = json!({"type": "object", "properties": {"count": {"type": "number"}}, "required": ["count"]});
    assert_eq!(
        validate_tool_arguments(
            "echo",
            &schema,
            &json!({"count": "42"}),
            SchemaOrigin::TypeBox
        ),
        Ok(json!({"count": 42}))
    );
}

#[test]
fn coerces_plain_json_schemas_with_ajv_compatible_primitive_rules() {
    let passing = [
        (json!({"type": "number"}), json!("42"), json!(42)),
        (json!({"type": "number"}), json!(true), json!(1)),
        (json!({"type": "number"}), json!(null), json!(0)),
        (json!({"type": "integer"}), json!("42"), json!(42)),
        (json!({"type": "boolean"}), json!("true"), json!(true)),
        (json!({"type": "boolean"}), json!("false"), json!(false)),
        (json!({"type": "boolean"}), json!(1), json!(true)),
        (json!({"type": "boolean"}), json!(0), json!(false)),
        (json!({"type": "string"}), json!(null), json!("")),
        (json!({"type": "string"}), json!(true), json!("true")),
        (json!({"type": "null"}), json!(""), json!(null)),
        (json!({"type": "null"}), json!(0), json!(null)),
        (json!({"type": "null"}), json!(false), json!(null)),
        (
            json!({"type": ["number", "string"]}),
            json!("1"),
            json!("1"),
        ),
        (json!({"type": ["boolean", "number"]}), json!("1"), json!(1)),
    ];
    for (schema, input, expected) in passing {
        assert_eq!(
            echo(schema.clone(), input.clone(), SchemaOrigin::PlainJson),
            Ok(json!({"value": expected})),
            "{schema} {input}"
        );
    }
}

#[test]
fn leaves_a_value_an_any_of_branch_accepts_uncoerced() {
    let schema = json!({"anyOf": [{"type": "object"}, {"type": "array"}, {"type": "string"}, {"type": "number"}, {"type": "null"}]});
    for input in [
        json!(5),
        json!(null),
        json!("5"),
        json!({"a": 1}),
        json!([1]),
    ] {
        assert_eq!(
            echo(schema.clone(), input.clone(), SchemaOrigin::PlainJson),
            Ok(json!({"value": input}))
        );
    }
}

#[test]
fn rejects_invalid_coercions_for_plain_json_schemas() {
    let failing = [
        (json!({"type": "boolean"}), json!("1")),
        (json!({"type": "boolean"}), json!("0")),
        (json!({"type": "null"}), json!("null")),
        (json!({"type": "integer"}), json!("42.1")),
    ];
    for (schema, input) in failing {
        let err = echo(schema.clone(), input.clone(), SchemaOrigin::PlainJson).unwrap_err();
        assert!(err.contains("Validation failed"), "{schema} {input}: {err}");
    }
}

#[test]
fn matches_hoocode_on_recorded_cases() {
    let fixture: Value =
        serde_json::from_str(include_str!("validation_fixture.json")).expect("fixture");
    let schema = &fixture["schema"];
    for (origin, key) in [
        (SchemaOrigin::TypeBox, "typebox"),
        (SchemaOrigin::PlainJson, "plain"),
    ] {
        for case in fixture[key].as_array().unwrap() {
            let got = validate_tool_arguments("read", schema, &case["args"], origin);
            match (case.get("ok"), case.get("err")) {
                (Some(ok), _) => assert_eq!(got.as_ref(), Ok(ok), "{key} {}", case["args"]),
                (_, Some(err)) => assert_eq!(
                    got.as_ref().map_err(String::as_str),
                    Err(err.as_str().unwrap()),
                    "{key} {}",
                    case["args"]
                ),
                _ => panic!("bad fixture"),
            }
        }
    }
}

#[test]
fn error_messages_follow_typebox_en_us() {
    let errors = schema_validation_errors(
        &json!({"type": ["string", "null"], "minLength": 3}),
        &json!(5),
    );
    assert_eq!(errors[0].message, "must be either string or null");
    let errors = schema_validation_errors(
        &json!({"type": "array", "items": {"type": "number"}, "maxItems": 1, "uniqueItems": true}),
        &json!([1, 1]),
    );
    let messages: Vec<&str> = errors.iter().map(|e| e.message.as_str()).collect();
    assert_eq!(
        messages,
        [
            "must not have more than 1 items",
            "must not have duplicate items"
        ]
    );
    let errors = schema_validation_errors(
        &json!({"type": "string", "pattern": "^a", "maxLength": 2}),
        &json!("bcd"),
    );
    let messages: Vec<&str> = errors.iter().map(|e| e.message.as_str()).collect();
    assert_eq!(
        messages,
        [
            "must not have more than 2 characters",
            "must match pattern \"^a\""
        ]
    );
    let errors = schema_validation_errors(&json!({"enum": ["x", "y"]}), &json!("z"));
    assert_eq!(
        errors[0].message,
        "must be equal to one of the allowed values"
    );
    let errors = schema_validation_errors(
        &json!({"type": "object", "properties": {"a": {"type": "object", "required": ["b"]}}}),
        &json!({"a": {}}),
    );
    assert_eq!(format_validation_path(&errors[0]), "a.b");
}

#[test]
fn typebox_convert_rules() {
    assert_eq!(
        typebox_convert(&json!({"type": "number"}), json!("0x10")),
        json!(16)
    );
    assert_eq!(
        typebox_convert(&json!({"type": "number"}), json!("TRUE")),
        json!(1)
    );
    assert_eq!(
        typebox_convert(&json!({"type": "number"}), json!("abc")),
        json!("abc")
    );
    assert_eq!(
        typebox_convert(&json!({"type": "integer"}), json!("-2.7")),
        json!(-2)
    );
    assert_eq!(
        typebox_convert(&json!({"type": "boolean"}), json!("0")),
        json!(false)
    );
    assert_eq!(
        typebox_convert(&json!({"type": "string"}), json!(null)),
        json!("null")
    );
    assert_eq!(
        typebox_convert(&json!({"type": "null"}), json!("NULL")),
        json!(null)
    );
    assert_eq!(
        typebox_convert(&json!({"type": "string", "enum": ["a"]}), json!(1)),
        json!(1)
    );
    assert_eq!(typebox_convert(&json!({"const": 5}), json!("5")), json!(5));
}
