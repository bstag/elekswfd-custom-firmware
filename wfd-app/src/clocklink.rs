//! clocklink: the ~30 Hz loop that drives the clock (port of `ClockLink` in wfd-control/app.py).
//!
//! Owns the connection (USB first unless set to WiFi), keeps the clock's settings in
//! `state.clock`, answers the web server's clock requests, performs queued flash uploads and
//! pushes everything the clock shows: spectrum, icons, logo, CPU/GPU/RAM bars, weather, the
//! message line and the 7x7 grid.
//!
//! Parity: driven by the simulated harness in tests/clocklink_sim, this produces the same
//! command stream, at the same simulated times, as app.py does under tests/conftest.py (see
//! the golden log). That means keeping the order of every `now()` / `sleep()` and the exact f64
//! arithmetic of the Python code (e.g. `now - last > 1.0 / max(2, fps)`).

use crate::ctx::{Ctx, UploadJob};
use crate::dsp::Levels;
use crate::font14;
use crate::grid_anim::{Extra, GridEngine};
use crate::icons::compute_icons;
use crate::link::{
    self, Clock, Link, LinkLost, RealClock, SerialIo, UsbLink, frame_cmd, per_key, truthy,
};
use crate::state::LevelsOut;
use chrono::{Datelike, NaiveDateTime, Timelike};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::Ordering;

/// One frame (app.py `ClockLink.TICK`).
pub const TICK: f64 = 1.0 / 30.0;

type Cfg = Map<String, Value>;
/// Opens the clock's serial port (app.py `open_serial`).
pub type OpenSerial = Box<dyn Fn() -> Option<Box<dyn SerialIo>>>;
/// Finds and checks the clock on WiFi (app.py `open_wifi`).
pub type OpenWifi = Box<dyn Fn(&Ctx) -> Option<Box<dyn Link>>>;

/// The outside world the loop talks to; `Platform::real()` in the app, fakes in tests.
pub struct Platform {
    pub clock: Rc<dyn Clock>,
    pub foreground_title: Box<dyn Fn() -> String>,
    pub open_serial: OpenSerial,
    /// Is the clock plugged in? (app.py `find_port`, for the auto switch to USB)
    pub find_port: Box<dyn Fn() -> Option<String>>,
    pub open_wifi: OpenWifi,
    pub grid_seed: u64,
}

impl Platform {
    pub fn real() -> Self {
        let clock: Rc<dyn Clock> = Rc::new(RealClock);
        let wifi_clock = Rc::clone(&clock);
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos() as u64);
        Platform {
            clock,
            foreground_title: Box::new(crate::window::foreground_title),
            open_serial: Box::new(|| link::open_serial().map(|s| Box::new(s) as Box<dyn SerialIo>)),
            find_port: Box::new(link::find_port),
            open_wifi: Box::new(move |ctx| link::open_wifi(ctx, Rc::clone(&wifi_clock))),
            grid_seed: seed,
        }
    }
}

/// Drive the clock until `ctx.stop` is set (then say BYE).
pub fn run(ctx: Arc<Ctx>) {
    ClockLink::new(ctx, Platform::real()).run();
}

// cfg accessors with app.py's semantics (`c["key"]`, truthiness, numbers as floats)
fn flag(c: &Cfg, key: &str) -> bool {
    truthy(c.get(key))
}

fn num(c: &Cfg, key: &str) -> f64 {
    match c.get(key) {
        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0),
        Some(Value::Bool(b)) => f64::from(u8::from(*b)),
        _ => 0.0,
    }
}

fn text<'a>(c: &'a Cfg, key: &str) -> &'a str {
    c.get(key).and_then(Value::as_str).unwrap_or("")
}

/// `{temp}C` / `{temp}F`.
fn temp_text(temp: i32, unit: u8) -> String {
    format!("{temp}{}", if unit != 0 { 'C' } else { 'F' })
}

