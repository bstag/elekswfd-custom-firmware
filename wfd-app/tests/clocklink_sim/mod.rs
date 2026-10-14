//! Port of wfd-control/tests/conftest.py: a simulated clock (fake serial port answering
//! CFG / CFGSET / GANIM / GCOMMIT, fake UDP socket, fake HTTP API) and simulated time, so
//! ClockLink runs exactly like app.py does under the Python harness. Nothing here touches real
//! hardware, the network or a real config file.
#![allow(dead_code)]

use chrono::{NaiveDate, NaiveDateTime, TimeDelta};
use serde_json::{Map, Value, json};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::io;
use std::rc::Rc;
use std::sync::atomic::Ordering;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};
use wfd_app::clocklink::{ClockLink, Platform};
use wfd_app::ctx::{ClockReply, ClockRequest, Ctx};
use wfd_app::link::{Clock, Datagram, HttpApi, Link, LinkLost, SerialIo, WifiLink};
use wfd_app::metrics::Metrics;
use wfd_app::weather::{Geo, Weather};

/// Simulated epoch for every run.
pub const T0: f64 = 1_760_000_000.0;

pub fn start() -> NaiveDateTime {
    NaiveDate::from_ymd_opt(2026, 10, 13)
        .unwrap()
        .and_hms_opt(16, 0, 0)
        .unwrap()
}

/// What a real clock reports for CFG.
pub fn clock_defaults() -> Map<String, Value> {
    let v = json!({
        "brightness": 6, "clock_24h": true, "style": 4, "units": "fahrenheit",
        "tz": "CST6CDT,M3.2.0,M11.1.0", "location": "76708", "offline_text": "HELLO",
        "msg_standalone": true, "standalone_msg_mode": "text", "standalone_rotate": ["weather", "text", "date"],
        "standalone_dwell": 8, "standalone_scroll_ms": 350, "mic_standalone": true, "grid_standalone": true,
        "standalone_anim": "spin", "standalone_fps": 8, "standalone_frame": false,
    });
    v.as_object().unwrap().clone()
}

/// A fresh app context with default settings (the `app` + `sim` fixtures' inputs).
pub fn app() -> Arc<Ctx> {
    let path = std::env::temp_dir()
        .join("wfd-clocklink-test-does-not-exist")
        .join("wfd_config.json");
    let ctx = Ctx::new(path, None, &[]);
    *ctx.cfg.lock().unwrap() = wfd_app::config::defaults();
    ctx.audio.lock().unwrap().energy = [0.02, 0.004];
    {
        let mut st = ctx.state();
        st.geo = Some(Geo {
            lat: 31.5765,
            lon: -97.1786,
            unit: 0,
        });
        st.weather = Weather {
            code: 1,
            temp: 92,
            unit: 0,
            label: "Waco".into(),
            cond: "Sunny".into(),
        };
        st.metrics = Metrics {
            cpu: 37,
            gpu: 12,
            ram: 61,
            battery: None,
        };
        st.online = true;
    }
    Arc::new(ctx)
}

/// Set app settings like `app.cfg.update(...)`.
pub fn set_cfg(ctx: &Ctx, v: Value) {
    let mut cfg = ctx.cfg.lock().unwrap();
    for (k, x) in v.as_object().unwrap() {
        cfg.insert(k.clone(), x.clone());
    }
}

/// Queue a clock request like the web server does; the receiver gets the link's answer.
pub fn clock_request(ctx: &Ctx, patch: Value) -> mpsc::Receiver<ClockReply> {
    let (tx, rx) = mpsc::channel();
    let patch = patch.as_object().unwrap().clone();
    ctx.clock_requests
        .lock()
        .unwrap()
        .push(ClockRequest { patch, web_password: None, reply: tx });
    rx
}

pub type Event = Box<dyn FnOnce(&Sim)>;

