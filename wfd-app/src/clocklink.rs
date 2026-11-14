//! clocklink: the ~30 Hz loop that drives the clock (port of `ClockLink` in legacy/python-app/app.py).
//!
//! Owns the connection (USB first unless set to WiFi), keeps the clock's settings in
//! `state.clock`, answers the web server's clock requests, performs queued flash uploads and
//! pushes everything the clock shows: spectrum, icons, logo, CPU/GPU/RAM bars, weather, the
//! message line and the 7x7 grid.
//!
//! Regression: driven by the simulated harness in tests/clocklink_sim, this produces the command
//! stream, at the simulated times, recorded in the golden log (tests/fixtures/clocklink_golden.log),
//! a snapshot of this app's behaviour. The log was first taken from app.py under its Python
//! harness, so the code keeps the order of every `now()` / `sleep()` and the exact f64 arithmetic
//! of the Python code (e.g. `now - last > 1.0 / max(2, fps)`); change either only on purpose.

use crate::applog;
use crate::config::{LinkMode, MessageMode, Settings};
use crate::ctx::{Ctx, UploadJob};
use crate::dsp::Levels;
use crate::font14;
use crate::grid_anim::{Extra, GridEngine, scene_from_name};
use crate::icons::compute_icons;
use crate::link::{
    self, Clock, Link, LinkLost, RealClock, SerialIo, UsbLink, frame_cmd, per_key, truthy,
};
use crate::state::LevelsOut;
use chrono::{Datelike, NaiveDateTime, Timelike};
use serde_json::Value;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::Ordering;

/// One frame (app.py `ClockLink.TICK`).
pub const TICK: f64 = 1.0 / 30.0;

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
    /// However the loop ends (also by a panic, after which main starts it again): the state
    /// must not go on saying "connected".
    struct NotConnected(Arc<Ctx>);
    impl Drop for NotConnected {
        fn drop(&mut self) {
            let mut st = self.0.state();
            st.connected = false;
            st.link = None;
        }
    }
    let _reset = NotConnected(Arc::clone(&ctx));
    ClockLink::new(ctx, Platform::real()).run();
}

/// `{temp}C` / `{temp}F`.
fn temp_text(temp: i32, unit: u8) -> String {
    format!("{temp}{}", if unit != 0 { 'C' } else { 'F' })
}

/// The settings a link was made with: a change to them may need a new link.
#[derive(Clone, Debug, Default, PartialEq)]
struct LinkSettings {
    mode: LinkMode,
    host: String,
    pass: String,
}

impl LinkSettings {
    fn of(s: &Settings) -> Self {
        LinkSettings { mode: s.link, host: s.clock_host.clone(), pass: s.clock_pass.clone() }
    }
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
    /// link, clock_host and clock_pass as the link was made with them
    link_cfg: LinkSettings,
    /// a CH340 port that didn't answer like the clock, and when we found out
    not_clock: Option<(String, f64)>,
    grid: GridEngine,
}

