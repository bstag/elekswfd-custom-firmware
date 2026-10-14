//! "Update clock" (OTA from the app): port of the update tests in
//! wfd-control/tests/test_firmware_update.py, plus the real upload / status poll against a
//! local HTTP server.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use wfd_app::ctx::Ctx;
use wfd_app::ota::{OtaError, OtaHooks, update_clock_with, upload_firmware, wait_for_build};
use wfd_app::release::Release;
use wfd_app::state::Update;

const FIRMWARE: &[u8] = b"\xe9firmware";

fn ctx_with_release() -> Arc<Ctx> {
    let path = std::env::temp_dir()
        .join("wfd-ota-test-does-not-exist")
        .join("wfd_config.json");
    let release = Release {
        fw: "wfd-clock NEW".into(),
        protocol: 3,
    };
    Arc::new(Ctx::new(path, Some(release), FIRMWARE))
}

fn wait_update(ctx: &Ctx) -> Update {
    let end = Instant::now() + Duration::from_secs(3);
    while ctx.state().update.state == "uploading" && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(5));
    }
    ctx.state().update.clone()
}

type Calls = Arc<Mutex<Vec<(String, Vec<u8>, String)>>>;

fn hooks(
    ip: Option<&'static str>,
    upload: impl Fn() -> Result<(), OtaError> + Send + 'static,
    built: impl Fn(&str, &str) -> bool + Send + 'static,
) -> (OtaHooks, Calls) {
    let calls: Calls = Arc::default();
    let c2 = Arc::clone(&calls);
    let h = OtaHooks {
        find_clock_ip: Box::new(move |_| ip.map(str::to_string)),
        upload_firmware: Box::new(move |ip, data, pw| {
            c2.lock()
                .unwrap()
                .push((ip.into(), data.to_vec(), pw.into()));
            upload()
        }),
        wait_for_build: Box::new(move |ip, fw, _| built(ip, fw)),
        clock_report: Box::new(|_, _| None),
        log: Box::new(|_, _| {}),
    };
    (h, calls)
}

type Logged = Arc<Mutex<Vec<serde_json::Value>>>;

/// Hooks whose attempts are logged into a Vec, with a clock that reports `report` each time.
fn logged_hooks(
    upload: impl Fn() -> Result<(), OtaError> + Send + 'static,
    built: impl Fn() -> bool + Send + 'static,
    report: serde_json::Value,
) -> (OtaHooks, Logged) {
    let logged: Logged = Arc::default();
    let l2 = Arc::clone(&logged);
    let h = OtaHooks {
        find_clock_ip: Box::new(|_| Some("10.0.0.9".into())),
        upload_firmware: Box::new(move |_, _, _| upload()),
        wait_for_build: Box::new(move |_, _, _| built()),
        clock_report: Box::new(move |ip, _| (ip == "10.0.0.9").then(|| report.clone())),
        log: Box::new(move |_, e| l2.lock().unwrap().push(e.clone())),
    };
    (h, logged)
}

#[test]
fn every_attempt_is_logged_with_the_clocks_report() {
    let ctx = ctx_with_release();
    let report = serde_json::json!({"result": "ok", "bytes": 9, "max_gap_ms": 40});
    let (h, logged) = logged_hooks(|| Ok(()), || true, report.clone());
    assert!(update_clock_with(&ctx, h));
    assert_eq!(wait_update(&ctx).state, "done");
    let logged = logged.lock().unwrap();
    assert_eq!(logged.len(), 1);
    let e = &logged[0];
    assert_eq!((e["attempt"].as_u64(), e["confirmed"].as_bool()), (Some(1), Some(true)));
    assert_eq!((e["ip"].as_str(), e["bytes"].as_u64()), (Some("10.0.0.9"), Some(FIRMWARE.len() as u64)));
    assert!(e["app"]["error"].is_null() && e["app"]["secs"].is_number());
    assert_eq!(e["clock"], report);
    assert_eq!(ctx.state().update.last.as_ref(), Some(e), "the dashboard sees the last one");
}

