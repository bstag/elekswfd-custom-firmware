//! wfd-app: the Rust WFD Control (port of `main()` in wfd-control/app.py).
//!
//! Starts the background readers (audio, metrics, weather), the clock link and the local web
//! UI, then opens the dashboard in the browser.
//!
//! ```text
//! wfd-app [--no-browser] [--config <path>] [--addr <host:port>]
//! ```
//!
//! Settings live in `wfd_config.json` next to the exe (written with defaults on first run).
//! To share the Python app's settings instead, point at its file, e.g. from the repo root:
//! `wfd-app --config wfd-control\wfd_config.json` (or `..\wfd-control\wfd_config.json` from
//! inside wfd-app\). Both apps read and write the same format.

// Release builds are a windowed app: no console window pops up at login. When started from a
// terminal, attach_parent_console() still shows --help and errors there.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde_json::Value;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock, mpsc};
use std::thread;
use std::time::{Duration, Instant};
use wfd_app::{audio, clocklink, config, ctx::Ctx, link, metrics, release::Release, usbdriver, weather, web};

/// The firmware this app ships (wfd-firmware/releases, made by make_release.py).
const VERSION_JSON: &str = include_str!("../../wfd-firmware/releases/version.json");
static FIRMWARE: &[u8] = include_bytes!("../../wfd-firmware/releases/wfd-clock-firmware.bin");

const DEFAULT_ADDR: &str = "127.0.0.1:8737";
const WEATHER_EVERY: Duration = Duration::from_secs(900);

const USAGE: &str = "usage: wfd-app [--no-browser] [--config <path>] [--addr <host:port>] [--version]

  --no-browser       don't open the dashboard in the browser
  --config <path>    settings file (default: wfd_config.json next to wfd-app.exe);
                     use ..\\wfd-control\\wfd_config.json to share the Python app's settings
  --addr <host:port> where to serve the dashboard (default: 127.0.0.1:8737)
  --version          print the app version and the clock firmware it ships";

struct Args {
    browser: bool,
    config: PathBuf,
    addr: String,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        browser: true,
        config: default_config_path(),
        addr: DEFAULT_ADDR.into(),
    };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--no-browser" => args.browser = false,
            "--config" => args.config = it.next().ok_or("--config needs a path")?.into(),
            "--addr" => args.addr = it.next().ok_or("--addr needs host:port")?,
            "-V" | "--version" => {
                let fw = embedded_release().map_or("?".into(), |r| format!("{} (protocol {})", r.fw, r.protocol));
                println!("wfd-app {}, ships {fw}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    Ok(args)
}

fn default_config_path() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("wfd_config.json")))
        .unwrap_or_else(|| PathBuf::from("wfd_config.json"))
}

/// The shipped release, as app.py `load_release` reads version.json (None if malformed).
fn embedded_release() -> Option<Release> {
    let info: Value = serde_json::from_str(VERSION_JSON).ok()?;
    let fw = match info.get("fw")? {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    let protocol = info
        .get("protocol")?
        .as_u64()
        .and_then(|p| u32::try_from(p).ok())?;
    Some(Release { fw, protocol })
}

/// A windowed (release) build has no console; if a terminal started us, write to that one.
fn attach_parent_console() {
    #[cfg(all(windows, not(debug_assertions)))]
    {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn AttachConsole(process_id: u32) -> i32;
        }
        const ATTACH_PARENT_PROCESS: u32 = u32::MAX;
        // SAFETY: plain Win32 call; failing (no parent console) is fine.
        unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
    }
}

/// Let go of the terminal we attached to once started. A console closes every process attached
/// to it (CTRL_CLOSE_EVENT), so otherwise the app would quit when the terminal that started it
/// (install.ps1, or a manual run) is closed. Later stderr writes go nowhere (null handles).
fn detach_console() {
    #[cfg(all(windows, not(debug_assertions)))]
    {
        use windows_sys::Win32::System::Console::{
            FreeConsole, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, SetStdHandle,
        };
        // SAFETY: plain Win32 calls; FreeConsole fails harmlessly when there is no console.
        unsafe {
            if FreeConsole() != 0 {
                for which in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
                    SetStdHandle(which, std::ptr::null_mut());
                }
            }
        }
    }
}