/// `grid_custom` frames as row bytes (each padded / cut to 7 rows).
fn custom_frames(c: &Cfg) -> Vec<[u8; 7]> {
    let Some(Value::Array(frames)) = c.get("grid_custom") else {
        return Vec::new();
    };
    frames
        .iter()
        .filter_map(Value::as_array)
        .map(|f| {
            let mut rows = [0u8; 7];
            for (r, v) in rows.iter_mut().zip(f) {
                *r = (v.as_u64().unwrap_or(0) & 0x7F) as u8;
            }
            rows
        })
        .collect()
}

pub struct ClockLink {
    ctx: Arc<Ctx>,
    p: Platform,
    clock: Rc<dyn Clock>,
    link: Option<Box<dyn Link>>,
    /// command -> last value sent; cleared on (re)connect
    sent: HashMap<&'static str, String>,
    last_sync: f64,
    last_bars: f64,
    last_wx: f64,
    last_scroll: f64,
    last_grid: f64,
    /// audio level auto-gain + fast-rise/slow-fall display
    levels: Levels,
    /// message text currently scrolling
    cur_src: Option<String>,
    scroll_pos: usize,
    rot_idx: i64,
    rot_until: f64,
    last_cfg: f64,
    last_usb_check: f64,
    /// (link, clock_host, clock_pass) the link was made with
    link_cfg: [Value; 3],
    grid: GridEngine,
}

impl ClockLink {
    pub fn new(ctx: Arc<Ctx>, p: Platform) -> Self {
        let clock = Rc::clone(&p.clock);
        let grid = GridEngine::new(p.grid_seed);
        ClockLink {
            ctx,
            p,
            clock,
            link: None,
            sent: HashMap::new(),
            last_sync: 0.0,
            last_bars: 0.0,
            last_wx: 0.0,
            last_scroll: 0.0,
            last_grid: 0.0,
            levels: Levels::new(),
            cur_src: None,
            scroll_pos: 0,
            rot_idx: -1,
            rot_until: 0.0,
            last_cfg: 0.0,
            last_usb_check: 0.0,
            link_cfg: Default::default(),
            grid,
        }
    }

    /// The loop: connect, one frame, flush, sleep a tick; until `ctx.stop`.
    pub fn run(&mut self) {
        while !self.ctx.stopping() {
            if !self.connect() {
                self.clock.sleep(2.0);
                continue;
            }
            let c = self.ctx.cfg();
            // tick() may have dropped the link (settings changed / USB plugged in): no flush then
            let res = self.tick(&c).and_then(|()| match self.link.as_mut() {
                Some(l) => l.flush(),
                None => Ok(()),
            });
            if res.is_err() {
                self.drop_link();
            }
            self.clock.sleep(TICK);
        }
        self.say_bye();
    }

    /// App quitting: tell the clock, so it takes over at once instead of after 2 s.
    fn say_bye(&mut self) {
        if let Some(l) = self.link.as_mut().filter(|l| l.alive()) {
            let _ = l.send("BYE").and_then(|()| l.flush());
        }
    }

    fn drop_link(&mut self) {
        if let Some(mut l) = self.link.take() {
            l.close();
        }
        let mut st = self.ctx.state();
        st.connected = false;
        st.link = None;
    }

    fn link(&mut self) -> &mut dyn Link {
        self.link.as_deref_mut().expect("connected")
    }

    // ---------------------------------------------------------------- plumbing

