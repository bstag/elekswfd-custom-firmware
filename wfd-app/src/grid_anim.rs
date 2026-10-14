//! 7x7 grid animations for the EleksWFD, a port of wfd-control/grid_anim.py.
//!
//! [`GridEngine::frame`] returns 7 row bytes; bit `c` of row `r` lights cell (r, c).
//! Deterministic animations produce exactly the Python frames. The random ones (rain,
//! sparkle, life, cpu flicker) follow the same algorithm with a small seeded PRNG instead
//! of Python's Mersenne Twister, so their frames differ but behave the same.
//!
//! "weather" (Rust only, not in the Python app) draws a scene for the current weather; the clock
//! firmware draws the same frames (grid_engine.h), checked against `tests/fixtures/weather_frames.json`.

/// Live data some animations draw from.
#[derive(Clone, Debug, Default)]
pub struct Extra {
    /// Spectrum bands, 0..1 (audio, mouth).
    pub bands: [f64; 7],
    /// Percentages 0..100.
    pub cpu: f64,
    pub gpu: f64,
    pub ram: f64,
    /// Frames for "custom", each 7 row bytes.
    pub custom: Vec<[u8; 7]>,
    /// The weather scene for "weather": an index into [`SCENES`].
    pub scene: usize,
}

pub const ANIMATIONS: [&str; 14] = [
    "spin", "bounce", "rain", "wave", "pulse", "sparkle", "scan", "life", "audio", "cpu", "mouth",
    "heart", "custom", "weather",
];

/// Weather scenes, in the firmware's order (grid_engine.h `Scene`); also the `weather_scene`
/// setting's values besides "auto".
pub const SCENES: [&str; 9] =
    ["clear", "partly", "cloudy", "fog", "rain", "snow", "storm", "night", "night_partly"];
const SC_CLEAR: usize = 0;
const SC_PARTLY: usize = 1;
const SC_CLOUDY: usize = 2;
const SC_FOG: usize = 3;
const SC_RAIN: usize = 4;
const SC_SNOW: usize = 5;
const SC_STORM: usize = 6;
const SC_NIGHT: usize = 7;
const SC_NIGHT_PARTLY: usize = 8;

/// Open-Meteo WMO weather code + day/night -> scene (firmware `sceneFromWmo`). The moon shows on
/// nights you'd see it.
pub fn scene_from_wmo(code: i64, day: bool) -> usize {
    match code {
        ..=1 => if day { SC_CLEAR } else { SC_NIGHT },
        2 => if day { SC_PARTLY } else { SC_NIGHT_PARTLY },
        45 | 48 => SC_FOG,
        51..=67 | 80..=82 => SC_RAIN,
        71..=77 | 85 | 86 => SC_SNOW,
        95.. => SC_STORM,
        _ => SC_CLOUDY, // 3 overcast, and anything unknown
    }
}

/// A scene by name, if it is one.
pub fn scene_from_name(name: &str) -> Option<usize> {
    SCENES.iter().position(|&s| s == name)
}

// Weather art: bit c = column c.
const WX_CLOUD: [u8; 3] = [0x0C, 0x3E, 0x7F]; // ..XX... / .XXXXX. / XXXXXXX
const WX_MOON: [u8; 7] = [0x1C, 0x06, 0x03, 0x03, 0x03, 0x06, 0x1C]; // a crescent opening right
const WX_RAYS: [(i64, i64); 8] = [(0, 3), (1, 5), (3, 6), (5, 5), (6, 3), (5, 1), (3, 0), (1, 1)];
const WX_DROPS: [(u64, i64); 5] = [(3, 0), (1, 2), (0, 4), (2, 6), (3, 3)]; // (phase, column)
const WX_FLAKES: [(i64, i64); 6] = [(0, 1), (2, 4), (4, 0), (1, 6), (5, 3), (3, 2)];
const WX_STARS: [(i64, i64); 5] = [(0, 5), (2, 6), (5, 5), (6, 3), (1, 3)]; // (row, column)
const WX_BOLT: [(i64, i64); 5] = [(3, 4), (4, 3), (4, 4), (5, 3), (6, 2)];

const CENTER: i32 = 3;

/// The outer ring, clockwise from the top-left corner (24 cells).
const RING: [(usize, usize); 24] = {
    let mut ring = [(0, 0); 24];
    let mut i = 0;
    let mut c = 0;
    while c < 7 {
        ring[i] = (0, c);
        i += 1;
        c += 1;
    }
    let mut r = 1;
    while r < 7 {
        ring[i] = (r, 6);
        i += 1;
        r += 1;
    }
    let mut c = 6;
    while c > 0 {
        ring[i] = (6, c - 1);
        i += 1;
        c -= 1;
    }
    let mut r = 5;
    while r > 0 {
        ring[i] = (r, 0);
        i += 1;
        r -= 1;
    }
    ring
};

