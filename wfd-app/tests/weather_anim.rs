//! The "weather" grid animation (src/grid_anim.rs). The clock firmware draws the same frames
//! (grid_engine.h): both are checked against tests/fixtures/weather_frames.json, this side here
//! and the C++ side in wfd-control/tests/test_grid_engine.py.
//!
//! Regenerate the fixture after changing the art (then check the C++ still matches):
//!     WFD_UPDATE_FIXTURES=1 cargo test --test weather_anim
use serde_json::{Map, Value, json};
use std::path::PathBuf;
use wfd_app::grid_anim::{Extra, GridEngine, SCENES, scene_from_name, scene_from_wmo};
use wfd_app::weather::parse_scene;

const FRAMES: usize = 48;

fn frames(scene: usize) -> Vec<[u8; 7]> {
    let mut eng = GridEngine::new(1);
    let extra = Extra { scene, ..Extra::default() };
    (0..FRAMES).map(|_| eng.frame("weather", &extra)).collect()
}

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/weather_frames.json")
}

#[test]
fn frames_match_the_shared_fixture() {
    let mut got = Map::new();
    for (i, name) in SCENES.iter().enumerate() {
        got.insert(name.to_string(), json!(frames(i)));
    }
    let got = Value::Object(got);
    let path = fixture_path();
    if std::env::var_os("WFD_UPDATE_FIXTURES").is_some() {
        std::fs::write(&path, serde_json::to_string_pretty(&got).unwrap() + "\n").unwrap();
    }
    let want: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(got, want, "weather frames changed: regenerate the fixture (see the top of this file)");
}

#[test]
fn every_scene_moves_and_stays_on_the_grid() {
    for (i, name) in SCENES.iter().enumerate() {
        let f = frames(i);
        assert!(f.iter().flatten().all(|&r| r < 128), "{name}: off the grid");
        assert!(f.iter().all(|rows| rows.iter().any(|&r| r != 0)), "{name}: a blank frame");
        let distinct: std::collections::HashSet<_> = f.iter().collect();
        assert!(distinct.len() >= 2, "{name}: doesn't move");
    }
}

#[test]
fn night_scenes_show_the_moon() {
    // the crescent's left edge (rows 2..=4, column 0) is lit on every night frame
    for name in ["night", "night_partly"] {
        for rows in frames(scene_from_name(name).unwrap()) {
            assert!((2..=4).all(|r| rows[r] & 1 != 0), "{name}: no moon");
        }
    }
}

#[test]
fn wmo_codes_pick_scenes() {
    let s = |code, day| SCENES[scene_from_wmo(code, day)];
    assert_eq!(s(0, true), "clear");
    assert_eq!(s(1, true), "clear");
    assert_eq!(s(0, false), "night");
    assert_eq!(s(2, true), "partly");
    assert_eq!(s(2, false), "night_partly");
    assert_eq!(s(3, true), "cloudy");
    assert_eq!(s(3, false), "cloudy", "no moon behind an overcast sky");
    assert_eq!(s(45, true), "fog");
    assert_eq!(s(48, false), "fog");
    for code in [51, 55, 61, 65, 67, 80, 82] {
        assert_eq!(s(code, true), "rain", "code {code}");
    }
    for code in [71, 75, 77, 85, 86] {
        assert_eq!(s(code, false), "snow", "code {code}");
    }
    for code in [95, 96, 99] {
        assert_eq!(s(code, true), "storm", "code {code}");
    }
    assert_eq!(s(42, true), "cloudy", "unknown codes");
}

#[test]
fn the_forecast_gives_the_scene() {
    let cur = |code: Value, is_day: Value| json!({"current": {"weather_code": code, "is_day": is_day}});
    assert_eq!(parse_scene(&cur(json!(0), json!(1))).unwrap(), "clear");
    assert_eq!(parse_scene(&cur(json!(0), json!(0))).unwrap(), "night");
    assert_eq!(parse_scene(&cur(json!(2.0), json!(0))).unwrap(), "night_partly");
    assert_eq!(parse_scene(&cur(json!(73), json!(1))).unwrap(), "snow");
    assert_eq!(parse_scene(&json!({"current": {"weather_code": 0}})).unwrap(), "clear", "day if is_day is missing");
    assert!(parse_scene(&json!({"current": {}})).is_err());
}
