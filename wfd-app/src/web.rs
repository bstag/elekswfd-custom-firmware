//! web: the local HTTP server (port of `class H` in wfd-control/app.py).
//!
//! Serves the pages shared with the clock (built into the binary) and the JSON API the
//! dashboard and the grid animator use. Same routes, status codes and bodies as app.py:
//!
//! - GET  `/`, `/index.html`        the dashboard;  `/draw`, `/draw.html` the grid animator
//! - GET  `/api/status`             {"state": {...}, "config": {app settings + clock settings}}
//! - POST (header `X-WFD: 1` required, else 403 "forbidden"; bad JSON -> 400)
//!   `/api/set`, `/api/sync`, `/api/grid_preview`, `/api/grid_save`, `/api/om_save`,
//!   `/api/update_clock`
//! - anything else -> 404 {"error": "not found"}
//!
//! Each request runs on its own thread (like ThreadingHTTPServer), so a slow trip to the clock
//! in `/api/set` doesn't hold up status polling.

use crate::config::{self, CLOCK_KEYS, PROTOCOL, py};
use crate::ctx::{Ctx, UploadJob};
use serde_json::{Map, Value, json};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use tiny_http::{Header, Method, Request, Response, Server};

/// The pages shared with the clock firmware (wfd-firmware/clock/web), built into the exe.
pub const DASHBOARD_HTML: &[u8] = include_bytes!("../../wfd-firmware/clock/web/dashboard.html");
pub const DRAW_HTML: &[u8] = include_bytes!("../../wfd-firmware/clock/web/draw.html");

/// Built-in animations the clock can run on its own (app.py BUILTIN_GRID).
pub const BUILTIN_GRID: [&str; 12] = [
    "spin", "bounce", "rain", "wave", "pulse", "sparkle", "scan", "life", "heart", "audio", "mouth",
    "weather",
];

/// How long `/api/set` waits for the clock (app.py set_clock default).
const CLOCK_TIMEOUT: Duration = Duration::from_secs(3);

/// Serve the dashboard + API on `addr` (e.g. "127.0.0.1:8737") until ctx.stop.
/// Panics if the address can't be bound (like app.py, which can't start without its port).
pub fn serve(ctx: Arc<Ctx>, addr: &str) {
    let server = bind(addr).unwrap_or_else(|e| panic!("can't listen on {addr}: {e}"));
    run(ctx, &server);
}

/// Bind the listening socket (use "127.0.0.1:0" for any free port; see `Server::server_addr`).
pub fn bind(addr: &str) -> Result<Server, String> {
    Server::http(addr).map_err(|e| e.to_string())
}

