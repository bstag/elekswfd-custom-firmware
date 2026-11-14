//! link: the two transports to the clock (port of `UsbLink`, `WifiLink`, `LinkLost`,
//! `find_port`, `open_serial`, `find_clock_ip` and `open_wifi` in legacy/python-app/app.py).
//!
//! Both transports implement [`Link`], so the clock loop (`clocklink`) doesn't care which one it
//! drives. Everything that touches the outside world sits behind a small seam so tests can use
//! fakes: time ([`Clock`]), the serial port ([`SerialIo`]), the UDP socket ([`Datagram`]) and the
//! clock's HTTP API ([`HttpApi`]).
//!
//! Wire behaviour (must match the firmware):
//! - USB: every command is written as its own line at once; settings via `CFG` / `CFGSET`,
//!   whose replies are picked out of the serial stream (other lines such as `OK` are skipped).
//! - WiFi: a frame's commands are buffered and sent as ONE UDP packet to port 4210, starting
//!   with `WFD1` (or `WFD1 <password>`); settings, uploads and the health check use the clock's
//!   HTTP API (`/api/status`, `/api/set`, `/api/frames`).
//!
//! The pieces live in submodules, re-exported here so `link::X` keeps working: the transport
//! trait and its seams (this file), the USB transport (`usb`), the WiFi transport (`wifi`),
//! finding the clock (`find`) and the JSON / encoding helpers (`json`).

mod find;
mod json;
mod usb;
mod wifi;

pub use find::{
    avoid_port, find_answering_clock_ip, find_clock_ip, find_port, load_last_ip, open_wifi,
    remember_ip,
};
pub use json::{base64, basic_auth, frame_cmd, per_key, py_json, truthy};
pub use usb::{RealSerial, UsbLink, open_serial};
pub use wifi::{UdpDatagram, UreqHttp, WifiLink};

use crate::ctx::ClockReply;
use chrono::NaiveDateTime;
use serde_json::{Map, Value};
use std::fmt::{self, Display};
use std::io;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The clock's USB-serial chip (CH340).
pub const CH340_VID: u16 = 0x1A86;
pub const CH340_PID: u16 = 0x7523;
pub const BAUD: u32 = 115_200;
/// The clock's UDP display port.
pub const UDP_PORT: u16 = 4210;
/// mDNS wait: the clock sometimes answers only after ~1.2 s.
pub const MDNS_TIMEOUT: Duration = Duration::from_millis(2500);
/// Settings the clock answers slowly (it geocodes a new ZIP before replying).
pub fn is_slow_patch(patch: &Map<String, Value>) -> bool {
    patch.contains_key("location")
}
const HTTP_TIMEOUT: Duration = Duration::from_secs(3);

/// The clock stopped answering (app.py `LinkLost`, plus the serial / socket errors it catches
/// alongside it). Any of these makes the clock loop drop the link and reconnect.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkLost(pub String);

impl Display for LinkLost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for LinkLost {}

impl From<io::Error> for LinkLost {
    fn from(e: io::Error) -> Self {
        LinkLost(e.to_string())
    }
}

// ------------------------------------------------------------------ seams

/// Time as the link and the clock loop see it (app.py `time.time` / `time.sleep` /
/// `datetime.now`), so tests can run everything on simulated time.
pub trait Clock {
    /// Seconds since the Unix epoch (`time.time()`).
    fn now(&self) -> f64;
    fn sleep(&self, secs: f64);
    /// `at` on the `now()` timeline (the web server stamps the grid preview with an `Instant`).
    fn epoch_of(&self, at: Instant) -> f64;
    /// Local wall-clock time (`datetime.datetime.now()`).
    fn local_now(&self) -> NaiveDateTime;
}

/// The real clock.
pub struct RealClock;

impl Clock for RealClock {
    fn now(&self) -> f64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0.0, |d| d.as_secs_f64())
    }

    fn sleep(&self, secs: f64) {
        if secs > 0.0 {
            std::thread::sleep(Duration::from_secs_f64(secs));
        }
    }

    fn epoch_of(&self, at: Instant) -> f64 {
        self.now() - at.elapsed().as_secs_f64()
    }

    fn local_now(&self) -> NaiveDateTime {
        chrono::Local::now().naive_local()
    }
}

/// A serial port as `UsbLink` uses it (the subset of pyserial app.py calls).
pub trait SerialIo {
    fn port_name(&self) -> String;
    fn is_open(&self) -> bool;
    fn write(&mut self, data: &[u8]) -> io::Result<()>;
    /// One line including its `\n`; whatever arrived (maybe nothing) after the read timeout.
    fn readline(&mut self) -> io::Result<Vec<u8>>;
    fn reset_input_buffer(&mut self) -> io::Result<()>;
    fn close(&mut self);
}

/// The UDP socket `WifiLink` streams the display through.
pub trait Datagram {
    fn send_to(&mut self, data: &[u8], ip: &str, port: u16) -> io::Result<()>;
    fn close(&mut self) {}
}

/// One call to the clock's HTTP API: (status, JSON body; `null` when empty or unparsable on an
/// error status). Transport failures (and a garbled 2xx body) are `LinkLost`.
pub trait HttpApi {
    fn call(
        &mut self,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Result<(u16, Value), LinkLost>;
}

// ------------------------------------------------------------------ the transport trait

/// A connection to the clock (USB or WiFi).
pub trait Link {
    /// "usb" | "wifi" (what `state.link` shows).
    fn kind(&self) -> &'static str;
    /// What `state.port` shows: "COM5" or "WiFi 10.0.0.9".
    fn name(&self) -> String;
    fn alive(&self) -> bool;
    /// The clock keeps its own (NTP) time: the app doesn't send the PC's.
    fn clock_ntp(&self) -> bool;
    /// Firmware build and protocol the clock reported on the last config read.
    fn version(&self) -> (Option<String>, Option<u32>);
    /// The clock's WiFi address, if known: reported over USB in CFG (`_ip`), the address in use
    /// over WiFi. Lets "Update clock" and a switch to WiFi skip the mDNS lookup.
    fn clock_ip(&self) -> Option<String>;
    /// Whether the clock has a web password, if it said (CFG `_pw` / status `net.key_required`).
    fn web_password_set(&self) -> Option<bool>;
    /// Set ("" = remove) the clock's web password. USB only: other links answer Ok(Err(reason)).
    fn set_web_password(&mut self, password: &str) -> Result<Result<bool, String>, LinkLost>;
    fn send(&mut self, line: &str) -> Result<(), LinkLost>;
    /// End of a frame (WiFi sends its packet here).
    fn flush(&mut self) -> Result<(), LinkLost>;
    /// The clock's own settings; None when it didn't answer (USB).
    fn get_config(&mut self) -> Result<Option<Map<String, Value>>, LinkLost>;
    /// Change clock settings: Ok(warning) or Err(per-key errors).
    fn set_config(&mut self, patch: &Map<String, Value>) -> Result<ClockReply, LinkLost>;
    /// Store a built-in grid animation for the clock to run itself.
    fn store_anim(&mut self, name: &str, fps: u32, frame: bool) -> Result<(), LinkLost>;
    /// Store drawn frames (at most 80) for the clock to play itself.
    fn store_frames(&mut self, frames: &[[u8; 7]], fps: u32, frame: bool) -> Result<(), LinkLost>;
    /// Called right after `SET` (USB waits and discards the reply).
    fn after_time(&mut self) -> Result<(), LinkLost>;
    /// Something the user should fix although the link is up (e.g. the clock rejects our
    /// display stream), as of the last config read.
    fn problem(&self) -> Option<String> {
        None
    }
    fn close(&mut self);
}
