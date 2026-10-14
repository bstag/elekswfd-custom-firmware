//! audio: capture of the PC's audio output (WASAPI loopback) and the default mic
//! (`audio_reader` in app.py). Each source runs on its own thread: per 1024-frame block it
//! publishes the RMS energy and, for the output, the 7-band spectrum (see `dsp`).
//!
//! Loopback works in cpal 0.15 by building an *input* stream on the default *output* device
//! with its output config. Like app.py, a failed device is retried every 1.5 s with energy 0
//! meanwhile, and the stream is reopened when the Windows default device changes.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, Host, SampleFormat, Stream, StreamConfig};

use crate::dsp::{BandTracker, rms_mono, to_mono};

/// What the capture threads publish. Index 0 = output, 1 = mic.
#[derive(Clone, Debug, Default)]
pub struct AudioShared {
    pub energy: [f64; 2],
    pub bands: [f64; 7],
    pub out_name: String,
    pub mic_name: String,
}

/// Frames per block, as app.py reads them (frames_per_buffer=1024).
const BLOCK: usize = 1024;
const RETRY: Duration = Duration::from_millis(1500);
/// How often a running stream checks for errors / stop.
const POLL: Duration = Duration::from_millis(100);
/// How often a running stream checks whether the default device changed.
const DEVICE_CHECK: Duration = Duration::from_millis(1500);
/// WASAPI loopback delivers nothing while nothing plays: after this long without data the
/// source counts as silent (energy and bands 0) instead of holding its last values.
const STALE: Duration = Duration::from_millis(300);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Out,
    Mic,
}

impl Kind {
    fn idx(self) -> usize {
        match self {
            Kind::Out => 0,
            Kind::Mic => 1,
        }
    }
}

/// Start capture threads for the output (loopback) and the mic; they run until `stop` is set.
pub fn start(shared: Arc<Mutex<AudioShared>>, stop: Arc<AtomicBool>) {
    for (kind, name) in [(Kind::Out, "audio-out"), (Kind::Mic, "audio-mic")] {
        let (shared, stop) = (Arc::clone(&shared), Arc::clone(&stop));
        thread::Builder::new()
            .name(name.into())
            .spawn(move || capture_loop(kind, &shared, &stop))
            .expect("spawn audio thread");
    }
}

fn lock(shared: &Mutex<AudioShared>) -> std::sync::MutexGuard<'_, AudioShared> {
    shared.lock().unwrap_or_else(|e| e.into_inner())
}

/// Sleep up to `d`, waking early when `stop` is set.
fn sleep_unless_stopped(stop: &AtomicBool, d: Duration) {
    let end = Instant::now() + d;
    while !stop.load(Ordering::Relaxed) {
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        thread::sleep(left.min(POLL));
    }
}

fn capture_loop(kind: Kind, shared: &Arc<Mutex<AudioShared>>, stop: &AtomicBool) {
    let host = cpal::default_host();
    while !stop.load(Ordering::Relaxed) {
        let failed = match open(&host, kind, shared) {
            Ok(session) => session.run(&host, stop) == End::Failed,
            Err(_) => true,
        };
        lock(shared).energy[kind.idx()] = 0.0;
        if failed {
            sleep_unless_stopped(stop, RETRY);
        }
    }
}

fn default_device(host: &Host, kind: Kind) -> Option<Device> {
    match kind {
        Kind::Out => host.default_output_device(),
        Kind::Mic => host.default_input_device(),
    }
}

/// Per-stream block assembly and analysis, fed from cpal's callback.
struct Blocks {
    kind: Kind,
    channels: usize,
    rate: u32,
    buf: Vec<f32>,
    bands: BandTracker,
    last_data: Instant,
    shared: Arc<Mutex<AudioShared>>,
}

