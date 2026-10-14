//! link: the two transports to the clock (port of `UsbLink`, `WifiLink`, `LinkLost`,
//! `find_port`, `open_serial`, `find_clock_ip` and `open_wifi` in wfd-control/app.py).
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

use crate::config::py;
use crate::ctx::{ClockReply, Ctx};
use chrono::NaiveDateTime;
use serde_json::{Map, Value, json};
use std::fmt::{self, Display, Write as _};
use std::io::{self, Read, Write};
use std::net::{ToSocketAddrs, UdpSocket};
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The clock's USB-serial chip (CH340).
pub const CH340_VID: u16 = 0x1A86;
pub const CH340_PID: u16 = 0x7523;
pub const BAUD: u32 = 115_200;
/// The clock's UDP display port.
pub const UDP_PORT: u16 = 4210;
/// mDNS wait: the clock sometimes answers only after ~1.2 s.
pub const MDNS_TIMEOUT: Duration = Duration::from_millis(2500);
/// How long a USB settings request waits for its reply (app.py `request(timeout=1.5)`).
const REQUEST_TIMEOUT: f64 = 1.5;
/// No good HTTP answer for this long: the WiFi clock is gone.
const WIFI_ALIVE: f64 = 15.0;
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
    fn close(&mut self);
}

/// `CMD v1 v2 ...` (app.py `_frame_cmd`).
pub fn frame_cmd<T: Display>(cmd: &str, values: impl IntoIterator<Item = T>) -> String {
    let mut out = String::from(cmd);
    out.push(' ');
    for (i, v) in values.into_iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        let _ = write!(out, "{v}");
    }
    out
}

/// `{key: msg}` for every key of a settings patch (how errors are reported per setting).
pub fn per_key(patch: &Map<String, Value>, msg: &str) -> Value {
    Value::Object(
        patch
            .keys()
            .map(|k| (k.clone(), Value::String(msg.into())))
            .collect(),
    )
}

/// Python truthiness of a JSON value (`bool(x)`; a missing value is falsy).
pub fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|x| x != 0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
    }
}

/// `json.dumps(v, separators=(",", ":"))`: compact, ASCII-only, Python float repr.
/// (Keys come out in serde_json's map order, which is sorted; Python keeps insertion order.
/// The clock parses the JSON, so only the bytes on the wire differ.)
pub fn py_json(v: &Value) -> String {
    let mut out = String::new();
    dump_compact(v, &mut out);
    out
}

fn dump_compact(v: &Value, out: &mut String) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) if n.is_f64() => {
            out.push_str(&py::repr_float(n.as_f64().unwrap_or(f64::NAN)))
        }
        Value::Number(n) => out.push_str(&n.to_string()),
        Value::String(s) => dump_str(s, out),
        Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                dump_compact(x, out);
            }
            out.push(']');
        }
        Value::Object(o) => {
            out.push('{');
            for (i, (k, x)) in o.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                dump_str(k, out);
                out.push(':');
                dump_compact(x, out);
            }
            out.push('}');
        }
    }
}

/// A JSON string with Python's ensure_ascii escaping.
fn dump_str(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            ' '..='~' => out.push(c),
            c => {
                let mut buf = [0u16; 2];
                for unit in c.encode_utf16(&mut buf) {
                    let _ = write!(out, "\\u{unit:04x}");
                }
            }
        }
    }
    out.push('"');
}

