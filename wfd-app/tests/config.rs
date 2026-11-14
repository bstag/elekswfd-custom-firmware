//! The app settings: the typed `Settings`, validation of `/api/set` values (a hand-written table
//! covering every setting), and the settings file.

use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use wfd_app::config::{
    self, CLOCK_KEYS, IconMode, LinkMode, MessageMode, PROTOCOL, Settings, load, save,
};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// A directory of its own, emptied first.
fn fresh_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("wfd-app-config-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The log every test in this binary shares (the first `applog::init` wins), and the lines in
/// it that mention `path`.
fn log_lines_about(path: &Path) -> Vec<String> {
    static LOG: OnceLock<PathBuf> = OnceLock::new();
    let log = LOG.get_or_init(|| {
        let p = fresh_dir("log").join("wfd-app.log");
        wfd_app::applog::init(p.clone());
        p
    });
    let text = std::fs::read_to_string(log).unwrap_or_default();
    let needle = path.display().to_string();
    text.lines().filter(|l| l.contains(&needle)).map(str::to_owned).collect()
}

/// Make sure logging is on before a test that checks the log does anything.
fn start_log() {
    log_lines_about(Path::new(""));
}

fn names_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

// ------------------------------------------------------------------ validation

const BOOLS: [&str; 11] = [
    "time_sync", "spectrum", "bars", "weather", "rotate_weather", "rotate_window", "rotate_text",
    "rotate_date", "grid_enable", "grid_frame", "logo",
];

/// (setting, value sent to `/api/set`, what `/api/status` then shows, or None: rejected).
fn cases() -> Vec<(&'static str, Value, Option<Value>)> {
    let mut c: Vec<(&'static str, Value, Option<Value>)> = Vec::new();
    for key in BOOLS {
        c.push((key, json!(true), Some(json!(true))));
        c.push((key, json!(false), Some(json!(false))));
        for bad in [json!(1), json!(0), json!("true"), Value::Null, json!([true])] {
            c.push((key, bad, None));
        }
    }
    // floats: any JSON number, clamped to 0..10, kept as a float
    for key in ["gain_out", "gain_mic"] {
        for (sent, shown) in [
            (json!(2.5), json!(2.5)),
            (json!(2), json!(2.0)),
            (json!(0), json!(0.0)),
            (json!(10), json!(10.0)),
            (json!(10.5), json!(10.0)),
            (json!(-1), json!(0.0)),
            (json!(1e300), json!(10.0)),
        ] {
            c.push((key, sent, Some(shown)));
        }
        for bad in [json!("1.5"), json!(true), Value::Null, json!([1]), json!({})] {
            c.push((key, bad, None));
        }
    }
    // whole numbers: any JSON number, cut to a whole one, clamped
    for (key, lo, hi) in [("scroll_ms", 50, 5000), ("rotate_dwell", 2, 600), ("grid_fps", 2, 30)] {
        for (sent, shown) in [
            (json!(lo), lo),
            (json!(hi), hi),
            (json!(lo - 1), lo),
            (json!(hi + 1), hi),
            (json!(-5), lo),
            (json!(lo as f64 + 0.9), lo),
            (json!(1e30), hi),
            (json!(12_345_678_901_234_567_890u64), hi),
        ] {
            c.push((key, sent, Some(json!(shown))));
        }
        for bad in [json!("10"), json!(false), Value::Null, json!([10])] {
            c.push((key, bad, None));
        }
    }
    c.push(("scroll_ms", json!(450), Some(json!(450))));
    c.push(("rotate_dwell", json!(6), Some(json!(6))));
    c.push(("grid_fps", json!(9.7), Some(json!(9))));
    // free text
    for key in ["custom_text", "grid_anim", "clock_host", "clock_pass"] {
        for ok in ["", "x", "HI {time} caf\u{e9} \u{1F600}", "line\nbreak"] {
            c.push((key, json!(ok), Some(json!(ok))));
        }
        for bad in [json!(5), Value::Null, json!(["a"]), json!(true)] {
            c.push((key, bad, None));
        }
    }
    // choices: exactly these names
    for (key, names) in [
        ("message_mode", &["weather", "window", "text", "date", "forecast", "rotate"][..]),
        ("icon_mode", &["off", "audio", "system", "all"][..]),
        ("link", &["auto", "usb", "wifi"][..]),
    ] {
        for n in names {
            c.push((key, json!(n), Some(json!(n))));
        }
        for bad in [json!("bogus"), json!(names[0].to_uppercase()), json!(""), json!(1), Value::Null] {
            c.push((key, bad, None));
        }
    }
    // drawn frames: 7 rows each (padded / cut), low 7 bits of each row number
    for (sent, shown) in [
        (json!([]), json!([])),
        (json!([[]]), json!([[0, 0, 0, 0, 0, 0, 0]])),
        (json!([[1, 2]]), json!([[1, 2, 0, 0, 0, 0, 0]])),
        (json!([[1, 2, 3, 4, 5, 6, 7, 8]]), json!([[1, 2, 3, 4, 5, 6, 7]])),
        (json!([[300, -1, 127, 128]]), json!([[44, 127, 127, 0, 0, 0, 0]])),
        (json!([[u64::MAX]]), json!([[127, 0, 0, 0, 0, 0, 0]])),
    ] {
        c.push(("grid_custom", sent, Some(shown)));
    }
    for bad in [
        json!([1, 2]),
        json!([[1], 2]),
        json!("x"),
        json!({}),
        Value::Null,
        json!([[1.5]]),
        json!([["5"]]),
        json!([[true]]),
        json!([[null]]),
    ] {
        c.push(("grid_custom", bad, None));
    }
    // not app settings
    for key in ["brightness", "nope", ""] {
        c.push((key, json!(1), None));
    }
    c
}

#[test]
fn every_setting_validates_as_the_table_says() {
    let cases = cases();
    for (key, sent, want) in &cases {
        let mut s = Settings::default();
        let got = s.set(key, sent);
        match want {
            Some(shown) => {
                assert_eq!(got, Ok(()), "{key} {sent}");
                let now = &s.to_json()[*key];
                // 2 and 2.0 must not be confused either
                assert_eq!(now.to_string(), shown.to_string(), "{key} {sent}");
            }
            None => {
                assert!(got.is_err(), "{key} {sent} should be rejected");
                assert_eq!(s, Settings::default(), "{key} {sent}: nothing changed");
            }
        }
    }
    // the table covers every setting, accepted and rejected
    for name in Settings::NAMES {
        assert!(cases.iter().any(|(k, _, w)| k == name && w.is_some()), "{name} accepted");
        assert!(cases.iter().any(|(k, _, w)| k == name && w.is_none()), "{name} rejected");
    }
}

#[test]
fn error_messages() {
    let mut s = Settings::default();
    let mut err = |k: &str, v: Value| s.set(k, &v).unwrap_err();
    assert_eq!(err("brightness", json!(3)), "unknown setting");
    assert_eq!(err("logo", json!("yes")), "expected true/false");
    assert_eq!(err("grid_fps", json!("9")), "expected a number");
    assert_eq!(err("gain_out", json!(null)), "expected a number");
    assert_eq!(err("custom_text", json!(5)), "expected a string");
    assert_eq!(
        err("message_mode", json!("bogus")),
        "expected one of: weather, window, text, date, forecast, rotate"
    );
    assert_eq!(err("icon_mode", json!(1)), "expected one of: off, audio, system, all");
    assert_eq!(err("link", json!("serial")), "expected one of: auto, usb, wifi");
    assert_eq!(err("grid_custom", json!([1])), "expected a list of frames, each a list of rows");
    assert_eq!(err("grid_custom", json!([["1"]])), "grid rows must be whole numbers");
}

#[test]
fn grid_custom_keeps_at_most_80_frames() {
    let mut s = Settings::default();
    s.set("grid_custom", &Value::Array(vec![json!([1]); 100])).unwrap();
    assert_eq!(s.grid_custom.len(), 80);
    assert_eq!(s.grid_custom[0], [1, 0, 0, 0, 0, 0, 0]);
}

#[test]
fn apply_is_all_or_nothing() {
    let mut s = Settings::default();
    let bad = json!({"logo": true, "grid_fps": "x", "nope": 1, "gain_out": 2});
    let errors = s.apply(bad.as_object().unwrap()).unwrap_err();
    assert_eq!(
        errors.keys().map(String::as_str).collect::<Vec<_>>(),
        ["grid_fps", "nope"],
        "a reason for every bad key, none for the good ones"
    );
    assert!(errors.values().all(Value::is_string));
    assert_eq!(s, Settings::default(), "nothing applied");

    let good = json!({"logo": true, "grid_fps": 99, "message_mode": "rotate", "link": "usb"});
    assert_eq!(s.apply(good.as_object().unwrap()), Ok(()));
    assert!(s.logo);
    assert_eq!(s.grid_fps, 30);
    assert_eq!(s.message_mode, MessageMode::Rotate);
    assert_eq!(s.link, LinkMode::Usb);
    assert_eq!(s.apply(&Map::new()), Ok(()), "an empty patch is fine");
}

#[test]
fn defaults() {
    let d = Settings::default();
    assert_eq!(Settings::NAMES.len(), 24);
    let json = d.to_json();
    assert_eq!(json.keys().map(String::as_str).collect::<Vec<_>>(), Settings::NAMES);
    assert!(json["gain_out"].is_f64() && json["scroll_ms"].is_u64());
    assert_eq!(json["grid_custom"], json!([]));
    assert_eq!((d.gain_out, d.scroll_ms, d.rotate_dwell, d.grid_fps), (1.0, 350, 8, 12));
    assert_eq!((d.message_mode, d.icon_mode, d.link), (MessageMode::Weather, IconMode::Off, LinkMode::Auto));
    assert_eq!((d.custom_text.as_str(), d.grid_anim.as_str()), ("HELLO", "spin"));
    assert_eq!(PROTOCOL, 5);
    assert!(CLOCK_KEYS.iter().all(|k| !Settings::NAMES.contains(k)));
    // every default passes its own validation unchanged
    let mut again = Settings::default();
    assert_eq!(again.apply(&json), Ok(()));
    assert_eq!(again, d);
}

// ------------------------------------------------------------------ the file

/// What the app writes for the defaults: one setting per line, in `Settings::NAMES` order.
const DEFAULTS_FILE: &str = r#"{
  "time_sync": true,
  "spectrum": true,
  "gain_out": 1.0,
  "gain_mic": 1.0,
  "bars": true,
  "weather": true,
  "message_mode": "weather",
  "custom_text": "HELLO",
  "scroll_ms": 350,
  "rotate_weather": true,
  "rotate_window": true,
  "rotate_text": false,
  "rotate_date": false,
  "rotate_dwell": 8,
  "grid_enable": false,
  "grid_anim": "spin",
  "grid_frame": false,
  "grid_fps": 12,
  "grid_custom": [],
  "icon_mode": "off",
  "link": "auto",
  "clock_host": "",
  "clock_pass": "",
  "logo": false
}
"#;

#[test]
fn file_format() {
    assert_eq!(config::file_text(&Settings::default()), DEFAULTS_FILE);
    let dir = fresh_dir("format");
    let path = dir.join("wfd_config.json");
    save(&path, &Settings::default()).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), DEFAULTS_FILE);
    // frames and odd text stay on their setting's line
    let s = Settings {
        grid_custom: vec![[8, 8, 8, 127, 8, 8, 8], [0; 7]],
        custom_text: "caf\u{e9} \"q\"\nline".into(),
        ..Settings::default()
    };
    let text = config::file_text(&s);
    assert_eq!(text.lines().count(), Settings::NAMES.len() + 2);
    assert!(text.contains("\n  \"grid_custom\": [[8,8,8,127,8,8,8],[0,0,0,0,0,0,0]],\n"), "{text}");
    assert!(text.contains("\n  \"custom_text\": \"caf\u{e9} \\\"q\\\"\\nline\",\n"), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn files_written_by_v1_4_0_load_the_same() {
    // v1.4.0 wrote Python's json.dumps(indent=2) format (CRLF, \u escapes, a value per line)
    assert_eq!(load(&fixture("config_defaults_v1.4.0.json")), Settings::default());
}

/// A v1.4.0-format file with odd values: valid ones load as they were (numbers clamped to their
/// range, frames padded), each wrong-typed one gives its default and one log line.
#[test]
fn odd_v1_4_0_file_loads_to_valid_settings() {
    start_log();
    let path = fixture("config_saved.json");
    let loaded = load(&path);
    let want: Map<String, Value> =
        serde_json::from_slice(&std::fs::read(fixture("config_saved_loaded.json")).unwrap())
            .unwrap();
    assert_eq!(loaded.to_json(), want);
    let logged = log_lines_about(&path);
    for key in ["grid_anim", "link", "clock_host"] {
        assert_eq!(
            logged.iter().filter(|l| l.contains(&format!(": {key} = "))).count(),
            1,
            "{key}: {logged:?}"
        );
    }
}

#[test]
fn wrong_typed_values_give_the_default_and_a_log_line() {
    start_log();
    let dir = fresh_dir("wrongtype");
    let path = dir.join("wfd_config.json");
    std::fs::write(&path, r#"{"logo": "yes", "grid_fps": [1], "spectrum": false, "scroll_ms": 9999}"#)
        .unwrap();
    let loaded = load(&path);
    assert_eq!(
        loaded,
        Settings { spectrum: false, scroll_ms: 5000, ..Settings::default() }
    );
    let logged = log_lines_about(&path);
    assert_eq!(logged.len(), 2, "{logged:?}");
    assert!(logged[0].contains("logo = \"yes\" isn't valid (expected true/false); using the default"));
    assert!(logged[1].contains("grid_fps = [1]"));
    assert!(!config::bad_copy_path(&path).exists(), "the file itself is fine");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn load_ignores_clock_keys_and_comments() {
    let dir = fresh_dir("old");
    let path = dir.join("old.json");
    std::fs::write(
        &path,
        r#"{"brightness": 3, "style": 1, "logo": true, "_comment": "x"}"#,
    )
    .unwrap();
    assert_eq!(load(&path), Settings { logo: true, ..Settings::default() });
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn bad_or_missing_file_gives_defaults() {
    assert_eq!(load(Path::new("no/such/wfd_config.json")), Settings::default());
    let dir = fresh_dir("bad");
    for content in ["not json", "[1, 2]", "", "\u{feff}{\"logo\": true}"] {
        let path = dir.join("bad.json");
        std::fs::write(&path, content).unwrap();
        assert_eq!(load(&path), Settings::default(), "{content:?}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn garbled_file_is_kept_as_bad_copy_and_logged() {
    start_log();
    let dir = fresh_dir("garbled");
    let path = dir.join("wfd_config.json");
    let garbled = "{\"logo\": tr";
    std::fs::write(&path, garbled).unwrap();
    assert_eq!(load(&path), Settings::default());
    let bad = config::bad_copy_path(&path);
    assert_eq!(bad, dir.join("wfd_config.json.bad"));
    assert_eq!(std::fs::read_to_string(&bad).unwrap(), garbled);
    // the next save replaces the garbled file but leaves the copy
    save(&path, &Settings::default()).unwrap();
    assert_eq!(std::fs::read_to_string(&bad).unwrap(), garbled);
    let logged = log_lines_about(&path);
    assert!(
        logged.iter().any(|l| l.contains("wfd_config.json isn't valid JSON")
            && l.contains("wfd_config.json.bad")),
        "{logged:?}"
    );
    // a top level that isn't an object counts as damaged too
    let other = dir.join("list.json");
    std::fs::write(&other, "[1, 2]").unwrap();
    assert_eq!(load(&other), Settings::default());
    assert!(dir.join("list.json.bad").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn missing_file_makes_no_bad_copy() {
    let dir = fresh_dir("missing");
    let path = dir.join("wfd_config.json");
    assert_eq!(load(&path), Settings::default());
    assert!(names_in(&dir).is_empty(), "{:?}", names_in(&dir));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn save_is_atomic_and_leaves_no_temp_file() {
    let dir = fresh_dir("atomic");
    let path = dir.join("wfd_config.json");
    let mut s = Settings { custom_text: "FIRST".into(), ..Settings::default() };
    save(&path, &s).unwrap();
    s.custom_text = "SECOND".into();
    save(&path, &s).unwrap(); // replaces an existing file
    assert_eq!(names_in(&dir), ["wfd_config.json"]);
    assert_eq!(load(&path), s);
    // a save that can't happen (the target is a directory) cleans up after itself
    let blocked = dir.join("blocked.json");
    std::fs::create_dir(&blocked).unwrap();
    assert!(save(&blocked, &s).is_err());
    assert_eq!(names_in(&dir), ["blocked.json", "wfd_config.json"]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn save_load_round_trip() {
    let s = Settings {
        gain_mic: 0.3,
        gain_out: 1e-5,
        custom_text: "TODAY {date} caf\u{e9} \u{1F600} \u{1}\u{7f} \"q\" back\\slash\nline\ttab/".into(),
        grid_custom: vec![[8, 8, 8, 127, 8, 8, 8], [0; 7]],
        message_mode: MessageMode::Forecast,
        icon_mode: IconMode::System,
        link: LinkMode::Wifi,
        clock_host: "10.0.0.9".into(),
        clock_pass: "hunter2".into(),
        scroll_ms: 50,
        rotate_dwell: 600,
        logo: true,
        ..Settings::default()
    };
    let dir = fresh_dir("roundtrip");
    let path = dir.join("wfd_config.json");
    save(&path, &s).unwrap();
    assert_eq!(load(&path), s);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The user's real settings file (if present next to the retired Python app, in
/// legacy/python-app/ or its old home wfd-control/) loads, and survives a save unchanged.
#[test]
fn real_config_file_round_trips() {
    // WFD_REAL_CONFIG can point at it when testing from a worktree without one
    let real = std::env::var_os("WFD_REAL_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
            let new = root.join("legacy/python-app/wfd_config.json");
            if new.exists() { new } else { root.join("wfd-control/wfd_config.json") }
        });
    if !real.exists() {
        return;
    }
    let loaded = load(&real);
    let dir = fresh_dir("real");
    let path = dir.join("wfd_config.json");
    save(&path, &loaded).unwrap();
    assert_eq!(load(&path), loaded);
    let _ = std::fs::remove_dir_all(&dir);
}
