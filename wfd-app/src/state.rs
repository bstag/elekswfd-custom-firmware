//! Shared app state: what the background threads know, served as `state` by GET /api/status.
//!
//! The JSON must keep the exact keys of the Python app's `state` dict (wfd-control/app.py): the
//! shared dashboard (wfd-firmware/clock/web/dashboard.html) reads them, and tells the app apart
//! from the clock by the presence of `link`.
use crate::{metrics::Metrics, release::Release, weather::{Geo, Weather}};
use serde::Serialize;
use serde_json::{Map, Value};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, Default, Serialize)]
pub struct LevelsOut { pub out: u8, pub mic: u8 }

#[derive(Clone, Debug, Default, Serialize)]
pub struct AudioNames { pub out: String, pub mic: String }

#[derive(Clone, Debug, Serialize)]
pub struct Update { pub state: String, pub message: String }   // idle | uploading | done | failed

impl Default for Update {
    fn default() -> Self { Update { state: "idle".into(), message: String::new() } }
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct State {
    pub connected: bool,
    pub port: Option<String>,
    pub time: Option<String>,               // "Tue 16:42:48"
    pub metrics: Metrics,                   // cpu/gpu/ram (+ battery, which Python keeps at top level)
    pub weather: Weather,
    pub levels: LevelsOut,
    pub audio: AudioNames,
    pub message: String,
    pub saved: String,
    pub online: bool,
    pub battery: Option<u8>,
    pub geo: Option<Geo>,
    pub clock: Option<Map<String, Value>>,  // the clock's own settings (CFG / its /api/status config)
    pub link: Option<String>,               // "usb" | "wifi" while connected — always serialized (the dashboard checks the key)
    pub clock_ip: Option<String>,
    pub clock_fw: Option<String>,
    pub clock_protocol: Option<u32>,
    pub release: Option<Release>,
    pub update: Update,
    pub grid_rows: Option<[u8; 7]>,
}

pub type Shared = Arc<Mutex<State>>;
