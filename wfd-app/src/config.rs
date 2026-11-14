//! App settings (wfd_config.json): the typed [`Settings`], validation, load and save, and which
//! settings the clock owns.
//!
//! The app reads its settings as fields of [`Settings`]. JSON maps appear only at the edges:
//! - the settings file: [`load`] (per-key validation; a bad value falls back to its default and
//!   is logged) and [`save`] (one setting per line, in [`Settings::NAMES`] order, atomic);
//! - `POST /api/set`: [`Settings::apply`] (a partial update, all-or-nothing, per-key errors);
//! - `GET /api/status`: [`Settings::to_json`] (the `config` object).
//!
//! The clock's own settings ([`CLOCK_KEYS`]) are validated by the firmware and stay JSON.

use serde::Serialize;
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

/// Firmware `WFD_PROTOCOL` this app speaks.
pub const PROTOCOL: u32 = 6;

/// Settings owned by the clock (firmware configJson / applySettings). The clock validates them;
/// the app never stores them in its own file.
pub const CLOCK_KEYS: [&str; 22] = [
    "brightness",
    "clock_24h",
    "style",
    "units",
    "tz",
    "location",
    "offline_text",
    "msg_standalone",
    "standalone_msg_mode",
    "standalone_rotate",
    "standalone_dwell",
    "standalone_scroll_ms",
    "mic_standalone",
    "mic_sampling",
    "grid_standalone",
    "standalone_anim",
    "standalone_fps",
    "standalone_frame",
    "weather_scene",
    "alarms",
    "timer",
    "alert",
];

/// One 7x7 grid frame: a row byte per row, top first, 7 bits each.
pub type GridFrame = [u8; 7];

/// At most this many drawn frames are kept (the clock stores no more).
pub const MAX_FRAMES: usize = 80;

/// A setting with a fixed list of choices, stored as its name in JSON.
trait Choice: Sized + Copy + 'static {
    const ALL: &'static [(&'static str, Self)];
}

/// An enum of choices with their JSON names (`as_str`, `Serialize`, [`Choice`]).
macro_rules! choices {
    ($(#[$meta:meta])* $name:ident { $($(#[$vmeta:meta])* $variant:ident = $text:literal,)* }) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
        pub enum $name { $($(#[$vmeta])* $variant,)* }

        impl $name {
            /// The name the settings file and the API use.
            pub fn as_str(self) -> &'static str {
                match self { $($name::$variant => $text,)* }
            }
        }

        impl Choice for $name {
            const ALL: &'static [(&'static str, Self)] = &[$(($text, $name::$variant),)*];
        }

        impl Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(self.as_str())
            }
        }
    };
}

choices! {
    /// What the message line shows while the app runs.
    MessageMode {
        /// the clock draws the temperature itself
        #[default]
        Weather = "weather",
        /// the foreground window's title
        Window = "window",
        /// `custom_text` with its placeholders
        Text = "text",
        Date = "date",
        /// HI / LO / tomorrow, once two days are known
        Forecast = "forecast",
        /// the `rotate_*` sources in turn, `rotate_dwell` seconds each
        Rotate = "rotate",
    }
}

choices! {
    /// What the top-row icons show.
    IconMode {
        #[default]
        Off = "off",
        Audio = "audio",
        System = "system",
        All = "all",
    }
}

choices! {
    /// How the app talks to the clock.
    LinkMode {
        /// USB if plugged in, else WiFi
        #[default]
        Auto = "auto",
        Usb = "usb",
        Wifi = "wifi",
    }
}

