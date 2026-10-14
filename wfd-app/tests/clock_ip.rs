//! The clock's WiFi address, learned over USB (CFG `_ip`) and used before any mDNS lookup.
//! Port of wfd-control/tests/test_clock_ip.py.

#[path = "clocklink_sim/mod.rs"]
mod sim;

use serde_json::json;
use sim::{Sim, app, set_cfg};
use std::rc::Rc;
use wfd_app::link::{RealClock, find_clock_ip, open_wifi};

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
    // a name that no lookup can resolve: getting 10.0.0.9 back proves none was needed
    set_cfg(&ctx, json!({"clock_host": "no-such-clock.invalid"}));
    ctx.state().clock_ip = Some("10.0.0.9".into());
    assert_eq!(find_clock_ip(&ctx).as_deref(), Some("10.0.0.9"));
}

#[test]
fn a_configured_ip_still_wins() {
    let ctx = app();
    set_cfg(&ctx, json!({"clock_host": "192.168.1.50"}));
    ctx.state().clock_ip = Some("10.0.0.9".into());
    assert_eq!(find_clock_ip(&ctx).as_deref(), Some("192.168.1.50"));
}

#[test]
fn a_stale_address_is_forgotten() {
    let ctx = app();
    ctx.state().clock_ip = Some("192.0.2.1".into()); // TEST-NET-1: nothing answers there
    assert!(open_wifi(&ctx, Rc::new(RealClock)).is_none());
    assert_eq!(ctx.state().clock_ip, None, "next time: look it up by name");
}