/// Standard base64 (for HTTP Basic auth).
pub fn base64(data: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = u32::from(b[0]) << 16 | u32::from(b[1]) << 8 | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ABC[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// `Authorization` header value for the clock's web password (user "admin").
pub fn basic_auth(password: &str) -> String {
    format!("Basic {}", base64(format!("admin:{password}").as_bytes()))
}

// ------------------------------------------------------------------ USB

/// The clock on USB serial: commands go out as they're made; settings via CFG / CFGSET.
pub struct UsbLink {
    s: Box<dyn SerialIo>,
    clock: Rc<dyn Clock>,
    name: String,
    fw: Option<String>,
    protocol: Option<u32>,
    ip: Option<String>,
}

impl UsbLink {
    pub fn new(s: Box<dyn SerialIo>, clock: Rc<dyn Clock>) -> Self {
        let name = s.port_name();
        UsbLink {
            s,
            clock,
            name,
            fw: None,
            protocol: None,
            ip: None,
        }
    }

    /// Send a command and wait for a reply line starting with one of `prefixes` + " ".
    /// Other lines (e.g. `OK` from earlier commands) are skipped.
    fn request(
        &mut self,
        line: &str,
        prefixes: &[&'static str],
    ) -> Result<Option<(&'static str, Value)>, LinkLost> {
        self.send(line)?;
        let deadline = self.clock.now() + REQUEST_TIMEOUT;
        while self.clock.now() < deadline {
            let raw = self.s.readline()?;
            if raw.is_empty() {
                continue;
            }
            let text = String::from_utf8_lossy(&raw);
            let text = text.trim();
            for &pre in prefixes {
                if let Some(rest) = text.strip_prefix(pre).and_then(|r| r.strip_prefix(' ')) {
                    // app.py lets a garbled reply raise (logged, link kept); here it's skipped.
                    if let Ok(v) = serde_json::from_str(rest) {
                        return Ok(Some((pre, v)));
                    }
                }
            }
        }
        Ok(None)
    }
}

impl Link for UsbLink {
    fn kind(&self) -> &'static str {
        "usb"
    }

    fn name(&self) -> String {
        self.name.clone()
    }

    fn alive(&self) -> bool {
        self.s.is_open()
    }

    /// Unknown over USB: keep sending the PC's time.
    fn clock_ntp(&self) -> bool {
        false
    }

    fn version(&self) -> (Option<String>, Option<u32>) {
        (self.fw.clone(), self.protocol)
    }

    fn clock_ip(&self) -> Option<String> {
        self.ip.clone()
    }

    fn send(&mut self, line: &str) -> Result<(), LinkLost> {
        self.s.write(format!("{line}\n").as_bytes())?;
        Ok(())
    }

    fn flush(&mut self) -> Result<(), LinkLost> {
        Ok(())
    }

    fn get_config(&mut self) -> Result<Option<Map<String, Value>>, LinkLost> {
        let Some((_, Value::Object(mut c))) = self.request("CFG", &["CFG"])? else {
            return Ok(None);
        };
        // the build info travels with the settings but isn't one
        self.fw = c.remove("_fw").and_then(|v| v.as_str().map(str::to_owned));
        self.protocol = c
            .remove("_protocol")
            .and_then(|v| v.as_u64())
            .and_then(|p| u32::try_from(p).ok());
        self.ip = c
            .remove("_ip")
            .and_then(|v| v.as_str().map(str::to_owned))
            .filter(|ip| !ip.is_empty()); // "" = the clock isn't on WiFi
        Ok(Some(c))
    }

    fn set_config(&mut self, patch: &Map<String, Value>) -> Result<ClockReply, LinkLost> {
        let line = format!("CFGSET {}", py_json(&Value::Object(patch.clone())));
        Ok(match self.request(&line, &["CFGOK", "CFGERR"])? {
            Some(("CFGOK", mut body)) => Ok(body
                .as_object_mut()
                .and_then(|b| b.remove("_warning"))
                .filter(|w| !w.is_null())),
            Some((_, body)) => match body.get("error") {
                Some(err @ Value::Object(_)) => Err(err.clone()),
                other => Err(per_key(
                    patch,
                    &py::str_value(other.unwrap_or(&Value::Null)),
                )),
            },
            None => Err(per_key(patch, "clock didn't answer")),
        })
    }

    fn store_anim(&mut self, name: &str, fps: u32, frame: bool) -> Result<(), LinkLost> {
        self.send(&format!("GANIM {name} {fps} {}", u8::from(frame)))
    }

    fn store_frames(&mut self, frames: &[[u8; 7]], fps: u32, frame: bool) -> Result<(), LinkLost> {
        // paced like app.py so the clock's serial buffer keeps up
        self.send("GCLR")?;
        self.clock.sleep(0.03);
        for fr in frames.iter().take(80) {
            self.send(&frame_cmd("GADD", fr))?;
            self.clock.sleep(0.015);
        }
        self.send(&format!("GCOMMIT {fps} {}", u8::from(frame)))
    }

    fn after_time(&mut self) -> Result<(), LinkLost> {
        self.clock.sleep(0.15);
        self.s.reset_input_buffer()?;
        Ok(())
    }

    fn close(&mut self) {
        self.s.close();
    }
}

// ------------------------------------------------------------------ WiFi

/// The clock over WiFi: each frame's commands go out as one UDP packet; settings, uploads and
/// the health check use its HTTP API.
pub struct WifiLink {
    ip: String,
    password: String,
    sock: Box<dyn Datagram>,
    http: Box<dyn HttpApi>,
    clock: Rc<dyn Clock>,
    buf: Vec<String>,
    clock_ntp: bool,
    fw: Option<String>,
    protocol: Option<u32>,
    ok_at: f64,
}

impl WifiLink {
    /// A link over the real network (UDP socket + HTTP via ureq).
    pub fn new(ip: &str, password: &str, clock: Rc<dyn Clock>) -> io::Result<Self> {
        let sock = UdpDatagram(Some(UdpSocket::bind("0.0.0.0:0")?));
        let http = UreqHttp::new(ip, password, HTTP_TIMEOUT);
        Ok(Self::with(
            ip,
            password,
            Box::new(sock),
            Box::new(http),
            clock,
        ))
    }

    /// A link over the given socket and HTTP seams (tests use fakes).
    pub fn with(
        ip: &str,
        password: &str,
        sock: Box<dyn Datagram>,
        http: Box<dyn HttpApi>,
        clock: Rc<dyn Clock>,
    ) -> Self {
        let ok_at = clock.now();
        WifiLink {
            ip: ip.into(),
            password: password.into(),
            sock,
            http,
            clock,
            buf: Vec::new(),
            clock_ntp: false,
            fw: None,
            protocol: None,
            ok_at,
        }
    }
}

impl Link for WifiLink {
    fn kind(&self) -> &'static str {
        "wifi"
    }

    fn name(&self) -> String {
        format!("WiFi {}", self.ip)
    }

    fn alive(&self) -> bool {
        self.clock.now() - self.ok_at < WIFI_ALIVE
    }

    fn clock_ntp(&self) -> bool {
        self.clock_ntp
    }

    fn version(&self) -> (Option<String>, Option<u32>) {
        (self.fw.clone(), self.protocol)
    }

    fn clock_ip(&self) -> Option<String> {
        Some(self.ip.clone())
    }

    fn send(&mut self, line: &str) -> Result<(), LinkLost> {
        self.buf.push(line.to_owned());
        Ok(())
    }

    fn flush(&mut self) -> Result<(), LinkLost> {
        if self.buf.is_empty() {
            return Ok(());
        }
        let head = if self.password.is_empty() {
            "WFD1".to_string()
        } else {
            format!("WFD1 {}", self.password)
        };
        let mut packet = head;
        for line in &self.buf {
            packet.push('\n');
            packet.push_str(line);
        }
        self.sock.send_to(packet.as_bytes(), &self.ip, UDP_PORT)?;
        self.buf.clear();
        Ok(())
    }

    fn get_config(&mut self) -> Result<Option<Map<String, Value>>, LinkLost> {
        let (code, body) = self.http.call("GET", "/api/status", None)?;
        if code != 200 || !truthy(Some(&body)) {
            return Err(LinkLost(format!("status: HTTP {code}")));
        }
        let (Some(st), Some(Value::Object(config))) = (body.get("state"), body.get("config"))
        else {
            return Err(LinkLost("status: no state/config".into()));
        };
        self.ok_at = self.clock.now();
        self.clock_ntp = truthy(st.get("ntp"));
        self.fw = st.get("fw").and_then(Value::as_str).map(str::to_owned);
        self.protocol = st
            .get("protocol")
            .and_then(Value::as_u64)
            .and_then(|p| u32::try_from(p).ok());
        Ok(Some(config.clone()))
    }

    fn set_config(&mut self, patch: &Map<String, Value>) -> Result<ClockReply, LinkLost> {
        let (code, body) =
            self.http
                .call("POST", "/api/set", Some(&Value::Object(patch.clone())))?;
        if code == 200 {
            return Ok(Ok(body.get("warning").filter(|w| !w.is_null()).cloned()));
        }
        let err = if code == 401 {
            Value::String("wrong clock password (clock_pass)".into())
        } else {
            match body.get("error") {
                e @ Some(_) if truthy(e) => e.cloned().unwrap_or_default(),
                _ => Value::String(format!("HTTP {code}")),
            }
        };
        Ok(Err(match err {
            Value::Object(_) => err,
            other => per_key(patch, &py::str_value(&other)),
        }))
    }

    fn store_anim(&mut self, name: &str, fps: u32, frame: bool) -> Result<(), LinkLost> {
        let patch = json!({"standalone_anim": name, "standalone_fps": fps,
                           "standalone_frame": frame, "grid_standalone": true});
        let Value::Object(patch) = patch else {
            unreachable!()
        };
        self.set_config(&patch).map(|_| ())
    }

    fn store_frames(&mut self, frames: &[[u8; 7]], fps: u32, frame: bool) -> Result<(), LinkLost> {
        let frames: Vec<&[u8; 7]> = frames.iter().take(80).collect();
        let body = json!({"frames": frames, "fps": fps, "frame": frame});
        self.http
            .call("POST", "/api/frames", Some(&body))
            .map(|_| ())
    }

    fn after_time(&mut self) -> Result<(), LinkLost> {
        Ok(())
    }

    fn close(&mut self) {
        let _ = self.flush();
        self.sock.close();
    }
}

/// The real UDP socket (None once closed).
pub struct UdpDatagram(pub Option<UdpSocket>);

impl Datagram for UdpDatagram {
    fn send_to(&mut self, data: &[u8], ip: &str, port: u16) -> io::Result<()> {
        let sock = self
            .0
            .as_ref()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "socket closed"))?;
        sock.send_to(data, (ip, port)).map(|_| ())
    }

    fn close(&mut self) {
        self.0 = None;
    }
}

