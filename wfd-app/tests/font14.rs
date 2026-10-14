//! Parity with wfd-control/font14.py and app.py scroll_frame / clean_omtext.
//! Fixture: tests/fixtures/font14_cases.json (regenerate with gen_font14_grid_icons.py).

use serde_json::Value;
use wfd_app::font14::{c_header, clean_omtext, glyph, render6, scroll_frame};

fn fixture() -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/font14_cases.json"
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn bytes(v: &Value) -> Vec<u8> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|b| b.as_u64().unwrap() as u8)
        .collect()
}

#[test]
fn glyph_matches_python() {
    let fx = fixture();
    let cases = fx["glyphs"].as_array().unwrap();
    assert!(cases.len() >= 95);
    for case in cases {
        let s = case[0].as_str().unwrap();
        let ch = s.chars().next().unwrap();
        let want = bytes(&case[1]);
        let (lo, hi) = glyph(ch);
        assert_eq!(vec![lo, hi], want, "glyph({ch:?})");
    }
}

#[test]
fn glyph_basics() {
    assert_eq!(glyph('a'), glyph('A'));
    assert_eq!(glyph(' '), (0, 0));
    assert_eq!(glyph('~'), (0, 0));
    assert_eq!(glyph('\u{df}'), (0, 0)); // 'ß'.upper() == "SS": not a table key
}

#[test]
fn render6_matches_python() {
    let fx = fixture();
    for case in fx["render6"].as_array().unwrap() {
        let text = case["text"].as_str().unwrap();
        let start = case["start"].as_i64().unwrap() as isize;
        assert_eq!(
            render6(text, start).to_vec(),
            bytes(&case["out"]),
            "render6({text:?}, {start})"
        );
    }
}

#[test]
fn scroll_frame_matches_python() {
    let fx = fixture();
    for case in fx["scroll"].as_array().unwrap() {
        let text = case["text"].as_str().unwrap();
        let pos = case["pos"].as_u64().unwrap() as usize;
        assert_eq!(
            scroll_frame(text, pos).to_vec(),
            bytes(&case["out"]),
            "scroll_frame({text:?}, {pos})"
        );
    }
}

#[test]
fn clean_omtext_matches_python() {
    let fx = fixture();
    for case in fx["omtext"].as_array().unwrap() {
        let text = case["text"].as_str().unwrap();
        assert_eq!(
            clean_omtext(text),
            case["out"].as_str().unwrap(),
            "clean_omtext({text:?})"
        );
    }
    assert_eq!(clean_omtext(&"z".repeat(200)).len(), 96);
}

#[test]
fn c_header_is_byte_identical() {
    let fx = fixture();
    assert_eq!(c_header(), fx["c_header"].as_str().unwrap());
}