impl Blocks {
    fn push(&mut self, samples: impl Iterator<Item = f32>) {
        self.buf.extend(samples);
        self.last_data = Instant::now();
        let len = BLOCK * self.channels;
        while self.buf.len() >= len {
            let block: Vec<f32> = self.buf.drain(..len).collect();
            let energy = rms_mono(&block, self.channels);
            let bands = (self.kind == Kind::Out)
                .then(|| self.bands.bands(&to_mono(&block, self.channels), self.rate));
            let mut s = lock(&self.shared);
            s.energy[self.kind.idx()] = energy;
            if let Some(b) = bands {
                s.bands = b;
            }
        }
    }
}

struct Session {
    kind: Kind,
    name: String,
    _stream: Stream,
    failed: Arc<AtomicBool>,
    blocks: Arc<Mutex<Blocks>>,
    shared: Arc<Mutex<AudioShared>>,
}

#[derive(PartialEq, Eq)]
enum End {
    Stopped,
    DeviceChanged,
    Failed,
}

fn open(host: &Host, kind: Kind, shared: &Arc<Mutex<AudioShared>>) -> Result<Session, String> {
    let dev = default_device(host, kind).ok_or("no default device")?;
    let name = dev.name().map_err(|e| e.to_string())?;
    let supported = match kind {
        Kind::Out => dev.default_output_config(),
        Kind::Mic => dev.default_input_config(),
    }
    .map_err(|e| e.to_string())?;
    let config: StreamConfig = supported.config();
    let blocks = Arc::new(Mutex::new(Blocks {
        kind,
        channels: usize::from(config.channels.max(1)),
        rate: config.sample_rate.0,
        buf: Vec::with_capacity(BLOCK * usize::from(config.channels) * 2),
        bands: BandTracker::new(),
        last_data: Instant::now(),
        shared: Arc::clone(shared),
    }));
    let failed = Arc::new(AtomicBool::new(false));
    let on_err = {
        let failed = Arc::clone(&failed);
        move |_e: cpal::StreamError| failed.store(true, Ordering::Relaxed)
    };
    let stream = match supported.sample_format() {
        SampleFormat::F32 => {
            let b = Arc::clone(&blocks);
            dev.build_input_stream(
                &config,
                move |d: &[f32], _: &_| {
                    b.lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .push(d.iter().copied())
                },
                on_err,
                None,
            )
        }
        SampleFormat::I16 => {
            let b = Arc::clone(&blocks);
            dev.build_input_stream(
                &config,
                // like Python: int16 -> float32 / 32768
                move |d: &[i16], _: &_| {
                    b.lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .push(d.iter().map(|&v| f32::from(v) / 32768.0))
                },
                on_err,
                None,
            )
        }
        other => return Err(format!("unsupported sample format {other}")),
    }
    .map_err(|e| e.to_string())?;
    stream.play().map_err(|e| e.to_string())?;
    {
        let mut s = lock(shared);
        match kind {
            Kind::Out => s.out_name = name.clone(),
            Kind::Mic => s.mic_name = name.clone(),
        }
    }
    Ok(Session {
        kind,
        name,
        _stream: stream,
        failed,
        blocks,
        shared: Arc::clone(shared),
    })
}

impl Session {
    /// Keep the stream running until stop, a stream error, or a new default device.
    fn run(self, host: &Host, stop: &AtomicBool) -> End {
        let mut last_check = Instant::now();
        loop {
            thread::sleep(POLL);
            if stop.load(Ordering::Relaxed) {
                return End::Stopped;
            }
            if self.failed.load(Ordering::Relaxed) {
                return End::Failed;
            }
            let stale = self
                .blocks
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .last_data
                .elapsed()
                > STALE;
            if stale {
                let mut s = lock(&self.shared);
                s.energy[self.kind.idx()] = 0.0;
                if self.kind == Kind::Out {
                    s.bands = [0.0; 7];
                }
            }
            if last_check.elapsed() >= DEVICE_CHECK {
                last_check = Instant::now();
                let now = default_device(host, self.kind).and_then(|d| d.name().ok());
                if now.as_deref() != Some(self.name.as_str()) {
                    return End::DeviceChanged;
                }
            }
        }
    }
}
