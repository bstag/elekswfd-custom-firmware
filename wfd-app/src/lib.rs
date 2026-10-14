//! WFD Control in Rust: a port of wfd-control/app.py (the Python app stays the reference until
//! this one reproduces its behaviour; see each module's notes and the parity tests in tests/).
//!
//! Modules, roughly in the order data flows:
//! - `config`    app settings (wfd_config.json), validation, which settings the clock owns
//! - `audio`     PC output (WASAPI loopback) + mic capture  -> energies + 7 FFT bands
//! - `dsp`       audio levels (0..12 meters) and band tracking
//! - `metrics`   CPU / GPU / RAM / battery
//! - `weather`   location lookup + current weather
//! - `window`    foreground window title
//! - `font14`    14-segment glyphs for the 6-char message line, scrolling
//! - `grid_anim` 7x7 grid animations
//! - `icons`     top-row icon bitmask
//! - `mdns`      find wfd-clock.local
//! - `release`   firmware the app ships, version check, OTA upload body
//! - `link`      USB / WiFi transports to the clock          (wave 2)
//! - `clocklink` the 30 Hz loop that drives the clock          (wave 2)
//! - `web`       local HTTP server + API, serves the shared pages (wave 2)

pub mod audio;
pub mod config;
pub mod dsp;
pub mod font14;
pub mod grid_anim;
pub mod icons;
pub mod mdns;
pub mod metrics;
pub mod release;
pub mod weather;
pub mod window;
