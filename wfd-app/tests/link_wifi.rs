//! The app driving the clock over WiFi (WifiLink): UDP for the live display, HTTP for the rest.
//! Port of wfd-control/tests/test_wifi_link.py, plus the real HTTP seam against a local server.

#[path = "clocklink_sim/mod.rs"]
mod sim;

use serde_json::{Value, json};
use sim::{Sim, app, clock_request, set_cfg};
use std::collections::BTreeSet;
use std::path::Path;
use std::time::Duration;
use wfd_app::ctx::UploadJob;
use wfd_app::link::{HttpApi, LinkLost, UreqHttp};

fn sent_cmds(s: &Sim) -> Vec<String> {
    s.commands(None)
}

#[test]
fn one_packet_per_frame_with_the_header() {
    let ctx = app();
    let s = Sim::new(&ctx, 3.0).wifi("");
    s.run();
    assert_eq!(ctx.state().port.as_deref(), Some("WiFi 10.0.0.9"));
    let st = s.st.borrow();
    let n = st.packets.len();
    assert!((80..=95).contains(&n), "about 30 packets a second, got {n}");
    assert!(st.packets.iter().all(|(h, _)| h == "WFD1"));
    let first: BTreeSet<&str> = st.packets[0]
        .1
        .iter()
        .map(|l| l.split(' ').next().unwrap())
        .collect();
    assert!(
        ["SPEC", "ICON", "LOGO"].iter().all(|c| first.contains(c)),
        "a frame's commands travel together"
    );
    assert!(
        st.packets
            .iter()
            .all(|(h, ls)| h.len() + ls.iter().map(|l| l.len() + 1).sum::<usize>() < 1400)
    );
}

#[test]
fn password_goes_in_the_header() {
    let ctx = app();
    let s = Sim::new(&ctx, 1.0).wifi("hunter2");
    s.run();
    let heads: BTreeSet<String> =
        s.st.borrow()
            .packets
            .iter()
            .map(|(h, _)| h.clone())
            .collect();
    assert_eq!(heads, BTreeSet::from(["WFD1 hunter2".to_string()]));
}

#[test]
fn only_display_commands_go_over_udp() {
    let ctx = app();
    ctx.state().geo = Some(wfd_app::weather::Geo {
        lat: 1.0,
        lon: 2.0,
        unit: 0,
    });
    let s = Sim::new(&ctx, 12.0).wifi("");
    s.st.borrow_mut()
        .clock_cfg
        .insert("location".into(), json!("")); // would get GEO over USB
    s.run();
    // read from the firmware's netPoll(): static const char *ALLOWED[] = {"SPEC", ...};
    let main = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../wfd-firmware/clock/src/main.cpp"),
    )
    .unwrap();
    let start = main.find("ALLOWED[] = {").unwrap();
    let list = &main[start..start + main[start..].find('}').unwrap()];
    let allowed: BTreeSet<&str> = list.split('"').skip(1).step_by(2).collect();
    assert!(["SPEC", "GRID", "BYE"].iter().all(|c| allowed.contains(c)));
    let cmds = sent_cmds(&s);
    assert!(
        cmds.iter().all(|c| allowed.contains(c.as_str())),
        "{cmds:?}"
    );
    assert!(
        !cmds.contains(&"SET".to_string()),
        "a WiFi clock with NTP keeps its own time"
    );
}

#[test]
fn time_is_sent_if_the_clock_has_no_internet_time() {
    let ctx = app();
    let s = Sim::new(&ctx, 2.0).wifi("");
    s.st.borrow_mut().clock_ntp = false;
    s.run();
    assert!(sent_cmds(&s).contains(&"SET".to_string()));
}

#[test]
fn settings_and_health_use_http() {
    let ctx = app();
    let replies = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let r2 = std::rc::Rc::clone(&replies);
    let s = Sim::new(&ctx, 12.0).wifi("").event(2.0, move |s| {
        r2.borrow_mut()
            .push(clock_request(&s.ctx, json!({"brightness": 9})));
        r2.borrow_mut()
            .push(clock_request(&s.ctx, json!({"brightness": 99})));
    });
    s.run();
    let replies = replies.borrow();
    assert_eq!(replies[0].try_recv().unwrap(), Ok(None));
    assert_eq!(ctx.state().clock.as_ref().unwrap()["brightness"], 9);
    assert_eq!(
        replies[1].try_recv().unwrap(),
        Err(json!({"brightness": "expected an integer 1..16"}))
    );
    let gets =
        s.st.borrow()
            .http_calls
            .iter()
            .filter(|c| c.1 == "GET" && c.2 == "/api/status")
            .count();
    assert!(
        gets >= 3,
        "settings re-read on connect, every 5 s, after a change"
    );
}

