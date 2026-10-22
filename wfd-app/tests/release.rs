//! Firmware version check and OTA body (ports of test_firmware_update.py cases).

use serde_json::Value;
use std::path::{Path, PathBuf};
use wfd_app::release::{Release, firmware_status, load_release, multipart};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn rel(fw: &str, protocol: u32) -> Release {
    Release {
        fw: fw.into(),
        protocol,
    }
}

/// (clock_fw, clock_protocol, release, compat, update_available)
type StatusCase = (
    &'static str,
    Option<u32>,
    Option<Release>,
    &'static str,
    bool,
);

#[test]
fn firmware_status_table() {
    const P: u32 = wfd_app::config::PROTOCOL;
    let cases: [StatusCase; 6] = [
        // relative to the protocol this app speaks
        ("A", Some(P), Some(rel("A", P)), "ok", false),
        ("A", Some(P), Some(rel("B", P)), "ok", true), // newer build, same protocol
        ("old", None, Some(rel("B", P)), "older", true), // before versions were reported
        ("A", Some(P - 1), Some(rel("B", P)), "older", true),
        ("A", Some(P + 1), Some(rel("B", P)), "newer", false), // never "update" to an older protocol
        ("A", Some(P), None, "ok", false),                     // no release shipped
    ];
    for (fw, proto, release, compat, available) in cases {
        assert_eq!(
            firmware_status(true, true, Some(fw), proto, release.as_ref()),
            (compat, available),
            "{fw} {proto:?} {release:?}"
        );
    }
}

#[test]
fn disconnected_or_unread_clock_is_unknown() {
    let r = rel("B", 3);
    assert_eq!(
        firmware_status(false, true, Some("A"), Some(3), Some(&r)),
        ("unknown", false)
    );
    // connected, but neither the version nor the settings have been read yet
    assert_eq!(
        firmware_status(true, false, None, None, Some(&r)),
        ("unknown", true)
    );
    // settings read from a build that reports no version
    assert_eq!(
        firmware_status(true, true, None, None, Some(&r)),
        ("older", true)
    );
}

/// Split a single-part multipart body back into (headers, payload).
fn parse_one_part<'a>(body: &'a [u8], ctype: &str) -> (String, &'a [u8]) {
    let boundary = ctype.split("boundary=").nth(1).unwrap();
    let open = format!("--{boundary}\r\n");
    let close = format!("\r\n--{boundary}--\r\n");
    assert!(body.starts_with(open.as_bytes()) && body.ends_with(close.as_bytes()));
    let inner = &body[open.len()..body.len() - close.len()];
    let split = inner.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    (
        String::from_utf8(inner[..split].to_vec()).unwrap(),
        &inner[split + 4..],
    )
}

#[test]
fn multipart_body_is_what_the_clock_expects() {
    let data: Vec<u8> = (0..4).flat_map(|_| 0..=255u8).collect();
    let (body, ctype) = multipart("fw", "wfd-clock-firmware.bin", &data);
    assert!(ctype.starts_with("multipart/form-data; boundary="));
    let (headers, payload) = parse_one_part(&body, &ctype);
    assert!(headers.contains(
        "Content-Disposition: form-data; name=\"fw\"; filename=\"wfd-clock-firmware.bin\""
    ));
    assert!(headers.contains("Content-Type: application/octet-stream"));
    assert_eq!(payload, &data[..]);
}

#[test]
fn multipart_matches_python_bytes() {
    let data: Vec<u8> = (0..4).flat_map(|_| 0..=255u8).collect();
    let (body, ctype) = multipart("fw", "wfd-clock-firmware.bin", &data);
    assert_eq!(
        body,
        std::fs::read(fixture("release_multipart.bin")).unwrap()
    );
    assert_eq!(
        ctype,
        std::fs::read_to_string(fixture("release_multipart_ctype.txt")).unwrap()
    );
}

#[test]
fn load_release_reads_version_json() {
    let dir = std::env::temp_dir().join(format!("wfd-app-release-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    assert_eq!(load_release(&dir.join("missing")), None);
    // every version.json shape app.load_release was run on
    let cases: Vec<Value> =
        serde_json::from_slice(&std::fs::read(fixture("release_load_cases.json")).unwrap())
            .unwrap();
    for c in cases {
        let content = c["content"].as_str().unwrap();
        std::fs::write(dir.join("version.json"), content).unwrap();
        let want = match &c["release"] {
            Value::Null => None,
            r => Some(rel(
                r["fw"].as_str().unwrap(),
                r["protocol"].as_u64().unwrap() as u32,
            )),
        };
        assert_eq!(load_release(&dir), want, "{content}");
    }
}

#[test]
fn shipped_release_loads() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../wfd-firmware/releases");
    if dir.join("version.json").exists() {
        let r = load_release(&dir).expect("shipped version.json should parse");
        assert!(r.fw.starts_with("wfd-clock") && r.protocol >= 3);
    }
}