/// Answer requests on `server` until ctx.stop, each on its own thread.
pub fn run(ctx: Arc<Ctx>, server: &Server) {
    while !ctx.stop.load(Ordering::Relaxed) {
        match server.recv_timeout(Duration::from_millis(100)) {
            Ok(Some(req)) => {
                let ctx = Arc::clone(&ctx);
                let spawned = std::thread::Builder::new()
                    .name("http".into())
                    .spawn(move || handle(&ctx, req));
                if let Err(e) = spawned {
                    eprintln!("web: can't start a request thread: {e}");
                }
            }
            Ok(None) => {}
            Err(e) => {
                eprintln!("web: {e}");
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
}

/// A finished reply: status, body, content type.
struct Reply(u16, Vec<u8>, &'static str);

fn json_reply(code: u16, body: Value) -> Reply {
    Reply(
        code,
        serde_json::to_vec(&body).unwrap_or_default(),
        "application/json",
    )
}

fn html(page: &[u8]) -> Reply {
    Reply(200, page.to_vec(), "text/html; charset=utf-8")
}

fn not_found() -> Reply {
    json_reply(404, json!({"error": "not found"}))
}

fn handle(ctx: &Arc<Ctx>, mut req: Request) {
    let reply = match req.method() {
        Method::Get => get(ctx, req.url()),
        Method::Post => post(ctx, &mut req),
        _ => Reply(501, b"Unsupported method".to_vec(), "text/plain"),
    };
    let Reply(code, body, ctype) = reply;
    let resp = Response::from_data(body)
        .with_status_code(code)
        .with_header(header("Content-Type", ctype))
        .with_header(header("Cache-Control", "no-store"));
    let _ = req.respond(resp);
}

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("valid header")
}

fn get(ctx: &Ctx, path: &str) -> Reply {
    match path {
        "/" | "/index.html" => html(DASHBOARD_HTML),
        "/draw" | "/draw.html" => html(DRAW_HTML),
        "/api/status" => json_reply(200, status(ctx)),
        _ => not_found(),
    }
}

/// GET /api/status: state (+ clock_pass_set, app_protocol, compat, update_available) and
/// config = app settings + the clock's own, with the password blanked.
pub fn status(ctx: &Ctx) -> Value {
    let cfg = ctx.cfg.lock().unwrap();
    let st = ctx.state();
    let mut shown = cfg.clone();
    if let Some(clock) = &st.clock {
        shown.extend(clock.clone());
    }
    shown.insert("clock_pass".into(), json!(""));

    let (compat, update_available) = crate::release::firmware_status(
        st.connected,
        st.clock.is_some(),
        st.clock_fw.as_deref(),
        st.clock_protocol,
        st.release.as_ref(),
    );
    let mut state = match serde_json::to_value(&*st) {
        Ok(Value::Object(m)) => m,
        _ => Map::new(),
    };
    // Python keeps the battery at the top level only; state.metrics is {cpu, gpu, ram}.
    if let Some(Value::Object(m)) = state.get_mut("metrics") {
        m.remove("battery");
    }
    let pass_set = cfg.get("clock_pass").is_some_and(truthy);
    state.insert("clock_pass_set".into(), json!(pass_set));
    state.insert("app_protocol".into(), json!(PROTOCOL));
    state.insert("compat".into(), json!(compat));
    state.insert("update_available".into(), json!(update_available));
    json!({"state": state, "config": shown})
}

fn post(ctx: &Arc<Ctx>, req: &mut Request) -> Reply {
    let has_header = req
        .headers()
        .iter()
        .any(|h| h.field.equiv("X-WFD") && h.value.as_str() == "1");
    if !has_header {
        return Reply(403, b"forbidden".to_vec(), "text/plain");
    }
    let mut raw = Vec::new();
    if req.as_reader().read_to_end(&mut raw).is_err() {
        return json_reply(400, json!({"error": "bad json"}));
    }
    let body = if raw.is_empty() {
        Map::new()
    } else {
        match serde_json::from_slice::<Value>(&raw) {
            Ok(Value::Object(m)) => m,
            // (Python would fail later on a non-object body; answer it as bad JSON instead)
            _ => return json_reply(400, json!({"error": "bad json"})),
        }
    };
    match req.url() {
        "/api/set" => api_set(ctx, body),
        "/api/update_clock" => {
            if ctx.state().release.is_none() {
                json_reply(
                    400,
                    json!({"error": "no firmware in wfd-firmware/releases"}),
                )
            } else if !crate::ota::update_clock(ctx) {
                json_reply(409, json!({"error": "an update is already running"}))
            } else {
                json_reply(202, json!({"ok": true}))
            }
        }
        "/api/sync" => {
            ctx.sync_now.store(true, Ordering::Relaxed);
            json_reply(200, json!({"ok": true}))
        }
        "/api/grid_preview" => grid_preview(ctx, &body),
        "/api/grid_save" => grid_save(ctx, &body),
        "/api/om_save" => om_save(ctx, &body),
        "/api/clock_password" => clock_password(ctx, &body),
        _ => not_found(),
    }
}

/// POST /api/set: app settings are validated and saved here, clock settings go to the clock
/// (which validates its own). All-or-nothing: an error anywhere applies nothing.
fn api_set(ctx: &Ctx, body: Map<String, Value>) -> Reply {
    let mut clean = Map::new();
    let mut errors = Map::new();
    let mut clock = Map::new();
    for (k, v) in body {
        if CLOCK_KEYS.contains(&k.as_str()) {
            clock.insert(k, v);
            continue;
        }
        match config::coerce(&k, &v) {
            Ok(v) => {
                clean.insert(k, v);
            }
            Err(e) => {
                errors.insert(k, json!(e));
            }
        }
    }
    if !errors.is_empty() {
        // nothing reaches the clock unless the app part is valid
        return json_reply(400, json!({"error": errors}));
    }
    let mut warning = None;
    if !clock.is_empty() {
        match ctx.set_clock(clock, CLOCK_TIMEOUT) {
            Ok(w) => warning = w.filter(truthy),
            Err(info) => return json_reply(400, json!({"error": info})),
        }
    }
    {
        let mut cfg = ctx.cfg.lock().unwrap();
        cfg.extend(clean);
        if let Err(e) = config::save(&ctx.cfg_path, &cfg) {
            return json_reply(500, json!({"error": format!("can't save settings: {e}")}));
        }
    }
    match warning {
        Some(w) => json_reply(200, json!({"ok": true, "warning": w})),
        None => json_reply(200, json!({"ok": true})),
    }
}

/// POST /api/grid_preview: the drawing tool's live frame, shown for a moment.
fn grid_preview(ctx: &Ctx, body: &Map<String, Value>) -> Reply {
    let given = match body.get("rows") {
        Some(Value::Array(a)) if !a.is_empty() => a.clone(),
        Some(v) if truthy(v) => return json_reply(400, json!({"error": "rows: expected a list"})),
        _ => Vec::new(),
    };
    let mut rows = [0u8; 7];
    for (slot, x) in rows.iter_mut().zip(given.iter()) {
        match py::int_low7(x) {
            Ok(v) => *slot = v as u8,
            Err(e) => return json_reply(400, json!({"error": format!("rows: {e}")})),
        }
    }
    let frame = body.get("frame").is_some_and(truthy);
    *ctx.grid_preview.lock().unwrap() = crate::ctx::GridPreview {
        rows,
        frame,
        at: Some(Instant::now()),
    };
    json_reply(200, json!({"ok": true}))
}

/// POST /api/grid_save: store an animation on the clock (runs without the app).
/// Built-ins go by name; custom / drawn frames are uploaded; live-data ones (cpu) can't.
fn grid_save(ctx: &Ctx, body: &Map<String, Value>) -> Reply {
    let mut drawn = None;
    if let Some(f) = body.get("frames").filter(|f| truthy(f)) {
        match config::coerce("grid_custom", f) {
            Ok(v) => drawn = Some(v),
            Err(e) => return json_reply(400, json!({"error": format!("frames: {e}")})),
        }
    }
    let drawn = drawn.filter(truthy);
    let (anim, custom, fps, frame) = {
        let cfg = ctx.cfg.lock().unwrap();
        let anim = body
            .get("anim")
            .or(cfg.get("grid_anim"))
            .cloned()
            .unwrap_or(Value::Null);
        let custom = drawn.clone().or_else(|| cfg.get("grid_custom").cloned());
        let fps = body
            .get("fps")
            .or(cfg.get("grid_fps"))
            .cloned()
            .unwrap_or(Value::Null);
        let frame = body
            .get("frame")
            .or(cfg.get("grid_frame"))
            .is_some_and(truthy);
        (anim, custom, fps, frame)
    };
    let fps = match py_int(&fps) {
        Ok(f) => f.clamp(0, u32::MAX as i64) as u32,
        Err(e) => return json_reply(400, json!({"error": format!("fps: {e}")})),
    };
    let job = if drawn.is_some() || anim == "custom" {
        let frames = match custom.as_ref().filter(|c| truthy(c)).map(to_frames) {
            None => return json_reply(400, json!({"error": "no frames drawn yet"})),
            Some(Err(e)) => return json_reply(400, json!({"error": format!("frames: {e}")})),
            Some(Ok(f)) => f,
        };
        UploadJob::Frames { frames, fps, frame }
    } else if let Some(name) = anim.as_str().filter(|a| BUILTIN_GRID.contains(a)) {
        UploadJob::Anim {
            name: name.to_string(),
            fps,
            frame,
        }
    } else {
        let name = py::str_value(&anim);
        return json_reply(
            400,
            json!({"error": format!("'{name}' needs live data; can't store it standalone")}),
        );
    };
    let queued = match &job {
        UploadJob::Frames { frames, .. } => frames.len(),
        UploadJob::Anim { .. } => 1,
    };
    ctx.uploads.lock().unwrap().push_back(job);
    json_reply(200, json!({"queued": queued}))
}

/// Validated grid frames (grid_custom format) as row arrays.
fn to_frames(v: &Value) -> Result<Vec<[u8; 7]>, String> {
    let v = config::coerce("grid_custom", v)?;
    Ok(v.as_array()
        .map(|frames| {
            frames
                .iter()
                .map(|f| {
                    let mut rows = [0u8; 7];
                    for (slot, x) in rows.iter_mut().zip(f.as_array().into_iter().flatten()) {
                        *slot = x.as_u64().unwrap_or(0) as u8;
                    }
                    rows
                })
                .collect()
        })
        .unwrap_or_default())
}

/// POST /api/om_save (kept for older pages): same as setting the clock's offline_text.
fn om_save(ctx: &Ctx, body: &Map<String, Value>) -> Reply {
    let text = match body.get("text") {
        Some(v) => v.clone(),
        None => ctx.clock_setting("offline_text", json!("")),
    };
    let Value::String(text) = text else {
        return json_reply(400, json!({"error": "text: expected a string"}));
    };
    let text = crate::font14::clean_omtext(&text);
    let mut patch = Map::new();
    patch.insert("offline_text".into(), json!(text));
    match ctx.set_clock(patch, CLOCK_TIMEOUT) {
        Ok(_) => json_reply(200, json!({"queued": 1, "text": text})),
        Err(info) => json_reply(400, json!({"error": info})),
    }
}

/// POST /api/clock_password {"password": "..."}: set ("" = remove) the clock's web password.
/// USB only — physical access is what allows it; the clock checks it again (WEBPASS).
fn clock_password(ctx: &Ctx, body: &Map<String, Value>) -> Reply {
    let Some(Value::String(pw)) = body.get("password") else {
        return json_reply(400, json!({"error": {"password": "expected a string"}}));
    };
    if pw.len() > 64 || !pw.bytes().all(|b| b > 32 && b < 127) {
        return json_reply(400, json!({"error": {"password": "up to 64 printable characters, no spaces"}}));
    }
    match ctx.set_web_password(pw, CLOCK_TIMEOUT) {
        Ok(_) => json_reply(200, json!({"ok": true, "set": !pw.is_empty()})),
        Err(info) => json_reply(400, json!({"error": info})),
    }
}

/// Python truthiness of a JSON value.
fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|x| x != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// Python `int(x)` of a JSON value.
fn py_int(v: &Value) -> Result<i64, String> {
    match v {
        Value::Bool(b) => Ok(*b as i64),
        Value::Number(n) => match n.as_i64() {
            Some(i) => Ok(i),
            None => py::int_from_float(n.as_f64().unwrap_or(f64::NAN)),
        },
        Value::String(s) => py::int_str(s)
            .and_then(|(neg, d)| d.parse::<i64>().ok().map(|x| if neg { -x } else { x }))
            .ok_or_else(|| format!("invalid literal for int() with base 10: '{s}'")),
        _ => Err("expected a number".into()),
    }
}
