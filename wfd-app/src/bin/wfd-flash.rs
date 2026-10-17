//! wfd-flash: put the custom firmware on an EleksWFD clock over USB, backing it up first.
//!
//!     wfd-flash                  back up, then flash the firmware built into this exe
//!     wfd-flash --backup-only    just back up the whole flash
//!     wfd-flash --restore FILE   write a backup back (e.g. to return to the stock firmware)
//!     add --yes to skip the questions
//!
//! Every write starts with a full 4 MB backup, checked against the clock's own MD5, into
//! Documents\WFD Clock backups. If the backup fails, nothing is written. A clock that already
//! runs our firmware keeps its WiFi and settings (see wfd_app::flashplan).
use std::io::{BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use espflash::connection::{Connection, ResetAfterOperation, ResetBeforeOperation};
use espflash::flasher::Flasher;
use espflash::target::{Chip, ProgressCallbacks};
use serialport::{FlowControl, SerialPortType, UsbPortInfo};
use wfd_app::{flashplan, link, usbdriver};

static MERGED: &[u8] = include_bytes!("../../../wfd-firmware/releases/wfd-clock-merged.bin");
const VERSION_JSON: &str = include_str!("../../../wfd-firmware/releases/version.json");
const FAST_BAUD: u32 = 460_800;

struct Opts {
    yes: bool,
    backup_only: bool,
    restore: Option<PathBuf>,
}

fn main() {
    let code = match run() {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("\nStopped: {e}");
            1
        }
    };
    pause();
    std::process::exit(code);
}

fn run() -> Result<(), String> {
    let opts = parse_args()?;
    let shipped: serde_json::Value = serde_json::from_str(VERSION_JSON).map_err(|e| e.to_string())?;
    println!("WFD Clock flasher {}", env!("CARGO_PKG_VERSION"));
    println!("Firmware in this tool: {} (protocol {})\n", shipped["fw"].as_str().unwrap_or("?"), shipped["protocol"]);

    // 1. the clock's port, and the app off it
    let port = find_clock()?;
    let restart_app = free_port(&port, opts.yes)?;

    // 2. what it runs now (our firmware answers CFG)
    print!("Checking what the clock runs... ");
    flush();
    let ours = ask_cfg(&port);
    match &ours {
        Some(fw) if opts.restore.is_none() && !opts.backup_only => {
            println!("our firmware ({fw}). Your WiFi and settings will be kept.")
        }
        Some(fw) => println!("our firmware ({fw})."),
        None => println!("not our firmware (the stock one, or it didn't answer)."),
    }

    // 3. back up everything first
    let backup = backup_path()?;
    println!("\nBacking up the whole flash (4 MB, about 2 minutes) to\n  {}", backup.display());
    let mut flasher = connect(&port)?;
    flasher
        .read_flash(0, flashplan::FLASH_SIZE, 0x1000, 64, backup.clone())
        .map_err(|e| format!("backup failed, nothing was written: {e}"))?;
    let size = std::fs::metadata(&backup).map(|m| m.len()).unwrap_or(0);
    if size != u64::from(flashplan::FLASH_SIZE) {
        return Err(format!("the backup is {size} bytes, not 4 MB; nothing was written"));
    }
    println!("Backup done and checked (MD5 matches the clock's own).");

    if opts.backup_only {
        reset(flasher);
        return finish(restart_app, "Backup only: nothing was written.");
    }

    // 4. what to write
    let restored;
    let (segments, what) = if let Some(file) = &opts.restore {
        restored = std::fs::read(file).map_err(|e| format!("{}: {e}", file.display()))?;
        if restored.len() as u32 != flashplan::FLASH_SIZE {
            return Err(format!("{} isn't a 4 MB backup ({} bytes)", file.display(), restored.len()));
        }
        (vec![(0, restored.as_slice())], format!("restore {}", file.display()))
    } else {
        let keep = ours.is_some();
        let what = if keep {
            "write the new firmware and keep your settings".to_string()
        } else {
            "replace the stock firmware (your backup can put it back)".to_string()
        };
        (flashplan::plan(MERGED, keep)?, what)
    };
    if !opts.yes && !confirm(&format!("\nReady to {what}. Go ahead?"))? {
        reset(flasher);
        return finish(restart_app, "Nothing was written.");
    }

    // 5. write, verify, restart the clock
    println!();
    let segs: Vec<espflash::image_format::Segment<'_>> = segments
        .iter()
        .map(|&(addr, data)| espflash::image_format::Segment { addr, data: data.into() })
        .collect();
    flasher
        .write_bins_to_flash(&segs, &mut Progress::default())
        .map_err(|e| format!("writing failed: {e}. Run this tool again, or restore {}", backup.display()))?;
    reset(flasher);
    let next = if opts.restore.is_some() {
        "Restored. The clock is restarting on what's in the backup."
    } else if ours.is_some() {
        "Done. The clock is restarting on the new firmware with your settings."
    } else {
        "Done. On your phone, join the clock's \"WFD-Setup\" WiFi and pick your network (2.4 GHz).\n\
         From now on, firmware updates go over WiFi: no USB needed."
    };
    finish(restart_app, next)
}