pub struct SimState {
    pub t: f64,
    pub seconds: f64,
    pub unplug_after: Option<usize>,
    pub events: Vec<(f64, Event)>,
    pub lines: Vec<String>,
    pub packets: Vec<(String, Vec<String>)>,
    pub http_calls: Vec<(f64, String, String, Option<Value>)>,
    pub wifi_down: bool,
    pub clock_ntp: bool,
    pub usb_present: bool,
    pub wifi: bool,
    pub password: String,
    pub wifi_opens: usize,
    pub opens: usize,
    pub clock_cfg: Map<String, Value>,
    pub version: Map<String, Value>,
}

/// Drives ClockLink on simulated time. wifi: no USB (until usb_present), the clock answers over
/// FakeUdp / FakeHttp.
#[derive(Clone)]
pub struct Sim {
    pub ctx: Arc<Ctx>,
    pub st: Rc<RefCell<SimState>>,
    base: Instant,
}

impl Sim {
    pub fn new(ctx: &Arc<Ctx>, seconds: f64) -> Sim {
        let version = json!({"_fw": "wfd-clock Oct 13 2026 19:57:32", "_protocol": 3, "_ip": "10.0.0.9"});
        let st = SimState {
            t: T0,
            seconds,
            unplug_after: None,
            events: Vec::new(),
            lines: Vec::new(),
            packets: Vec::new(),
            http_calls: Vec::new(),
            wifi_down: false,
            clock_ntp: true,
            usb_present: true,
            wifi: false,
            password: String::new(),
            wifi_opens: 0,
            opens: 0,
            clock_cfg: clock_defaults(),
            version: version.as_object().unwrap().clone(),
        };
        Sim {
            ctx: Arc::clone(ctx),
            st: Rc::new(RefCell::new(st)),
            base: Instant::now(),
        }
    }

    /// No USB; the clock is on WiFi (with this web password).
    pub fn wifi(self, password: &str) -> Sim {
        {
            let mut s = self.st.borrow_mut();
            s.wifi = true;
            s.usb_present = false;
            s.password = password.into();
        }
        self
    }

    pub fn unplug_after(self, n: usize) -> Sim {
        self.st.borrow_mut().unplug_after = Some(n);
        self
    }

    /// Run `f` once simulated time reaches `at` seconds.
    pub fn event(self, at: f64, f: impl FnOnce(&Sim) + 'static) -> Sim {
        {
            let mut s = self.st.borrow_mut();
            s.events.push((at, Box::new(f)));
            s.events.sort_by(|a, b| a.0.total_cmp(&b.0));
        }
        self
    }

    pub fn now_rel(&self) -> f64 {
        self.st.borrow().t - T0
    }

    /// An `Instant` standing for the current simulated time (for the grid preview stamp).
    pub fn instant_now(&self) -> Instant {
        self.base + Duration::from_secs_f64(self.now_rel())
    }

    fn log(&self, port: &str, line: &str) {
        let now = self.now_rel();
        self.st
            .borrow_mut()
            .lines
            .push(format!("{now:8.3} {port} {line}"));
    }

    pub fn platform(&self) -> Platform {
        let (a, b, c, d) = (self.clone(), self.clone(), self.clone(), self.clone());
        Platform {
            clock: Rc::new(self.clone()),
            foreground_title: Box::new(|| "Visual Studio Code - app.py".to_string()),
            open_serial: Box::new(move || {
                let mut s = a.st.borrow_mut();
                if !s.usb_present {
                    return None;
                }
                s.opens += 1;
                let n = s.opens;
                Some(Box::new(FakeSerial::new(b.clone(), n)) as Box<dyn SerialIo>)
            }),
            find_port: Box::new(move || c.st.borrow().usb_present.then(|| "COM9".to_string())),
            open_wifi: Box::new(move |_ctx| {
                let password = {
                    let mut s = d.st.borrow_mut();
                    if !s.wifi || s.wifi_down {
                        return None; // tests never touch the real network
                    }
                    s.wifi_opens += 1;
                    s.password.clone()
                };
                let link = WifiLink::with(
                    "10.0.0.9",
                    &password,
                    Box::new(FakeUdp(d.clone())),
                    Box::new(FakeHttp(d.clone())),
                    Rc::new(d.clone()),
                );
                Some(Box::new(link) as Box<dyn Link>)
            }),
            grid_seed: 0,
        }
    }

