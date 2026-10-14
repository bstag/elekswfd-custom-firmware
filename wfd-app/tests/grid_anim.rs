//! Parity with wfd-control/grid_anim.py.
//! Deterministic animations are compared frame by frame with tests/fixtures/grid_frames.json
//! (regenerate with gen_font14_grid_icons.py); random ones are checked for their properties.

use serde_json::Value;
use wfd_app::grid_anim::{ANIMATIONS, Extra, GridEngine};

type Frame = [u8; 7];

fn fixture() -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/grid_frames.json"
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn frame_of(v: &Value) -> Frame {
    let rows: Vec<u8> = v
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b.as_u64().unwrap() as u8)
        .collect();
    rows.try_into().unwrap()
}

fn frames_of(v: &Value) -> Vec<Frame> {
    v.as_array().unwrap().iter().map(frame_of).collect()
}

fn run(name: &str, n: usize, extra: &Extra, seed: u64) -> Vec<Frame> {
    let mut eng = GridEngine::new(seed);
    (0..n).map(|_| eng.frame(name, extra)).collect()
}

fn lit(f: &Frame) -> u32 {
    f.iter().map(|r| r.count_ones()).sum()
}

#[test]
fn animation_list_matches_python() {
    let fx = fixture();
    let names: Vec<&str> = fx["animations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n.as_str().unwrap())
        .collect();
    // Python's list, plus "weather" (Rust only; see tests/weather_anim.rs)
    assert_eq!(ANIMATIONS.last(), Some(&"weather"));
    assert_eq!(names, ANIMATIONS[..ANIMATIONS.len() - 1]);
}

#[test]
fn deterministic_animations_match_python() {
    let fx = fixture();
    // "nope" is an unknown name: Python falls back to spin.
    for name in ["spin", "bounce", "wave", "pulse", "scan", "heart", "nope"] {
        let want = frames_of(&fx["anims"][name]["frames"]);
        assert_eq!(want.len(), 400);
        let got = run(name, want.len(), &Extra::default(), 1);
        for (i, (g, w)) in got.iter().zip(&want).enumerate() {
            assert_eq!(g, w, "{name} frame {i}");
        }
    }
}

#[test]
fn audio_and_mouth_match_python() {
    let fx = fixture();
    for name in ["audio", "mouth"] {
        let case = &fx["anims"][name];
        let want = frames_of(&case["frames"]);
        let mut eng = GridEngine::new(7);
        for (i, (bits, w)) in case["bands_bits"]
            .as_array()
            .unwrap()
            .iter()
            .zip(&want)
            .enumerate()
        {
            let mut extra = Extra::default();
            for (b, v) in extra.bands.iter_mut().zip(bits.as_array().unwrap()) {
                *b = f64::from_bits(v.as_u64().unwrap());
            }
            assert_eq!(&eng.frame(name, &extra), w, "{name} frame {i}");
        }
    }
}

#[test]
fn custom_matches_python() {
    let fx = fixture();
    let case = &fx["anims"]["custom"];
    let extra = Extra {
        custom: frames_of(&case["custom"]),
        ..Extra::default()
    };
    let want = frames_of(&case["frames"]);
    assert_eq!(run("custom", want.len(), &extra, 3), want);

    let want = frames_of(&fx["anims"]["custom_empty"]["frames"]);
    assert_eq!(run("custom", want.len(), &Extra::default(), 3), want);
}

#[test]
fn cpu_extremes_match_python() {
    let fx = fixture();
    for pct in [0u32, 100] {
        let want = frames_of(&fx["anims"][format!("cpu{pct}")]["frames"]);
        let extra = Extra {
            cpu: f64::from(pct),
            ..Extra::default()
        };
        assert_eq!(run("cpu", want.len(), &extra, 5), want, "cpu {pct}%");
    }
}

#[test]
fn switching_animation_resets_state() {
    let fx = fixture();
    let mut eng = GridEngine::new(9);
    for (i, step) in fx["switch"].as_array().unwrap().iter().enumerate() {
        let name = step[0].as_str().unwrap();
        assert_eq!(
            eng.frame(name, &Extra::default()),
            frame_of(&step[1]),
            "step {i} ({name})"
        );
    }
}

#[test]
fn every_animation_stays_in_seven_columns() {
    let extra = Extra {
        bands: [0.9; 7],
        cpu: 63.0,
        custom: vec![[0xFF; 7]],
        ..Extra::default()
    };
    for name in ANIMATIONS {
        for f in run(name, 300, &extra, 11) {
            assert!(f.iter().all(|&r| r < 128), "{name}: {f:?}");
        }
    }
}

#[test]
fn rain_falls_one_row_per_frame() {
    for seed in [1, 2, 3] {
        let frames = run("rain", 400, &Extra::default(), seed);
        assert!(
            frames.iter().filter(|f| f[0] != 0).count() > 100,
            "drops spawn"
        );
        assert!(frames[0][1..].iter().all(|&r| r == 0));
        for w in frames.windows(2) {
            let (prev, next) = (&w[0], &w[1]);
            assert_eq!(
                &next[1..],
                &prev[..6],
                "every drop moves down exactly one row"
            );
            assert!(next[0].count_ones() <= 1, "at most one new drop per frame");
        }
    }
}

#[test]
fn sparkle_twinkles() {
    let frames = run("sparkle", 400, &Extra::default(), 4);
    // Two sparks a frame (possibly the same cell); each stays above 0.3 for five frames.
    assert!(frames.iter().all(|f| (1..=10).contains(&lit(f))));
    // A frame can repeat when new sparks land on lit cells and none expire, but rarely.
    let changes = frames.windows(2).filter(|w| w[0] != w[1]).count();
    assert!(changes > 360, "changes {changes}");
}

/// One Conway step on the 7x7 torus.
fn life_step(f: &Frame) -> Frame {
    let cell = |r: usize, c: usize| f[r % 7] >> (c % 7) & 1 == 1;
    let mut out = [0u8; 7];
    for (r, row) in out.iter_mut().enumerate() {
        for c in 0..7 {
            let mut n = 0;
            for dr in [6, 0, 1] {
                for dc in [6, 0, 1] {
                    if (dr, dc) != (0, 0) && cell(r + dr, c + dc) {
                        n += 1;
                    }
                }
            }
            if n == 3 || (cell(r, c) && n == 2) {
                *row |= 1 << c;
            }
        }
    }
    out
}

#[test]
fn life_follows_conway_and_reseeds() {
    for seed in [1, 2, 3, 4] {
        let frames = run("life", 1000, &Extra::default(), seed);
        let mut reseeds = 0;
        for (i, w) in frames.windows(2).enumerate() {
            let t = i as u64 + 2; // t of w[1]
            let (prev, next) = (&w[0], &w[1]);
            if !t.is_multiple_of(3) {
                assert_eq!(next, prev, "seed {seed} t {t}: holds between steps");
                continue;
            }
            assert_ne!(
                lit(next),
                0,
                "seed {seed} t {t}: reseeds instead of going blank"
            );
            let stepped = life_step(prev);
            if t.is_multiple_of(90) || lit(&stepped) == 0 {
                reseeds += 1;
            } else {
                assert_eq!(next, &stepped, "seed {seed} t {t}: Conway step");
            }
        }
        assert!(reseeds >= 11, "periodic reseed every 90 frames");
    }
}

#[test]
fn cpu_fills_and_flickers() {
    let extra = Extra {
        cpu: 50.0,
        ..Extra::default()
    };
    let frames = run("cpu", 400, &extra, 6);
    // round(3.5) == 4 (half-to-even): four full rows, flicker on row 2 about half the time.
    for f in &frames {
        assert_eq!(&f[3..], &[0x7F; 4]);
        assert_eq!(&f[..2], &[0, 0]);
        assert!(f[2].count_ones() <= 1);
    }
    let flickers = frames.iter().filter(|f| f[2] != 0).count();
    assert!((120..=280).contains(&flickers), "flickers {flickers}");
}

#[test]
fn seeds_are_reproducible_and_distinct() {
    for name in ["rain", "sparkle", "life"] {
        assert_eq!(
            run(name, 200, &Extra::default(), 42),
            run(name, 200, &Extra::default(), 42)
        );
        assert_ne!(
            run(name, 200, &Extra::default(), 42),
            run(name, 200, &Extra::default(), 43)
        );
    }
}