fn main() {
    attach_parent_console();
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}\n\n{USAGE}");
            std::process::exit(2);
        }
    };

    let _ = LOG.set(args.config.with_file_name("wfd-app.log"));
    log(&format!("started: wfd-app {} (pid {})", env!("CARGO_PKG_VERSION"), std::process::id()));
    let default_panic = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let name = thread::current().name().unwrap_or("?").to_string();
        log(&format!("panic in thread {name}: {info}"));
        default_panic(info);
    }));

    let ctx = Arc::new(Ctx::new(args.config.clone(), embedded_release(), FIRMWARE));
    if !args.config.exists()
        && let Err(e) = config::save(&args.config, &ctx.cfg())
    {
        eprintln!("can't write {}: {e}", args.config.display());
    }
    on_ctrl_c(Arc::clone(&ctx.stop));

    audio::start(Arc::clone(&ctx.audio), Arc::clone(&ctx.stop));
    metrics::start(Arc::clone(&ctx.metrics), Arc::clone(&ctx.stop));
    spawn("readers", {
        let ctx = Arc::clone(&ctx);
        move || copy_readers(&ctx)
    });
    spawn("weather", {
        let ctx = Arc::clone(&ctx);
        move || weather_loop(&ctx)
    });
    let (link_done_tx, link_done) = mpsc::channel::<()>();
    spawn("clocklink", {
        let ctx = Arc::clone(&ctx);
        move || {
            clocklink::run(ctx);
            let _ = link_done_tx.send(());
        }
    });

    let server = match web::bind(&args.addr) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("can't listen on {}: {e}", args.addr);
            log(&format!("stopped: can't listen on {} ({e}); is another copy running?", args.addr));
            ctx.stop.store(true, Ordering::Relaxed);
            std::process::exit(1);
        }
    };
    let url = format!("http://{}", args.addr);
    println!("WFD Control at {url}");
    detach_console();
    if args.browser {
        spawn("browser", move || {
            thread::sleep(Duration::from_millis(600));
            open_browser(&url);
        });
    }
    web::run(Arc::clone(&ctx), &server);

    // Stopping: give the clock link a moment to let go of the clock, then leave.
    let _ = link_done.recv_timeout(Duration::from_secs(2));
    log("stopped: the dashboard server ended");
    std::process::exit(0);
}

fn spawn(name: &str, f: impl FnOnce() + Send + 'static) {
    thread::Builder::new()
        .name(name.into())
        .spawn(f)
        .expect("spawn thread");
}

/// Once a second: metrics and audio device names into the shared state (what app.py's
/// metrics_reader / audio_reader write with set_state).
fn copy_readers(ctx: &Ctx) {
    let mut tick = 0u32;
    while !ctx.stopping() {
        if tick.is_multiple_of(5) {
            // no COM port for the clock: is it plugged in without a working driver?
            let problem = if link::find_port().is_some() { None } else { usbdriver::check() };
            ctx.state().usb_problem = problem;
        }
        tick = tick.wrapping_add(1);
        let m = ctx.metrics.lock().unwrap().clone();
        let (out, mic) = {
            let a = ctx.audio.lock().unwrap();
            (a.out_name.clone(), a.mic_name.clone())
        };
        {
            let mut st = ctx.state();
            st.battery = m.battery;
            st.metrics = m;
            st.audio.out = out;
            st.audio.mic = mic;
        }
        sleep_unless_stopped(ctx, Duration::from_secs(1));
    }
}

