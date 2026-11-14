//! The clock's WiFi address, learned over USB (CFG `_ip`) and used before any mDNS lookup.
//! Port of legacy/python-app/tests/test_clock_ip.py. Finding the clock runs against a fake
//! network ([`FakeNet`]): no test here waits on a dead address or asks the real LAN, except
//! the one marked `#[ignore]`.

#[path = "clocklink_sim/mod.rs"]
mod sim;

use serde_json::{Value, json};
use sim::{Sim, app, set_cfg};
use std::cell::RefCell;
use std::io;
use std::rc::Rc;
use wfd_app::link::{
    Clock, Datagram, HttpApi, LinkLost, Net, RealClock, WifiLink, find_answering_clock_ip_with,
    find_clock_ip_with, open_wifi, open_wifi_with,
};

/// A network where the clock answers at `answering` addresses only; the name lookups give
/// `dns` / `mdns`. Every question asked is recorded in `asked`.
#[derive(Default)]
struct FakeNet {
    dns: Option<&'static str>,
    mdns: Option<&'static str>,
    answering: Vec<&'static str>,
    asked: RefCell<Vec<String>>,
}

impl FakeNet {
    fn asked(&self) -> Vec<String> {
        self.asked.borrow().clone()
    }
}

/// UDP that goes nowhere.
struct NoUdp;

impl Datagram for NoUdp {
    fn send_to(&mut self, _data: &[u8], _ip: &str, _port: u16) -> io::Result<()> {
        Ok(())
    }
}

/// The clock's status page at one address: there, or nothing answering.
struct Status(bool);

impl HttpApi for Status {
    fn call(&mut self, _m: &str, _p: &str, _b: Option<&Value>) -> Result<(u16, Value), LinkLost> {
        if self.0 {
            Ok((200, json!({"state": {"fw": "F"}, "config": {"brightness": 6}})))
        } else {
            Err(LinkLost("timed out".into()))
        }
    }
}

impl Net for FakeNet {
    fn dns(&self, host: &str) -> Option<String> {
        self.asked.borrow_mut().push(format!("dns {host}"));
        self.dns.map(str::to_string)
    }

    fn mdns(&self, name: &str) -> Option<String> {
        self.asked.borrow_mut().push(format!("mdns {name}"));
        self.mdns.map(str::to_string)
    }

    fn answers(&self, ip: &str) -> bool {
        self.asked.borrow_mut().push(format!("answers {ip}"));
        self.answering.contains(&ip)
    }

    fn wifi_link(&self, ip: &str, password: &str, clock: Rc<dyn Clock>) -> io::Result<WifiLink> {
        self.asked.borrow_mut().push(format!("link {ip}"));
        let up = self.answering.contains(&ip);
        Ok(WifiLink::with(ip, password, Box::new(NoUdp), Box::new(Status(up)), clock))
    }
}

fn real_clock() -> Rc<dyn Clock> {
    Rc::new(RealClock)
}

#[test]
fn usb_reports_the_clock_ip() {
    let ctx = app();
    Sim::new(&ctx, 1.0).run();
    let st = ctx.state();
    assert_eq!(st.clock_ip.as_deref(), Some("10.0.0.9"));
    assert!(!st.clock.as_ref().unwrap().contains_key("_ip"), "the address isn't a setting");
}

#[test]
fn clock_off_wifi_leaves_the_ip_alone() {
    let ctx = app();
    let s = Sim::new(&ctx, 1.0);
    s.st.borrow_mut().version.insert("_ip".into(), json!("")); // the clock isn't on WiFi
    s.run();
    assert_eq!(ctx.state().clock_ip, None);
}

#[test]
fn the_reported_address_is_used_without_a_lookup() {
    let ctx = app();
    set_cfg(&ctx, json!({"clock_host": "my-clock.lan"}));
    ctx.state().clock_ip = Some("10.0.0.9".into());
    let net = FakeNet { dns: Some("10.9.9.9"), ..Default::default() };
    assert_eq!(find_clock_ip_with(&ctx, &net).as_deref(), Some("10.0.0.9"));
    assert!(net.asked().is_empty(), "{:?}", net.asked());
}

#[test]
fn a_configured_ip_still_wins() {
    let ctx = app();
    set_cfg(&ctx, json!({"clock_host": "192.168.1.50"}));
    ctx.state().clock_ip = Some("10.0.0.9".into());
    let net = FakeNet::default();
    assert_eq!(find_clock_ip_with(&ctx, &net).as_deref(), Some("192.168.1.50"));
    assert!(net.asked().is_empty());
}

