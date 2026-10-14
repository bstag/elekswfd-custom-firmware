//! The shared context every thread holds (an `Arc<Ctx>`): settings, state, the background
//! readers' data, and the queues the web server uses to ask the clock link to do things.
//!
//! Mirrors the module-level globals of wfd-control/app.py (cfg + cfg_lock, state + state_lock,
//! clock_requests, uploads, grid_preview, sync_now, stop).
use crate::{audio::AudioShared, metrics::Metrics, release::Release, state::State};
use serde_json::{Map, Value};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// A flash upload for the clock (app.py `uploads` jobs).
#[derive(Clone, Debug, PartialEq)]
pub enum UploadJob {
    /// a built-in animation, run by the clock itself (GANIM / /api/set)
    Anim { name: String, fps: u32, frame: bool },
    /// drawn frames (GCLR/GADD/GCOMMIT / POST /api/frames), at most 80
    Frames { frames: Vec<[u8; 7]>, fps: u32, frame: bool },
}

/// Result of changing clock-owned settings: Ok(warning map, if any) or Err(per-key errors).
pub type ClockReply = Result<Option<Value>, Value>;

/// One CFGSET / POST /api/set for the link thread; it answers on `reply`.
pub struct ClockRequest {
    pub patch: Map<String, Value>,
    pub reply: mpsc::Sender<ClockReply>,
}

/// The drawing tool's live preview (app.py grid_preview): shown for 2 s after `at`.
#[derive(Clone, Debug)]
pub struct GridPreview {
    pub rows: [u8; 7],
    pub frame: bool,
    pub at: Option<Instant>,
}

pub struct Ctx {
    pub cfg_path: PathBuf,
    pub cfg: Mutex<Map<String, Value>>,
    pub state: Mutex<State>,
    pub audio: Arc<Mutex<AudioShared>>,
    pub metrics: Arc<Mutex<Metrics>>,
    pub clock_requests: Mutex<Vec<ClockRequest>>,
    pub uploads: Mutex<VecDeque<UploadJob>>,
    pub grid_preview: Mutex<GridPreview>,
    pub sync_now: AtomicBool,
    pub stop: Arc<AtomicBool>,
    /// the firmware this app ships (built into the exe) and its version
    pub release: Option<Release>,
    pub firmware: &'static [u8],
}

impl Ctx {
    pub fn new(cfg_path: PathBuf, release: Option<Release>, firmware: &'static [u8]) -> Self {
        let cfg = crate::config::load(&cfg_path);
        let state = State { release: release.clone(), ..State::default() };
        Ctx {
            cfg_path,
            cfg: Mutex::new(cfg),
            state: Mutex::new(state),
            audio: Arc::default(),
            metrics: Arc::default(),
            clock_requests: Mutex::default(),
            uploads: Mutex::default(),
            grid_preview: Mutex::new(GridPreview { rows: [0; 7], frame: false, at: None }),
            sync_now: AtomicBool::new(false),
            stop: Arc::new(AtomicBool::new(false)),
            release,
            firmware,
        }
    }

    /// A snapshot of the app settings (like `dict(cfg)` under cfg_lock).
    pub fn cfg(&self) -> Map<String, Value> {
        self.cfg.lock().unwrap().clone()
    }

    pub fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap()
    }

    pub fn stopping(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    /// Change clock-owned settings through the link thread and wait for its answer
    /// (app.py set_clock). Errors are per-key maps, like the clock's own.
    pub fn set_clock(&self, patch: Map<String, Value>, timeout: Duration) -> ClockReply {
        let per_key = |msg: &str| {
            Value::Object(patch.keys().map(|k| (k.clone(), Value::String(msg.into()))).collect())
        };
        if !self.state().connected {
            return Err(per_key("clock not connected"));
        }
        let (tx, rx) = mpsc::channel();
        let keys = per_key("clock didn't answer");
        self.clock_requests.lock().unwrap().push(ClockRequest { patch, reply: tx });
        rx.recv_timeout(timeout).unwrap_or(Err(keys))
    }

    /// The setting the clock reports for `key` (state.clock), else `default`.
    pub fn clock_setting(&self, key: &str, default: Value) -> Value {
        self.state().clock.as_ref().and_then(|c| c.get(key).cloned()).unwrap_or(default)
    }
}