fn parse_args() -> Result<Opts, String> {
    let mut o = Opts { yes: false, backup_only: false, restore: None };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--yes" | "-y" => o.yes = true,
            "--backup-only" => o.backup_only = true,
            "--restore" => o.restore = Some(it.next().ok_or("--restore needs a backup file")?.into()),
            "-h" | "--help" => {
                println!("usage: wfd-flash [--backup-only | --restore FILE] [--yes]");
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument {other:?} (try --help)")),
        }
    }
    Ok(o)
}

fn find_clock() -> Result<String, String> {
    print!("Looking for the clock on USB... ");
    flush();
    if let Some(port) = link::find_port() {
        println!("{port}");
        return Ok(port);
    }
    println!("not found.");
    Err(match usbdriver::check() {
        Some(problem) => format!(
            "{problem}.\nWindows normally installs it by itself; if it hasn't, get CH341SER.EXE from\n  {}\nrun it, then unplug and replug the clock.",
            usbdriver::DRIVER_URL
        ),
        None => "plug the clock into a USB 2.0 port (USB 3 ports can't put it into flashing mode)".into(),
    })
}

/// The WFD Control app holds the port while it runs: offer to stop it. Returns whether we did.
fn free_port(port: &str, yes: bool) -> Result<bool, String> {
    if serialport::new(port, 115_200).open().is_ok() {
        return Ok(false);
    }
    let app_running = Command::new("tasklist")
        .args(["/FI", "IMAGENAME eq wfd-app.exe", "/NH"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("wfd-app.exe"))
        .unwrap_or(false);
    if !app_running {
        return Err(format!("{port} is in use by another program; close it and run this again"));
    }
    if !yes && !confirm("The WFD Control app is using the clock's port. Stop it for now (it restarts after)?")? {
        return Err("close WFD Control and run this again".into());
    }
    let _ = Command::new("taskkill").args(["/IM", "wfd-app.exe", "/F"]).output();
    std::thread::sleep(Duration::from_millis(1000));
    Ok(true)
}

/// Ask over serial; our firmware replies `CFG {json}` (it may be restarting from the port opening).
fn ask_cfg(port: &str) -> Option<String> {
    let mut p = serialport::new(port, 115_200).timeout(Duration::from_millis(200)).dtr_on_open(false).open().ok()?;
    let _ = p.write_data_terminal_ready(false);
    let _ = p.write_request_to_send(false);
    let mut got = String::new();
    let start = Instant::now();
    let mut asked = Instant::now() - Duration::from_secs(1);
    while start.elapsed() < Duration::from_secs(5) {
        if asked.elapsed() >= Duration::from_millis(700) {
            let _ = p.write_all(b"CFG\n");
            asked = Instant::now();
        }
        let mut buf = [0u8; 512];
        if let Ok(n) = p.read(&mut buf) {
            got.push_str(&String::from_utf8_lossy(&buf[..n]));
            if let Some(fw) = flashplan::our_firmware(&got) {
                let _ = p.write_all(b"BYE\n");
                return Some(fw);
            }
        }
    }
    None
}

fn connect(port: &str) -> Result<Flasher, String> {
    println!("Putting the clock into flashing mode...");
    let serial = serialport::new(port, 115_200)
        .flow_control(FlowControl::None)
        .open_native()
        .map_err(|e| format!("can't open {port}: {e}"))?;
    let info = serialport::available_ports()
        .ok()
        .and_then(|ps| ps.into_iter().find(|p| p.port_name == port))
        .and_then(|p| match p.port_type {
            SerialPortType::UsbPort(u) => Some(u),
            _ => None,
        })
        .unwrap_or(UsbPortInfo { vid: 0, pid: 0, serial_number: None, manufacturer: None, product: None });
    let conn = Connection::new(serial, info, ResetAfterOperation::HardReset, ResetBeforeOperation::DefaultReset, 115_200);
    Flasher::connect(conn, true, true, false, Some(Chip::Esp32), Some(FAST_BAUD)).map_err(|e| {
        format!("couldn't put the clock into flashing mode ({e}).\nUse a USB 2.0 port (USB 3 ports often can't), then try again")
    })
}

fn reset(flasher: Flasher) {
    let mut conn = flasher.into_connection();
    let _ = conn.reset();
}

fn backup_path() -> Result<PathBuf, String> {
    let docs = std::env::var_os("USERPROFILE").map(|h| Path::new(&h).join("Documents")).unwrap_or_else(|| ".".into());
    let dir = docs.join("WFD Clock backups");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    Ok(dir.join(flashplan::backup_name(&chrono::Local::now().format("%Y%m%d-%H%M%S").to_string())))
}

fn finish(restart_app: bool, msg: &str) -> Result<(), String> {
    println!("\n{msg}");
    let app = std::env::var_os("LOCALAPPDATA").map(|d| Path::new(&d).join("WFD Clock").join("wfd-app.exe"));
    if restart_app && let Some(exe) = app.filter(|e| e.exists()) {
        std::thread::sleep(Duration::from_secs(2)); // let the clock boot first
        // via `start`, so the app isn't tied to this window: it would otherwise attach to this
        // console (and our output pipe) and close with it
        keep_std_handles_to_ourselves();
        let _ = Command::new("cmd")
            .args(["/d", "/c", "start", "", "/d"])
            .arg(exe.parent().unwrap())
            .arg(&exe)
            .arg("--no-browser")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        println!("Restarted WFD Control.");
    }
    Ok(())
}

/// Windows hands inheritable handles down to child processes: make our stdin/stdout/stderr
/// non-inheritable so the restarted app doesn't hold a pipe from whatever ran this tool.
fn keep_std_handles_to_ourselves() {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::{HANDLE_FLAG_INHERIT, SetHandleInformation};
        use windows_sys::Win32::System::Console::{GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE};
        for which in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            // SAFETY: plain Win32 calls on this process's own standard handles
            unsafe {
                SetHandleInformation(GetStdHandle(which), HANDLE_FLAG_INHERIT, 0);
            }
        }
    }
}

