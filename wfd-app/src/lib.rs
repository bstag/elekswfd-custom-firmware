//! WFD Control in Rust: the PC app, and the source of truth for its behaviour. It was ported from
//! the Python app, now retired to legacy/python-app/; module notes say where it still follows
//! that code, and tests/ hold regression tests of this app's own behaviour.
//!
//! Modules, roughly in the order data flows:
//! - `config`    typed app settings (`Settings`, saved in wfd_config.json), their validation,
//!   which settings the clock owns
//! - `audio`     PC output (WASAPI loopback) + mic capture  -> energies + 7 FFT bands
//! - `dsp`       audio levels (0..12 meters) and band tracking
//! - `metrics`   CPU / GPU / RAM / battery
//! - `gpu`       GPU load from Windows' performance counters (every GPU)
//! - `weather`   location lookup + current weather
//! - `window`    foreground window title
//! - `font14`    14-segment glyphs for the 6-char message line, scrolling
//! - `grid_anim` 7x7 grid animations
//! - `icons`     top-row icon bitmask
//! - `mdns`      find wfd-clock.local
//! - `release`   firmware the app ships, version check, OTA upload body
//! - `applog`    wfd-app.log: starts, stops, panics, link events
//! - `link`      USB / WiFi transports to the clock          (wave 2)
//! - `clocklink` the 30 Hz loop that drives the clock          (wave 2)
//! - `ota`       "Update clock" over WiFi                 (wave 2)
//! - `web`       local HTTP server + API, serves the shared pages (wave 2)
//! - `state`/`ctx` shared state (JSON = /api/status state) and the context all threads share

pub mod applog;
pub mod audio;
pub mod clocklink;
pub mod config;
pub mod ctx;
pub mod dsp;
pub mod flashplan;
pub mod font14;
pub mod gpu;
pub mod grid_anim;
pub mod icons;
pub mod link;
pub mod mdns;
pub mod metrics;
pub mod ota;
pub mod release;
pub mod state;
pub mod supervise;
pub mod usbdriver;
pub mod weather;
pub mod web;
pub mod window;