#[rustfmt::skip]
const HEART: [(usize, usize); 27] = [
    (1, 1), (1, 2), (1, 4), (1, 5),
    (2, 0), (2, 1), (2, 2), (2, 3), (2, 4), (2, 5), (2, 6),
    (3, 0), (3, 1), (3, 2), (3, 3), (3, 4), (3, 5), (3, 6),
    (4, 1), (4, 2), (4, 3), (4, 4), (4, 5),
    (5, 2), (5, 3), (5, 4), (6, 3),
];
const HEART_SMALL: [(usize, usize); 6] = [(2, 2), (2, 4), (3, 2), (3, 3), (3, 4), (4, 3)];

type Grid = [[bool; 7]; 7];

/// Python `round()`: half-to-even, as an index.
fn round_idx(x: f64) -> i64 {
    x.round_ties_even() as i64
}

/// SplitMix64: tiny, std-only, good enough for eye candy.
#[derive(Clone, Debug)]
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in [0, 1), like `random.random()`.
    fn random(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }

    /// Uniform in 0..=6, like `random.randint(0, 6)`.
    fn rand7(&mut self) -> usize {
        (self.next_u64() % 7) as usize
    }

    fn life_seed(&mut self) -> Grid {
        let mut g = [[false; 7]; 7];
        for cell in g.iter_mut().flatten() {
            *cell = self.random() < 0.32;
        }
        g
    }
}

pub struct GridEngine {
    name: Option<String>,
    t: u64,
    ball: [f64; 2],
    vel: [f64; 2],
    /// Rain drops as (column, row).
    drops: Vec<(usize, usize)>,
    spark: [[f64; 7]; 7],
    life: Grid,
    rng: Rng,
}

impl GridEngine {
    pub fn new(seed: u64) -> Self {
        let mut eng = GridEngine {
            name: None,
            t: 0,
            ball: [0.0; 2],
            vel: [0.0; 2],
            drops: Vec::new(),
            spark: [[0.0; 7]; 7],
            life: [[false; 7]; 7],
            rng: Rng(seed),
        };
        eng.reset();
        eng
    }

    fn reset(&mut self) {
        self.ball = [3.0, 3.0];
        self.vel = [0.55, 0.37];
        self.drops.clear();
        self.spark = [[0.0; 7]; 7];
        self.life = self.rng.life_seed();
    }

    /// Next frame of animation `name`; an unknown name plays "spin". Switching animations
    /// restarts the clock `t` and resets all state; `t` counts from 1 on the first frame.
    pub fn frame(&mut self, name: &str, extra: &Extra) -> [u8; 7] {
        if self.name.as_deref() != Some(name) {
            self.name = Some(name.to_owned());
            self.t = 0;
            self.reset();
        }
        self.t += 1;
        let mut g: Grid = [[false; 7]; 7];
        match name {
            "bounce" => self.bounce(&mut g),
            "rain" => self.rain(&mut g),
            "wave" => self.wave(&mut g),
            "pulse" => self.pulse(&mut g),
            "sparkle" => self.sparkle(&mut g),
            "scan" => self.scan(&mut g),
            "life" => self.life(&mut g),
            "audio" => audio(&mut g, extra),
            "cpu" => self.cpu(&mut g, extra),
            "mouth" => mouth(&mut g, extra),
            "heart" => self.heart(&mut g),
            "custom" => self.custom(&mut g, extra),
            "weather" => self.weather(&mut g, extra.scene),
            _ => self.spin(&mut g),
        }
        g.map(|row| {
            row.iter()
                .enumerate()
                .filter(|&(_, &on)| on)
                .fold(0u8, |b, (c, _)| b | 1 << c)
        })
    }

    fn spin(&self, g: &mut Grid) {
        let n = RING.len() as i64;
        for k in 0..5 {
            let (r, c) = RING[(self.t as i64 - k).rem_euclid(n) as usize];
            g[r][c] = true;
        }
    }

    fn bounce(&mut self, g: &mut Grid) {
        for i in 0..2 {
            self.ball[i] += self.vel[i];
            if self.ball[i] < 0.0 {
                self.ball[i] = 0.0;
                self.vel[i] = self.vel[i].abs();
            } else if self.ball[i] > 6.0 {
                self.ball[i] = 6.0;
                self.vel[i] = -self.vel[i].abs();
            }
        }
        g[round_idx(self.ball[0]) as usize][round_idx(self.ball[1]) as usize] = true;
    }

