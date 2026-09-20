//! JSON-RPC 2.0 envelope parsing for the SAAA MCP server.
//!
//! Request IDs keep their JSON type: the integer `1` and the string `"1"` are different requests.
//! Invalid JSON is a parse error, a non-object or a batch is an invalid request, and an id that is
//! null or fractional is rejected before any session state is touched.

use serde_json::{json, Value};

pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;
pub const INTERNAL_ERROR: i64 = -32603;
pub const SERVER_BUSY: i64 = -32000;
pub const DEADLINE_EXCEEDED: i64 = -32001;
pub const NOT_INITIALIZED: i64 = -32002;

/// A request id whose JSON type is preserved. It is `Hash`/`Eq` so it can key per-session ledgers
/// without collapsing `1` and `"1"`.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum TypedRequestId {
    Integer(i64),
    String(String),
}

impl TypedRequestId {
    pub fn from_value(value: &Value) -> Option<Self> {
        match value {
            Value::Number(number) if number.is_i64() || number.is_u64() => {
                number.as_i64().map(Self::Integer)
            }
            Value::String(text) => Some(Self::String(text.clone())),
            _ => None,
        }
    }

    pub fn as_value(&self) -> Value {
        match self {
            Self::Integer(value) => json!(value),
            Self::String(value) => json!(value),
        }
    }
}

#[derive(Clone, Debug)]
pub struct JsonRpcRequest {
    pub id: TypedRequestId,
    pub method: String,
    pub params: Value,
}

#[derive(Clone, Debug)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: &'static str,
}

impl JsonRpcError {
    pub fn new(code: i64, message: &'static str) -> Self {
        Self { code, message }
    }
}

pub enum Parsed {
    Request(JsonRpcRequest),
    Notification { method: String, params: Value },
    Error(JsonRpcError),
}

/// Parses one HTTP body into a request, a notification, or a JSON-RPC error. Batch requests are
/// not part of this server's contract and are rejected as invalid.
pub fn parse(bytes: &[u8]) -> Parsed {
    let value: Value = match serde_json::from_slice(bytes) {
        Ok(value) => value,
        Err(_) => return Parsed::Error(JsonRpcError::new(PARSE_ERROR, "Parse error")),
    };
    let Some(object) = value.as_object() else {
        return Parsed::Error(JsonRpcError::new(INVALID_REQUEST, "Invalid Request"));
    };
    if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Parsed::Error(JsonRpcError::new(INVALID_REQUEST, "Invalid Request"));
    }
    let Some(method) = object.get("method").and_then(Value::as_str) else {
        return Parsed::Error(JsonRpcError::new(INVALID_REQUEST, "Invalid Request"));
    };
    let params = object.get("params").cloned().unwrap_or(Value::Null);
    match object.get("id") {
        None => Parsed::Notification {
            method: method.to_string(),
            params,
        },
        Some(value) => match TypedRequestId::from_value(value) {
            Some(id) => Parsed::Request(JsonRpcRequest {
                id,
                method: method.to_string(),
                params,
            }),
            None => Parsed::Error(JsonRpcError::new(INVALID_REQUEST, "Invalid Request")),
        },
    }
}

pub fn success_response(id: &TypedRequestId, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id.as_value(), "result": result })
}

pub fn error_response(id: Option<&TypedRequestId>, error: &JsonRpcError) -> Value {
    let id = match id {
        Some(id) => id.as_value(),
        None => Value::Null,
    };
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": error.code, "message": error.message }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_ids_preserve_json_type() {
        assert_eq!(
            TypedRequestId::from_value(&json!(1)),
            Some(TypedRequestId::Integer(1))
        );
        assert_eq!(
            TypedRequestId::from_value(&json!("1")),
            Some(TypedRequestId::String("1".to_string()))
        );
        assert_ne!(
            TypedRequestId::from_value(&json!(1)),
            TypedRequestId::from_value(&json!("1"))
        );
        assert_eq!(TypedRequestId::from_value(&Value::Null), None);
        assert_eq!(TypedRequestId::from_value(&json!(1.5)), None);
    }

    #[test]
    fn batch_and_null_id_are_invalid() {
        assert!(matches!(
            parse(br#"[{"jsonrpc":"2.0","id":1,"method":"ping"}]"#),
            Parsed::Error(_)
        ));
        match parse(br#"{"jsonrpc":"2.0","id":null,"method":"ping"}"#) {
            Parsed::Error(error) => assert_eq!(error.code, INVALID_REQUEST),
            _ => panic!("null id must be invalid"),
        }
        match parse(b"{") {
            Parsed::Error(error) => assert_eq!(error.code, PARSE_ERROR),
            _ => panic!("malformed json must be a parse error"),
        }
        match parse(br#"{"jsonrpc":"1.0","id":1,"method":"ping"}"#) {
            Parsed::Error(error) => assert_eq!(error.code, INVALID_REQUEST),
            _ => panic!("wrong jsonrpc version must be invalid"),
        }
    }

    #[test]
    fn notification_has_no_id() {
        match parse(br#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#) {
            Parsed::Notification { method, .. } => {
                assert_eq!(method, "notifications/initialized")
            }
            _ => panic!("expected notification"),
        }
    }

    #[test]
    fn error_response_carries_the_typed_id() {
        let id = TypedRequestId::String("abc".to_string());
        let response = error_response(
            Some(&id),
            &JsonRpcError::new(METHOD_NOT_FOUND, "Method not found"),
        );
        assert_eq!(response.pointer("/id"), Some(&json!("abc")));
        assert_eq!(response.pointer("/error/code"), Some(&json!(-32601)));
    }
}