#[test]
fn uploads_use_http() {
    let ctx = app();
    {
        let mut up = ctx.uploads.lock().unwrap();
        up.push_back(UploadJob::Anim {
            name: "rain".into(),
            fps: 9,
            frame: true,
        });
        up.push_back(UploadJob::Frames {
            frames: vec![[1; 7], [2; 7]],
            fps: 5,
            frame: false,
        });
    }
    let s = Sim::new(&ctx, 1.0).wifi("");
    s.run();
    let posts: Vec<(String, Value)> =
        s.st.borrow()
            .http_calls
            .iter()
            .filter(|c| c.1 == "POST")
            .map(|c| (c.2.clone(), c.3.clone().unwrap()))
            .collect();
    assert!(posts.contains(&(
        "/api/set".into(),
        json!({"standalone_anim": "rain", "standalone_fps": 9, "standalone_frame": true, "grid_standalone": true})
    )));
    assert!(posts.contains(&(
        "/api/frames".into(),
        json!({"frames": [[1, 1, 1, 1, 1, 1, 1], [2, 2, 2, 2, 2, 2, 2]], "fps": 5, "frame": false})
    )));
    assert!(!sent_cmds(&s).contains(&"GCLR".to_string()));
}

#[test]
fn reconnects_after_the_clock_drops_off() {
    let ctx = app();
    let s = Sim::new(&ctx, 40.0)
        .wifi("")
        .event(5.0, |s| s.st.borrow_mut().wifi_down = true)
        .event(25.0, |s| s.st.borrow_mut().wifi_down = false);
    s.run();
    assert_eq!(s.st.borrow().wifi_opens, 2);
    let st = ctx.state();
    assert!(st.connected && st.link.as_deref() == Some("wifi"));
    let late = s.lines().iter().any(|l| {
        let t: f64 = l.split_whitespace().next().unwrap().parse().unwrap();
        26.0 < t && t < 40.0
    });
    assert!(late, "streaming again after the clock came back");
}

#[test]
fn auto_switches_to_usb_when_plugged_in() {
    let ctx = app();
    let s = Sim::new(&ctx, 20.0)
        .wifi("")
        .event(3.0, |s| s.st.borrow_mut().usb_present = true);
    s.run();
    assert_eq!(ctx.state().link.as_deref(), Some("usb"));
    let first_usb = s
        .lines()
        .into_iter()
        .find(|l| l.contains(" COM"))
        .expect("USB lines");
    let t: f64 = first_usb
        .split_whitespace()
        .next()
        .unwrap()
        .parse()
        .unwrap();
    assert!(t < 3.0 + 11.0, "switches within ~10 s");
}

#[test]
fn wifi_mode_ignores_usb() {
    let ctx = app();
    set_cfg(&ctx, json!({"link": "wifi"}));
    let s = Sim::new(&ctx, 15.0).wifi("");
    s.st.borrow_mut().usb_present = true;
    s.run();
    assert_eq!(ctx.state().link.as_deref(), Some("wifi"));
    assert_eq!(s.st.borrow().opens, 0);
}

#[test]
fn bye_when_the_app_stops() {
    let ctx = app();
    let s = Sim::new(&ctx, 1.0).wifi("");
    s.run();
    assert_eq!(s.st.borrow().packets.last().unwrap().1, ["BYE"]);
}

#[test]
fn switching_to_wifi_takes_effect_at_once() {
    let ctx = app();
    let s = Sim::new(&ctx, 4.0)
        .wifi("")
        .event(2.0, |s| set_cfg(&s.ctx, json!({"link": "wifi"})));
    s.st.borrow_mut().usb_present = true; // starts on USB
    s.run();
    assert_eq!(ctx.state().link.as_deref(), Some("wifi"));
    let first_wifi = s
        .lines()
        .into_iter()
        .find(|l| l.contains(" WIFI "))
        .unwrap();
    let t: f64 = first_wifi
        .split_whitespace()
        .next()
        .unwrap()
        .parse()
        .unwrap();
    assert!((2.0..2.5).contains(&t), "{t}");
}