    /// Run ClockLink until the simulated time is up; the lines sent.
    pub fn run(&self) -> Vec<String> {
        self.ctx.stop.store(false, Ordering::Relaxed);
        ClockLink::new(Arc::clone(&self.ctx), self.platform()).run();
        self.lines()
    }

    pub fn lines(&self) -> Vec<String> {
        self.st.borrow().lines.clone()
    }

    /// The text after the timestamp and port of every line sent.
    pub fn sent(&self) -> Vec<String> {
        self.lines()
            .iter()
            .map(|l| l.split_whitespace().skip(2).collect::<Vec<_>>().join(" "))
            .collect()
    }

    /// The command word of every line sent (optionally only on one port).
    pub fn commands(&self, port: Option<&str>) -> Vec<String> {
        self.lines()
            .iter()
            .filter_map(|l| {
                let mut it = l.split_whitespace();
                let (_, p, cmd) = (it.next()?, it.next()?, it.next()?);
                (port.is_none() || port == Some(p)).then(|| cmd.to_string())
            })
            .collect()
    }
}

impl Clock for Sim {
    fn now(&self) -> f64 {
        self.st.borrow().t
    }

    fn sleep(&self, dt: f64) {
        self.st.borrow_mut().t += dt;
        loop {
            let ev = {
                let mut s = self.st.borrow_mut();
                let now = s.t - T0;
                if s.events.first().is_some_and(|e| e.0 <= now) {
                    Some(s.events.remove(0).1)
                } else {
                    None
                }
            };
            match ev {
                Some(f) => f(self),
                None => break,
            }
        }
        let s = self.st.borrow();
        if s.t - T0 > s.seconds {
            self.ctx.stop.store(true, Ordering::Relaxed);
        }
    }

    fn epoch_of(&self, at: Instant) -> f64 {
        // exact: `instant_now` stores t - T0 to the nanosecond, far finer than f64 spacing at T0
        T0 + at.duration_since(self.base).as_secs_f64()
    }

    fn local_now(&self) -> NaiveDateTime {
        // datetime + timedelta(seconds=x) rounds to microseconds
        let us = (self.now_rel() * 1e6).round_ties_even() as i64;
        start() + TimeDelta::microseconds(us)
    }
}

/// Records every line written; answers CFG / CFGSET like the firmware. Can simulate an unplug
/// after N writes on its first connection.
pub struct FakeSerial {
    sim: Sim,
    port: String,
    open: bool,
    writes: usize,
    n: usize,
    replies: VecDeque<String>,
}

impl FakeSerial {
    fn new(sim: Sim, n: usize) -> Self {
        FakeSerial {
            sim,
            port: format!("COM{n}"),
            open: true,
            writes: 0,
            n,
            replies: VecDeque::new(),
        }
    }
}

/// The firmware's check of a settings patch: per-key errors, if any.
fn validate(clock_cfg: &Map<String, Value>, patch: &Map<String, Value>) -> Map<String, Value> {
    let mut bad: Map<String, Value> = patch
        .keys()
        .filter(|k| !clock_cfg.contains_key(*k))
        .map(|k| (k.clone(), json!("unknown setting")))
        .collect();
    if let Some(b) = patch.get("brightness")
        && !b.as_i64().is_some_and(|b| (1..=16).contains(&b))
    {
        bad.insert("brightness".into(), json!("expected an integer 1..16"));
    }
    bad
}

impl SerialIo for FakeSerial {
    fn port_name(&self) -> String {
        self.port.clone()
    }

    fn is_open(&self) -> bool {
        self.open
    }

