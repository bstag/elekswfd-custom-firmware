//! Top-row icon bitmask, a port of `ICON` / `compute_icons` in wfd-control/app.py.

/// Icon bit order; must match the firmware ICON_* arrays (bit i = `ICON_NAMES[i]`).
pub const ICON_NAMES: [&str; 13] = [
    "speaker",
    "mic",
    "headphones",
    "vinyl",
    "mixer",
    "monitor",
    "tower",
    "server",
    "clock",
    "hourglass",
    "battery",
    "calendar",
    "wifi",
];

/// Bit for a named icon.
fn bit(name: &str) -> u16 {
    let i = ICON_NAMES
        .iter()
        .position(|n| *n == name)
        .expect("known icon name");
    1 << i
}

/// Icons lit for an icon `mode` ("all", "audio", "system"; anything else lights none).
///
/// `out_level` / `mic_level` are the 0..12 meters, `cpu` / `gpu` / `ram` percentages,
/// `battery` a charge percentage when there is a battery, and `now_secs` the Unix time
/// (the clock icon blinks on odd seconds).
#[allow(clippy::too_many_arguments)]
pub fn compute_icons(
    mode: &str,
    out_level: u8,
    mic_level: u8,
    cpu: f64,
    gpu: f64,
    ram: f64,
    online: bool,
    battery: Option<u8>,
    now_secs: u64,
) -> u16 {
    let checks: Vec<(bool, &str)> = match mode {
        "all" => return 0x1FFF,
        "audio" => vec![
            (out_level >= 1, "speaker"),
            (out_level >= 4, "headphones"),
            (out_level >= 7, "vinyl"),
            (out_level >= 10, "mixer"),
            (mic_level >= 2, "mic"),
        ],
        "system" => vec![
            (cpu > 50.0, "tower"),
            (gpu > 30.0, "monitor"),
            (ram > 75.0, "server"),
            (online, "wifi"),
            (now_secs % 2 == 1, "clock"), // blink each second
            (battery.is_some_and(|b| b < 30), "battery"),
        ],
        _ => return 0,
    };
    checks
        .into_iter()
        .filter(|&(lit, _)| lit)
        .fold(0, |m, (_, n)| m | bit(n))
}
