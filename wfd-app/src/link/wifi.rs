//! The WiFi transport: [`WifiLink`] (UDP display stream + the clock's HTTP API), the real UDP
//! socket ([`UdpDatagram`]) and the real HTTP client ([`UreqHttp`]).

use super::{
    Clock, Datagram, HTTP_TIMEOUT, HttpApi, Link, LinkLost, UDP_PORT, basic_auth, error_text,
    is_slow_patch, per_key, truthy,
};
use crate::applog;
use crate::ctx::ClockReply;
use serde_json::{Map, Value, json};
use std::io;
use std::net::UdpSocket;
use std::rc::Rc;
use std::time::Duration;

const SLOW_HTTP_TIMEOUT: Duration = Duration::from_secs(10);
/// No good HTTP answer for this long: the WiFi clock is gone.
const WIFI_ALIVE: f64 = 15.0;
/// Display packets failing in a row (about a second of frames) before the socket is replaced.
const REOPEN_AFTER: u32 = 30;

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
    pw_set: Option<bool>,
    ok_at: f64,
    /// when the first display packet went out (None: nothing sent yet)
    streaming_since: Option<f64>,
    /// the clock's count of rejected packets at the last status read
    rejected: Option<u64>,
    /// status reads in a row that showed the clock not taking our stream
    deaf_reads: u32,
    problem: Option<String>,
    /// display packets in a row that didn't go out, and why the last one didn't
    send_failures: u32,
    send_error: Option<String>,
    /// the last status read was refused with 401 (the clock wants its web password)
    password_refused: bool,
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
            pw_set: None,
            ok_at,
            streaming_since: None,
            rejected: None,
            deaf_reads: 0,
            problem: None,
            send_failures: 0,
            send_error: None,
            password_refused: false,
        }
    }

    /// Did the clock refuse the last status read for want of the right web password (401)?
    pub fn password_refused(&self) -> bool {
        self.password_refused
    }

    /// A display packet didn't go out (e.g. Windows' "no buffer space", os error 10055). That
    /// frame is dropped and the next one tries again: the link stays up as long as the clock
    /// answers its status poll ([`Link::alive`]). Sends that keep failing get a fresh socket.
    fn send_failed(&mut self, e: &io::Error) {
        applog::log_once(&format!(
            "can't send the display to the clock ({}): {e}; dropping frames until it works again",
            self.ip
        ));
        self.send_failures += 1;
        self.send_error = Some(e.to_string());
        if self.send_failures.is_multiple_of(REOPEN_AFTER) {
            match self.sock.reopen() {
                Ok(()) => applog::log_once(&format!(
                    "display packets to {} kept failing: opened a new socket",
                    self.ip
                )),
                Err(e) => applog::log_once(&format!(
                    "display packets to {} kept failing, and a new socket can't be opened: {e}",
                    self.ip
                )),
            }
        }
    }

    /// Is the clock taking the display stream? Its status says whether it heard the app in the
    /// last 2 s and how many packets it threw away (wrong password). One bad read can be the
    /// clock having been busy, so it takes two in a row.
    fn check_stream(&mut self, st: &Value) {
        let net = st.get("net");
        let rejected = net.and_then(|n| n.get("udp_rejected")).and_then(Value::as_u64);
        let more_rejected = matches!((self.rejected, rejected), (Some(a), Some(b)) if b > a);
        self.rejected = rejected;
        let hears_us = st.get("app_connected").and_then(Value::as_bool);
        let streaming = self
            .streaming_since
            .is_some_and(|t| self.clock.now() - t > 3.0);
        if !streaming || hears_us != Some(false) {
            self.deaf_reads = 0;
            self.problem = None;
            return;
        }
        self.deaf_reads += 1;
        if self.deaf_reads < 2 {
            return;
        }
        self.problem = Some(if self.pw_set == Some(true) && self.password.is_empty() {
            "the clock has a web password: enter it under Connection (App uses password)".into()
        } else if more_rejected {
            "the clock is rejecting the display: wrong clock password (Connection, App uses password)".into()
        } else {
            "the clock answers, but the live display isn't reaching it (a firewall or the router is blocking UDP port 4210)".into()
        });
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

    fn web_password_set(&self) -> Option<bool> {
        self.pw_set
    }

    fn set_web_password(&mut self, _password: &str) -> Result<Result<bool, String>, LinkLost> {
        Ok(Err("connect the clock by USB to change its password".into()))
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
        // A failed send drops this frame only; the link ends when the clock stops answering.
        let sent = self.sock.send_to(packet.as_bytes(), &self.ip, UDP_PORT);
        self.buf.clear();
        if let Err(e) = sent {
            self.send_failed(&e);
            return Ok(());
        }
        if self.send_failures > 0 {
            applog::log_once(&format!(
                "display packets reach the clock ({}) again after {} failed",
                self.ip, self.send_failures
            ));
            self.send_failures = 0;
            self.send_error = None;
        }
        if self.streaming_since.is_none() {
            self.streaming_since = Some(self.clock.now());
        }
        Ok(())
    }

    fn get_config(&mut self) -> Result<Option<Map<String, Value>>, LinkLost> {
        let (code, body) = self.http.call("GET", "/api/status", None)?;
        self.password_refused = code == 401;
        if code != 200 || !truthy(&body) {
            return Err(LinkLost(format!("status: HTTP {code}")));
        }
        let (Some(st), Some(Value::Object(config))) = (body.get("state"), body.get("config"))
        else {
            return Err(LinkLost("status: no state/config".into()));
        };
        self.ok_at = self.clock.now();
        self.clock_ntp = st.get("ntp").is_some_and(truthy);
        self.pw_set = st.get("net").and_then(|n| n.get("key_required")).and_then(Value::as_bool);
        self.fw = st.get("fw").and_then(Value::as_str).map(str::to_owned);
        self.protocol = st
            .get("protocol")
            .and_then(Value::as_u64)
            .and_then(|p| u32::try_from(p).ok());
        let config = config.clone();
        let st = st.clone();
        self.check_stream(&st);
        Ok(Some(config))
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
                Some(e) if truthy(e) => e.clone(),
                _ => Value::String(format!("HTTP {code}")),
            }
        };
        Ok(Err(match err {
            Value::Object(_) => err,
            other => per_key(patch, &error_text(&other)),
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

    fn problem(&self) -> Option<String> {
        if self.send_failures >= REOPEN_AFTER {
            let why = self.send_error.as_deref().unwrap_or("unknown error");
            return Some(format!(
                "the clock answers, but this PC can't send it the live display ({why})"
            ));
        }
        self.problem.clone()
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

    fn reopen(&mut self) -> io::Result<()> {
        // (the old one stays if no new one can be had)
        self.0 = Some(UdpSocket::bind("0.0.0.0:0")?);
        Ok(())
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
            let slow = path == "/api/set"
                && body.and_then(Value::as_object).is_some_and(is_slow_patch);
            let req = if slow {
                req.config().timeout_global(Some(SLOW_HTTP_TIMEOUT)).build()
            } else {
                req
            };
            match body {
                Some(b) => req
                    .header("Content-Type", "application/json")
                    .send(b.to_string().as_bytes())
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