/// The clock's HTTP API over ureq: `X-WFD: 1`, Basic auth admin:<password> when set,
/// JSON bodies, `timeout` for the whole call.
pub struct UreqHttp {
    host: String,
    auth: Option<String>,
    agent: ureq::Agent,
}

impl UreqHttp {
    /// `host` is an IP or name, optionally with `:port`.
    pub fn new(host: &str, password: &str, timeout: Duration) -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(timeout))
            .http_status_as_error(false)
            .build()
            .into();
        UreqHttp {
            host: host.into(),
            auth: (!password.is_empty()).then(|| basic_auth(password)),
            agent,
        }
    }
}

impl HttpApi for UreqHttp {
    fn call(
        &mut self,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Result<(u16, Value), LinkLost> {
        let url = format!("http://{}{}", self.host, path);
        let lost = |e: ureq::Error| LinkLost(e.to_string());
        let mut resp = if method == "GET" {
            let mut req = self.agent.get(&url).header("X-WFD", "1");
            if let Some(a) = &self.auth {
                req = req.header("Authorization", a);
            }
            req.call().map_err(lost)?
        } else {
            let mut req = self.agent.post(&url).header("X-WFD", "1");
            if let Some(a) = &self.auth {
                req = req.header("Authorization", a);
            }
            match body {
                Some(b) => req
                    .header("Content-Type", "application/json")
                    .send(py_json(b).as_bytes())
                    .map_err(lost)?,
                None => req.send_empty().map_err(lost)?,
            }
        };
        let code = resp.status().as_u16();
        let raw = resp.body_mut().read_to_vec().map_err(lost)?;
        let value = if raw.is_empty() {
            Ok(Value::Null)
        } else {
            serde_json::from_slice(&raw)
        };
        match value {
            Ok(v) => Ok((code, v)),
            Err(_) if code >= 400 => Ok((code, Value::Null)),
            Err(e) => Err(LinkLost(format!("bad reply: {e}"))),
        }
    }
}

// ------------------------------------------------------------------ finding the clock

/// The clock's serial port (CH340), if plugged in.
pub fn find_port() -> Option<String> {
    serialport::available_ports()
        .ok()?
        .into_iter()
        .find_map(|p| match p.port_type {
            serialport::SerialPortType::UsbPort(u) if u.vid == CH340_VID && u.pid == CH340_PID => {
                Some(p.port_name)
            }
            _ => None,
        })
}

/// A real serial port with pyserial-like `readline` (1 s timeout).
pub struct RealSerial {
    port: Option<Box<dyn serialport::SerialPort>>,
    name: String,
    buf: Vec<u8>,
}

const READ_TIMEOUT: Duration = Duration::from_secs(1);

impl SerialIo for RealSerial {
    fn port_name(&self) -> String {
        self.name.clone()
    }

