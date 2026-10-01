//! The message envelope: JSON-RPC shaped, without the `"jsonrpc"` field.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::fmt;

pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;
pub const INTERNAL_ERROR: i64 = -32603;

/// A request id: an integer or a string.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RequestId {
    Integer(i64),
    String(String),
}

impl fmt::Display for RequestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RequestId::Integer(n) => write!(f, "{n}"),
            RequestId::String(s) => f.write_str(s),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErrorObject {
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl ErrorObject {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }
}

/// One message on the wire, in either direction.
#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    Request {
        id: RequestId,
        method: String,
        params: Option<Value>,
    },
    Notification {
        method: String,
        params: Option<Value>,
    },
    Response {
        id: RequestId,
        result: Value,
    },
    Error {
        /// `None` when the failing message had no usable id.
        id: Option<RequestId>,
        error: ErrorObject,
    },
}

impl Message {
    /// Classify a parsed JSON value by which keys it has. A `"jsonrpc"` key is
    /// ignored.
    pub fn from_value(value: Value) -> Result<Message, String> {
        let Value::Object(mut map) = value else {
            return Err("message is not a JSON object".into());
        };
        let id = match map.remove("id") {
            None | Some(Value::Null) => None,
            Some(v) => Some(
                serde_json::from_value::<RequestId>(v)
                    .map_err(|_| "id must be a string or an integer".to_string())?,
            ),
        };
        let params = map.remove("params").filter(|p| !p.is_null());
        if let Some(method) = map.remove("method") {
            let Value::String(method) = method else {
                return Err("method must be a string".into());
            };
            return Ok(match id {
                Some(id) => Message::Request { id, method, params },
                None => Message::Notification { method, params },
            });
        }
        if let Some(error) = map.remove("error") {
            let error = serde_json::from_value::<ErrorObject>(error)
                .map_err(|e| format!("invalid error object: {e}"))?;
            return Ok(Message::Error { id, error });
        }
        if let Some(result) = map.remove("result") {
            let Some(id) = id else {
                return Err("response without id".into());
            };
            return Ok(Message::Response { id, result });
        }
        Err("message has neither method, result nor error".into())
    }

    /// The wire form (no `"jsonrpc"` field).
    pub fn to_value(&self) -> Value {
        let mut map = Map::new();
        match self {
            Message::Request { id, method, params } => {
                map.insert("id".into(), id_value(id));
                map.insert("method".into(), Value::String(method.clone()));
                if let Some(p) = params {
                    map.insert("params".into(), p.clone());
                }
            }
            Message::Notification { method, params } => {
                map.insert("method".into(), Value::String(method.clone()));
                if let Some(p) = params {
                    map.insert("params".into(), p.clone());
                }
            }
            Message::Response { id, result } => {
                map.insert("id".into(), id_value(id));
                map.insert("result".into(), result.clone());
            }
            Message::Error { id, error } => {
                map.insert(
                    "id".into(),
                    id.as_ref().map(id_value).unwrap_or(Value::Null),
                );
                map.insert(
                    "error".into(),
                    serde_json::to_value(error).unwrap_or(Value::Null),
                );
            }
        }
        Value::Object(map)
    }
}

fn id_value(id: &RequestId) -> Value {
    serde_json::to_value(id).unwrap_or(Value::Null)
}

fn to_params<T: Serialize>(params: &T) -> Value {
    serde_json::to_value(params).unwrap_or(Value::Null)
}

/// A notification `{method, params}`.
pub fn notification<T: Serialize>(method: &str, params: &T) -> Value {
    Message::Notification {
        method: method.into(),
        params: Some(to_params(params)),
    }
    .to_value()
}

/// A request `{id, method, params}`.
pub fn request<T: Serialize>(id: RequestId, method: &str, params: &T) -> Value {
    Message::Request {
        id,
        method: method.into(),
        params: Some(to_params(params)),
    }
    .to_value()
}

/// A success response `{id, result}`.
pub fn response<T: Serialize>(id: RequestId, result: &T) -> Value {
    Message::Response {
        id,
        result: to_params(result),
    }
    .to_value()
}

/// An error response `{id, error: {code, message}}`; `id` is `null` when unknown.
pub fn error_response(id: Option<RequestId>, code: i64, message: impl Into<String>) -> Value {
    Message::Error {
        id,
        error: ErrorObject::new(code, message),
    }
    .to_value()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn classifies_by_keys_and_ignores_jsonrpc() {
        let m =
            Message::from_value(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}))
                .unwrap();
        assert_eq!(
            m,
            Message::Request {
                id: RequestId::Integer(1),
                method: "initialize".into(),
                params: Some(json!({}))
            }
        );
        let m = Message::from_value(json!({"method":"initialized"})).unwrap();
        assert_eq!(
            m,
            Message::Notification {
                method: "initialized".into(),
                params: None
            }
        );
        let m = Message::from_value(json!({"id":"a","result":{"x":1}})).unwrap();
        assert_eq!(
            m,
            Message::Response {
                id: RequestId::String("a".into()),
                result: json!({"x":1})
            }
        );
        let m =
            Message::from_value(json!({"id":7,"error":{"code":-32601,"message":"no"}})).unwrap();
        assert!(matches!(
            m,
            Message::Error {
                id: Some(RequestId::Integer(7)),
                ..
            }
        ));
        assert!(Message::from_value(json!([1])).is_err());
        assert!(Message::from_value(json!({"id":1})).is_err());
    }

    #[test]
    fn wire_form_has_no_jsonrpc_field() {
        let v = request(RequestId::Integer(3), "item/x", &json!({"a":1}));
        assert_eq!(v, json!({"id":3,"method":"item/x","params":{"a":1}}));
        let v = error_response(None, METHOD_NOT_FOUND, "nope");
        assert_eq!(
            v,
            json!({"id":null,"error":{"code":-32601,"message":"nope"}})
        );
        let v = response(RequestId::String("x".into()), &json!({}));
        assert_eq!(v, json!({"id":"x","result":{}}));
    }
}