/// The `Settings` struct, its defaults, its names and the per-key setter, from one list.
macro_rules! settings {
    ($($(#[$meta:meta])* $name:ident: $ty:ty = $default:expr, parse $parse:expr;)*) => {
        /// The app's own settings, one typed field each (saved in wfd_config.json).
        #[derive(Clone, Debug, PartialEq, Serialize)]
        pub struct Settings {
            $($(#[$meta])* pub $name: $ty,)*
        }

        impl Default for Settings {
            fn default() -> Self {
                Settings { $($name: $default,)* }
            }
        }

        impl Settings {
            /// Every setting's name, in the order the file and `/api/status` list them.
            pub const NAMES: &'static [&'static str] = &[$(stringify!($name),)*];

            /// Set one setting from a JSON value, validated: numbers are clamped to their range,
            /// choices checked. On an error the setting keeps its value.
            pub fn set(&mut self, key: &str, v: &Value) -> Result<(), String> {
                match key {
                    $(stringify!($name) => self.$name = ($parse)(v)?,)*
                    _ => return Err("unknown setting".into()),
                }
                Ok(())
            }
        }
    };
}

settings! {
    /// Set the clock's time from the PC's (unless the clock has internet time).
    time_sync: bool = true, parse boolean;
    /// Show the PC audio spectrum.
    spectrum: bool = true, parse boolean;
    /// Gain of the PC output level, 0..10.
    gain_out: f64 = 1.0, parse number(0.0, 10.0);
    /// Gain of the mic level, 0..10.
    gain_mic: f64 = 1.0, parse number(0.0, 10.0);
    /// Show the CPU / GPU / RAM bars.
    bars: bool = true, parse boolean;
    /// Send the PC's weather.
    weather: bool = true, parse boolean;
    message_mode: MessageMode = MessageMode::Weather, parse choice;
    /// The message line's text in "text" mode (placeholders {date} {time} {time12} {day} {temp}).
    custom_text: String = "HELLO".into(), parse text;
    /// Message line scroll step, 50..5000 ms.
    scroll_ms: u32 = 350, parse whole(50, 5000);
    rotate_weather: bool = true, parse boolean;
    rotate_window: bool = true, parse boolean;
    rotate_text: bool = false, parse boolean;
    rotate_date: bool = false, parse boolean;
    /// Seconds per item in rotate mode, 2..600.
    rotate_dwell: u32 = 8, parse whole(2, 600);
    /// Drive the 7x7 grid from the app.
    grid_enable: bool = false, parse boolean;
    /// The grid animation (a built-in name, "custom" for the drawn frames, or a live one).
    grid_anim: String = "spin".into(), parse text;
    /// The frame around the grid.
    grid_frame: bool = false, parse boolean;
    /// Grid animation speed, 2..30 frames a second.
    grid_fps: u32 = 12, parse whole(2, 30);
    /// The drawn frames (at most [`MAX_FRAMES`]).
    grid_custom: Vec<GridFrame> = Vec::new(), parse grid_frames;
    icon_mode: IconMode = IconMode::Off, parse choice;
    link: LinkMode = LinkMode::Auto, parse choice;
    /// WiFi: the clock's name or IP; blank = wfd-clock.local.
    clock_host: String = String::new(), parse text;
    /// The clock's web password (WEBPASS), if it has one.
    clock_pass: String = String::new(), parse text;
    /// The ELEKSMAKER logo.
    logo: bool = false, parse boolean;
}

impl Settings {
    /// Apply a partial update (`POST /api/set`'s app settings): all of `patch`, or nothing when
    /// any key is unknown or has a bad value. Err: a reason for every bad key.
    pub fn apply(&mut self, patch: &Map<String, Value>) -> Result<(), Map<String, Value>> {
        let mut next = self.clone();
        let errors: Map<String, Value> = patch
            .iter()
            .filter_map(|(k, v)| next.set(k, v).err().map(|e| (k.clone(), Value::String(e))))
            .collect();
        if !errors.is_empty() {
            return Err(errors);
        }
        *self = next;
        Ok(())
    }

    /// Every setting as JSON, in [`Settings::NAMES`] order (`/api/status` `config`, the file).
    pub fn to_json(&self) -> Map<String, Value> {
        match serde_json::to_value(self) {
            Ok(Value::Object(m)) => m,
            _ => Map::new(),
        }
    }
}

// ------------------------------------------------------------------ value checks

fn boolean(v: &Value) -> Result<bool, String> {
    v.as_bool().ok_or_else(|| "expected true/false".into())
}

fn text(v: &Value) -> Result<String, String> {
    v.as_str().map(str::to_owned).ok_or_else(|| "expected a string".into())
}

/// A JSON number (not a string or a bool) clamped to `lo..=hi`.
fn number(lo: f64, hi: f64) -> impl Fn(&Value) -> Result<f64, String> {
    move |v| {
        let x = v.as_f64().ok_or("expected a number")?;
        Ok(x.clamp(lo, hi))
    }
}

/// A JSON number, cut to a whole number (2.9 -> 2) and clamped to `lo..=hi`.
fn whole(lo: u32, hi: u32) -> impl Fn(&Value) -> Result<u32, String> {
    move |v| {
        let x = v.as_f64().ok_or("expected a number")?;
        Ok(x.trunc().clamp(f64::from(lo), f64::from(hi)) as u32)
    }
}

fn choice<T: Choice>(v: &Value) -> Result<T, String> {
    let names = || T::ALL.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(", ");
    let s = v.as_str().ok_or_else(|| format!("expected one of: {}", names()))?;
    T::ALL
        .iter()
        .find(|(n, _)| *n == s)
        .map(|(_, c)| *c)
        .ok_or_else(|| format!("expected one of: {}", names()))
}

/// One grid row: a whole JSON number, of which the low 7 bits are kept (300 -> 44, -1 -> 127).
pub fn grid_row(v: &Value) -> Result<u8, String> {
    let n = match v {
        Value::Number(n) => n.as_i64().or_else(|| n.as_u64().map(|u| (u & 0x7F) as i64)),
        _ => None,
    };
    n.map(|i| (i & 0x7F) as u8)
        .ok_or_else(|| "grid rows must be whole numbers".into())
}

/// Drawn frames: a list of frames, each a list of row numbers ([`grid_row`]). Each frame is
/// padded with empty rows or cut to 7; frames past [`MAX_FRAMES`] are dropped.
pub fn grid_frames(v: &Value) -> Result<Vec<GridFrame>, String> {
    let frames = match v {
        Value::Array(f) if f.iter().all(Value::is_array) => f,
        _ => return Err("expected a list of frames, each a list of rows".into()),
    };
    frames
        .iter()
        .take(MAX_FRAMES)
        .map(|f| {
            let mut rows = [0u8; 7];
            for (slot, x) in rows.iter_mut().zip(f.as_array().into_iter().flatten()) {
                *slot = grid_row(x)?;
            }
            Ok(rows)
        })
        .collect()
}

// ------------------------------------------------------------------ the file

/// The app's settings from the file at `path`. A missing file gives the defaults. Keys that
/// aren't app settings (clock settings from older files, `_comment`, ...) are ignored. A value
/// that fails validation gives that setting's default, with a line in the log. A file that
/// can't be read or isn't a JSON object gives the defaults too, but is first copied to
/// `<name>.bad` (and a line logged), since the next save overwrites it.
pub fn load(path: &Path) -> Settings {
    let mut settings = Settings::default();
    let why = match std::fs::read(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return settings,
        Err(e) => format!("can't be read ({e})"),
        Ok(b) => match serde_json::from_slice::<Value>(&b) {
            Ok(Value::Object(file)) => {
                for (k, v) in &file {
                    if !Settings::NAMES.contains(&k.as_str()) {
                        continue;
                    }
                    if let Err(e) = settings.set(k, v) {
                        crate::applog::log(&format!(
                            "settings file {}: {k} = {} isn't valid ({e}); using the default",
                            path.display(),
                            short(v)
                        ));
                    }
                }
                return settings;
            }
            Ok(_) => "isn't a JSON object".to_string(),
            Err(e) => format!("isn't valid JSON ({e})"),
        },
    };
    keep_bad_copy(path, &why);
    settings
}

/// `v` as compact JSON, cut short for a log line.
fn short(v: &Value) -> String {
    let s = v.to_string();
    match s.char_indices().nth(60) {
        Some((i, _)) => format!("{}...", &s[..i]),
        None => s,
    }
}

/// `wfd_config.json` -> `wfd_config.json.bad`, next to it.
pub fn bad_copy_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".bad");
    path.with_file_name(name)
}

fn keep_bad_copy(path: &Path, why: &str) {
    let bad = bad_copy_path(path);
    let kept = match std::fs::copy(path, &bad) {
        Ok(_) => format!("a copy is in {}", bad.display()),
        Err(e) => format!("couldn't keep a copy at {} ({e})", bad.display()),
    };
    crate::applog::log(&format!(
        "settings file {} {why}; using the defaults, {kept}",
        path.display()
    ));
}

/// The settings file's text: a JSON object with one setting per line, in [`Settings::NAMES`]
/// order, each value as compact JSON.
pub fn file_text(settings: &Settings) -> String {
    let lines: Vec<String> = settings
        .to_json()
        .iter()
        .map(|(k, v)| format!("  {}: {v}", Value::String(k.clone())))
        .collect();
    format!("{{\n{}\n}}\n", lines.join(",\n"))
}

/// Write `settings` to `path` ([`file_text`]), atomically.
pub fn save(path: &Path, settings: &Settings) -> std::io::Result<()> {
    write_atomic(path, file_text(settings).as_bytes())
}

/// Write `data` to a temp file next to `path`, flush it to disk, then rename it over `path`, so
/// a crash mid-write leaves the old file whole. The temp file is removed if anything fails.
fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(
        ".{}-{}.tmp",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let tmp = path.with_file_name(name);
    let res = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)
    })();
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    res
}
