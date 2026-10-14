//! Parity with app.py ICON / compute_icons.
//! Fixture: tests/fixtures/icons_cases.json (regenerate with gen_font14_grid_icons.py).

use serde_json::Value;
use wfd_app::icons::{ICON_NAMES, compute_icons};

fn fixture() -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/icons_cases.json"
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn icon_names_match_python_order() {
    let fx = fixture();
    let names: Vec<&str> = fx["names"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n.as_str().unwrap())
        .collect();
    assert_eq!(names, ICON_NAMES);
}

#[test]
fn compute_icons_matches_python() {
    let fx = fixture();
    let cases = fx["cases"].as_array().unwrap();
    assert!(!cases.is_empty());
    for c in cases {
        let mode = c["mode"].as_str().unwrap();
        let out = c["out"].as_u64().unwrap() as u8;
        let mic = c["mic"].as_u64().unwrap() as u8;
        let (cpu, gpu, ram) = (
            c["cpu"].as_f64().unwrap(),
            c["gpu"].as_f64().unwrap(),
            c["ram"].as_f64().unwrap(),
        );
        let online = c["online"].as_bool().unwrap();
        let battery = c["battery"].as_u64().map(|b| b as u8);
        let now = c["now"].as_u64().unwrap();
        let want = c["mask"].as_u64().unwrap() as u16;
        assert_eq!(
            compute_icons(mode, out, mic, cpu, gpu, ram, online, battery, now),
            want,
            "case {c}"
        );
    }
}

#[test]
fn all_mode_lights_every_icon() {
    assert_eq!(
        compute_icons("all", 0, 0, 0.0, 0.0, 0.0, false, None, 0),
        (1 << ICON_NAMES.len()) - 1
    );
}