    /// Ensure a link (USB first unless set to WiFi); on a fresh one, forget what was sent, read
    /// the clock's settings and sync its time.
    fn connect(&mut self) -> bool {
        if self.link.as_ref().is_some_and(|l| l.alive()) {
            return true;
        }
        if self.link.is_some() {
            self.drop_link();
        }
        let mode = {
            let cfg = self.ctx.cfg.lock().unwrap();
            cfg.get("link")
                .and_then(Value::as_str)
                .unwrap_or("auto")
                .to_string()
        };
        let serial = if mode == "auto" || mode == "usb" {
            (self.p.open_serial)()
        } else {
            None
        };
        self.link = match serial {
            Some(s) => Some(Box::new(UsbLink::new(s, Rc::clone(&self.clock)))),
            None if mode == "auto" || mode == "wifi" => (self.p.open_wifi)(&self.ctx),
            None => None,
        };
        {
            let mut st = self.ctx.state();
            st.connected = self.link.is_some();
            st.port = self.link.as_ref().map(|l| l.name());
            st.link = self.link.as_ref().map(|l| l.kind().to_string());
        }
        if self.link.is_none() {
            return false;
        }
        self.link_cfg = {
            let cfg = self.ctx.cfg.lock().unwrap();
            ["link", "clock_host", "clock_pass"].map(|k| cfg.get(k).cloned().unwrap_or_default())
        };
        self.sent.clear(); // a fresh (or power-cycled) clock needs everything again
        // First the clock's settings: they also tell whether it has NTP time.
        // (app.py lets an error here escape and end the thread; here it just drops the link)
        let res = self.read_clock_config().and_then(|()| {
            let sync = truthy(self.ctx.cfg.lock().unwrap().get("time_sync"));
            if sync && !self.link().clock_ntp() {
                self.set_time()
            } else {
                Ok(())
            }
        });
        if res.is_err() {
            self.drop_link();
            return false;
        }
        self.last_sync = self.clock.now();
        true
    }

    fn read_clock_config(&mut self) -> Result<(), LinkLost> {
        let link = self.link();
        let config = link.get_config()?;
        let (fw, protocol) = link.version();
        let ip = link.clock_ip();
        let pw_set = link.web_password_set();
        if let Some(c) = config {
            let mut st = self.ctx.state();
            st.clock = Some(c);
            st.clock_fw = fw;
            st.clock_protocol = protocol;
            st.clock_pw_set = pw_set;
            if ip.is_some() {
                st.clock_ip = ip;
            }
        }
        self.last_cfg = self.clock.now();
        Ok(())
    }

    /// Answer the web server's CFGSET requests (then re-read the clock's settings).
    fn run_clock_requests(&mut self) -> Result<(), LinkLost> {
        let reqs = std::mem::take(&mut *self.ctx.clock_requests.lock().unwrap());
        // On a lost link the rest are dropped unanswered: their senders go away, so
        // Ctx::set_clock reports "clock didn't answer" (app.py: after its 3 s timeout).
        for req in reqs {
            if let Some(pw) = &req.web_password {
                let res = self.link().set_web_password(pw);
                match res {
                    Ok(Ok(set)) => {
                        self.remember_clock_password(pw);
                        self.ctx.state().clock_pw_set = Some(set);
                        let _ = req.reply.send(Ok(None));
                    }
                    Ok(Err(why)) => {
                        let _ = req.reply.send(Err(serde_json::json!({ "password": why })));
                    }
                    Err(e) => {
                        let _ = req.reply.send(Err(serde_json::json!({ "password": "clock didn't answer" })));
                        return Err(e);
                    }
                }
                continue;
            }
            let res = self.link().set_config(&req.patch).and_then(|r| {
                if r.is_ok() {
                    self.read_clock_config()?;
                }
                Ok(r)
            });
            match res {
                Ok(r) => {
                    let _ = req.reply.send(r);
                }
                Err(e) => {
                    let _ = req
                        .reply
                        .send(Err(per_key(&req.patch, "clock didn't answer")));
                    return Err(e);
                }
            }
        }
        Ok(())
    }

