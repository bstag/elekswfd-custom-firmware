//! Setting the clock's web password from the app: USB only (physical access), and the app keeps
//! its own `clock_pass` in step so the WiFi link and "Update clock" still authenticate.

#[path = "clocklink_sim/mod.rs"]
mod sim;

use serde_json::{Map, Value, json};
use sim::{Sim, app};
use std::sync::Arc;
use std::sync::mpsc;
use wfd_app::ctx::{ClockReply, ClockRequest, Ctx};

/// Queue a password change for the link thread (what Ctx::set_web_password does), return the reply.
fn ask(ctx: &Arc<Ctx>, pw: &str) -> mpsc::Receiver<ClockReply> {
    let (tx, rx) = mpsc::channel();
    ctx.clock_requests.lock().unwrap().push(ClockRequest {
        patch: Map::new(),
        web_password: Some(pw.to_string()),
        reply: tx,
    });
    rx
}

fn sent(s: &Sim) -> Vec<String> {
    s.st.borrow().lines.iter().map(|l| l.split_whitespace().skip(2).collect::<Vec<_>>().join(" ")).collect()
}

#[test]
fn set_over_usb_and_the_app_remembers_it() {
    // a real settings file, so we can check the password is saved
    let dir = std::env::temp_dir().join(format!("wfd-pw-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let ctx = Arc::new(Ctx::new(dir.join("wfd_config.json"), None, &[]));
    let s = Sim::new(&ctx, 1.0);
    let rx = ask(&ctx, "hunter2");
    s.run();
    assert_eq!(rx.try_recv().unwrap(), Ok(None));
    assert!(sent(&s).contains(&"WEBPASS hunter2".to_string()));
    assert_eq!(ctx.state().clock_pw_set, Some(true));
    assert_eq!(ctx.cfg()["clock_pass"], json!("hunter2"), "WiFi and updates keep working");
    let saved: Value = serde_json::from_str(&std::fs::read_to_string(&ctx.cfg_path).unwrap()).unwrap();
    assert_eq!(saved["clock_pass"], json!("hunter2"), "and it's saved");
}

#[test]
fn remove_it() {
    let ctx = app();
    let s = Sim::new(&ctx, 1.0);
    s.st.borrow_mut().version.insert("_pw".into(), json!(true));
    let rx = ask(&ctx, "");
    s.run();
    assert_eq!(rx.try_recv().unwrap(), Ok(None));
    assert_eq!(ctx.state().clock_pw_set, Some(false));
    assert_eq!(ctx.cfg()["clock_pass"], json!(""));
}

#[test]
fn the_clock_rejects_a_bad_one_and_nothing_changes() {
    let ctx = app();
    let s = Sim::new(&ctx, 1.0);
    let rx = ask(&ctx, "has space");
    s.run();
    let err = rx.try_recv().unwrap().unwrap_err();
    assert!(err["password"].as_str().unwrap().contains("no spaces"), "{err}");
    assert_eq!(ctx.cfg()["clock_pass"], json!(""), "app setting untouched");
}

#[test]
fn reported_over_usb() {
    let ctx = app();
    let s = Sim::new(&ctx, 1.0);
    s.st.borrow_mut().version.insert("_pw".into(), json!(true));
    s.run();
    assert_eq!(ctx.state().clock_pw_set, Some(true));
    assert!(!ctx.state().clock.as_ref().unwrap().contains_key("_pw"), "not a setting");
}

#[test]
fn not_over_wifi() {
    let ctx = app();
    let s = Sim::new(&ctx, 1.0).wifi("");
    let rx = ask(&ctx, "hunter2");
    s.run();
    let err = rx.try_recv().unwrap().unwrap_err();
    assert!(err["password"].as_str().unwrap().contains("USB"), "{err}");
    assert!(!sent(&s).iter().any(|l| l.starts_with("WEBPASS")), "never sent over WiFi");
}

#[test]
fn ctx_refuses_before_queueing_when_not_on_usb() {
    let ctx = app();
    {
        let mut st = ctx.state();
        st.connected = true;
        st.link = Some("wifi".into());
    }
    let err = ctx.set_web_password("hunter2", std::time::Duration::from_millis(50)).unwrap_err();
    assert!(err["password"].as_str().unwrap().contains("USB"));
    assert!(ctx.clock_requests.lock().unwrap().is_empty());
}