    fn write(&mut self, data: &[u8]) -> io::Result<()> {
        self.writes += 1;
        if self.n == 1 && Some(self.writes) == self.sim.st.borrow().unplug_after {
            self.open = false;
            return Err(io::Error::other("unplugged"));
        }
        let line = String::from_utf8_lossy(data).trim_end().to_string();
        self.sim.log(&self.port, &line);
        let (cmd, arg) = line.split_once(' ').unwrap_or((&line, ""));
        let mut s = self.sim.st.borrow_mut();
        match cmd {
            "CFG" => {
                let mut all = s.clock_cfg.clone();
                all.extend(s.version.clone());
                self.replies
                    .push_back(format!("CFG {}", Value::Object(all)));
            }
            "CFGSET" => {
                let patch: Map<String, Value> = serde_json::from_str(arg).unwrap();
                let bad = validate(&s.clock_cfg, &patch);
                if bad.is_empty() {
                    s.clock_cfg.extend(patch);
                    self.replies
                        .push_back(format!("CFGOK {}", Value::Object(s.clock_cfg.clone())));
                } else {
                    self.replies
                        .push_back(format!("CFGERR {}", json!({"error": bad})));
                }
            }
            "WEBPASS" => {
                // same rule as the firmware: up to 64 printable chars, no spaces; "" removes it
                if arg.len() <= 64 && arg.bytes().all(|b| b > 32 && b < 127) {
                    s.version.insert("_pw".into(), json!(!arg.is_empty()));
                    self.replies.push_back(format!("PWOK {}", json!({"set": !arg.is_empty()})));
                } else {
                    self.replies.push_back(
                        r#"PWERR {"error":"up to 64 printable characters, no spaces"}"#.into(),
                    );
                }
            }
            "GANIM" | "GCOMMIT" => self.replies.push_back("OK".into()), // unrelated replies to skip
            _ => {}
        }
        Ok(())
    }

    fn readline(&mut self) -> io::Result<Vec<u8>> {
        if let Some(r) = self.replies.pop_front() {
            return Ok(format!("{r}\r\n").into_bytes());
        }
        self.sim.sleep(0.05); // like the real port's read timeout
        Ok(Vec::new())
    }

    fn reset_input_buffer(&mut self) -> io::Result<()> {
        self.replies.clear();
        Ok(())
    }

    fn close(&mut self) {}
}

/// The clock's UDP port: logs each command of each packet like FakeSerial logs lines.
pub struct FakeUdp(Sim);

impl Datagram for FakeUdp {
    fn send_to(&mut self, data: &[u8], _ip: &str, _port: u16) -> io::Result<()> {
        let text = String::from_utf8_lossy(data).to_string();
        let mut parts = text.split('\n');
        let head = parts.next().unwrap_or("").to_string();
        let lines: Vec<String> = parts.map(str::to_string).collect();
        for line in &lines {
            self.0.log("WIFI", line);
        }
        self.0.st.borrow_mut().packets.push((head, lines));
        Ok(())
    }
}

/// The clock's HTTP API over the same simulated settings as FakeSerial.
pub struct FakeHttp(Sim);

impl HttpApi for FakeHttp {
    fn call(
        &mut self,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Result<(u16, Value), LinkLost> {
        let now = (self.0.now_rel() * 1000.0).round() / 1000.0;
        let mut s = self.0.st.borrow_mut();
        s.http_calls
            .push((now, method.into(), path.into(), body.cloned()));
        if s.wifi_down {
            return Err(LinkLost("timed out".into()));
        }
        Ok(match (method, path) {
            ("GET", "/api/status") => (
                200,
                json!({"state": {"ntp": s.clock_ntp}, "config": s.clock_cfg.clone()}),
            ),
            ("POST", "/api/set") => {
                let patch = body.unwrap().as_object().unwrap().clone();
                let bad = validate(&s.clock_cfg, &patch);
                if bad.is_empty() {
                    s.clock_cfg.extend(patch);
                    (200, json!({"ok": true}))
                } else {
                    (400, json!({"error": bad}))
                }
            }
            ("POST", "/api/frames") => {
                let n = body.unwrap()["frames"].as_array().unwrap().len();
                (200, json!({"ok": true, "frames": n}))
            }
            _ => (404, json!({"error": "not found"})),
        })
    }
}