impl ClockLink {
    /// The "weather" animation's scene, like the firmware's `currentScene`: the clock's
    /// `weather_scene` pick, else our weather's scene, else the 3-way code with day/night by
    /// the local hour.
    fn weather_scene(&self) -> usize {
        let pick = self.ctx.clock_setting("weather_scene", Value::from("auto"));
        if let Some(i) = pick.as_str().and_then(scene_from_name) {
            return i;
        }
        let (scene, code) = {
            let st = self.ctx.state();
            (scene_from_name(&st.weather.scene), st.weather.code)
        };
        scene.unwrap_or_else(|| {
            let day = (6..20).contains(&chrono::Local::now().hour());
            let name = match code {
                2 => "cloudy",
                3 => "rain",
                _ if day => "clear",
                _ => "night",
            };
            scene_from_name(name).unwrap_or(0)
        })
    }

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
            not_clock: None,
            grid,
        }
    }

    /// The loop: connect, one frame, flush, sleep a tick; until `ctx.stop`.
    pub fn run(&mut self) {
        while !self.ctx.stopping() {
            if self.ctx.state().update.state == "uploading" {
                // "Update clock" is sending firmware: leave the clock alone until it's done
                if self.link.is_some() {
                    self.say_bye();
                    self.drop_link();
                }
                self.clock.sleep(0.5);
                continue;
            }
            if !self.connect() {
                self.clock.sleep(2.0);
                continue;
            }
            let c = self.ctx.settings();
            // tick() may have dropped the link (settings changed / USB plugged in): no flush then
            let res = self.tick(&c).and_then(|()| match self.link.as_mut() {
                Some(l) => l.flush(),
                None => Ok(()),
            });
            if let Err(e) = res {
                let name = self.link.as_ref().map(|l| l.name()).unwrap_or_default();
                applog::log_once(&format!("clock link lost ({name}): {e}"));
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
        st.link_problem = None;
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
        if let Some(l) = &self.link {
            applog::log_once(&format!("clock link dropped ({}): it stopped answering", l.name()));
            self.drop_link();
        }
        let mode = self.ctx.settings_mut().link;
        // In auto mode, a CH340 port that just turned out not to be the clock is left alone
        // for a while (WiFi serves meanwhile) instead of being probed again on every connect.
        let port = (self.p.find_port)();
        let skip_usb = mode == LinkMode::Auto && self.is_not_clock(port.as_deref());
        let serial = if mode != LinkMode::Wifi && !skip_usb {
            (self.p.open_serial)()
        } else {
            None
        };
        if let Some(s) = serial {
            let link: Box<dyn Link> = Box::new(UsbLink::new(s, Rc::clone(&self.clock)));
            match self.start_link(link) {
                Ok(true) => return true,
                Ok(false) => {
                    let name = port.clone().unwrap_or_else(|| self.link().name());
                    self.set_not_clock(name);
                }
                Err(_) => {
                    self.drop_link();
                    return false;
                }
            }
        }
        if mode != LinkMode::Usb
            && let Some(link) = (self.p.open_wifi)(&self.ctx)
        {
            return match self.start_link(link) {
                Ok(_) => true,
                Err(_) => {
                    self.drop_link();
                    false
                }
            };
        }
        self.not_clock = None; // nothing else to use: probe that USB port again next time
        if port.is_none() {
            self.ctx.state().link_problem = None; // nothing plugged in: no USB complaint to show
        }
        if let Some(port) = port.filter(|_| mode != LinkMode::Wifi && self.ctx.state().link_problem.is_none()) {
            // (reached only when the port didn't open: a silent one set its own message above)
            self.ctx.state().link_problem = Some(format!(
                "{port} is the clock's USB port, but another program has it open (a serial monitor, or a second copy of this app)"
            ));
        }
        false
    }

    /// Nothing on `port` speaks our protocol (another board with the same USB chip): drop it,
    /// say so, and leave it alone for a while.
    fn set_not_clock(&mut self, port: String) {
        self.drop_link();
        applog::log_once(&format!("{port} doesn't answer like the clock; leaving it alone for 5 minutes"));
        link::avoid_port(&port);
        self.ctx.state().link_problem = Some(format!(
            "{port} doesn't answer like the clock (another device with the same USB chip, or the stock firmware)"
        ));
        self.not_clock = Some((port, self.clock.now()));
    }

    /// Is `port` the one that recently didn't answer like the clock?
    fn is_not_clock(&self, port: Option<&str>) -> bool {
        match (&self.not_clock, port) {
            (Some((name, at)), Some(port)) => name == port && self.clock.now() - at < 300.0,
            _ => false,
        }
    }

    /// Take `link` into use: forget what was sent, read the clock's settings, sync its time.
    /// Ok(false): the link is open but nothing answered like the clock (USB only).
    fn start_link(&mut self, link: Box<dyn Link>) -> Result<bool, LinkLost> {
        self.link = Some(link);
        {
            let mut st = self.ctx.state();
            st.connected = true;
            st.port = self.link.as_ref().map(|l| l.name());
            st.link = self.link.as_ref().map(|l| l.kind().to_string());
        }
        self.link_cfg = LinkSettings::of(&self.ctx.settings_mut());
        self.sent.clear(); // a fresh (or power-cycled) clock needs everything again
        // First the clock's settings: they also tell whether it has NTP time.
        // (app.py lets an error here escape and end the thread; here it just drops the link)
        let mut answered = self.read_clock_config()?;
        if !answered {
            answered = self.read_clock_config()?; // it may have been busy: once more
        }
        if !answered {
            return Ok(false);
        }
        let sync = self.ctx.settings_mut().time_sync;
        if sync && !self.link().clock_ntp() {
            self.set_time()?;
        }
        self.last_sync = self.clock.now();
        let (name, fw) = (self.link().name(), self.link().version().0.unwrap_or_default());
        applog::log_once(&format!("clock connected: {name} ({fw})"));
        Ok(true)
    }

    /// Read the clock's settings into `state.clock`. Ok(false): it didn't answer (USB).
    fn read_clock_config(&mut self) -> Result<bool, LinkLost> {
        let link = self.link();
        let config = link.get_config()?;
        let (fw, protocol) = link.version();
        let ip = link.clock_ip();
        let pw_set = link.web_password_set();
        let problem = link.problem();
        let answered = config.is_some();
        if let Some(c) = config {
            {
                let mut st = self.ctx.state();
                st.clock = Some(c);
                st.clock_read_at = Some(std::time::Instant::now());
                st.clock_fw = fw;
                st.clock_protocol = protocol;
                st.clock_pw_set = pw_set;
                st.link_problem = problem;
            }
            if let Some(ip) = ip {
                link::remember_ip(&self.ctx, &ip);
            }
        }
        self.last_cfg = self.clock.now();
        Ok(answered)
    }

    /// Answer the web server's CFGSET requests (then re-read the clock's settings).
    fn run_clock_requests(&mut self) -> Result<(), LinkLost> {
        let reqs = std::mem::take(&mut *self.ctx.clock_requests.lock().unwrap_or_else(|e| e.into_inner()));
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
        let settings = {
            let mut s = self.ctx.settings_mut();
            s.clock_pass = pw.to_string();
            s.clone()
        };
        if let Err(e) = crate::config::save(&self.ctx.cfg_path, &settings) {
            crate::applog::log(&format!(
                "can't save the clock password to {}: {e}",
                self.ctx.cfg_path.display()
            ));
        }
        self.link_cfg.pass = pw.to_string();
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

    fn tick(&mut self, c: &Settings) -> Result<(), LinkLost> {
        let ntp = self.link().clock_ntp();
        let due = c.time_sync && self.clock.now() - self.last_sync > 600.0 && !ntp;
        if due || self.ctx.sync_now.load(Ordering::Relaxed) {
            // a WiFi clock keeps its own time (NTP)
            self.set_time()?;
            self.last_sync = self.clock.now();
            self.ctx.sync_now.store(false, Ordering::Relaxed);
        }
        let want = LinkSettings::of(c);
        if want != self.link_cfg {
            // connection settings changed: reconnect if they concern this link
            let kind = self.link().kind();
            let wrong_kind = (want.mode == LinkMode::Usb && kind == "wifi")
                || (want.mode == LinkMode::Wifi && kind == "usb");
            let wifi_target_changed =
                want.host != self.link_cfg.host || want.pass != self.link_cfg.pass;
            if wrong_kind || (kind == "wifi" && wifi_target_changed) {
                applog::log_once("clock link dropped: the connection settings changed");
                self.drop_link();
                return Ok(());
            }
            self.link_cfg = want;
        }
        if self.link().kind() == "wifi"
            && c.link == LinkMode::Auto
            && self.clock.now() - self.last_usb_check > 10.0
        {
            self.last_usb_check = self.clock.now(); // plugged in meanwhile? USB is the better link
            let port = (self.p.find_port)();
            if let Some(port) = port.filter(|p| !self.is_not_clock(Some(p))) {
                // Only leave WiFi once the port has opened: a port another program holds would
                // otherwise cost a reconnect every time we look.
                let Some(serial) = (self.p.open_serial)() else {
                    applog::log_once(&format!("{port} is there but can't be opened (another program has it); staying on WiFi"));
                    self.not_clock = Some((port, self.clock.now())); // look again in a few minutes
                    return Ok(());
                };
                applog::log_once(&format!("clock plugged in on {port}: switching from WiFi to USB"));
                self.drop_link();
                let usb: Box<dyn Link> = Box::new(UsbLink::new(serial, Rc::clone(&self.clock)));
                match self.start_link(usb) {
                    Ok(true) => {}
                    Ok(false) => self.set_not_clock(port),
                    Err(_) => self.drop_link(),
                }
                return Ok(());
            }
        }
        self.run_clock_requests()?;
        if self.clock.now() - self.last_cfg > 5.0 {
            // pick up changes made on the clock itself
            if let Err(e) = self.read_clock_config() {
                // Over WiFi one missed answer isn't a lost clock (it is busy for seconds when it
                // fetches the weather): keep going and ask again soon. The link ends itself when
                // nothing has answered for a while (`alive`).
                if !self.link.as_ref().is_some_and(|l| l.kind() == "wifi" && l.alive()) {
                    return Err(e);
                }
                self.last_cfg = self.clock.now() - 3.0;
            }
        }
        self.push_location()?;
        self.run_upload()?;
        let levels = self.audio_levels(c);
        let vals = if c.spectrum { levels } else { [0, 0] };
        self.send(&format!("SPEC {} {}", vals[0], vals[1]))?;
        let icons = {
            let st = self.ctx.state();
            let m = &st.metrics;
            compute_icons(
                c.icon_mode.as_str(),
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
        self.send(&format!("LOGO {}", u8::from(c.logo)))?;
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
                // `{:?}` keeps a whole number's ".0" (31.0, as the golden log has it)
                (Some(g), Some(clk)) if !truthy(clk.get("location")) => {
                    Some(format!("{:?} {:?} {}", g.lat, g.lon, g.unit))
                }
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
        let job = self.ctx.uploads.lock().unwrap_or_else(|e| e.into_inner()).pop_front();
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
    fn audio_levels(&mut self, c: &Settings) -> [u8; 2] {
        let energy = self.ctx.audio.lock().unwrap_or_else(|e| e.into_inner()).energy;
        let levels = self.levels.update(energy, [c.gain_out, c.gain_mic]);
        self.ctx.state().levels = LevelsOut {
            out: levels[0],
            mic: levels[1],
        };
        levels
    }

    fn send_bars_weather(&mut self, c: &Settings) -> Result<(), LinkLost> {
        if c.bars && self.clock.now() - self.last_bars > 1.0 {
            let m = self.ctx.state().metrics.clone();
            self.send(&format!("BARS {} {} {}", m.cpu, m.gpu, m.ram))?;
            self.last_bars = self.clock.now();
        }
        let w = self.ctx.state().weather.clone();
        // only once real weather has loaded
        if c.weather && !w.label.is_empty() && self.clock.now() - self.last_wx > 60.0 {
            self.send(&format!("WX {} {} {}", w.code, w.temp, w.unit))?;
            self.last_wx = self.clock.now();
        }
        Ok(())
    }

    /// Which source the message line shows now, or None to let the firmware show the temp.
    fn message_source(&mut self, c: &Settings) -> Option<&'static str> {
        match c.message_mode {
            MessageMode::Window => return Some("window"),
            MessageMode::Text => return Some("text"),
            MessageMode::Date => return Some("date"),
            // the forecast once two days are known; until then the firmware shows the temp
            MessageMode::Forecast => {
                return (self.ctx.state().weather.forecast.len() >= 2).then_some("forecast");
            }
            MessageMode::Rotate => {}
            MessageMode::Weather => return None, // the firmware draws the temperature itself
        }
        let srcs: Vec<&'static str> = [
            ("weather", c.rotate_weather),
            ("window", c.rotate_window),
            ("text", c.rotate_text),
            ("date", c.rotate_date),
        ]
        .into_iter()
        .filter_map(|(src, on)| on.then_some(src))
        .collect();
        if srcs.is_empty() {
            return None;
        }
        let n = srcs.len() as i64;
        if self.clock.now() >= self.rot_until {
            self.rot_idx = (self.rot_idx + 1).rem_euclid(n);
            self.rot_until = self.clock.now() + f64::from(c.rotate_dwell).max(2.0);
            self.cur_src = None;
            self.scroll_pos = 0;
        }
        Some(srcs[self.rot_idx.rem_euclid(n) as usize])
    }

    fn send_message(&mut self, c: &Settings, now: NaiveDateTime) -> Result<(), LinkLost> {
        let cur = self.message_source(c);
        let Some(cur) = cur else { return Ok(()) };
        if self.clock.now() - self.last_scroll <= f64::from(c.scroll_ms) / 1000.0 {
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
                // HI / LO / tomorrow, 3 s each (like the clock)
                "forecast" => crate::weather::forecast_text((self.clock.now() / 3.0) as u64, &w.forecast, w.unit)
                    .unwrap_or_default(),
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
                        .fold(c.custom_text.clone(), |t, (k, v)| {
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

    fn send_grid(&mut self, c: &Settings) -> Result<(), LinkLost> {
        let since = self.clock.now() - self.last_grid;
        let preview = self.ctx.grid_preview.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let previewing = preview
            .at
            .is_some_and(|at| self.clock.now() - self.clock.epoch_of(at) < 2.0);
        let (enable, framed) = (c.grid_enable, c.grid_frame);
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
        } else if (enable || framed) && since > 1.0 / f64::from(c.grid_fps).max(2.0) {
            let rows = if enable {
                let scene = self.weather_scene();
                let extra = {
                    let st = self.ctx.state();
                    Extra {
                        bands: self.ctx.audio.lock().unwrap_or_else(|e| e.into_inner()).bands,
                        cpu: f64::from(st.metrics.cpu),
                        gpu: f64::from(st.metrics.gpu),
                        ram: f64::from(st.metrics.ram),
                        custom: c.grid_custom.clone(),
                        scene,
                    }
                };
                self.grid.frame(&c.grid_anim, &extra)
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
