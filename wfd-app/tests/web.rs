//! The local HTTP API the dashboard and drawing tool use (port of wfd-control/tests/test_api.py).
//!
//! Each test starts `web::run` on a free port with its own Ctx and config file. Routes that
//! change clock settings get a fake link thread that answers `ctx.clock_requests`.

use serde_json::{Map, Value, json};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use wfd_app::config;
use wfd_app::ctx::{ClockReply, Ctx, UploadJob};
use wfd_app::release::Release;

struct App {
    ctx: Arc<Ctx>,
    base: String,
    dir: PathBuf,
}

impl Drop for App {
    fn drop(&mut self) {
        self.ctx.stop.store(true, Ordering::Relaxed);
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn temp_dir() -> PathBuf {
    use std::sync::atomic::AtomicUsize;
    static N: AtomicUsize = AtomicUsize::new(0);
    let d = std::env::temp_dir().join(format!(
        "wfd-web-test-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn start_with(release: Option<Release>) -> App {
    let dir = temp_dir();
    let ctx = Arc::new(Ctx::new(dir.join("wfd_config.json"), release, &[]));
    let server = wfd_app::web::bind("127.0.0.1:0").unwrap();
    let port = server.server_addr().to_ip().unwrap().port();
    let c = Arc::clone(&ctx);
    std::thread::spawn(move || wfd_app::web::run(c, &server));
    App {
        ctx,
        base: format!("http://127.0.0.1:{port}"),
        dir,
    }
}

fn start() -> App {
    start_with(None)
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(10)))
        .build()
        .into()
}

/// GET path -> (status, body as JSON or as a string).
fn get(app: &App, path: &str) -> (u16, Value) {
    let mut r = agent().get(&format!("{}{path}", app.base)).call().unwrap();
    let status = r.status().as_u16();
    let text = r.body_mut().read_to_string().unwrap();
    (
        status,
        serde_json::from_str(&text).unwrap_or(Value::String(text)),
    )
}

fn post_raw(app: &App, path: &str, body: &str, header: bool) -> (u16, Value) {
    let mut req = agent()
        .post(&format!("{}{path}", app.base))
        .header("Content-Type", "application/json");
    if header {
        req = req.header("X-WFD", "1");
    }
    let mut r = req.send(body).unwrap();
    let status = r.status().as_u16();
    let text = r.body_mut().read_to_string().unwrap();
    (
        status,
        serde_json::from_str(&text).unwrap_or(Value::String(text)),
    )
}

fn post(app: &App, path: &str, body: Value) -> (u16, Value) {
    post_raw(app, path, &body.to_string(), true)
}

fn cfg(app: &App, key: &str) -> Value {
    app.ctx.cfg().get(key).cloned().unwrap_or(Value::Null)
}

fn default(key: &str) -> Value {
    config::defaults()[key].clone()
}

/// A fake link thread: records each patch and answers with `reply`.
struct Clock {
    calls: Arc<Mutex<Vec<Map<String, Value>>>>,
    reply: Arc<Mutex<ClockReply>>,
}

fn fake_clock(app: &App) -> Clock {
    app.ctx.state().connected = true;
    let calls = Arc::new(Mutex::new(Vec::new()));
    let reply: Arc<Mutex<ClockReply>> = Arc::new(Mutex::new(Ok(None)));
    let (ctx, c, r) = (Arc::clone(&app.ctx), Arc::clone(&calls), Arc::clone(&reply));
    std::thread::spawn(move || {
        while !ctx.stopping() {
            let reqs: Vec<_> = std::mem::take(&mut *ctx.clock_requests.lock().unwrap());
            for req in reqs {
                c.lock().unwrap().push(req.patch);
                let _ = req.reply.send(r.lock().unwrap().clone());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    });
    Clock { calls, reply }
}

impl Clock {
    fn calls(&self) -> Vec<Value> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .cloned()
            .map(Value::Object)
            .collect()
    }
}

#[test]
fn post_requires_header() {
    let app = start();
    let (status, body) = post_raw(&app, "/api/set", r#"{"logo": true}"#, false);
    assert_eq!((status, body), (403, json!("forbidden")));
    assert_eq!(cfg(&app, "logo"), json!(false));
}

#[test]
fn bad_json_is_400() {
    let app = start();
    assert_eq!(
        post_raw(&app, "/api/set", "{nope", true),
        (400, json!({"error": "bad json"}))
    );
}

#[test]
fn set_normalizes_values() {
    let app = start();
    assert_eq!(
        post(
            &app,
            "/api/set",
            json!({"grid_fps": "9", "logo": true, "gain_out": 2})
        ),
        (200, json!({"ok": true}))
    );
    assert_eq!(cfg(&app, "grid_fps"), json!(9));
    assert_eq!(cfg(&app, "logo"), json!(true));
    let g = cfg(&app, "gain_out");
    assert!(g.is_f64() && g.as_f64() == Some(2.0), "{g}");
    // and it was saved
    let saved = config::load(&app.ctx.cfg_path);
    assert_eq!(saved["grid_fps"], json!(9));
}

#[test]
fn set_rejects_bad_values_and_applies_nothing() {
    let app = start();
    let (status, body) = post(
        &app,
        "/api/set",
        json!({"grid_fps": 3, "scroll_ms": "abc", "nope": 1}),
    );
    assert_eq!(status, 400);
    let keys: Vec<&String> = body["error"].as_object().unwrap().keys().collect();
    let mut keys: Vec<&str> = keys.iter().map(|k| k.as_str()).collect();
    keys.sort();
    assert_eq!(keys, ["nope", "scroll_ms"]);
    assert_eq!(body["error"]["nope"], json!("unknown setting"));
    assert_eq!(cfg(&app, "grid_fps"), default("grid_fps"));
    assert!(!app.ctx.cfg_path.exists(), "nothing saved");
}

#[test]
fn status_returns_state_and_config() {
    let app = start();
    {
        let mut st = app.ctx.state();
        st.message = "HELLO".into();
        let mut clock = Map::new();
        clock.insert("brightness".into(), json!(5));
        clock.insert("style".into(), json!(2));
        st.clock = Some(clock);
    }
    app.ctx
        .cfg
        .lock()
        .unwrap()
        .insert("clock_pass".into(), json!("secret"));
    let (status, body) = get(&app, "/api/status");
    assert_eq!(status, 200);
    let st = &body["state"];
    assert_eq!(st["message"], json!("HELLO"));
    assert_eq!(body["config"]["grid_fps"], default("grid_fps"));
    assert_eq!(
        (&body["config"]["brightness"], &body["config"]["style"]),
        (&json!(5), &json!(2))
    );
    assert_eq!(
        body["config"]["clock_pass"],
        json!(""),
        "the password is never sent"
    );
    assert_eq!(st["clock_pass_set"], json!(true));
    assert_eq!(st["app_protocol"], json!(config::PROTOCOL));
    assert_eq!(st["compat"], json!("unknown"));
    assert_eq!(st["update_available"], json!(false));
    // the page tells the app from the clock by the `link` key (null while disconnected)
    assert!(st.as_object().unwrap().contains_key("link"));
    assert_eq!(st["link"], Value::Null);
    // every key of app.py's state dict, plus the computed ones
    let mut keys: Vec<&str> = st.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort();
    let mut want = vec![
        "connected",
        "port",
        "time",
        "metrics",
        "weather",
        "levels",
        "audio",
        "message",
        "saved",
        "online",
        "battery",
        "geo",
        "clock",
        "link",
        "clock_ip",
        "clock_fw",
        "clock_protocol",
        "clock_pw_set",
        "release",
        "update",
        "grid_rows",
        "usb_problem",
        "clock_pass_set",
        "app_protocol",
        "compat",
        "update_available",
    ];
    want.sort();
    assert_eq!(keys, want);
    let mut mkeys: Vec<&str> = st["metrics"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    mkeys.sort();
    assert_eq!(mkeys, ["cpu", "gpu", "ram"]);
    assert_eq!(st["update"], json!({"state": "idle", "message": "", "last": null}));
    assert_eq!(
        st["weather"].as_object().unwrap().keys().len(),
        5,
        "code, temp, unit, label, cond"
    );
}

#[test]
fn status_reports_firmware_compat() {
    let app = start_with(Some(Release {
        fw: "wfd-clock new".into(),
        protocol: 3,
    }));
    {
        let mut st = app.ctx.state();
        st.connected = true;
        st.link = Some("usb".into());
        st.clock_fw = Some("wfd-clock old".into());
        st.clock_protocol = Some(3);
    }
    let (_, body) = get(&app, "/api/status");
    assert_eq!(body["state"]["compat"], json!("ok"));
    assert_eq!(body["state"]["update_available"], json!(true));
    assert_eq!(body["state"]["link"], json!("usb"));
    assert_eq!(
        body["state"]["release"],
        json!({"fw": "wfd-clock new", "protocol": 3})
    );
}

#[test]
fn clock_settings_go_to_the_clock() {
    let app = start();
    let clock = fake_clock(&app);
    assert_eq!(
        post(
            &app,
            "/api/set",
            json!({"brightness": 5, "standalone_anim": "rain", "logo": true})
        ),
        (200, json!({"ok": true}))
    );
    assert_eq!(
        clock.calls(),
        vec![json!({"brightness": 5, "standalone_anim": "rain"})]
    );
    assert_eq!(cfg(&app, "logo"), json!(true));
    assert!(
        !app.ctx.cfg().contains_key("brightness"),
        "the app keeps no copy"
    );
}

#[test]
fn clock_rejection_applies_nothing() {
    let app = start();
    let clock = fake_clock(&app);
    *clock.reply.lock().unwrap() = Err(json!({"brightness": "expected an integer 1..16"}));
    let (status, body) = post(&app, "/api/set", json!({"brightness": 99, "logo": true}));
    assert_eq!(status, 400);
    assert_eq!(
        body["error"],
        json!({"brightness": "expected an integer 1..16"})
    );
    assert_eq!(cfg(&app, "logo"), json!(false));
}

#[test]
fn app_errors_stop_before_the_clock() {
    let app = start();
    let clock = fake_clock(&app);
    let (status, _) = post(&app, "/api/set", json!({"brightness": 5, "grid_fps": "x"}));
    assert_eq!(status, 400);
    assert!(clock.calls().is_empty());
}

#[test]
fn clock_warnings_are_passed_on() {
    let app = start();
    let clock = fake_clock(&app);
    let w = json!({"location": "ZIP lookup failed; location unchanged"});
    *clock.reply.lock().unwrap() = Ok(Some(w.clone()));
    assert_eq!(
        post(&app, "/api/set", json!({"location": "00000"})),
        (200, json!({"ok": true, "warning": w}))
    );
}

#[test]
fn clock_settings_need_the_clock() {
    let app = start();
    app.ctx.state().connected = false;
    let (status, body) = post(&app, "/api/set", json!({"brightness": 5}));
    assert_eq!(status, 400);
    assert_eq!(body["error"], json!({"brightness": "clock not connected"}));
}

#[test]
fn sync_sets_event() {
    let app = start();
    assert_eq!(post(&app, "/api/sync", json!({})).0, 200);
    assert!(app.ctx.sync_now.load(Ordering::Relaxed));
}

#[test]
fn empty_body_counts_as_empty_object() {
    let app = start();
    assert_eq!(
        post_raw(&app, "/api/sync", "", true),
        (200, json!({"ok": true}))
    );
}

#[test]
fn grid_preview_masks_rows() {
    let app = start();
    assert_eq!(
        post(
            &app,
            "/api/grid_preview",
            json!({"rows": [127, 300], "frame": 1})
        ),
        (200, json!({"ok": true}))
    );
    let p = app.ctx.grid_preview.lock().unwrap().clone();
    assert_eq!(p.rows, [127, (300 & 0x7F) as u8, 0, 0, 0, 0, 0]);
    assert!(p.frame);
    assert!(p.at.is_some());
}

#[test]
fn grid_save_queues_validated_frames() {
    let app = start();
    assert_eq!(
        post(
            &app,
            "/api/grid_save",
            json!({"frames": [[1, 2, 3]], "fps": 10})
        ),
        (200, json!({"queued": 1}))
    );
    assert_eq!(
        app.ctx.uploads.lock().unwrap().back().cloned(),
        Some(UploadJob::Frames {
            frames: vec![[1, 2, 3, 0, 0, 0, 0]],
            fps: 10,
            frame: false
        })
    );
    assert_eq!(
        post(&app, "/api/grid_save", json!({"frames": "junk"})).0,
        400
    );
}

#[test]
fn grid_save_builtin_sends_name_not_frames() {
    let app = start();
    {
        let mut c = app.ctx.cfg.lock().unwrap();
        c.insert("grid_anim".into(), json!("rain"));
        c.insert("grid_fps".into(), json!(14));
        c.insert("grid_frame".into(), json!(true));
    }
    assert_eq!(
        post(&app, "/api/grid_save", json!({})),
        (200, json!({"queued": 1}))
    );
    assert_eq!(
        app.ctx.uploads.lock().unwrap().back().cloned(),
        Some(UploadJob::Anim {
            name: "rain".into(),
            fps: 14,
            frame: true
        })
    );
    // the live-audio ones run on the clock's own mic: built-in too
    for name in ["audio", "mouth"] {
        assert_eq!(
            post(&app, "/api/grid_save", json!({"anim": name})),
            (200, json!({"queued": 1}))
        );
        assert!(matches!(
            app.ctx.uploads.lock().unwrap().back(),
            Some(UploadJob::Anim { name: n, .. }) if n == name
        ));
    }
}

#[test]
fn grid_save_custom_needs_frames() {
    let app = start();
    {
        let mut c = app.ctx.cfg.lock().unwrap();
        c.insert("grid_anim".into(), json!("custom"));
        c.insert("grid_custom".into(), json!([]));
    }
    assert_eq!(post(&app, "/api/grid_save", json!({})).0, 400);
    app.ctx.cfg.lock().unwrap().insert(
        "grid_custom".into(),
        json!([[1, 1, 1, 1, 1, 1, 1], [2, 2, 2, 2, 2, 2, 2]]),
    );
    assert_eq!(
        post(&app, "/api/grid_save", json!({})),
        (200, json!({"queued": 2}))
    );
    assert!(matches!(
        app.ctx.uploads.lock().unwrap().back(),
        Some(UploadJob::Frames { frames, .. }) if frames == &vec![[1u8; 7], [2u8; 7]]
    ));
}

#[test]
fn grid_save_refuses_live_only_animation() {
    let app = start();
    let (status, body) = post(&app, "/api/grid_save", json!({"anim": "cpu"}));
    assert_eq!(status, 400);
    assert!(
        body["error"].as_str().unwrap().contains("live data"),
        "{body}"
    );
    assert!(app.ctx.uploads.lock().unwrap().is_empty());
}

#[test]
fn om_save_sets_the_clocks_text() {
    let app = start();
    let clock = fake_clock(&app);
    let (status, body) = post(&app, "/api/om_save", json!({"text": "  HELLO {time}\n"}));
    assert_eq!(status, 200);
    assert_eq!(body, json!({"queued": 1, "text": "HELLO {time}"}));
    assert_eq!(clock.calls(), vec![json!({"offline_text": "HELLO {time}"})]);
    assert_eq!(post(&app, "/api/om_save", json!({"text": 5})).0, 400);
}

#[test]
fn unknown_paths_404() {
    let app = start();
    assert_eq!(get(&app, "/nope"), (404, json!({"error": "not found"})));
    assert_eq!(
        post(&app, "/api/nope", json!({})),
        (404, json!({"error": "not found"}))
    );
}

#[test]
fn update_clock_needs_a_release() {
    let app = start();
    let (status, body) = post(&app, "/api/update_clock", json!({}));
    assert_eq!(status, 400);
    assert_eq!(
        body,
        json!({"error": "no firmware in wfd-firmware/releases"})
    );
}

#[test]
fn old_config_files_dont_bring_back_clock_settings() {
    let dir = temp_dir();
    let path = dir.join("old.json");
    std::fs::write(
        &path,
        r#"{"brightness": 3, "style": 1, "logo": true, "_comment": "x"}"#,
    )
    .unwrap();
    let loaded = config::load(&path);
    assert_eq!(loaded["logo"], json!(true));
    for k in ["brightness", "style", "_comment"] {
        assert!(!loaded.contains_key(k), "{k}");
    }
    let _ = std::fs::remove_dir_all(dir);
}

fn page(path: &str) -> (u16, String, String, String) {
    let app = start();
    let mut r = agent().get(&format!("{}{path}", app.base)).call().unwrap();
    let h = |n: &str| {
        r.headers()
            .get(n)
            .map(|v| v.to_str().unwrap().to_string())
            .unwrap_or_default()
    };
    let (ctype, cache) = (h("content-type"), h("cache-control"));
    (
        r.status().as_u16(),
        ctype,
        cache,
        r.body_mut().read_to_string().unwrap(),
    )
}

fn shared_page(name: &str) -> String {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../wfd-firmware/clock/web")
        .join(name);
    std::fs::read_to_string(p).unwrap().replace("\r\n", "\n")
}

#[test]
fn serves_the_shared_pages() {
    for (paths, file) in [
        (["/", "/index.html"], "dashboard.html"),
        (["/draw", "/draw.html"], "draw.html"),
    ] {
        for path in paths {
            let (status, ctype, cache, body) = page(path);
            assert_eq!(status, 200);
            assert_eq!(ctype, "text/html; charset=utf-8");
            assert_eq!(cache, "no-store");
            assert_eq!(body.replace("\r\n", "\n"), shared_page(file), "{path}");
        }
    }
}

#[test]
fn embedded_pages_match_the_firmware_sources() {
    let norm = |b: &[u8]| String::from_utf8(b.to_vec()).unwrap().replace("\r\n", "\n");
    assert_eq!(
        norm(wfd_app::web::DASHBOARD_HTML),
        shared_page("dashboard.html")
    );
    assert_eq!(norm(wfd_app::web::DRAW_HTML), shared_page("draw.html"));
}

#[test]
fn json_replies_are_not_cached() {
    let app = start();
    let r = agent()
        .get(&format!("{}/api/status", app.base))
        .call()
        .unwrap();
    assert_eq!(r.headers().get("cache-control").unwrap(), "no-store");
    assert_eq!(r.headers().get("content-type").unwrap(), "application/json");
}

#[test]
fn slow_clock_does_not_block_status() {
    let app = start();
    app.ctx.state().connected = true; // nobody answers clock_requests: /api/set waits 3 s
    let base = app.base.clone();
    let slow = std::thread::spawn(move || {
        agent()
            .post(&format!("{base}/api/set"))
            .header("X-WFD", "1")
            .send(r#"{"brightness": 5}"#)
            .unwrap()
            .status()
            .as_u16()
    });
    std::thread::sleep(Duration::from_millis(200));
    let t = std::time::Instant::now();
    assert_eq!(get(&app, "/api/status").0, 200);
    assert!(
        t.elapsed() < Duration::from_secs(1),
        "status waited {:?}",
        t.elapsed()
    );
    assert_eq!(slow.join().unwrap(), 400, "clock didn't answer");
}

#[test]
fn stops_when_asked() {
    let dir = temp_dir();
    let ctx = Arc::new(Ctx::new(dir.join("c.json"), None, &[]));
    let server = wfd_app::web::bind("127.0.0.1:0").unwrap();
    let c = Arc::clone(&ctx);
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        wfd_app::web::run(c, &server);
        let _ = tx.send(());
    });
    std::thread::sleep(Duration::from_millis(50));
    ctx.stop.store(true, Ordering::Relaxed);
    assert!(
        rx.recv_timeout(Duration::from_secs(2)).is_ok(),
        "serve didn't stop"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn clock_password_checks_its_input() {
    let app = start();
    for bad in [json!({"password": 5}), json!({}), json!({"password": "has space"}),
                json!({"password": "x".repeat(65)}), json!({"password": "caf\u{e9}"})] {
        let (code, body) = post(&app, "/api/clock_password", bad.clone());
        assert_eq!(code, 400, "{bad}");
        assert!(body["error"]["password"].is_string(), "{body}");
    }
    assert!(app.ctx.clock_requests.lock().unwrap().is_empty(), "nothing reached the clock");
}

#[test]
fn clock_password_needs_the_clock_on_usb() {
    let app = start();
    let (code, body) = post(&app, "/api/clock_password", json!({"password": "hunter2"}));
    assert_eq!((code, body["error"]["password"].as_str()), (400, Some("clock not connected")));
    {
        let mut st = app.ctx.state();
        st.connected = true;
        st.link = Some("wifi".into());
    }
    let (code, body) = post(&app, "/api/clock_password", json!({"password": "hunter2"}));
    assert_eq!(code, 400);
    assert!(body["error"]["password"].as_str().unwrap().contains("USB"));
}
