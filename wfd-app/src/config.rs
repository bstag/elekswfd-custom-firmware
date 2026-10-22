//! App settings (wfd_config.json): defaults, validation of `/api/set` values, load and save.
//!
//! Port of `DEFAULTS`, `CLOCK_KEYS`, `coerce`, `load_config` and `save_config` in app.py.
//! The file format matches Python's `json.dumps(indent=2)` byte for byte (including CRLF line
//! ends on Windows, which Python's text-mode `write_text` produces), so both apps can share it.

use serde_json::{Map, Number, Value, json};
use std::path::Path;

/// Firmware `WFD_PROTOCOL` this app speaks.
pub const PROTOCOL: u32 = 5;

/// Settings owned by the clock (firmware configJson / applySettings). The clock validates them;
/// the app never stores them in its own file.
pub const CLOCK_KEYS: [&str; 21] = [
    "brightness",
    "clock_24h",
    "style",
    "units",
    "tz",
    "location",
    "offline_text",
    "msg_standalone",
    "standalone_msg_mode",
    "standalone_rotate",
    "standalone_dwell",
    "standalone_scroll_ms",
    "mic_standalone",
    "grid_standalone",
    "standalone_anim",
    "standalone_fps",
    "standalone_frame",
    "weather_scene",
    "alarms",
    "timer",
    "alert",
];

/// app.py `DEFAULTS`, in its declaration order (the order `save` writes keys in).
fn default_entries() -> Vec<(&'static str, Value)> {
    vec![
        ("time_sync", json!(true)),
        ("spectrum", json!(true)),
        ("gain_out", json!(1.0)),
        ("gain_mic", json!(1.0)),
        ("bars", json!(true)),
        ("weather", json!(true)),
        ("message_mode", json!("weather")), // weather | window | text | date | forecast | rotate
        ("custom_text", json!("HELLO")),
        ("scroll_ms", json!(350)),
        ("rotate_weather", json!(true)),
        ("rotate_window", json!(true)),
        ("rotate_text", json!(false)),
        ("rotate_date", json!(false)),
        ("rotate_dwell", json!(8)), // seconds per item in rotate mode
        ("grid_enable", json!(false)),
        ("grid_anim", json!("spin")),
        ("grid_frame", json!(false)),
        ("grid_fps", json!(12)),
        ("grid_custom", json!([])), // list of frames, each = [r0..r6] row bytes
        ("icon_mode", json!("off")), // off | audio | system | all
        ("link", json!("auto")),    // auto (USB if plugged in, else WiFi) | usb | wifi
        ("clock_host", json!("")),  // WiFi: name or IP; blank = wfd-clock.local
        ("clock_pass", json!("")),  // the clock's web password (WEBPASS), if it has one
        ("logo", json!(false)),     // ELEKSMAKER logo on/off
    ]
}

/// Every app setting with its default value (exactly app.py `DEFAULTS`).
pub fn defaults() -> Map<String, Value> {
    default_entries()
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect()
}

/// Clamp ranges for numeric settings (app.py `_RANGES`).
fn range(key: &str) -> Option<(f64, f64)> {
    match key {
        "gain_out" | "gain_mic" => Some((0.0, 10.0)),
        "scroll_ms" => Some((50.0, 5000.0)),
        "rotate_dwell" => Some((2.0, 600.0)),
        "grid_fps" => Some((2.0, 30.0)),
        _ => None,
    }
}

/// Allowed values for choice settings, sorted (app.py `_CHOICES`).
fn choices(key: &str) -> Option<&'static [&'static str]> {
    match key {
        "message_mode" => Some(&["date", "forecast", "rotate", "text", "weather", "window"]),
        "icon_mode" => Some(&["all", "audio", "off", "system"]),
        "link" => Some(&["auto", "usb", "wifi"]),
        _ => None,
    }
}