fn confirm(q: &str) -> Result<bool, String> {
    print!("{q} [y/N] ");
    flush();
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line).map_err(|e| e.to_string())?;
    Ok(matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes"))
}

/// Keep the window open when started by double-click (not when run with --yes from a script).
fn pause() {
    if std::env::args().any(|a| a == "--yes" || a == "-y") {
        return;
    }
    print!("\nPress Enter to close.");
    flush();
    let _ = std::io::stdin().lock().read_line(&mut String::new());
}

fn flush() {
    let _ = std::io::stdout().flush();
}

#[derive(Default)]
struct Progress {
    total: usize,
    last: usize,
}

impl ProgressCallbacks for Progress {
    fn init(&mut self, addr: u32, total: usize) {
        self.total = total.max(1);
        self.last = usize::MAX;
        print!("Writing at {addr:#08x}: ");
        flush();
    }
    fn update(&mut self, current: usize) {
        let pct = current * 100 / self.total;
        if pct / 10 != self.last / 10 || self.last == usize::MAX {
            print!("{pct}% ");
            flush();
            self.last = pct;
        }
    }
    fn verifying(&mut self) {
        print!("verifying... ");
        flush();
    }
    fn finish(&mut self, skipped: bool) {
        println!("{}", if skipped { "already there" } else { "done" });
    }
}
