//! The JSON and encoding helpers the transports share: the clock's error texts, per-setting
//! errors, truthiness, display command lines, and HTTP Basic auth.

use serde_json::{Map, Value};
use std::fmt::{Display, Write as _};

/// `CMD v1 v2 ...` (app.py `_frame_cmd`).
pub fn frame_cmd<T: Display>(cmd: &str, values: impl IntoIterator<Item = T>) -> String {
    let mut out = String::from(cmd);
    out.push(' ');
    for (i, v) in values.into_iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        let _ = write!(out, "{v}");
    }
    out
}

/// `{key: msg}` for every key of a settings patch (how errors are reported per setting).
pub fn per_key(patch: &Map<String, Value>, msg: &str) -> Value {
    Value::Object(
        patch
            .keys()
            .map(|k| (k.clone(), Value::String(msg.into())))
            .collect(),
    )
}

/// Python truthiness of a JSON value (`bool(x)`). For a value that may be missing (falsy):
/// `map.get(key).is_some_and(truthy)`.
pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|x| x != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// The text of an error the clock sent: a string as it is, anything else as JSON.
pub fn error_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "the clock refused the change".into(),
        other => other.to_string(),
    }
}

/// Standard base64 (for HTTP Basic auth).
pub fn base64(data: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = u32::from(b[0]) << 16 | u32::from(b[1]) << 8 | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ABC[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// `Authorization` header value for the clock's web password (user "admin").
pub fn basic_auth(password: &str) -> String {
    format!("Basic {}", base64(format!("admin:{password}").as_bytes()))
}
