//! The JSON and encoding helpers the transports share: Python-compatible JSON, per-setting
//! errors, truthiness, display command lines, and HTTP Basic auth.

use crate::config::py;
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

/// Python truthiness of a JSON value (`bool(x)`; a missing value is falsy).
pub fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|x| x != 0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
    }
}

/// `json.dumps(v, separators=(",", ":"))`: compact, ASCII-only, Python float repr.
/// (Keys come out in serde_json's map order, which is sorted; Python keeps insertion order.
/// The clock parses the JSON, so only the bytes on the wire differ.)
pub fn py_json(v: &Value) -> String {
    let mut out = String::new();
    dump_compact(v, &mut out);
    out
}

fn dump_compact(v: &Value, out: &mut String) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) if n.is_f64() => {
            out.push_str(&py::repr_float(n.as_f64().unwrap_or(f64::NAN)))
        }
        Value::Number(n) => out.push_str(&n.to_string()),
        Value::String(s) => dump_str(s, out),
        Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                dump_compact(x, out);
            }
            out.push(']');
        }
        Value::Object(o) => {
            out.push('{');
            for (i, (k, x)) in o.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                dump_str(k, out);
                out.push(':');
                dump_compact(x, out);
            }
            out.push('}');
        }
    }
}

/// A JSON string with Python's ensure_ascii escaping.
fn dump_str(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            ' '..='~' => out.push(c),
            c => {
                let mut buf = [0u16; 2];
                for unit in c.encode_utf16(&mut buf) {
                    let _ = write!(out, "\\u{unit:04x}");
                }
            }
        }
    }
    out.push('"');
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