/// app.py weather_reader: every 15 min, the weather for the clock's location and units — and
/// again within 30 s when those change (the clock's settings arrive after startup, and the user
/// can change the ZIP on the dashboard), which Python only noticed at the next 15-minute fetch.
fn weather_loop(ctx: &Ctx) {
    let mut reader = weather::WeatherReader::new();
    let wanted = |ctx: &Ctx| {
        let loc = ctx.clock_setting("location", Value::from(""));
        let units = ctx.clock_setting("units", Value::from("fahrenheit"));
        (loc.as_str().unwrap_or("").to_string(), units.as_str().unwrap_or("fahrenheit").to_string())
    };
    while !ctx.stopping() {
        let (loc, units) = wanted(ctx);
        match reader.run_once(&loc, &units) {
            Ok((geo, w)) => {
                let mut st = ctx.state();
                st.geo = Some(geo);
                st.weather = w;
                st.online = true;
            }
            Err(_) => ctx.state().online = false,
        }
        let next = Instant::now() + WEATHER_EVERY;
        while !ctx.stopping() && Instant::now() < next && wanted(ctx) == (loc.clone(), units.clone()) {
            sleep_unless_stopped(ctx, Duration::from_secs(30));
        }
    }
}

fn sleep_unless_stopped(ctx: &Ctx, d: Duration) {
    let end = Instant::now() + d;
    while !ctx.stopping() {
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        thread::sleep(left.min(Duration::from_millis(200)));
    }
}

fn open_browser(url: &str) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let _ = std::process::Command::new("cmd")
            .args(["/c", "start", "", url])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
    }
    #[cfg(not(windows))]
    {
        let _ = std::process::Command::new("xdg-open").arg(url).spawn();
    }
}

static STOP: OnceLock<Arc<AtomicBool>> = OnceLock::new();
static LOG: OnceLock<PathBuf> = OnceLock::new();

/// One line in wfd-app.log (next to the settings): starts, why it stopped, panics. Kept small:
/// past 256 KB the old log is renamed to wfd-app.log.old.
fn log(msg: &str) {
    use std::io::Write;
    let Some(path) = LOG.get() else { return };
    if std::fs::metadata(path).is_ok_and(|m| m.len() > 256 * 1024) {
        let _ = std::fs::rename(path, path.with_extension("log.old"));
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{} {msg}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"));
    }
}

/// Ctrl-C / closing the console sets ctx.stop; the web loop then returns and main exits.
fn on_ctrl_c(stop: Arc<AtomicBool>) {
    let _ = STOP.set(stop);
    #[cfg(windows)]
    {
        // kernel32's SetConsoleCtrlHandler, declared here (the windows-sys Console feature
        // isn't enabled). The handler runs on a thread the system creates.
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn SetConsoleCtrlHandler(
                handler: Option<unsafe extern "system" fn(u32) -> i32>,
                add: i32,
            ) -> i32;
        }
        unsafe extern "system" fn handler(ctrl_type: u32) -> i32 {
            const CTRL_LOGOFF_EVENT: u32 = 5;
            let what = match ctrl_type {
                0 => "Ctrl+C",
                1 => "Ctrl+Break",
                2 => "its console was closed",
                CTRL_LOGOFF_EVENT => "a sign-out",
                6 => "Windows shutting down",
                _ => "a console event",
            };
            if ctrl_type == CTRL_LOGOFF_EVENT {
                // a console passes on any user's sign-out; ours ends the session (and us) anyway
                log("ignored a sign-out signal from the console");
                return 1;
            }
            log(&format!("stopping: {what} (console event {ctrl_type})"));
            if let Some(stop) = STOP.get() {
                stop.store(true, Ordering::Relaxed);
            }
            // Give main a moment to wind down (the process ends when main exits, or when
            // this returns for a console close).
            thread::sleep(Duration::from_secs(3));
            1
        }
        // SAFETY: registers a handler with the documented signature; it only touches statics.
        unsafe {
            SetConsoleCtrlHandler(Some(handler), 1);
        }
    }
}
