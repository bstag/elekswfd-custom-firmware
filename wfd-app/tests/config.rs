//! Settings validation and the config file (ports of test_rendering.py / test_api.py cases,
//! plus parity tables generated from app.py; see tests/fixtures/config_*).

use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};
use wfd_app::config::{self, CLOCK_KEYS, PROTOCOL, coerce, defaults, load, save};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn temp_file(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("wfd-app-config-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(tag)
}

#[test]
fn coerce_accepts_and_normalizes() {
    let cases = [
        ("grid_fps", json!("6"), json!(6)),
        ("grid_fps", json!(99), json!(30)), // clamped
        ("grid_fps", json!(0), json!(2)),
        ("rotate_dwell", json!(700.9), json!(600)),
        ("scroll_ms", json!(10), json!(50)),
        ("gain_mic", json!("1.5"), json!(1.5)),
        ("message_mode", json!("date"), json!("date")),
        (
            "grid_custom",
            json!([[1, 2], vec![300; 9]]),
            json!([[1, 2, 0, 0, 0, 0, 0], vec![300 & 0x7F; 7]]),
        ),
    ];
    for (key, value, expected) in cases {
        assert_eq!(coerce(key, &value), Ok(expected), "{key} {value}");
    }
}

#[test]
fn coerce_rejects() {
    let cases = [
        ("grid_fps", json!("abc")),
        ("grid_fps", json!(true)), // bools are not numbers here
        ("grid_fps", Value::Null),
        ("logo", json!("yes")),
        ("message_mode", json!("bogus")),
        ("icon_mode", json!("disco")),
        ("custom_text", json!(5)),
        ("grid_custom", json!([1, 2])),
        ("gain_out", json!("nan")),
    ];
    for (key, value) in cases {
        assert!(coerce(key, &value).is_err(), "{key} {value}");
    }
    assert_eq!(
        coerce("brightness", &json!(3)),
        Err("unknown setting".into())
    );
}

#[test]
fn every_default_passes_its_own_validation() {
    for (key, value) in defaults() {
        assert_eq!(coerce(&key, &value), Ok(value), "{key}");
    }
}

#[test]
fn defaults_match_app_py() {
    let d = defaults();
    assert_eq!(d.len(), 24);
    assert_eq!(d["gain_out"], json!(1.0));
    assert!(d["gain_out"].is_f64() && d["scroll_ms"].is_i64());
    assert_eq!(d["grid_custom"], json!([]));
    assert_eq!(PROTOCOL, 3);
    assert!(CLOCK_KEYS.iter().all(|k| !d.contains_key(*k)));
    // Python writes exactly this for DEFAULTS
    let path = temp_file("defaults.json");
    save(&path, &d).unwrap();
    assert_eq!(
        std::fs::read(&path).unwrap(),
        std::fs::read(fixture("config_defaults_saved.json")).unwrap()
    );
}

#[test]
fn grid_custom_is_normalized() {
    let many = Value::Array(vec![json!([1]); 100]);
    let out = coerce("grid_custom", &many).unwrap();
    assert_eq!(out.as_array().unwrap().len(), 80);
    assert_eq!(out[0], json!([1, 0, 0, 0, 0, 0, 0]));
    assert_eq!(
        coerce(
            "grid_custom",
            &json!([[-1, 128, 255.9, true, "5", " +7 ", "1_0", "x"]])
        ),
        Ok(json!([[127, 0, 127, 1, 5, 7, 10]])) // 8th entry is cut before it's looked at
    );
    assert!(coerce("grid_custom", &json!([[null]])).is_err());
    assert!(coerce("grid_custom", &json!([["5.5"]])).is_err());
    assert!(coerce("grid_custom", &json!([[1], 2])).is_err());
}

/// Every (setting, value) pair app.coerce was run on: same result, same error text.
#[test]
fn coerce_matches_python_table() {
    let cases: Vec<Value> =
        serde_json::from_slice(&std::fs::read(fixture("config_coerce_cases.json")).unwrap())
            .unwrap();
    assert!(cases.len() > 1000);
    for c in &cases {
        let (key, value) = (c["key"].as_str().unwrap(), &c["value"]);
        let got = coerce(key, value);
        if c["ok"] == json!(true) {
            let want = &c["result"];
            assert_eq!(got.as_ref(), Ok(want), "{key} {value}");
            // int vs float must match too (Python writes 6 vs 6.0)
            assert_eq!(got.unwrap().to_string(), want.to_string(), "{key} {value}");
        } else {
            let err = got.expect_err(&format!("{key} {value} should fail"));
            if let Some(msg) = c["error"].as_str() {
                assert_eq!(err, msg, "{key} {value}");
            }
        }
    }
}

#[test]
fn load_ignores_clock_keys_and_comments() {
    let path = temp_file("old.json");
    std::fs::write(
        &path,
        r#"{"brightness": 3, "style": 1, "logo": true, "_comment": "x"}"#,
    )
    .unwrap();
    let loaded = load(&path);
    assert_eq!(loaded["logo"], json!(true));
    assert!(
        !["brightness", "style", "_comment"]
            .iter()
            .any(|k| loaded.contains_key(*k))
    );
    assert_eq!(loaded.len(), defaults().len());
}

#[test]
fn bad_or_missing_file_gives_defaults() {
    assert_eq!(load(Path::new("no/such/wfd_config.json")), defaults());
    for content in ["not json", "[1, 2]", "", "\u{feff}{\"logo\": true}"] {
        let path = temp_file("bad.json");
        std::fs::write(&path, content).unwrap();
        assert_eq!(load(&path), defaults(), "{content:?}");
    }
}

#[test]
fn save_load_round_trip() {
    let mut cfg = defaults();
    cfg.insert("gain_mic".into(), json!(0.3));
    cfg.insert("custom_text".into(), json!("TODAY {date} caf\u{e9}"));
    cfg.insert(
        "grid_custom".into(),
        json!([[8, 8, 8, 127, 8, 8, 8], [0, 0, 0, 0, 0, 0, 0]]),
    );
    cfg.insert("brightness".into(), json!(5)); // not an app setting: not written
    let path = temp_file("roundtrip.json");
    save(&path, &cfg).unwrap();
    cfg.remove("brightness");
    assert_eq!(load(&path), cfg);
}

/// A file Python's save_config wrote (odd values, unicode, nesting): Rust reads it to the same
/// settings Python reads, and writes it back byte for byte.
#[test]
fn python_written_file_round_trips_exactly() {
    let loaded = load(&fixture("config_saved.json"));
    let want: Map<String, Value> =
        serde_json::from_slice(&std::fs::read(fixture("config_saved_loaded.json")).unwrap())
            .unwrap();
    assert_eq!(loaded, want);
    let path = temp_file("resaved.json");
    save(&path, &loaded).unwrap();
    assert_eq!(
        String::from_utf8(std::fs::read(&path).unwrap()).unwrap(),
        String::from_utf8(std::fs::read(fixture("config_saved.json")).unwrap()).unwrap()
    );
}

/// The user's real settings file (if present next to the Python app) survives a Rust save.
#[test]
fn real_config_file_round_trips() {
    // WFD_REAL_CONFIG can point at it when testing from a worktree without one
    let real = std::env::var_os("WFD_REAL_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../wfd-control/wfd_config.json")
        });
    let Ok(bytes) = std::fs::read(&real) else {
        return;
    };
    let path = temp_file("real.json");
    save(&path, &load(&real)).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}

#[test]
fn module_path_is_public() {
    // the functions are reachable as config::* too (as other modules will call them)
    assert_eq!(config::coerce("link", &json!("usb")), Ok(json!("usb")));
}