    /// After setting the clock's web password, keep the app's `clock_pass` in step, so the WiFi
    /// link and "Update clock" authenticate without the user typing it twice. The link reconnect
    /// that a clock_pass change normally triggers isn't needed here (it's the same clock).
    fn remember_clock_password(&mut self, pw: &str) {
        let cfg = {
            let mut cfg = self.ctx.cfg.lock().unwrap();
            cfg.insert("clock_pass".into(), Value::String(pw.to_string()));
            cfg.clone()
        };
        let _ = crate::config::save(&self.ctx.cfg_path, &cfg);
        self.link_cfg[2] = Value::String(pw.to_string()); // [link, clock_host, clock_pass]
    }

    fn send(&mut self, line: &str) -> Result<(), LinkLost> {
        self.link().send(line)
    }

    /// Send `CMD value` only when value differs from what the clock last got.
    fn push(&mut self, cmd: &'static str, value: String) -> Result<(), LinkLost> {
        if self.sent.get(cmd) != Some(&value) {
            self.send(&format!("{cmd} {value}"))?;
            self.sent.insert(cmd, value);
        }
        Ok(())
    }

    fn set_time(&mut self) -> Result<(), LinkLost> {
        let now = self.clock.local_now();
        self.send(&format!(
            "SET {} {} {} {} {} {} {}",
            now.year() % 100,
            now.month(),
            now.day(),
            now.weekday().number_from_monday(),
            now.hour(),
            now.minute(),
            now.second()
        ))?;
        self.link().after_time()?;
        self.ctx.state().time = Some(now.format("%a %H:%M:%S").to_string());
        Ok(())
    }

    // ---------------------------------------------------------------- one frame

    fn tick(&mut self, c: &Cfg) -> Result<(), LinkLost> {
        let ntp = self.link().clock_ntp();
        let due = flag(c, "time_sync") && self.clock.now() - self.last_sync > 600.0 && !ntp;
        if due || self.ctx.sync_now.load(Ordering::Relaxed) {
            // a WiFi clock keeps its own time (NTP)
            self.set_time()?;
            self.last_sync = self.clock.now();
            self.ctx.sync_now.store(false, Ordering::Relaxed);
        }
        let want =
            ["link", "clock_host", "clock_pass"].map(|k| c.get(k).cloned().unwrap_or_default());
        if want != self.link_cfg {
            // connection settings changed: reconnect if they concern this link
            let kind = self.link().kind();
            let wrong_kind =
                (want[0] == "usb" && kind == "wifi") || (want[0] == "wifi" && kind == "usb");
            if wrong_kind || (kind == "wifi" && want[1..] != self.link_cfg[1..]) {
                self.drop_link();
                return Ok(());
            }
            self.link_cfg = want;
        }
        if self.link().kind() == "wifi"
            && text(c, "link") == "auto"
            && self.clock.now() - self.last_usb_check > 10.0
        {
            self.last_usb_check = self.clock.now(); // plugged in meanwhile? USB is the better link
            if (self.p.find_port)().is_some() {
                self.drop_link();
                return Ok(());
            }
        }
        self.run_clock_requests()?;
        if self.clock.now() - self.last_cfg > 5.0 {
            self.read_clock_config()?; // pick up changes made on the clock itself
        }
        self.push_location()?;
        self.run_upload()?;
        let levels = self.audio_levels(c);
        let vals = if flag(c, "spectrum") { levels } else { [0, 0] };
        self.send(&format!("SPEC {} {}", vals[0], vals[1]))?;
        let icons = {
            let st = self.ctx.state();
            let m = &st.metrics;
            compute_icons(
                text(c, "icon_mode"),
                levels[0],
                levels[1],
                f64::from(m.cpu),
                f64::from(m.gpu),
                f64::from(m.ram),
                st.online,
                st.battery,
                self.clock.now() as u64, // int(time.time())
            )
        };
        self.send(&format!("ICON {icons}"))?;
        self.send(&format!("LOGO {}", u8::from(flag(c, "logo"))))?;
        let now = self.clock.local_now();
        self.ctx.state().time = Some(now.format("%a %H:%M:%S").to_string());
        self.send_bars_weather(c)?;
        self.send_message(c, now)?;
        self.send_grid(c)
    }

