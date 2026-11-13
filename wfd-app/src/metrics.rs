//! metrics: CPU / GPU / RAM / battery, refreshed every second (`metrics_reader` in app.py).

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
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
    /// Not part of `state.metrics` (app.py keeps battery at the top of `state`): main copies it there.
    #[serde(skip_serializing)]
    pub battery: Option<u8>,
}

/// Start the metrics thread; it updates `shared` once a second until `stop` is set.
pub fn start(shared: Arc<Mutex<Metrics>>, stop: Arc<AtomicBool>) {
    thread::Builder::new()
        .name("metrics".into())
        .spawn(move || run(&shared, &stop))
        .expect("spawn metrics thread");
}

fn run(shared: &Mutex<Metrics>, stop: &Arc<AtomicBool>) {
    // GPU load: Windows' own counters (every GPU, no helper); where a PC doesn't have them,
    // nvidia-smi if it's there.
    #[cfg(windows)]
    let mut counters = crate::gpu::Counters::open();
    #[cfg(windows)]
    let have_counters = counters.is_some();
    #[cfg(not(windows))]
    let have_counters = false;
    let gpu_reading = if have_counters {
        None
    } else {
        which("nvidia-smi").map(|p| start_gpu_reader(p, Arc::clone(stop)))
    };
    let mut sys = System::new();
    sys.refresh_cpu_usage(); // prime, like psutil.cpu_percent(None)
    while !stop.load(Ordering::Relaxed) {
        // a reading older than a few seconds means nvidia-smi stopped talking: show 0
        let from_nvsmi = gpu_reading
            .as_ref()
            .and_then(|g| *g.lock().unwrap_or_else(|e| e.into_inner()))
            .filter(|(at, _)| at.elapsed() < GPU_STALE)
            .map_or(0, |(_, v)| v);
        #[cfg(windows)]
        let gpu = counters.as_mut().map_or(from_nvsmi, |c| c.read().unwrap_or(0));
        #[cfg(not(windows))]
        let gpu = from_nvsmi;
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

/// The latest GPU reading and when it arrived.
type GpuReading = Arc<Mutex<Option<(Instant, u8)>>>;

const GPU_STALE: Duration = Duration::from_secs(5);
/// After nvidia-smi ends by itself (a driver update, an error): wait this long, start it again.
const GPU_RESTART: Duration = Duration::from_secs(30);

/// The fallback where Windows has no GPU counters: ONE long-running `nvidia-smi -l 1`, which prints a line per GPU every second.
/// (Starting nvidia-smi afresh for every reading cost about 40 ms of CPU each time: three times
/// what the rest of the app uses.) The helper is killed when the app stops, however it stops.
fn start_gpu_reader(nvsmi: PathBuf, stop: Arc<AtomicBool>) -> GpuReading {
    let reading: GpuReading = Arc::default();
    let out = Arc::clone(&reading);
    thread::Builder::new()
        .name("gpu".into())
        .spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                if let Some(mut child) = spawn_nvsmi(&nvsmi) {
                    read_gpu_lines(&mut child, &out, &stop);
                    let _ = child.kill();
                    let _ = child.wait();
                }
                let again = Instant::now() + GPU_RESTART;
                while !stop.load(Ordering::Relaxed) && Instant::now() < again {
                    thread::sleep(Duration::from_millis(200));
                }
            }
        })
        .expect("spawn gpu thread");
    reading
}

fn spawn_nvsmi(nvsmi: &PathBuf) -> Option<Child> {
    let mut cmd = Command::new(nvsmi);
    cmd.args([
        "--query-gpu=index,utilization.gpu",
        "--format=csv,noheader,nounits",
        "-l",
        "1",
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
    let child = cmd.spawn().ok()?;
    kill_with_us(&child);
    Some(child)
}

/// Read the helper's lines until it ends or the app stops; publish the busiest GPU.
fn read_gpu_lines(child: &mut Child, out: &GpuReading, stop: &AtomicBool) {
    let Some(stdout) = child.stdout.take() else { return };
    let mut per_gpu: Vec<u8> = Vec::new();
    for line in BufReader::new(stdout).lines() {
        let Ok(line) = line else { break };
        if stop.load(Ordering::Relaxed) {
            break;
        }
        if let Some((index, load)) = parse_gpu_line(&line) {
            if per_gpu.len() <= index {
                per_gpu.resize(index + 1, 0);
            }
            per_gpu[index] = load;
            let busiest = per_gpu.iter().copied().max().unwrap_or(0);
            *out.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), busiest));
        }
    }
}

/// One line of `--query-gpu=index,utilization.gpu`: "0, 37" -> (0, 37). None for anything else
/// (a GPU that reports "[N/A]", a blank line).
fn parse_gpu_line(line: &str) -> Option<(usize, u8)> {
    let (index, load) = line.split_once(',')?;
    let index: usize = index.trim().parse().ok()?;
    let load: f64 = load.trim().parse().ok()?;
    (index < 16).then(|| (index, pct(load)))
}

/// Tie `child` to this process: Windows ends it when we end, also when we're killed (the
/// installer and wfd-flash stop the app with taskkill), so no helper is ever left running.
#[cfg(windows)]
fn kill_with_us(child: &Child) {
    use std::os::windows::io::AsRawHandle;
    use std::sync::OnceLock;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation, SetInformationJobObject,
    };
    // One job for the life of the process; its handle is never closed, so the job (and with it
    // every process in it) ends exactly when this process does.
    static JOB: OnceLock<usize> = OnceLock::new();
    let job = *JOB.get_or_init(|| {
        // SAFETY: plain Win32 calls; `info` is zeroed and sized as the API expects.
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if !job.is_null() {
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    (&raw const info).cast(),
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
            }
            job as usize
        }
    });
    if job != 0 {
        // SAFETY: both handles are valid for the duration of the call.
        unsafe {
            AssignProcessToJobObject(job as _, child.as_raw_handle() as _);
        }
    }
}

#[cfg(not(windows))]
fn kill_with_us(_child: &Child) {}

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
    fn nvsmi_lines() {
        assert_eq!(parse_gpu_line("0, 37"), Some((0, 37)));
        assert_eq!(parse_gpu_line("1, 88\r"), Some((1, 88)));
        assert_eq!(parse_gpu_line("0, 42.9"), Some((0, 42)));
        assert_eq!(parse_gpu_line("0, 250"), Some((0, 100)));
        assert_eq!(parse_gpu_line("0, [N/A]"), None);
        assert_eq!(parse_gpu_line(""), None);
        assert_eq!(parse_gpu_line("37"), None);
    }

    #[test]
    fn one_reading() {
        let shared = Arc::new(Mutex::new(Metrics::default()));
        let stop = Arc::new(AtomicBool::new(false));
        start(shared.clone(), stop.clone());
        // (the GPU reading arrives on its own, a moment after nvidia-smi starts)
        let deadline = Instant::now() + Duration::from_secs(5);
        while shared.lock().unwrap_or_else(|e| e.into_inner()).ram == 0 && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(100));
        }
        stop.store(true, Ordering::Relaxed);
        let m = shared.lock().unwrap_or_else(|e| e.into_inner()).clone();
        println!("{m:?}");
        assert!(m.ram > 0 && m.cpu <= 100 && m.gpu <= 100);
    }
}