    fn rain(&mut self, g: &mut Grid) {
        // Every drop falls one row; drops leaving the bottom vanish.
        self.drops = self
            .drops
            .iter()
            .filter(|&&(_, r)| r + 1 < 7)
            .map(|&(c, r)| (c, r + 1))
            .collect();
        if self.rng.random() < 0.55 {
            let c = self.rng.rand7();
            self.drops.push((c, 0));
        }
        for &(c, r) in &self.drops {
            g[r][c] = true;
        }
    }

    fn wave(&self, g: &mut Grid) {
        let rows =
            (0..7).map(|c| round_idx(3.0 + 3.0 * (self.t as f64 * 0.35 + c as f64 * 0.7).sin()));
        for (c, r) in rows.enumerate() {
            g[r.clamp(0, 6) as usize][c] = true;
        }
    }

    fn pulse(&self, g: &mut Grid) {
        let rad = ((self.t / 2) % 4) as i32;
        for (r, row) in g.iter_mut().enumerate() {
            for (c, cell) in row.iter_mut().enumerate() {
                let d = (r as i32 - CENTER).abs().max((c as i32 - CENTER).abs());
                if d == rad {
                    *cell = true;
                }
            }
        }
    }

    fn sparkle(&mut self, g: &mut Grid) {
        for v in self.spark.iter_mut().flatten() {
            *v *= 0.78;
        }
        for _ in 0..2 {
            let r = self.rng.rand7();
            let c = self.rng.rand7();
            self.spark[r][c] = 1.0;
        }
        for (cell, &v) in g.iter_mut().flatten().zip(self.spark.iter().flatten()) {
            *cell = v > 0.3;
        }
    }

    fn scan(&self, g: &mut Grid) {
        let c = (self.t % 7) as usize;
        for row in g.iter_mut() {
            row[c] = true;
        }
    }

    fn life(&mut self, g: &mut Grid) {
        if self.t.is_multiple_of(3) {
            let cur = self.life;
            let mut nxt = [[false; 7]; 7];
            let mut alive = 0;
            for r in 0..7 {
                for c in 0..7 {
                    let mut n = 0;
                    for dr in [6, 0, 1] {
                        for dc in [6, 0, 1] {
                            // (r + 6) % 7 is the torus neighbour r - 1.
                            if (dr, dc) != (0, 0) && cur[(r + dr) % 7][(c + dc) % 7] {
                                n += 1;
                            }
                        }
                    }
                    nxt[r][c] = n == 3 || (cur[r][c] && n == 2);
                    alive += usize::from(nxt[r][c]);
                }
            }
            // Reseed when the colony dies out, and every 90 frames so it never stalls.
            if alive == 0 || self.t.is_multiple_of(90) {
                nxt = self.rng.life_seed();
            }
            self.life = nxt;
        }
        *g = self.life;
    }

    fn cpu(&mut self, g: &mut Grid, e: &Extra) {
        let pct = e.cpu / 100.0;
        let h = round_idx(pct * 7.0);
        fill_bottom_up(g, h);
        // Flicker one cell just above the surface, more often the busier the CPU.
        if h < 7 && self.rng.random() < pct {
            let c = self.rng.rand7();
            g[(6 - h) as usize][c] = true;
        }
    }

    fn heart(&self, g: &mut Grid) {
        let phase = self.t % 16; // lub-dub beat
        let big = phase < 4 || (6..9).contains(&phase);
        let cells: &[(usize, usize)] = if big { &HEART } else { &HEART_SMALL };
        for &(r, c) in cells {
            g[r][c] = true;
        }
    }

    fn custom(&self, g: &mut Grid, e: &Extra) {
        if e.custom.is_empty() {
            return;
        }
        let rows = &e.custom[(self.t % e.custom.len() as u64) as usize];
        for (r, row) in g.iter_mut().enumerate() {
            for (c, cell) in row.iter_mut().enumerate() {
                *cell = rows[r] & (1 << c) != 0;
            }
        }
    }
}