#[test]
fn a_stalled_first_attempt_leaves_two_entries() {
    let ctx = ctx_with_release();
    let tries = Arc::new(Mutex::new(0));
    let t2 = Arc::clone(&tries);
    let (h, logged) = logged_hooks(
        || Err(OtaError::Transport("timed out".into())),
        move || { let mut n = t2.lock().unwrap(); *n += 1; *n == 2 },
        serde_json::json!({"result": "aborted", "max_gap_ms": 5012}),
    );
    assert!(update_clock_with(&ctx, h));
    assert_eq!(wait_update(&ctx).state, "done");
    let logged = logged.lock().unwrap();
    assert_eq!(logged.len(), 2);
    assert_eq!(logged[0]["app"]["error"], "timed out");
    assert_eq!(logged[0]["confirmed"], false);
    assert_eq!(logged[0]["clock"]["result"], "aborted");
    assert_eq!((logged[1]["attempt"].as_u64(), logged[1]["confirmed"].as_bool()), (Some(2), Some(true)));
}

#[test]
fn a_refused_upload_is_logged_too() {
    let ctx = ctx_with_release();
    let (h, logged) = logged_hooks(
        || Err(OtaError::Refused("wrong clock password".into())),
        || panic!("no waiting after a refusal"),
        serde_json::json!({"result": "no auth"}),
    );
    assert!(update_clock_with(&ctx, h));
    assert_eq!(wait_update(&ctx).state, "failed");
    let logged = logged.lock().unwrap();
    assert_eq!(logged.len(), 1);
    assert_eq!(logged[0]["app"]["error"], "wrong clock password");
    assert_eq!(logged[0]["clock"]["result"], "no auth");
}

#[test]
fn update_uploads_the_shipped_firmware() {
    let ctx = ctx_with_release();
    ctx.cfg
        .lock()
        .unwrap()
        .insert("clock_pass".into(), "pw".into());
    let (h, calls) = hooks(
        Some("10.0.0.9"),
        || Ok(()),
        |ip, fw| (ip, fw) == ("10.0.0.9", "wfd-clock NEW"),
    );
    assert!(update_clock_with(&ctx, h));
    let up = wait_update(&ctx);
    assert_eq!(up.state, "done");
    assert_eq!(up.message, "the clock now runs NEW");
    assert_eq!(
        *calls.lock().unwrap(),
        [("10.0.0.9".to_string(), FIRMWARE.to_vec(), "pw".to_string())]
    );
}

#[test]
fn lost_reply_still_counts_if_the_clock_runs_the_new_build() {
    let ctx = ctx_with_release();
    // the clock restarted before its reply arrived
    let (h, _) = hooks(
        Some("10.0.0.9"),
        || Err(OtaError::Transport("timed out".into())),
        |_, _| true,
    );
    update_clock_with(&ctx, h);
    assert_eq!(wait_update(&ctx).state, "done");
}

#[test]
fn update_fails_if_the_new_build_never_shows_up() {
    let ctx = ctx_with_release();
    let (h, calls) = hooks(Some("10.0.0.9"), || Ok(()), |_, _| false);
    update_clock_with(&ctx, h);
    let up = wait_update(&ctx);
    assert_eq!(up.state, "failed");
    assert!(up.message.contains("didn't come back"), "{}", up.message);
    assert_eq!(calls.lock().unwrap().len(), 2, "one retry before giving up");
}

#[test]
fn a_stalled_upload_is_retried() {
    let ctx = ctx_with_release();
    let tries = Mutex::new(vec![true, false]);
    let (h, calls) = hooks(
        Some("10.0.0.9"),
        || Ok(()),
        move |_, _| tries.lock().unwrap().pop().unwrap(),
    );
    update_clock_with(&ctx, h);
    assert_eq!(wait_update(&ctx).state, "done");
    assert_eq!(calls.lock().unwrap().len(), 2);
}

#[test]
fn a_refused_upload_fails_at_once() {
    let ctx = ctx_with_release();
    let (h, calls) = hooks(
        Some("10.0.0.9"),
        || Err(OtaError::Refused("wrong clock password".into())),
        |_, _| true,
    );
    update_clock_with(&ctx, h);
    let up = wait_update(&ctx);
    assert_eq!(
        (up.state.as_str(), up.message.as_str()),
        ("failed", "wrong clock password")
    );
    assert_eq!(calls.lock().unwrap().len(), 1);
}