/// Check a `/api/set` value against the type of `DEFAULTS[key]`; returns it normalized.
/// Errors carry the same messages as Python's ValueError/TypeError.
pub fn coerce(key: &str, v: &Value) -> Result<Value, String> {
    let d = defaults().remove(key).ok_or("unknown setting")?;
    match d {
        Value::Bool(_) => match v {
            Value::Bool(_) => Ok(v.clone()),
            _ => Err("expected true/false".into()),
        },
        Value::Number(n) => {
            let x = match v {
                Value::Number(x) => x.as_f64().unwrap_or(f64::NAN),
                Value::String(s) => py::float(s)?,
                _ => return Err("expected a number".into()),
            };
            let clamp = |x: f64| match range(key) {
                Some((lo, hi)) => x.max(lo).min(hi),
                None => x,
            };
            if n.is_f64() {
                // Python lets "nan" through as NaN; a serde_json Value can't hold NaN, so reject it.
                if x.is_nan() {
                    return Err("expected a number".into());
                }
                Number::from_f64(clamp(x))
                    .map(Value::Number)
                    .ok_or_else(|| "expected a number".into())
            } else {
                Ok(json!(clamp(py::int_from_float(x)? as f64) as i64))
            }
        }
        Value::String(_) => {
            let Value::String(s) = v else {
                return Err("expected a string".into());
            };
            match choices(key) {
                Some(c) if !c.contains(&s.as_str()) => {
                    let list: Vec<String> = c.iter().map(|o| py::repr_str(o)).collect();
                    Err(format!("expected one of [{}]", list.join(", ")))
                }
                _ => Ok(v.clone()),
            }
        }
        // grid_custom: up to 80 frames of 7 row bytes (padded with 0 / cut), 7 bits each.
        Value::Array(_) => {
            let frames = match v {
                Value::Array(f) if f.iter().all(Value::is_array) => f,
                _ => return Err("expected a list of frames".into()),
            };
            frames
                .iter()
                .take(80)
                .map(|f| {
                    let f = f.as_array().map(Vec::as_slice).unwrap_or_default();
                    // Only the first 7 entries are looked at, as in Python's (f + [0] * 7)[:7].
                    (0..7)
                        .map(|i| f.get(i).map_or(Ok(0), py::int_low7).map(Value::from))
                        .collect::<Result<Vec<_>, _>>()
                        .map(Value::Array)
                })
                .collect::<Result<Vec<_>, _>>()
                .map(Value::Array)
        }
        _ => Ok(v.clone()),
    }
}

/// Defaults plus the app's own settings from the file at `path`. Keys that aren't app settings
/// (clock settings from older files, `_comment`, ...) are dropped; values aren't validated
/// (same as Python). A missing or unreadable file gives the defaults.
pub fn load(path: &Path) -> Map<String, Value> {
    let mut cfg = defaults();
    if let Ok(Value::Object(file)) = std::fs::read(path)
        .map_err(|_| ())
        .and_then(|b| serde_json::from_slice(&b).map_err(|_| ()))
    {
        for (k, v) in file {
            if let Some(slot) = cfg.get_mut(&k) {
                *slot = v;
            }
        }
    }
    cfg
}

/// Write the app settings in `cfg` (DEFAULTS keys only, in DEFAULTS order) like Python's
/// `json.dumps(..., indent=2)` written in text mode.
pub fn save(path: &Path, cfg: &Map<String, Value>) -> std::io::Result<()> {
    let own: Vec<(&str, &Value)> = default_entries()
        .iter()
        .filter_map(|(k, _)| cfg.get_key_value(*k).map(|(k, v)| (k.as_str(), v)))
        .collect();
    let mut out = String::new();
    py::dump_object(&own, 0, &mut out);
    let nl = if cfg!(windows) { "\r\n" } else { "\n" };
    std::fs::write(path, out.replace('\n', nl))
}

/// Small pieces of Python semantics the port has to reproduce (`float()`, `int()`, `repr()`,
/// `json.dumps`). Shared with `release`.
pub(crate) mod py {
    use serde_json::Value;
    use std::fmt::Write;