#[test]
fn a_name_is_looked_up_by_dns_then_mdns() {
    let ctx = app();
    let net = FakeNet { mdns: Some("10.0.0.7"), ..Default::default() };
    assert_eq!(find_clock_ip_with(&ctx, &net).as_deref(), Some("10.0.0.7"));
    assert_eq!(net.asked(), ["dns wfd-clock.local", "mdns wfd-clock.local"]);

    let net = FakeNet { dns: Some("10.0.0.8"), mdns: Some("10.0.0.7"), ..Default::default() };
    assert_eq!(find_clock_ip_with(&ctx, &net).as_deref(), Some("10.0.0.8"));
    assert_eq!(net.asked(), ["dns wfd-clock.local"], "DNS answered: no mDNS");

    // only .local names go to mDNS
    set_cfg(&ctx, json!({"clock_host": "my-clock.lan"}));
    let net = FakeNet { mdns: Some("10.0.0.7"), ..Default::default() };
    assert_eq!(find_clock_ip_with(&ctx, &net), None);
    assert_eq!(net.asked(), ["dns my-clock.lan"]);
}

#[test]
fn a_stale_address_is_forgotten() {
    let ctx = app();
    ctx.state().clock_ip = Some("192.0.2.1".into()); // nothing answers there
    let net = FakeNet::default();
    assert!(open_wifi_with(&ctx, real_clock(), &net).is_none());
    assert_eq!(ctx.state().clock_ip, None, "next time: look it up by name");
    assert_eq!(net.asked(), ["link 192.0.2.1"], "a remembered address doesn't fall back to the name");
}

#[test]
fn the_clock_is_connected_and_remembered_where_it_answers() {
    let ctx = app();
    let net = FakeNet { mdns: Some("10.0.0.7"), answering: vec!["10.0.0.7"], ..Default::default() };
    let link = open_wifi_with(&ctx, real_clock(), &net).expect("connected");
    assert_eq!(link.name(), "WiFi 10.0.0.7");
    assert_eq!(ctx.state().clock_ip.as_deref(), Some("10.0.0.7"));
}

#[test]
fn a_typed_ip_that_stopped_answering_falls_back_to_the_name() {
    let ctx = app();
    set_cfg(&ctx, json!({"clock_host": "192.0.2.1"}));
    let net = FakeNet { mdns: Some("10.0.0.7"), answering: vec!["10.0.0.7"], ..Default::default() };
    let link = open_wifi_with(&ctx, real_clock(), &net).expect("found by name");
    assert_eq!(link.name(), "WiFi 10.0.0.7");
    assert_eq!(net.asked(), ["link 192.0.2.1", "mdns wfd-clock.local", "link 10.0.0.7"]);
}

#[test]
fn an_update_skips_addresses_that_do_not_answer() {
    // a typed-in IP and a remembered one, both dead; the clock answers where its name points
    let ctx = app();
    set_cfg(&ctx, json!({"clock_host": "192.0.2.1"}));
    ctx.state().clock_ip = Some("192.0.2.2".into());
    let net = FakeNet { mdns: Some("10.0.0.7"), answering: vec!["10.0.0.7"], ..Default::default() };
    assert_eq!(find_answering_clock_ip_with(&ctx, &net).as_deref(), Some("10.0.0.7"));
    assert_eq!(
        net.asked(),
        ["answers 192.0.2.1", "answers 192.0.2.2", "mdns wfd-clock.local", "answers 10.0.0.7"]
    );

    // and when nothing answers anywhere, there is no address
    let net = FakeNet { mdns: Some("192.0.2.1"), ..Default::default() };
    assert_eq!(find_answering_clock_ip_with(&ctx, &net), None);
    assert_eq!(
        net.asked(),
        ["answers 192.0.2.1", "answers 192.0.2.2", "mdns wfd-clock.local"],
        "an address already tried isn't asked again"
    );
}

/// The same as `a_stale_address_is_forgotten`, over the real network: waits ~3 s on a
/// TEST-NET address nothing answers on. `cargo test --test clock_ip -- --ignored`.
#[test]
#[ignore]
fn a_stale_address_is_forgotten_on_the_real_network() {
    let ctx = app();
    ctx.state().clock_ip = Some("192.0.2.1".into()); // TEST-NET-1: nothing answers there
    assert!(open_wifi(&ctx, real_clock()).is_none());
    assert_eq!(ctx.state().clock_ip, None);
}