    fn is_open(&self) -> bool {
        self.port.is_some()
    }

    fn write(&mut self, data: &[u8]) -> io::Result<()> {
        let port = self.port.as_mut().ok_or_else(closed)?;
        port.write_all(data)
    }

    fn readline(&mut self) -> io::Result<Vec<u8>> {
        let deadline = Instant::now() + READ_TIMEOUT;
        loop {
            if let Some(i) = self.buf.iter().position(|&b| b == b'\n') {
                return Ok(self.buf.drain(..=i).collect());
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(std::mem::take(&mut self.buf));
            }
            let port = self.port.as_mut().ok_or_else(closed)?;
            port.set_timeout(left)?;
            let mut chunk = [0u8; 256];
            match port.read(&mut chunk) {
                Ok(n) => self.buf.extend_from_slice(&chunk[..n]),
                Err(e) if e.kind() == io::ErrorKind::TimedOut => {
                    return Ok(std::mem::take(&mut self.buf));
                }
                Err(e) => return Err(e),
            }
        }
    }

    fn reset_input_buffer(&mut self) -> io::Result<()> {
        self.buf.clear();
        let port = self.port.as_mut().ok_or_else(closed)?;
        port.clear(serialport::ClearBuffer::Input)
            .map_err(io::Error::from)
    }