#[test]
fn update_reports_failures() {
    let ctx = ctx_with_release();
    let (h, _) = hooks(None, || Ok(()), |_, _| true);
    update_clock_with(&ctx, h);
    let up = wait_update(&ctx);
    assert_eq!(up.state, "failed");
    assert!(up.message.contains("not found on WiFi"), "{}", up.message);
}

#[test]
fn no_release_no_update() {
    let path = std::env::temp_dir()
        .join("wfd-ota-test-does-not-exist")
        .join("wfd_config.json");
    let ctx = Arc::new(Ctx::new(path, None, &[]));
    let (h, calls) = hooks(Some("10.0.0.9"), || Ok(()), |_, _| true);
    update_clock_with(&ctx, h);
    assert_eq!(wait_update(&ctx).state, "failed");
    assert!(calls.lock().unwrap().is_empty());
}

#[test]
fn one_update_at_a_time() {
    let ctx = ctx_with_release();
    let mut h = hooks(Some("10.0.0.9"), || Ok(()), |_, _| true).0;
    h.find_clock_ip = Box::new(|_| {
        std::thread::sleep(Duration::from_millis(300));
        Some("10.0.0.9".into())
    });
    assert!(update_clock_with(&ctx, h));
    assert!(!update_clock_with(
        &ctx,
        hooks(Some("x"), || Ok(()), |_, _| true).0
    ));
    assert_eq!(wait_update(&ctx).state, "done");
}

// ------------------------------------------------------------------ real HTTP against localhost

fn server() -> (tiny_http::Server, String) {
    let s = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let addr = format!("127.0.0.1:{}", s.server_addr().to_ip().unwrap().port());
    (s, addr)
}

#[test]
fn upload_posts_multipart_with_auth() {
    let (s, addr) = server();
    let h = std::thread::spawn(move || {
        let mut req = s.recv().unwrap();
        let mut body = Vec::new();
        req.as_reader().read_to_end(&mut body).unwrap();
        let hdr = |n: &'static str| {
            req.headers()
                .iter()
                .find(|h| h.field.equiv(n))
                .map(|h| h.value.to_string())
        };
        let seen = (
            req.url().to_string(),
            hdr("Authorization"),
            hdr("Content-Type"),
            body,
        );
        req.respond(tiny_http::Response::from_string("ok")).unwrap();
        seen
    });
    upload_firmware(&addr, FIRMWARE, "pw").unwrap();
    let (url, auth, ctype, body) = h.join().unwrap();
    assert_eq!(url, "/update");
    assert_eq!(auth.as_deref(), Some("Basic YWRtaW46cHc="));
    let (want, want_ctype) = wfd_app::release::multipart("fw", "wfd-clock-firmware.bin", FIRMWARE);
    assert_eq!(ctype, Some(want_ctype));
    assert_eq!(body, want);
}

#[test]
fn upload_maps_refusals() {
    for (code, msg) in [
        (401, "wrong clock password"),
        (500, "clock refused the update (HTTP 500)"),
        (202, "clock answered HTTP 202"),
    ] {
        let (s, addr) = server();
        let h = std::thread::spawn(move || {
            let req = s.recv().unwrap();
            req.respond(tiny_http::Response::empty(code)).unwrap();
        });
        assert_eq!(
            upload_firmware(&addr, FIRMWARE, ""),
            Err(OtaError::Refused(msg.into()))
        );
        h.join().unwrap();
    }
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    assert!(matches!(
        upload_firmware(&format!("127.0.0.1:{port}"), FIRMWARE, ""),
        Err(OtaError::Transport(_))
    ));
}

#[test]
fn wait_for_build_polls_the_status() {
    let (s, addr) = server();
    let h = std::thread::spawn(move || {
        for fw in ["OLD", "NEW"] {
            let req = s.recv().unwrap();
            assert_eq!(req.url(), "/api/status");
            let body = format!(r#"{{"state":{{"fw":"{fw}"}},"config":{{}}}}"#);
            req.respond(tiny_http::Response::from_string(body)).unwrap();
        }
    });
    assert!(wait_for_build(&addr, "NEW", Duration::from_secs(10)));
    h.join().unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    assert!(!wait_for_build(
        &format!("127.0.0.1:{port}"),
        "NEW",
        Duration::from_millis(50)
    ));
}
