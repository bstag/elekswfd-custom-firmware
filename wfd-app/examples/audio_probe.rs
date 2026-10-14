//! Capture the PC output (loopback) and the mic for 3 s and print what the app would see.
//! cargo run --example audio_probe

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use wfd_app::audio::{self, AudioShared};

fn main() {
    let shared = Arc::new(Mutex::new(AudioShared::default()));
    let stop = Arc::new(AtomicBool::new(false));
    audio::start(Arc::clone(&shared), Arc::clone(&stop));
    for _ in 0..12 {
        std::thread::sleep(Duration::from_millis(250));
        let s = shared.lock().unwrap().clone();
        println!(
            "out {:.3e}  mic {:.3e}  bands {:?}",
            s.energy[0],
            s.energy[1],
            s.bands.map(|b| (b * 100.0).round() / 100.0)
        );
    }
    let s = shared.lock().unwrap().clone();
    println!(
        "output device: {:?}\nmic device:    {:?}",
        s.out_name, s.mic_name
    );
    stop.store(true, Ordering::Relaxed);
    std::thread::sleep(Duration::from_millis(200));
}
