//! metrics: CPU / GPU / RAM / battery, refreshed every second (`metrics_reader` in app.py).

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use sysinfo::System;

/// Percentages, truncated like Python's int(). `battery` is None without a battery.
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct Metrics {
    pub cpu: u8,
    pub gpu: u8,
    pub ram: u8,
    pub battery: Option<u8>,
}

/// Start the metrics thread; it updates `shared` once a second until `stop` is set.
pub fn start(shared: Arc<Mutex<Metrics>>, stop: Arc<AtomicBool>) {
    thread::Builder::new()
        .name("metrics".into())
        .spawn(move || run(&shared, &stop))
        .expect("spawn metrics thread");
}

fn run(shared: &Mutex<Metrics>, stop: &AtomicBool) {
    let nvsmi = which("nvidia-smi");
    let mut sys = System::new();
    sys.refresh_cpu_usage(); // prime, like psutil.cpu_percent(None)
    while !stop.load(Ordering::Relaxed) {
        let gpu = nvsmi
            .as_ref()
            .map_or(0, |p| gpu_utilization(p).unwrap_or(0));
        let battery = battery_percent();
        sys.refresh_cpu_usage();
        sys.refresh_memory();
        let ram = match sys.total_memory() {
            0 => 0.0,
            t => (t - sys.available_memory()) as f64 * 100.0 / t as f64,
        };
        let m = Metrics {
            cpu: pct(f64::from(sys.global_cpu_usage())),
            gpu,
            ram: pct(ram),
            battery,
        };
        if let Ok(mut s) = shared.lock() {
            *s = m;
        }
        thread::sleep(Duration::from_secs(1));
    }
}

fn pct(v: f64) -> u8 {
    v.clamp(0.0, 100.0) as u8
}

/// `shutil.which`: first PATH entry holding the program (with .exe on Windows).
fn which(name: &str) -> Option<PathBuf> {
    let file = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    };
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|d| d.join(&file))
        .find(|p| p.is_file())
}

/// Highest GPU utilization nvidia-smi reports (several GPUs: one line each); 3 s timeout.
fn gpu_utilization(nvsmi: &PathBuf) -> Option<u8> {
    let mut cmd = Command::new(nvsmi);
    cmd.args([
        "--query-gpu=utilization.gpu",
        "--format=csv,noheader,nounits",
    ])
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd.spawn().ok()?;
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let out = child.wait_with_output().ok()?;
    parse_nvsmi(&String::from_utf8_lossy(&out.stdout))
}

/// Max of the numbers in nvidia-smi's output; None if any token isn't a number (Python: 0).
fn parse_nvsmi(out: &str) -> Option<u8> {
    let vals: Vec<f64> = out
        .split_whitespace()
        .map(str::parse)
        .collect::<Result<_, _>>()
        .ok()?;
    Some(vals.into_iter().reduce(f64::max).map_or(0, pct))
}

/// Battery charge in percent, None without a system battery (psutil.sensors_battery()).
#[cfg(windows)]
fn battery_percent() -> Option<u8> {
    use windows_sys::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
    // SAFETY: GetSystemPowerStatus fills the zeroed struct we pass it.
    let s = unsafe {
        let mut s: SYSTEM_POWER_STATUS = std::mem::zeroed();
        if GetSystemPowerStatus(&mut s) == 0 {
            return None;
        }
        s
    };
    // flag 128 = no system battery; percent 255 = unknown
    (s.BatteryFlag & 128 == 0 && s.BatteryLifePercent <= 100).then_some(s.BatteryLifePercent)
}

#[cfg(not(windows))]
fn battery_percent() -> Option<u8> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nvsmi_output() {
        assert_eq!(parse_nvsmi("37\n"), Some(37));
        assert_eq!(parse_nvsmi("12\r\n88\r\n"), Some(88));
        assert_eq!(parse_nvsmi("42.9"), Some(42));
        assert_eq!(parse_nvsmi(""), Some(0));
        assert_eq!(parse_nvsmi("[N/A]"), None);
    }

    #[test]
    fn one_reading() {
        let shared = Arc::new(Mutex::new(Metrics::default()));
        let stop = Arc::new(AtomicBool::new(false));
        start(shared.clone(), stop.clone());
        // the first reading waits for nvidia-smi (up to its 3 s timeout)
        let deadline = Instant::now() + Duration::from_secs(5);
        while shared.lock().unwrap().ram == 0 && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(100));
        }
        stop.store(true, Ordering::Relaxed);
        let m = shared.lock().unwrap().clone();
        println!("{m:?}");
        assert!(m.ram > 0 && m.cpu <= 100 && m.gpu <= 100);
    }
}