    fn close(&mut self) {
        self.port = None;
    }
}

fn closed() -> io::Error {
    io::Error::new(io::ErrorKind::NotConnected, "port closed")
}

/// Open the clock's serial port: 115200 baud, DTR/RTS low (so the ESP32 doesn't reset),
/// 1 s read timeout. None if it isn't plugged in or can't be opened.
pub fn open_serial() -> Option<RealSerial> {
    let name = find_port()?;
    let mut port = serialport::new(&name, BAUD)
        .timeout(READ_TIMEOUT)
        .dtr_on_open(false)
        .open()
        .ok()?;
    let _ = port.write_data_terminal_ready(false);
    let _ = port.write_request_to_send(false);
    std::thread::sleep(Duration::from_millis(200));
    Some(RealSerial {
        port: Some(port),
        name,
        buf: Vec::new(),
    })
}

/// `\d+\.\d+\.\d+\.\d+` (app.py's test for "already an IP").
fn is_dotted_quad(host: &str) -> bool {
    let parts: Vec<&str> = host.split('.').collect();
    parts.len() == 4
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
}

/// The clock's IP for WiFi: a configured IP; else the address the clock itself reported (over
/// USB, `_ip`) or last answered on; else the configured name / wfd-clock.local via DNS or mDNS.
pub fn find_clock_ip(ctx: &Ctx) -> Option<String> {
    let host = {
        let cfg = ctx.cfg.lock().unwrap();
        cfg.get("clock_host")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string()
    };
    let host = if host.is_empty() {
        "wfd-clock.local".to_string()
    } else {
        host
    };
    if is_dotted_quad(&host) {
        return Some(host);
    }
    if let Some(ip) = ctx.state().clock_ip.clone() {
        return Some(ip); // open_wifi forgets it if it stops answering, then we look it up again
    }
    // normal DNS, or .local where Windows resolves it
    if let Ok(addrs) = (host.as_str(), 0).to_socket_addrs()
        && let Some(a) = addrs.into_iter().find(|a| a.is_ipv4())
    {
        return Some(a.ip().to_string());
    }
    let ip = if host.ends_with(".local") {
        crate::mdns::resolve(&host, MDNS_TIMEOUT).map(|ip| ip.to_string())
    } else {
        None
    };
    ip.or_else(|| ctx.state().clock_ip.clone())
}

/// Connect over WiFi: find the clock, check it answers (reads its settings), remember its IP.
pub fn open_wifi(ctx: &Ctx, clock: Rc<dyn Clock>) -> Option<Box<dyn Link>> {
    let ip = find_clock_ip(ctx)?;
    let pw = {
        let cfg = ctx.cfg.lock().unwrap();
        cfg.get("clock_pass")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    let mut link = WifiLink::new(&ip, &pw, clock).ok()?;
    if link.get_config().is_err() {
        link.close();
        let mut st = ctx.state();
        if st.clock_ip.as_deref() == Some(ip.as_str()) {
            st.clock_ip = None; // stale: look it up by name next time
        }
        return None;
    }
    ctx.state().clock_ip = Some(ip);
    Some(Box::new(link))
}
