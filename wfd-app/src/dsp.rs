//! dsp: audio levels (0..NLEV meters) and the 7-band spectrum, ported from app.py
//! (`ClockLink.audio_levels` and the band block in `audio_reader`). Pure; parity-tested in
//! tests/dsp.rs against fixtures produced by the Python code.

use std::sync::Arc;

use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};

/// Spectrum column height on the clock.
pub const NLEV: u8 = 12;

/// Band edges in Hz (`_BAND_EDGES`): band i covers [EDGES[i], EDGES[i+1]).
pub const BAND_EDGES: [f64; 8] = [60.0, 160.0, 320.0, 640.0, 1280.0, 2560.0, 5120.0, 12000.0];

/// Auto-gained meter levels for (output, mic): fast rise, slow fall.
#[derive(Clone, Debug)]
pub struct Levels {
    peak: [f64; 2],
    disp: [f64; 2],
}

impl Default for Levels {
    fn default() -> Self {
        Self::new()
    }
}

impl Levels {
    pub fn new() -> Self {
        Self {
            peak: [1e-5; 2],
            disp: [0.0; 2],
        }
    }

    /// One step of `audio_levels`: energies and gains are (out, mic); returns 0..=NLEV levels.
    pub fn update(&mut self, energy: [f64; 2], gain: [f64; 2]) -> [u8; 2] {
        let nlev = f64::from(NLEV);
        let mut levels = [0u8; 2];
        for k in 0..2 {
            let e = energy[k] * gain[k];
            self.peak[k] = e.max(self.peak[k] * 0.995);
            let frac = if self.peak[k] < 1e-5 {
                0.0
            } else {
                e / self.peak[k]
            };
            let target = nlev * 0f64.max((frac - 0.12) / 0.88);
            let d = self.disp[k];
            self.disp[k] = if target >= d {
                target
            } else {
                target.max(d - 0.8)
            };
            levels[k] = nlev.min(self.disp[k]).round_ties_even() as u8;
        }
        levels
    }
}

/// numpy's pairwise float32 summation (`FLOAT_pairwise_sum`), so sums round like Python's.
fn pairwise_sum(a: &[f32]) -> f32 {
    let n = a.len();
    if n < 8 {
        a.iter().fold(0.0, |s, &v| s + v)
    } else if n <= 128 {
        let mut r = [0f32; 8];
        r.copy_from_slice(&a[..8]);
        let whole = n - n % 8;
        for chunk in a[8..whole].as_chunks::<8>().0 {
            for (acc, &v) in r.iter_mut().zip(chunk) {
                *acc += v;
            }
        }
        let mut res = ((r[0] + r[1]) + (r[2] + r[3])) + ((r[4] + r[5]) + (r[6] + r[7]));
        for &v in &a[whole..] {
            res += v;
        }
        res
    } else {
        let mut n2 = n / 2;
        n2 -= n2 % 8;
        pairwise_sum(&a[..n2]) + pairwise_sum(&a[n2..])
    }
}

/// Interleaved frames -> mono by averaging channels (float32, like `x.reshape(-1, ch).mean(axis=1)`).
pub fn to_mono(samples: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return samples.to_vec();
    }
    samples
        .chunks_exact(channels)
        .map(|f| pairwise_sum(f) / channels as f32)
        .collect()
}

/// RMS of the channel-averaged signal, plus 1e-9. All float32 like the Python code: numpy 2
/// keeps `float32 + 1e-9` in float32, so the floor itself is the f32 nearest 1e-9.
pub fn rms_mono(samples: &[f32], channels: usize) -> f64 {
    let x = to_mono(samples, channels);
    if x.is_empty() {
        return f64::from(1e-9f32);
    }
    let sq: Vec<f32> = x.iter().map(|v| v * v).collect();
    let mean = pairwise_sum(&sq) / sq.len() as f32;
    f64::from(mean.sqrt() + 1e-9)
}

/// 7-band spectrum with per-band auto-gain (`_band_peak`), values 0..1.
pub struct BandTracker {
    peak: [f64; 7],
    fft: Option<(usize, Arc<dyn Fft<f64>>)>,
}

impl Default for BandTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl BandTracker {
    pub fn new() -> Self {
        Self {
            peak: [1e-4; 7],
            fft: None,
        }
    }

    /// Band levels for one mono block: hanning window, rfft magnitudes, RMS per band, peak decay.
    pub fn bands(&mut self, mono: &[f32], rate: u32) -> [f64; 7] {
        let n = mono.len();
        let mut out = [0.0; 7];
        if n == 0 {
            return out;
        }
        let fft = match &self.fft {
            Some((len, f)) if *len == n => Arc::clone(f),
            _ => {
                let f = FftPlanner::new().plan_fft_forward(n);
                self.fft = Some((n, Arc::clone(&f)));
                f
            }
        };
        let win = hanning(n);
        let mut buf: Vec<Complex<f64>> = mono
            .iter()
            .zip(&win)
            .map(|(&x, &w)| Complex::new(f64::from(x) * w, 0.0))
            .collect();
        fft.process(&mut buf);
        let half = n / 2 + 1;
        // np.fft.rfftfreq(n, 1 / sr): k * (1 / (n * (1 / sr)))
        let val = 1.0 / (n as f64 * (1.0 / f64::from(rate)));
        for (bi, o) in out.iter_mut().enumerate() {
            let (lo, hi) = (BAND_EDGES[bi], BAND_EDGES[bi + 1]);
            let (mut sum, mut count) = (0.0f64, 0usize);
            for (k, c) in buf[..half].iter().enumerate() {
                let f = k as f64 * val;
                if f >= lo && f < hi {
                    sum += c.norm_sqr();
                    count += 1;
                }
            }
            let e = if count > 0 {
                (sum / count as f64).sqrt()
            } else {
                0.0
            };
            self.peak[bi] = e.max(self.peak[bi] * 0.99);
            *o = if self.peak[bi] < 1e-5 {
                0.0
            } else {
                (e / self.peak[bi]).min(1.0)
            };
        }
        out
    }
}

/// `np.hanning(m)`: 0.5 + 0.5 * cos(pi * n / (m - 1)) for n = 1-m, 3-m, ..., m-1.
fn hanning(m: usize) -> Vec<f64> {
    if m == 1 {
        return vec![1.0];
    }
    let mf = m as f64;
    (0..m)
        .map(|k| {
            let n = 1.0 - mf + 2.0 * k as f64;
            0.5 + 0.5 * (std::f64::consts::PI * n / (mf - 1.0)).cos()
        })
        .collect()
}