#[test]
fn changing_the_clock_password_reconnects() {
    let ctx = app();
    let s = Sim::new(&ctx, 3.0)
        .wifi("")
        .event(1.0, |s| set_cfg(&s.ctx, json!({"clock_pass": "new"})));
    s.run();
    assert_eq!(s.st.borrow().wifi_opens, 2);
}

// ------------------------------------------------------------------ the real HTTP seam

struct Seen {
    method: String,
    url: String,
    headers: Vec<(String, String)>,
    body: String,
}

/// A one-request HTTP server on localhost answering `status` / `reply`.
fn serve_once(status: u16, reply: &'static str) -> (String, std::thread::JoinHandle<Seen>) {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let addr = format!("127.0.0.1:{}", server.server_addr().to_ip().unwrap().port());
    let h = std::thread::spawn(move || {
        let mut req = server.recv().unwrap();
        let mut body = String::new();
        req.as_reader().read_to_string(&mut body).unwrap();
        let seen = Seen {
            method: req.method().to_string(),
            url: req.url().to_string(),
            headers: req
                .headers()
                .iter()
                .map(|h| {
                    (
                        h.field.to_string().to_ascii_lowercase(),
                        h.value.to_string(),
                    )
                })
                .collect(),
            body,
        };
        req.respond(tiny_http::Response::from_string(reply).with_status_code(status))
            .unwrap();
        seen
    });
    (addr, h)
}

fn header<'a>(seen: &'a Seen, name: &str) -> Option<&'a str> {
    seen.headers
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.as_str())
}

#[test]
fn http_sends_the_marker_auth_and_json() {
    let (addr, h) = serve_once(200, r#"{"ok":true}"#);
    let mut http = UreqHttp::new(&addr, "hunter2", Duration::from_secs(3));
    let got = http
        .call(
            "POST",
            "/api/set",
            Some(&json!({"offline_text": "caf\u{e9}"})),
        )
        .unwrap();
    assert_eq!(got, (200, json!({"ok": true})));
    let seen = h.join().unwrap();
    assert_eq!(
        (seen.method.as_str(), seen.url.as_str()),
        ("POST", "/api/set")
    );
    assert_eq!(header(&seen, "x-wfd"), Some("1"));
    assert_eq!(
        header(&seen, "authorization"),
        Some("Basic YWRtaW46aHVudGVyMg==")
    );
    assert_eq!(header(&seen, "content-type"), Some("application/json"));
    assert_eq!(
        seen.body,
        [r#"{"offline_text":"caf"#, "\\", r#"u00e9"}"#].concat(),
        "ASCII-only JSON like Python's"
    );
}

#[test]
fn http_errors_come_back_with_their_body() {
    let (addr, h) = serve_once(400, r#"{"error":{"brightness":"bad"}}"#);
    let mut http = UreqHttp::new(&addr, "", Duration::from_secs(3));
    let got = http.call("GET", "/api/status", None).unwrap();
    assert_eq!(got, (400, json!({"error": {"brightness": "bad"}})));
    let seen = h.join().unwrap();
    assert_eq!(
        header(&seen, "authorization"),
        None,
        "no password, no auth header"
    );
    assert_eq!(header(&seen, "content-type"), None);

    let (addr, h) = serve_once(401, "not json");
    let mut http = UreqHttp::new(&addr, "x", Duration::from_secs(3));
    assert_eq!(
        http.call("GET", "/api/status", None).unwrap(),
        (401, Value::Null)
    );
    h.join().unwrap();
}

#[test]
fn http_garbage_or_no_answer_is_link_lost() {
    let (addr, h) = serve_once(200, "not json");
    let mut http = UreqHttp::new(&addr, "", Duration::from_secs(3));
    assert!(http.call("GET", "/api/status", None).is_err());
    h.join().unwrap();

    // nothing listening there any more
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let mut http = UreqHttp::new(&format!("127.0.0.1:{port}"), "", Duration::from_secs(3));
    let err: LinkLost = http.call("GET", "/api/status", None).unwrap_err();
    assert!(!err.0.is_empty());
}
