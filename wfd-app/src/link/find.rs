//! Finding the clock: its serial port ([`find_port`], [`avoid_port`]) and its WiFi address
//! ([`find_clock_ip`], [`find_answering_clock_ip`], [`open_wifi`], [`remember_ip`],
//! [`load_last_ip`]).
//!
//! The WiFi side reaches the network only through [`Net`] (name lookups, "does the clock answer
//! here", opening a link), so tests run the logic against fakes; the plain functions use
//! [`RealNet`].

use super::{CH340_PID, CH340_VID, Clock, HTTP_TIMEOUT, HttpApi, Link, MDNS_TIMEOUT, UreqHttp, WifiLink};
use crate::ctx::Ctx;
use serde_json::Value;
use std::io;
use std::net::ToSocketAddrs;
use std::rc::Rc;

/// The clock's name on the network (its firmware's mDNS hostname).
const CLOCK_NAME: &str = "wfd-clock.local";

/// A CH340 port that didn't answer like the clock (see [`avoid_port`]).
static AVOID: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// `name` has a CH340 chip but isn't the clock (plenty of boards use that chip): from now on
/// [`find_port`] prefers any other CH340 port, and only falls back to this one.
pub fn avoid_port(name: &str) {
    *AVOID.lock().unwrap_or_else(|e| e.into_inner()) = Some(name.to_string());
}

/// The clock's serial port (CH340), if plugged in.
pub fn find_port() -> Option<String> {
    let ports: Vec<String> = serialport::available_ports()
        .ok()?
        .into_iter()
        .filter_map(|p| match p.port_type {
            serialport::SerialPortType::UsbPort(u) if u.vid == CH340_VID && u.pid == CH340_PID => {
                Some(p.port_name)
            }
            _ => None,
        })
        .collect();
    let avoid = AVOID.lock().unwrap_or_else(|e| e.into_inner()).clone();
    ports
        .iter()
        .find(|p| Some(*p) != avoid.as_ref())
        .or(ports.first())
        .cloned()
}

// ------------------------------------------------------------------ the network seam

/// The network as finding the clock on WiFi uses it. [`RealNet`] in the app; tests use fakes,
/// so they neither wait on dead addresses nor depend on what answers on the LAN.
pub trait Net {
    /// `host` looked up the normal way (DNS, or a `.local` name where Windows resolves it).
    fn dns(&self, host: &str) -> Option<String>;
    /// A `.local` name asked for over mDNS (see [`crate::mdns::resolve`]).
    fn mdns(&self, name: &str) -> Option<String>;
    /// Does the clock's web API answer at `ip`?
    fn answers(&self, ip: &str) -> bool;
    /// A WiFi link to `ip`, not yet checked (the caller reads the clock's settings over it).
    fn wifi_link(&self, ip: &str, password: &str, clock: Rc<dyn Clock>) -> io::Result<WifiLink>;
}

/// The real network: the system resolver, our own mDNS query, ureq and a UDP socket.
pub struct RealNet;

impl Net for RealNet {
    fn dns(&self, host: &str) -> Option<String> {
        let addrs = (host, 0).to_socket_addrs().ok()?;
        addrs.into_iter().find(|a| a.is_ipv4()).map(|a| a.ip().to_string())
    }

    fn mdns(&self, name: &str) -> Option<String> {
        crate::mdns::resolve(name, MDNS_TIMEOUT).map(|ip| ip.to_string())
    }

    fn answers(&self, ip: &str) -> bool {
        // (its status page needs no password)
        UreqHttp::new(ip, "", HTTP_TIMEOUT)
            .call("GET", "/api/status", None)
            .is_ok_and(|(code, body)| code == 200 && body.get("state").is_some())
    }

    fn wifi_link(&self, ip: &str, password: &str, clock: Rc<dyn Clock>) -> io::Result<WifiLink> {
        WifiLink::new(ip, password, clock)
    }
}

// ------------------------------------------------------------------ the clock's address

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
    find_clock_ip_with(ctx, &RealNet)
}

/// [`find_clock_ip`] over the given network.
pub fn find_clock_ip_with(ctx: &Ctx, net: &dyn Net) -> Option<String> {
    let host = {
        let cfg = ctx.cfg.lock().unwrap_or_else(|e| e.into_inner());
        cfg.get("clock_host")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string()
    };
    let host = if host.is_empty() {
        CLOCK_NAME.to_string()
    } else {
        host
    };
    if is_dotted_quad(&host) {
        return Some(host);
    }
    if let Some(ip) = ctx.state().clock_ip.clone() {
        return Some(ip); // open_wifi forgets it if it stops answering, then we look it up again
    }
    if let Some(ip) = net.dns(&host) {
        return Some(ip);
    }
    let ip = if host.ends_with(".local") {
        net.mdns(&host)
    } else {
        None
    };
    ip.or_else(|| ctx.state().clock_ip.clone())
}