    /// Give the clock our weather location only if it has none of its own (no ZIP set).
    fn push_location(&mut self) -> Result<(), LinkLost> {
        if self.link().kind() != "usb" {
            return Ok(());
        }
        let geo = {
            let st = self.ctx.state();
            match (&st.geo, &st.clock) {
                (Some(g), Some(clk)) if !truthy(clk.get("location")) => Some(format!(
                    "{} {} {}",
                    crate::config::py::repr_float(g.lat),
                    crate::config::py::repr_float(g.lon),
                    g.unit
                )),
                _ => None,
            }
        };
        match geo {
            Some(g) => self.push("GEO", g),
            None => Ok(()),
        }
    }

    /// Perform one queued upload to the clock's flash, if any. Either changes the clock's
    /// settings, so they're re-read on the next frame.
    fn run_upload(&mut self) -> Result<(), LinkLost> {
        let job = self.ctx.uploads.lock().unwrap().pop_front();
        let saved = match job {
            None => return Ok(()),
            Some(UploadJob::Anim { name, fps, frame }) => {
                self.link().store_anim(&name, fps, frame)?; // the clock runs it itself
                format!("grid: {name}")
            }
            Some(UploadJob::Frames { frames, fps, frame }) => {
                self.link().store_frames(&frames, fps, frame)?;
                format!("grid: {} frames", frames.len())
            }
        };
        self.ctx.state().saved = saved;
        self.last_cfg = 0.0;
        Ok(())
    }

    /// 0..12 levels for output / mic (always computed: the spectrum and the icon row use them).
    fn audio_levels(&mut self, c: &Cfg) -> [u8; 2] {
        let energy = self.ctx.audio.lock().unwrap().energy;
        let levels = self
            .levels
            .update(energy, [num(c, "gain_out"), num(c, "gain_mic")]);
        self.ctx.state().levels = LevelsOut {
            out: levels[0],
            mic: levels[1],
        };
        levels
    }

    fn send_bars_weather(&mut self, c: &Cfg) -> Result<(), LinkLost> {
        if flag(c, "bars") && self.clock.now() - self.last_bars > 1.0 {
            let m = self.ctx.state().metrics.clone();
            self.send(&format!("BARS {} {} {}", m.cpu, m.gpu, m.ram))?;
            self.last_bars = self.clock.now();
        }
        let w = self.ctx.state().weather.clone();
        // only once real weather has loaded
        if flag(c, "weather") && !w.label.is_empty() && self.clock.now() - self.last_wx > 60.0 {
            self.send(&format!("WX {} {} {}", w.code, w.temp, w.unit))?;
            self.last_wx = self.clock.now();
        }
        Ok(())
    }