    /// Python `float(str)`: surrounding whitespace, `_` between digits, inf/nan allowed.
    pub fn float(s: &str) -> Result<f64, String> {
        let err = || format!("could not convert string to float: {}", repr_str(s));
        let t = strip_underscores(s.trim()).ok_or_else(err)?;
        t.parse::<f64>().map_err(|_| err())
    }

    /// Python `int(x)` for a float: truncate; NaN / infinity are errors.
    pub fn int_from_float(x: f64) -> Result<i64, String> {
        if x.is_nan() {
            Err("cannot convert float NaN to integer".into())
        } else if x.is_infinite() {
            Err("cannot convert float infinity to integer".into())
        } else {
            Ok(x.trunc() as i64) // saturates; only ever clamped to small ranges afterwards
        }
    }

    /// Python `int(s)` for a string, as (negative, ASCII decimal digits).
    pub fn int_str(s: &str) -> Option<(bool, String)> {
        let t = s.trim();
        let (neg, t) = match t.as_bytes().first() {
            Some(b'-') => (true, &t[1..]),
            Some(b'+') => (false, &t[1..]),
            _ => (false, t),
        };
        let digits = strip_underscores(t)?;
        (!digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())).then_some((neg, digits))
    }

    /// Python `int(x) & 0x7F` for one grid row value (exact for any size of number).
    pub fn int_low7(x: &Value) -> Result<i64, String> {
        match x {
            Value::Bool(b) => Ok(*b as i64),
            Value::Number(n) => match (n.as_i64(), n.as_u64()) {
                (Some(i), _) => Ok(i & 0x7F),
                (_, Some(u)) => Ok((u & 0x7F) as i64),
                // A float of 2^63 or more is a multiple of 2^11, so its low 7 bits are 0.
                _ => {
                    let f = int_from_float(n.as_f64().unwrap_or(f64::NAN))?;
                    Ok(if f == i64::MIN || f == i64::MAX {
                        0
                    } else {
                        f & 0x7F
                    })
                }
            },
            Value::String(s) => {
                let (neg, digits) = int_str(s).ok_or_else(|| {
                    format!("invalid literal for int() with base 10: {}", repr_str(s))
                })?;
                let m = digits
                    .bytes()
                    .fold(0i64, |m, d| (m * 10 + (d - b'0') as i64) % 128);
                Ok(if neg { (128 - m) % 128 } else { m })
            }
            other => Err(format!(
                "int() argument must be a string, a bytes-like object or a real number, not '{}'",
                match other {
                    Value::Null => "NoneType",
                    Value::Array(_) => "list",
                    _ => "dict",
                }
            )),
        }
    }

    /// Drop `_` digit separators; None if one isn't between two digits (Python rejects that).
    fn strip_underscores(s: &str) -> Option<String> {
        let b = s.as_bytes();
        for (i, &c) in b.iter().enumerate() {
            if c == b'_' {
                let ok = i > 0
                    && b[i - 1].is_ascii_digit()
                    && b.get(i + 1).is_some_and(u8::is_ascii_digit);
                if !ok {
                    return None;
                }
            }
        }
        Some(s.replace('_', ""))
    }

    /// Python `repr(str)`: single quotes unless the text has ' and no ".
    pub fn repr_str(s: &str) -> String {
        let q = if s.contains('\'') && !s.contains('"') {
            '"'
        } else {
            '\''
        };
        let mut out = String::from(q);
        for c in s.chars() {
            match c {
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                c if c == q => {
                    out.push('\\');
                    out.push(c);
                }
                c if (c as u32) < 0x20 || (0x7F..0xA0).contains(&(c as u32)) => {
                    let _ = write!(out, "\\x{:02x}", c as u32);
                }
                c => out.push(c),
            }
        }
        out.push(q);
        out
    }

    /// Python `repr(float)`: shortest round-trip digits, exponent form outside 1e-4 .. 1e16.
    pub fn repr_float(x: f64) -> String {
        if x.is_nan() {
            return "nan".into();
        }
        if x.is_infinite() {
            return if x > 0.0 { "inf" } else { "-inf" }.into();
        }
        let sci = format!("{x:e}"); // e.g. "-1.2345e-7": shortest round-trip digits
        let (mant, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
        let exp: i32 = exp.parse().unwrap_or(0);
        let (sign, mant) = mant.strip_prefix('-').map_or(("", mant), |m| ("-", m));
        let digits: String = mant.chars().filter(char::is_ascii_digit).collect();
        if (-4..16).contains(&exp) {
            let point = exp + 1; // digits before the decimal point
            let (int, frac) = if point <= 0 {
                ("0".to_string(), "0".repeat(-point as usize) + &digits)
            } else if point as usize >= digits.len() {
                (
                    digits.clone() + &"0".repeat(point as usize - digits.len()),
                    String::new(),
                )
            } else {
                (
                    digits[..point as usize].to_string(),
                    digits[point as usize..].to_string(),
                )
            };
            let frac = if frac.is_empty() {
                "0".to_string()
            } else {
                frac
            };
            format!("{sign}{int}.{frac}")
        } else {
            let m = if digits.len() > 1 {
                format!("{}.{}", &digits[..1], &digits[1..])
            } else {
                digits
            };
            format!(
                "{sign}{m}e{}{:02}",
                if exp < 0 { '-' } else { '+' },
                exp.abs()
            )
        }
    }

    /// Python `str(x)` of a JSON value (str of a str is itself; containers use repr).
    pub fn str_value(v: &Value) -> String {
        match v {
            Value::String(s) => s.clone(),
            other => repr_value(other),
        }
    }

    fn repr_value(v: &Value) -> String {
        match v {
            Value::Null => "None".into(),
            Value::Bool(b) => if *b { "True" } else { "False" }.into(),
            Value::Number(n) if n.is_f64() => repr_float(n.as_f64().unwrap_or(f64::NAN)),
            Value::Number(n) => n.to_string(),
            Value::String(s) => repr_str(s),
            Value::Array(a) => format!(
                "[{}]",
                a.iter().map(repr_value).collect::<Vec<_>>().join(", ")
            ),
            Value::Object(o) => format!(
                "{{{}}}",
                o.iter()
                    .map(|(k, v)| format!("{}: {}", repr_str(k), repr_value(v)))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }

    /// `json.dumps(v, indent=2)` (ensure_ascii, Python float repr), "\n" line ends.
    pub fn dump(v: &Value, level: usize, out: &mut String) {
        match v {
            Value::Null => out.push_str("null"),
            Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Value::Number(n) if n.is_f64() => {
                out.push_str(&repr_float(n.as_f64().unwrap_or(f64::NAN)))
            }
            Value::Number(n) => out.push_str(&n.to_string()),
            Value::String(s) => dump_str(s, out),
            Value::Array(a) if a.is_empty() => out.push_str("[]"),
            Value::Array(a) => {
                out.push('[');
                for (i, x) in a.iter().enumerate() {
                    out.push_str(if i == 0 { "\n" } else { ",\n" });
                    indent(level + 1, out);
                    dump(x, level + 1, out);
                }
                out.push('\n');
                indent(level, out);
                out.push(']');
            }
            Value::Object(o) => {
                let items: Vec<(&str, &Value)> = o.iter().map(|(k, v)| (k.as_str(), v)).collect();
                dump_object(&items, level, out);
            }
        }
    }

    pub fn dump_object(items: &[(&str, &Value)], level: usize, out: &mut String) {
        if items.is_empty() {
            out.push_str("{}");
            return;
        }
        out.push('{');
        for (i, (k, v)) in items.iter().enumerate() {
            out.push_str(if i == 0 { "\n" } else { ",\n" });
            indent(level + 1, out);
            dump_str(k, out);
            out.push_str(": ");
            dump(v, level + 1, out);
        }
        out.push('\n');
        indent(level, out);
        out.push('}');
    }

    fn indent(level: usize, out: &mut String) {
        out.push_str(&"  ".repeat(level));
    }

    /// JSON string with Python's ensure_ascii escaping (everything outside ' '..'~' escaped).
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
}