/// Where the last address the clock answered on is kept between runs (next to the settings).
fn last_ip_path(ctx: &Ctx) -> std::path::PathBuf {
    ctx.cfg_path.with_file_name("clock-ip.txt")
}

/// The address saved by [`remember_ip`] in an earlier run, if any.
pub fn load_last_ip(ctx: &Ctx) -> Option<String> {
    let text = std::fs::read_to_string(last_ip_path(ctx)).ok()?;
    let ip = text.trim();
    is_dotted_quad(ip).then(|| ip.to_string())
}

/// The clock is at `ip` (it answered there, or said so over USB): use it first from now on,
/// also after a restart of the app.
pub fn remember_ip(ctx: &Ctx, ip: &str) {
    let changed = {
        let mut st = ctx.state();
        let changed = st.clock_ip.as_deref() != Some(ip);
        st.clock_ip = Some(ip.to_string());
        changed
    };
    if changed && load_last_ip(ctx).as_deref() != Some(ip) {
        let _ = std::fs::write(last_ip_path(ctx), ip);
    }
}

/// The clock's address for "Update clock": like [`find_clock_ip`], but only an address the
/// clock answers on. A typed-in IP that has gone stale (the router moved the clock) gives way
/// to the address the clock itself reported or last answered on, then to a lookup by name.
pub fn find_answering_clock_ip(ctx: &Ctx) -> Option<String> {
    find_answering_clock_ip_with(ctx, &RealNet)
}

/// [`find_answering_clock_ip`] over the given network.
pub fn find_answering_clock_ip_with(ctx: &Ctx, net: &dyn Net) -> Option<String> {
    let mut tried: Vec<String> = Vec::new();
    let known = ctx.state().clock_ip.clone();
    for ip in [find_clock_ip_with(ctx, net), known].into_iter().flatten() {
        if tried.contains(&ip) {
            continue;
        }
        if net.answers(&ip) {
            return Some(ip);
        }
        tried.push(ip);
    }
    let found = net.mdns(CLOCK_NAME)?;
    (!tried.contains(&found) && net.answers(&found)).then_some(found)
}

fn try_wifi(
    ctx: &Ctx,
    net: &dyn Net,
    ip: &str,
    pw: &str,
    clock: &Rc<dyn Clock>,
) -> Option<Box<dyn Link>> {
    let mut link = match net.wifi_link(ip, pw, Rc::clone(clock)) {
        Ok(link) => link,
        Err(e) => {
            crate::applog::log_once(&format!("can't open a UDP socket to reach the clock: {e}"));
            return None;
        }
    };
    if link.get_config().is_err() {
        link.close();
        return None;
    }
    remember_ip(ctx, ip);
    Some(Box::new(link))
}

/// Connect over WiFi: find the clock, check it answers (reads its settings), remember its IP.
pub fn open_wifi(ctx: &Ctx, clock: Rc<dyn Clock>) -> Option<Box<dyn Link>> {
    open_wifi_with(ctx, clock, &RealNet)
}

/// [`open_wifi`] over the given network.
pub fn open_wifi_with(ctx: &Ctx, clock: Rc<dyn Clock>, net: &dyn Net) -> Option<Box<dyn Link>> {
    let ip = find_clock_ip_with(ctx, net)?;
    let (pw, host) = {
        let cfg = ctx.cfg.lock().unwrap_or_else(|e| e.into_inner());
        let text = |k: &str| cfg.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        (text("clock_pass"), text("clock_host").trim().to_string())
    };
    if let Some(link) = try_wifi(ctx, net, &ip, &pw, &clock) {
        return Some(link);
    }
    {
        let mut st = ctx.state();
        if st.clock_ip.as_deref() == Some(ip.as_str()) {
            st.clock_ip = None; // stale: look it up by name next time
        }
    }
    // A typed-in IP that doesn't answer: the router may have given the clock another address.
    // Ask for it by name before giving up.
    if host == ip {
        let found = net.mdns(CLOCK_NAME);
        if let Some(found) = found.filter(|f| *f != ip) {
            return try_wifi(ctx, net, &found, &pw, &clock);
        }
    }
    None
}
