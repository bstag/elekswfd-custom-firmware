//! The firmware this app ships (releases/version.json), the version check against the clock,
//! and the multipart body for the clock's OTA `/update` upload.
//!
//! Port of `load_release`, `firmware_status` and `multipart` in app.py.

use crate::config::{PROTOCOL, py};
use serde_json::Value;
use std::path::Path;

/// Boundary of the OTA upload body (app.py `multipart` default).
const BOUNDARY: &str = "wfd-clock-ota-7f3a";

/// The firmware build shipped with the app.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Release {
    pub fw: String,
    pub protocol: u32,
}

/// `dir/version.json` -> the shipped release, or None if it's missing or malformed.
/// Mirrors Python's `str(info["fw"])` / `int(info["protocol"])` conversions.
pub fn load_release(dir: &Path) -> Option<Release> {
    let bytes = std::fs::read(dir.join("version.json")).ok()?;
    let info: Value = serde_json::from_slice(&bytes).ok()?;
    let info = info.as_object()?;
    let fw = py::str_value(info.get("fw")?);
    let protocol = match info.get("protocol")? {
        Value::Bool(b) => *b as i64,
        Value::Number(n) => match n.as_i64() {
            Some(i) => i,
            None => py::int_from_float(n.as_f64()?).ok()?,
        },
        Value::String(s) => {
            let (neg, digits) = py::int_str(s)?;
            let v: i64 = digits.parse().ok()?;
            if neg { -v } else { v }
        }
        _ => return None,
    };
    Some(Release {
        fw,
        protocol: u32::try_from(protocol).ok()?,
    })
}

/// What the dashboard shows about the clock's firmware vs this app:
/// (compat: "unknown" | "older" | "newer" | "ok", update_available).
///
/// `clock_known` = the app has read the clock's settings at least once.
pub fn firmware_status(
    connected: bool,
    clock_known: bool,
    clock_fw: Option<&str>,
    clock_protocol: Option<u32>,
    release: Option<&Release>,
) -> (&'static str, bool) {
    let compat = match clock_protocol {
        _ if !connected || (clock_fw.is_none() && !clock_known) => "unknown",
        // builds before protocol 3 didn't report one
        None => "older",
        Some(p) if p < PROTOCOL => "older",
        Some(p) if p > PROTOCOL => "newer",
        Some(_) => "ok",
    };
    // Never "update" to a release with an older protocol than the clock's.
    let available = release.is_some_and(|r| {
        connected && clock_fw != Some(r.fw.as_str()) && r.protocol >= clock_protocol.unwrap_or(0)
    });
    (compat, available)
}

/// A multipart/form-data body with one file field, as the clock's `/update` page sends it.
/// Returns (body, content-type header value).
pub fn multipart(field: &str, filename: &str, data: &[u8]) -> (Vec<u8>, String) {
    let head = format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"{field}\"; filename=\"{filename}\"\r\n\
         Content-Type: application/octet-stream\r\n\r\n"
    );
    let tail = format!("\r\n--{BOUNDARY}--\r\n");
    let body = [head.as_bytes(), data, tail.as_bytes()].concat();
    (body, format!("multipart/form-data; boundary={BOUNDARY}"))
}
