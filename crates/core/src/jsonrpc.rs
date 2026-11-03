//! Minimal JSON-RPC 2.0 envelopes — the transport grammar the app-server will
//! speak over stdio (Phase 3). Requests, responses, and notifications; batches
//! and custom error payloads are deliberately out of scope until needed.

use serde_json::{json, Value};

pub const PROTOCOL: &str = "2.0";

/// A client call expecting a response. `method` names it (`thread/start`,
/// `turn/create`, `approval/respond`, …); `params` is method-specific.
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    pub id: u64,
    pub method: String,
    pub params: Value,
}

/// A server push with no reply expected — the `CoreEvent` stream rides these.
#[derive(Debug, Clone, PartialEq)]
pub struct Notification {
    pub method: String,
    pub params: Value,
}

impl Request {
    pub fn to_json(&self) -> Value {
        json!({
            "jsonrpc": PROTOCOL,
            "id": self.id,
            "method": self.method,
            "params": self.params,
        })
    }

    pub fn from_json(v: &Value) -> Option<Self> {
        if v.get("jsonrpc").and_then(Value::as_str)? != PROTOCOL {
            return None;
        }
        Some(Self {
            id: v.get("id")?.as_u64()?,
            method: v.get("method")?.as_str()?.to_string(),
            params: v.get("params").cloned().unwrap_or(Value::Null),
        })
    }
}

impl Notification {
    pub fn to_json(&self) -> Value {
        json!({ "jsonrpc": PROTOCOL, "method": self.method, "params": self.params })
    }

    pub fn from_json(v: &Value) -> Option<Self> {
        if v.get("jsonrpc").and_then(Value::as_str)? != PROTOCOL {
            return None;
        }
        // A notification must NOT carry an id; that distinction is load-bearing
        // for routing replies, so enforce it here.
        if v.get("id").is_some() {
            return None;
        }
        Some(Self {
            method: v.get("method")?.as_str()?.to_string(),
            params: v.get("params").cloned().unwrap_or(Value::Null),
        })
    }
}

/// Successful reply carrying the result value for `id`.
pub fn result(id: u64, value: Value) -> Value {
    json!({ "jsonrpc": PROTOCOL, "id": id, "result": value })
}

/// Error reply with a JSON-RPC error object. Standard codes: -32700 parse,
/// -32601 method not found, -32602 invalid params, -32603 internal.
pub fn error(id: u64, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": PROTOCOL, "id": id, "error": { "code": code, "message": message } })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_roundtrip_and_missing_params_defaults_null() {
        let r = Request { id: 4, method: "thread/start".into(), params: json!({"model": "m"}) };
        assert_eq!(Request::from_json(&r.to_json()).unwrap(), r);
        let bare: Value = serde_json::from_str("{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"x\"}")
            .unwrap();
        assert_eq!(Request::from_json(&bare).unwrap().params, Value::Null);
    }

    #[test]
    fn notification_with_id_is_not_a_notification() {
        let v = json!({"jsonrpc":"2.0","id":9,"method":"event/notify","params":null});
        assert!(Notification::from_json(&v).is_none());
        assert!(Request::from_json(&v).is_some());
    }

    #[test]
    fn wrong_protocol_version_is_rejected() {
        let v = json!({"jsonrpc":"1.0","id":1,"method":"x"});
        assert!(Request::from_json(&v).is_none());
    }

    #[test]
    fn result_and_error_envelopes_are_distinguishable() {
        let ok = result(7, json!({"ok": true}));
        let bad = error(7, -32601, "no such method");
        assert!(ok.get("result").is_some() && ok.get("error").is_none());
        assert!(bad.get("error").is_some() && bad.get("result").is_none());
    }
}