impl GridEngine {
    /// A weather scene (firmware `Engine::weather`): deterministic, from `t` and the scene only.
    fn weather(&self, g: &mut Grid, scene: usize) {
        let t = self.t;
        let s = (t / 2) as i64; // slow movers step every other frame
        match scene {
            SC_PARTLY => {
                // sun upper left, a cloud drifting past below
                const RAYS: [(i64, i64); 4] = [(0, 2), (2, 0), (0, 0), (0, 4)];
                sun_disc(g, 2, 2);
                let (r, c) = RAYS[(s % 4) as usize];
                dot(g, r, c);
                stamp(g, &WX_CLOUD, 4, s % 14 - 3, 14);
            }
            SC_CLOUDY => {
                // two layers of clouds scrolling left
                let x = -(s % 14);
                for (r0, c0) in [(0, x + 1), (0, x + 8), (3, x + 5), (3, x + 12)] {
                    stamp(g, &WX_CLOUD, r0, c0, 14);
                }
            }
            SC_FOG => {
                // dashed bands sliding against each other
                for i in 0..3i64 {
                    for c in 0..7i64 {
                        let p = if i % 2 == 1 { c + s } else { c - s };
                        if p.rem_euclid(4) < 3 {
                            dot(g, 1 + 2 * i, c);
                        }
                    }
                }
            }
            SC_RAIN => {
                stamp(g, &WX_CLOUD, 0, 0, 0);
                rain_drops(g, t, 5);
            }
            SC_SNOW => {
                // flakes falling slowly, drifting side to side
                for (i, &(o, c)) in WX_FLAKES.iter().enumerate() {
                    let r = (s + o) % 7;
                    let drift = if (s + i as i64) % 4 < 2 { 0 } else { 1 };
                    dot(g, r, (c + drift) % 7);
                }
            }
            SC_STORM => {
                // bolt, rain, and a flash of the whole grid
                let p = t % 12;
                if p == 0 || p == 2 {
                    *g = [[true; 7]; 7];
                    return;
                }
                stamp(g, &WX_CLOUD, 0, 0, 0);
                if p < 6 {
                    for &(r, c) in &WX_BOLT {
                        dot(g, r, c);
                    }
                } else {
                    rain_drops(g, t, 3);
                }
            }
            SC_NIGHT => {
                stamp(g, &WX_MOON, 0, 0, 0);
                stars(g, t, 5);
            }
            SC_NIGHT_PARTLY => {
                stamp(g, &WX_MOON, 0, 0, 0);
                stars(g, t, 2);
                stamp(g, &WX_CLOUD, 4, s % 14 - 3, 14);
            }
            _ => {
                // SC_CLEAR: the sun, rays turning
                sun_disc(g, 3, 3);
                for k in (0..8).step_by(2) {
                    let (r, c) = WX_RAYS[((k + t) % 8) as usize];
                    dot(g, r, c);
                }
            }
        }
    }
}

fn dot(g: &mut Grid, r: i64, c: i64) {
    if (0..7).contains(&r) && (0..7).contains(&c) {
        g[r as usize][c as usize] = true;
    }
}

/// Draw art rows at (r0, c0); with wrap > 0 the art scrolls around a strip `wrap` wide.
fn stamp(g: &mut Grid, art: &[u8], r0: i64, c0: i64, wrap: i64) {
    for (r, &bits) in art.iter().enumerate() {
        for c in 0..7i64 {
            if bits & (1 << c) == 0 {
                continue;
            }
            let cc = if wrap > 0 { (c0 + c).rem_euclid(wrap) } else { c0 + c };
            dot(g, r0 + r as i64, cc);
        }
    }
}

fn sun_disc(g: &mut Grid, r: i64, c: i64) {
    for (dr, dc) in [(0, 0), (-1, 0), (1, 0), (0, -1), (0, 1)] {
        dot(g, r + dr, c + dc);
    }
}

fn rain_drops(g: &mut Grid, t: u64, n: usize) {
    for &(o, c) in &WX_DROPS[..n] {
        dot(g, 3 + ((t + o) % 4) as i64, c);
    }
}

fn stars(g: &mut Grid, t: u64, n: usize) {
    for (i, &(r, c)) in WX_STARS[..n].iter().enumerate() {
        if (t + i as u64 * 3) % 7 < 4 {
            dot(g, r, c);
        }
    }
}

/// Light rows from the bottom so that `h` rows (h may be <= 0 or >= 7) are full.
fn fill_bottom_up(g: &mut Grid, h: i64) {
    for (r, row) in g.iter_mut().enumerate() {
        if (6 - r as i64) < h {
            *row = [true; 7];
        }
    }
}

fn audio(g: &mut Grid, e: &Extra) {
    for (c, &band) in e.bands.iter().enumerate() {
        let h = round_idx(band * 7.0);
        for (r, row) in g.iter_mut().enumerate() {
            if (6 - r as i64) < h {
                row[c] = true;
            }
        }
    }
}

fn mouth(g: &mut Grid, e: &Extra) {
    // Bands are 0..1. Out-of-range input (NaN, < 0) would crash or misbehave in Python;
    // here f64::max skips NaN and the rows are clamped so it cannot panic.
    let loudest = e.bands.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let op = round_idx(loudest.min(1.0) * 3.0);
    if op == 0 {
        g[3] = [true; 7];
        return;
    }
    // The corners of the mouth open one row less.
    for (c, edge) in [1, 0, 0, 0, 0, 0, 1].into_iter().enumerate() {
        let top = (3 - op + edge).clamp(0, 6) as usize;
        let bot = (3 + op - edge).clamp(0, 6) as usize;
        g[top][c] = true;
        g[bot][c] = true;
    }
}
