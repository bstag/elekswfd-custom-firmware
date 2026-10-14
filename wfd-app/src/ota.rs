//! ota: "Update clock" (port of `update_clock` / `upload_firmware` / `wait_for_build` in
//! wfd-control/app.py). Finds the clock on WiFi, POSTs the firmware the app ships to its
//! `/update` page and waits until it reports the new build, with one retry. Progress goes to
//! `state.update` (idle | uploading | done | failed) for the dashboard.

use crate::ctx::Ctx;
use crate::link::basic_auth;
use crate::release::multipart;
use crate::state::Update;
use serde_json::Value;
use std::io;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

const UPLOAD_TIMEOUT: Duration = Duration::from_secs(180);
const STATUS_TIMEOUT: Duration = Duration::from_secs(3);
/// How long the clock gets to restart with the new build, per attempt.
const RESTART_WAIT: Duration = Duration::from_secs(45);

/// Why an upload didn't go through.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OtaError {
    /// The clock answered and said no (wrong password, refused): give up.
    Refused(String),
    /// No (complete) answer: the reply can get lost in the restart, so check the result instead.
    Transport(String),
}

/// POST the app image to the clock's `/update` (same as its web page). `host` is an IP or name,
/// optionally with `:port`.
pub fn upload_firmware(host: &str, data: &[u8], password: &str) -> Result<(), OtaError> {
    let (body, ctype) = multipart("fw", "wfd-clock-firmware.bin", data);
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(UPLOAD_TIMEOUT))
        .http_status_as_error(false)
        .build()
        .into();
    let mut req = agent
        .post(format!("http://{host}/update"))
        .header("Content-Type", ctype);
    if !password.is_empty() {
        req = req.header("Authorization", basic_auth(password));
    }
    match req.send(&body[..]) {
        Ok(resp) => match resp.status().as_u16() {
            200 => Ok(()),
            401 => Err(OtaError::Refused("wrong clock password".into())),
            code if code >= 400 => Err(OtaError::Refused(format!(
                "clock refused the update (HTTP {code})"
            ))),
            code => Err(OtaError::Refused(format!("clock answered HTTP {code}"))),
        },
        // older firmware reboots before closing: the upload got there
        Err(ureq::Error::Io(e))
            if matches!(
                e.kind(),
                io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionAborted
            ) =>
        {
            Ok(())
        }
        Err(e) => Err(OtaError::Transport(e.to_string())),
    }
}

/// Poll the clock's /api/status until it reports build `fw` (true) or `timeout` runs out.
pub fn wait_for_build(host: &str, fw: &str, timeout: Duration) -> bool {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(STATUS_TIMEOUT))
        .build()
        .into();
    let url = format!("http://{host}/api/status");
    let end = Instant::now() + timeout;
    while Instant::now() < end {
        // errors are normal here: the clock is restarting / rejoining WiFi
        if let Ok(mut r) = agent.get(&url).call()
            && let Ok(v) = r.body_mut().read_json::<Value>()
            && v["state"]["fw"].as_str() == Some(fw)
        {
            return true;
        }
        thread::sleep(Duration::from_secs(2));
    }
    false
}

/// `find_clock_ip(ctx)`
pub type FindIp = Box<dyn Fn(&Ctx) -> Option<String> + Send>;
/// `upload_firmware(host, data, password)`
pub type Upload = Box<dyn Fn(&str, &[u8], &str) -> Result<(), OtaError> + Send>;
/// `wait_for_build(host, fw, timeout)`
pub type WaitForBuild = Box<dyn Fn(&str, &str, Duration) -> bool + Send>;

/// The steps of an update, replaceable for tests.
pub struct OtaHooks {
    pub find_clock_ip: FindIp,
    pub upload_firmware: Upload,
    pub wait_for_build: WaitForBuild,
}

impl OtaHooks {
    pub fn real() -> Self {
        OtaHooks {
            find_clock_ip: Box::new(crate::link::find_clock_ip),
            upload_firmware: Box::new(upload_firmware),
            wait_for_build: Box::new(wait_for_build),
        }
    }
}

/// Start uploading the shipped firmware to the clock in the background; false if one is running.
pub fn update_clock(ctx: &Arc<Ctx>) -> bool {
    update_clock_with(ctx, OtaHooks::real())
}

fn set_update(ctx: &Ctx, state: &str, message: impl Into<String>) {
    ctx.state().update = Update {
        state: state.into(),
        message: message.into(),
    };
}

/// `update_clock` with its steps injected.
pub fn update_clock_with(ctx: &Arc<Ctx>, hooks: OtaHooks) -> bool {
    {
        // check and claim under one lock, so two clicks can't both start one
        let mut st = ctx.state();
        if st.update.state == "uploading" {
            return false;
        }
        st.update = Update {
            state: "uploading".into(),
            message: "looking for the clock…".into(),
        };
    }
    let ctx = Arc::clone(ctx);
    thread::spawn(move || match job(&ctx, &hooks) {
        Ok(msg) => set_update(&ctx, "done", msg),
        Err(msg) => set_update(&ctx, "failed", msg),
    });
    true
}

fn job(ctx: &Ctx, hooks: &OtaHooks) -> Result<String, String> {
    let ip = (hooks.find_clock_ip)(ctx)
        .ok_or("clock not found on WiFi (updates go over the network)")?;
    let data = ctx.firmware;
    let want = match &ctx.release {
        Some(r) if !data.is_empty() => r.fw.clone(),
        _ => return Err("no firmware shipped with the app".into()),
    };
    set_update(
        ctx,
        "uploading",
        format!("sending {} KB to {ip}…", data.len() / 1024),
    );
    let pw = ctx
        .cfg
        .lock()
        .unwrap()
        .get("clock_pass")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    for attempt in 1..=2 {
        // a stalled upload (busy WiFi) is worth one more try
        if attempt == 2 {
            set_update(ctx, "uploading", "no new firmware yet; sending it again…");
        }
        if let Err(OtaError::Refused(msg)) = (hooks.upload_firmware)(&ip, data, &pw) {
            return Err(msg);
        }
        set_update(ctx, "uploading", "waiting for the clock to restart…");
        if (hooks.wait_for_build)(&ip, &want, RESTART_WAIT) {
            return Ok(format!(
                "the clock now runs {}",
                want.replace("wfd-clock ", "")
            ));
        }
    }
    Err("the clock didn't come back with the new firmware (tried twice)".into())
}
