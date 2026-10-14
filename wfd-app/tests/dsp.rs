//! Parity with the Python reference: fixtures from tests/fixtures/dsp_gen.py (runs app.py's code).

use serde_json::Value;
use wfd_app::dsp::{BandTracker, Levels, NLEV, rms_mono, to_mono};

const FIX: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/");

fn expected() -> Value {
    serde_json::from_str(&std::fs::read_to_string(format!("{FIX}dsp_expected.json")).unwrap())
        .unwrap()
}

/// All int16 samples, converted like Python: astype(float32) / 32768.0.
fn samples() -> Vec<f32> {
    std::fs::read(format!("{FIX}dsp_blocks.bin"))
        .unwrap()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| f32::from(i16::from_le_bytes([b[0], b[1]])) / 32768.0)
        .collect()
}

/// Numbers, or exact repr() strings (energies are compared bit for bit).
fn f64s(v: &Value) -> Vec<f64> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|x| {
            x.as_str()
                .map_or_else(|| x.as_f64().unwrap(), |s| s.parse().unwrap())
        })
        .collect()
}

/// Run every scenario through rms_mono + BandTracker; returns (name, energies) per scenario.
fn check_scenarios() -> Vec<(String, Vec<f64>)> {
    let exp = expected();
    let all = samples();
    let mut energies = Vec::new();
    for sc in exp["scenarios"].as_array().unwrap() {
        let name = sc["name"].as_str().unwrap().to_string();
        let ch = sc["channels"].as_u64().unwrap() as usize;
        let rate = sc["rate"].as_u64().unwrap() as u32;
        let off = sc["offset"].as_u64().unwrap() as usize;
        let blocks = sc["blocks"].as_u64().unwrap() as usize;
        let want_e = f64s(&sc["energy"]);
        let mut tracker = BandTracker::new();
        let mut got_e = Vec::new();
        for i in 0..blocks {
            let block = &all[off + i * 1024 * ch..off + (i + 1) * 1024 * ch];
            let e = rms_mono(block, ch);
            assert_eq!(e, want_e[i], "{name} block {i} energy");
            got_e.push(e);
            let bands = tracker.bands(&to_mono(block, ch), rate);
            let want_b = f64s(&sc["bands"][i]);
            for (b, (g, w)) in bands.iter().zip(&want_b).enumerate() {
                assert!(
                    (g - w).abs() < 1e-6,
                    "{name} block {i} band {b}: {g} vs {w}"
                );
            }
        }
        energies.push((name, got_e));
    }
    energies
}

#[test]
fn energy_and_bands_match_python() {
    let sc = check_scenarios();
    assert_eq!(sc.len(), 3);
}

#[test]
fn levels_match_python() {
    let exp = expected();
    let lv = &exp["levels"];
    let mut levels = Levels::new();
    let pairs = lv["energy"].as_array().unwrap();
    for (i, ((e, g), want)) in pairs
        .iter()
        .zip(lv["gain"].as_array().unwrap())
        .zip(lv["expected"].as_array().unwrap())
        .enumerate()
    {
        let e = f64s(e);
        let g = f64s(g);
        let got = levels.update([e[0], e[1]], [g[0], g[1]]);
        let want = [
            want[0].as_u64().unwrap() as u8,
            want[1].as_u64().unwrap() as u8,
        ];
        assert_eq!(got, want, "step {i}");
        assert!(got[0] <= NLEV && got[1] <= NLEV);
    }
}

#[test]
fn silence_gives_floor_energy_and_zero_bands() {
    let z = vec![0f32; 2048];
    assert_eq!(rms_mono(&z, 2), f64::from(1e-9f32));
    assert_eq!(BandTracker::new().bands(&z[..1024], 48000), [0.0; 7]);
}