    /// Which source the message line shows now, or None to let the firmware show the temp.
    fn message_source(&mut self, c: &Cfg) -> Option<&'static str> {
        match text(c, "message_mode") {
            "window" => return Some("window"),
            "text" => return Some("text"),
            "date" => return Some("date"),
            "rotate" => {}
            _ => return None, // "weather": the firmware draws the temperature itself
        }
        let srcs: Vec<&'static str> = ["weather", "window", "text", "date"]
            .into_iter()
            .filter(|x| flag(c, &format!("rotate_{x}")))
            .collect();
        if srcs.is_empty() {
            return None;
        }
        let n = srcs.len() as i64;
        if self.clock.now() >= self.rot_until {
            self.rot_idx = (self.rot_idx + 1).rem_euclid(n);
            self.rot_until = self.clock.now() + num(c, "rotate_dwell").max(2.0);
            self.cur_src = None;
            self.scroll_pos = 0;
        }
        Some(srcs[self.rot_idx.rem_euclid(n) as usize])
    }

    fn send_message(&mut self, c: &Cfg, now: NaiveDateTime) -> Result<(), LinkLost> {
        let cur = self.message_source(c);
        let Some(cur) = cur else { return Ok(()) };
        if self.clock.now() - self.last_scroll <= num(c, "scroll_ms") / 1000.0 {
            return Ok(());
        }
        let w = self.ctx.state().weather.clone();
        let (frame, message) = if cur == "weather" {
            // rotate's weather slot: temp glyphs from the PC
            let txt = temp_text(w.temp, w.unit);
            (
                font14::render6(&format!("{txt:>6}"), 0),
                format!("temp {txt}"),
            )
        } else {
            let txt = match cur {
                "window" => (self.p.foreground_title)(),
                "date" => now.format("%a %b %d").to_string().to_uppercase(),
                _ => {
                    // custom text with the firmware's placeholders (same order as app.py)
                    let fill = [
                        ("date", now.format("%a %b %d").to_string().to_uppercase()),
                        ("time", now.format("%H:%M").to_string()),
                        (
                            "time12",
                            format!(
                                "{}{}",
                                now.format("%I:%M").to_string().trim_start_matches('0'),
                                now.format("%p")
                            ),
                        ),
                        ("day", now.format("%A").to_string().to_uppercase()),
                        (
                            "temp",
                            if w.label.is_empty() {
                                "--".into()
                            } else {
                                temp_text(w.temp, w.unit)
                            },
                        ),
                    ];
                    fill.iter()
                        .fold(text(c, "custom_text").to_string(), |t, (k, v)| {
                            t.replace(&format!("{{{k}}}"), v)
                        })
                }
            };
            let txt = match txt.trim() {
                "" => " ".to_string(),
                t => t.to_string(),
            };
            if self.cur_src.as_deref() != Some(txt.as_str()) {
                self.cur_src = Some(txt.clone());
                self.scroll_pos = 0;
            }
            let frame = font14::scroll_frame(&txt, self.scroll_pos);
            self.scroll_pos += 1;
            (frame, txt)
        };
        self.ctx.state().message = message;
        self.send(&frame_cmd("MSG", frame))?;
        self.last_scroll = self.clock.now();
        Ok(())
    }

    fn send_grid(&mut self, c: &Cfg) -> Result<(), LinkLost> {
        let since = self.clock.now() - self.last_grid;
        let preview = self.ctx.grid_preview.lock().unwrap().clone();
        let previewing = preview
            .at
            .is_some_and(|at| self.clock.now() - self.clock.epoch_of(at) < 2.0);
        let (enable, framed) = (flag(c, "grid_enable"), flag(c, "grid_frame"));
        if previewing {
            // the drawing tool's live preview wins
            if since > 1.0 / 20.0 {
                let line = frame_cmd(
                    "GRID",
                    preview.rows.iter().chain([&u8::from(preview.frame)]),
                );
                self.send(&line)?;
                self.ctx.state().grid_rows = Some(preview.rows);
                self.last_grid = self.clock.now();
            }
        } else if (enable || framed) && since > 1.0 / num(c, "grid_fps").max(2.0) {
            let rows = if enable {
                let extra = {
                    let st = self.ctx.state();
                    Extra {
                        bands: self.ctx.audio.lock().unwrap().bands,
                        cpu: f64::from(st.metrics.cpu),
                        gpu: f64::from(st.metrics.gpu),
                        ram: f64::from(st.metrics.ram),
                        custom: custom_frames(c),
                    }
                };
                self.grid.frame(text(c, "grid_anim"), &extra)
            } else {
                [0; 7]
            };
            self.send(&frame_cmd("GRID", rows.iter().chain([&u8::from(framed)])))?;
            self.ctx.state().grid_rows = Some(rows);
            self.last_grid = self.clock.now();
        } else if !(enable || framed) && self.ctx.state().grid_rows.is_some() {
            self.ctx.state().grid_rows = None; // app grid off: the clock shows its own
        }
        Ok(())
    }
}
