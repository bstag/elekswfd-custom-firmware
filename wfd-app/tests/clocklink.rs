//! ClockLink on simulated time against a fake serial port: port of
//! wfd-control/tests/test_clocklink.py, the ClockLink parts of test_protocol.py and
//! test_firmware_update.py, and the golden log parity check.

#[path = "clocklink_sim/mod.rs"]
mod sim;

use serde_json::json;
use sim::{Sim, app, clock_request, set_cfg};
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use wfd_app::ctx::{Ctx, UploadJob};

fn golden_path() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/clocklink_golden.log")
}

/// 45 s of everything at once: rotate mode, icons, grid, uploads, a preview, a sync, an unplug.
fn busy_scenario(ctx: &Arc<Ctx>) -> Sim {
    set_cfg(
        ctx,
        json!({"message_mode": "rotate", "rotate_weather": true, "rotate_window": true,
               "rotate_text": true, "rotate_date": true, "rotate_dwell": 6,
               "custom_text": "HI {time} {day}", "icon_mode": "system", "grid_enable": true,
               "grid_anim": "spin", "grid_frame": true, "grid_fps": 12, "logo": true}),
    );
    Sim::new(ctx, 45.0)
        .unplug_after(400)
        .event(20.0, |s| {
            let mut p = s.ctx.grid_preview.lock().unwrap();
            p.rows = [1, 2, 4, 8, 16, 32, 64];
            p.frame = true;
            p.at = Some(s.instant_now());
        })
        .event(25.0, |s| {
            // e.g. brightness changed in the app's UI (the answer isn't awaited here)
            std::mem::forget(clock_request(&s.ctx, json!({"brightness": 9})));
        })
        .event(30.0, |s| {
            s.ctx.uploads.lock().unwrap().push_back(UploadJob::Frames {
                frames: vec![[1; 7], [2; 7], [4; 7]],
                fps: 9,
                frame: true,
            });
        })
        .event(35.0, |s| s.ctx.sync_now.store(true, Ordering::Relaxed))
}

/// Every line sent in the busy scenario matches the log recorded from app.py.
/// WFD_UPDATE_GOLDEN=1 writes the Rust output next to the golden file (…golden.log.new).
#[test]
fn matches_golden_log() {
    let ctx = app();
    let lines = busy_scenario(&ctx).run();
    let golden = std::fs::read_to_string(golden_path()).unwrap();
    let expected: Vec<&str> = golden.lines().collect();
    if std::env::var_os("WFD_UPDATE_GOLDEN").is_some() {
        std::fs::write(
            golden_path().with_extension("log.new"),
            lines.join("\n") + "\n",
        )
        .unwrap();
    }
    let first_diff = lines.iter().zip(&expected).position(|(a, b)| a != b);
    if let Some(i) =
        first_diff.or((lines.len() != expected.len()).then(|| lines.len().min(expected.len())))
    {
        let lo = i.saturating_sub(3);
        panic!(
            "serial output differs from app.py at line {} ({} lines vs {} golden)\n--- golden\n{}\n--- now\n{}",
            i + 1,
            lines.len(),
            expected.len(),
            expected[lo..(i + 5).min(expected.len())].join("\n"),
            lines[lo..(i + 5).min(lines.len())].join("\n"),
        );
    }
}

/// The clock owns its settings: on (re)connect the app asks for them and pushes none.
#[test]
fn reconnect_reads_the_clock_and_overwrites_nothing() {
    let ctx = app();
    let s = Sim::new(&ctx, 10.0).unplug_after(200);
    s.st.borrow_mut()
        .clock_cfg
        .insert("brightness".into(), json!(3)); // changed on the clock's dashboard
    s.run();
    assert_eq!(s.st.borrow().opens, 2);
    for port in ["COM1", "COM2"] {
        let sent = s.commands(Some(port));
        assert!(
            sent.iter().any(|c| c == "SET") && sent.iter().any(|c| c == "CFG"),
            "{port}"
        );
        for cmd in [
            "STYLE", "BRIGHT", "CLK24", "GAUTO", "OMAUTO", "GEO", "CFGSET",
        ] {
            assert!(!sent.iter().any(|c| c == cmd), "{cmd} sent on {port}");
        }
    }
    assert_eq!(ctx.state().clock.as_ref().unwrap()["brightness"], 3);
}

#[test]
fn changes_made_on_the_clock_show_up() {
    let ctx = app();
    let s = Sim::new(&ctx, 12.0).event(3.0, |s| {
        s.st.borrow_mut()
            .clock_cfg
            .insert("standalone_anim".into(), json!("rain"));
    });
    s.run();
    assert_eq!(
        ctx.state().clock.as_ref().unwrap()["standalone_anim"],
        "rain",
        "re-read every 5 s"
    );
}

#[test]
fn location_goes_to_the_clock_only_if_it_has_none() {
    let ctx = app();
    ctx.state().geo = Some(wfd_app::weather::Geo {
        lat: 31.5,
        lon: -97.1,
        unit: 0,
    });
    let s = Sim::new(&ctx, 2.0);
    s.run();
    assert!(!s.commands(None).contains(&"GEO".to_string()));
    let s = Sim::new(&ctx, 2.0);
    s.st.borrow_mut()
        .clock_cfg
        .insert("location".into(), json!(""));
    s.run();
    assert!(s.sent().contains(&"GEO 31.5 -97.1 0".to_string()));
}

#[test]
fn location_keeps_python_float_text() {
    let ctx = app();
    ctx.state().geo = Some(wfd_app::weather::Geo {
        lat: 31.0,
        lon: -97.0,
        unit: 1,
    });
    let s = Sim::new(&ctx, 1.0);
    s.st.borrow_mut()
        .clock_cfg
        .insert("location".into(), json!(""));
    s.run();
    let geo: Vec<String> = s
        .sent()
        .into_iter()
        .filter(|l| l.starts_with("GEO"))
        .collect();
    assert_eq!(
        geo,
        ["GEO 31.0 -97.0 1"],
        "sent once (deduped), floats like Python's str()"
    );
}

#[test]
fn clock_settings_round_trip() {
    let ctx = app();
    let ok = clock_request(&ctx, json!({"brightness": 9, "standalone_anim": "heart"}));
    let bad = clock_request(&ctx, json!({"brightness": 99}));
    let s = Sim::new(&ctx, 1.0);
    s.run();
    assert!(
        s.sent()
            .contains(&r#"CFGSET {"brightness":9,"standalone_anim":"heart"}"#.to_string())
    );
    assert_eq!(ok.try_recv().unwrap(), Ok(None));
    assert_eq!(
        bad.try_recv().unwrap(),
        Err(json!({"brightness": "expected an integer 1..16"}))
    );
    let st = ctx.state();
    let clk = st.clock.as_ref().unwrap();
    assert_eq!(
        (&clk["brightness"], &clk["standalone_anim"]),
        (&json!(9), &json!("heart"))
    );
}

#[test]
fn set_clock_through_the_link_thread() {
    // Ctx::set_clock (used by the web server) waits for the link thread's answer
    let ctx = app();
    ctx.state().connected = true;
    let c2 = Arc::clone(&ctx);
    let waiter = std::thread::spawn(move || {
        c2.set_clock(
            json!({"brightness": 0}).as_object().unwrap().clone(),
            std::time::Duration::from_secs(10),
        )
    });
    while ctx.clock_requests.lock().unwrap().is_empty() {
        std::thread::yield_now();
    }
    Sim::new(&ctx, 1.0).run();
    assert_eq!(
        waiter.join().unwrap(),
        Err(json!({"brightness": "expected an integer 1..16"}))
    );
}

#[test]
fn uploads_send_clear_add_commit_then_reread_settings() {
    let ctx = app();
    ctx.uploads.lock().unwrap().push_back(UploadJob::Frames {
        frames: vec![[1; 7], [2; 7]],
        fps: 12,
        frame: true,
    });
    let s = Sim::new(&ctx, 1.0);
    s.run();
    let sent = s.sent();
    let i = sent.iter().position(|l| l == "GCLR").unwrap();
    assert_eq!(
        sent[i..i + 4],
        [
            "GCLR",
            "GADD 1 1 1 1 1 1 1",
            "GADD 2 2 2 2 2 2 2",
            "GCOMMIT 12 1"
        ]
    );
    assert!(
        sent[i + 4..].contains(&"CFG".to_string()),
        "GCOMMIT changes the clock's settings: read them again"
    );
    assert!(!s.commands(None).contains(&"GAUTO".to_string()));
    assert_eq!(ctx.state().saved, "grid: 2 frames");
}

#[test]
fn builtin_grid_animation_is_sent_by_name() {
    let ctx = app();
    ctx.uploads.lock().unwrap().push_back(UploadJob::Anim {
        name: "life".into(),
        fps: 6,
        frame: false,
    });
    let s = Sim::new(&ctx, 1.0);
    s.run();
    let sent = s.sent();
    let i = sent
        .iter()
        .position(|l| l == "GANIM life 6 0")
        .expect("GANIM sent");
    assert!(
        sent[i..].contains(&"CFG".to_string()),
        "GANIM changes the clock's settings: read them again"
    );
    assert!(!sent.contains(&"GCLR".to_string()));
    assert_eq!(ctx.state().saved, "grid: life");
}

#[test]
fn temp_placeholder() {
    let ctx = app();
    set_cfg(
        &ctx,
        json!({"message_mode": "text", "custom_text": "{temp}"}),
    );
    Sim::new(&ctx, 1.0).run();
    assert_eq!(ctx.state().message, "92F");
}

#[test]
fn all_placeholders() {
    // 16:00:00 on Tue 13 Oct 2026, as the Python harness's START
    let ctx = app();
    set_cfg(
        &ctx,
        json!({"message_mode": "text",
                         "custom_text": " {date}|{time}|{time12}|{day}|{temp} "}),
    );
    Sim::new(&ctx, 0.1).run();
    assert_eq!(ctx.state().message, "TUE OCT 13|16:00|4:00PM|TUESDAY|92F");
    ctx.state().weather.label = String::new();
    set_cfg(&ctx, json!({"custom_text": "{temp}"}));
    Sim::new(&ctx, 0.1).run();
    assert_eq!(ctx.state().message, "--", "no weather yet");
}

#[test]
fn sync_now_pushes_time() {
    let ctx = app();
    let s = Sim::new(&ctx, 5.0).event(3.0, |s| s.ctx.sync_now.store(true, Ordering::Relaxed));
    s.run();
    let sets: Vec<String> = s
        .lines()
        .into_iter()
        .filter(|l| l.contains(" SET "))
        .collect();
    assert_eq!(sets.len(), 2);
    assert!(sets[1].starts_with("   3."), "{}", sets[1]);
    assert!(!ctx.sync_now.load(Ordering::Relaxed));
    assert_eq!(sets[0], "   0.000 COM1 SET 26 10 13 2 16 0 0");
}

#[test]
fn spectrum_off_sends_zero_levels() {
    let ctx = app();
    set_cfg(&ctx, json!({"spectrum": false}));
    let s = Sim::new(&ctx, 2.0);
    s.run();
    let specs: BTreeSet<String> = s
        .sent()
        .into_iter()
        .filter(|l| l.starts_with("SPEC "))
        .collect();
    assert_eq!(specs, BTreeSet::from(["SPEC 0 0".to_string()]));
    assert!(
        ctx.state().levels.out > 0,
        "levels are still computed for the icon row"
    );
}

#[test]
fn weather_mode_leaves_message_line_to_firmware() {
    let ctx = app();
    set_cfg(&ctx, json!({"message_mode": "weather"}));
    let s = Sim::new(&ctx, 3.0);
    s.run();
    let cmds = s.commands(None);
    assert!(!cmds.contains(&"MSG".to_string()));
    assert!(cmds.contains(&"WX".to_string()));
}

#[test]
fn grid_off_clears_the_preview_rows() {
    let ctx = app();
    ctx.state().grid_rows = Some([1; 7]);
    let s = Sim::new(&ctx, 1.0);
    s.run();
    assert!(!s.commands(None).contains(&"GRID".to_string()));
    assert_eq!(
        ctx.state().grid_rows,
        None,
        "app grid off: the clock shows its own"
    );
}

#[test]
fn state_shows_the_connection_and_time() {
    let ctx = app();
    Sim::new(&ctx, 2.0).run();
    let st = ctx.state();
    assert!(st.connected);
    assert_eq!(st.port.as_deref(), Some("COM1"));
    assert_eq!(st.link.as_deref(), Some("usb"));
    assert_eq!(st.time.as_deref(), Some("Tue 16:00:01")); // last frame before 2 s
}

#[test]
fn no_clock_no_connection() {
    let ctx = app();
    let s = Sim::new(&ctx, 6.0);
    s.st.borrow_mut().usb_present = false;
    s.run();
    assert!(s.lines().is_empty());
    let st = ctx.state();
    assert!(!st.connected && st.port.is_none() && st.link.is_none());
}

// ------------------------------------------------------------------ test_firmware_update.py

#[test]
fn version_is_read_over_usb_and_kept_out_of_settings() {
    let ctx = app();
    Sim::new(&ctx, 1.0).run();
    let st = ctx.state();
    assert_eq!(
        st.clock_fw.as_deref(),
        Some("wfd-clock Oct 13 2026 19:57:32")
    );
    assert_eq!(st.clock_protocol, Some(3));
    let clk = st.clock.as_ref().unwrap();
    assert!(!clk.contains_key("_fw") && !clk.contains_key("_protocol"));
}

// ------------------------------------------------------------------ test_protocol.py

fn main_cpp() -> String {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../wfd-firmware/clock/src/main.cpp");
    std::fs::read_to_string(p).unwrap()
}

/// Every `"word"` that follows `marker` in `src`.
fn quoted_after(src: &str, marker: &str) -> BTreeSet<String> {
    src.match_indices(marker)
        .filter_map(|(i, _)| {
            let rest = &src[i + marker.len()..];
            let end = rest.find('"')?;
            let w = &rest[..end];
            (!w.is_empty() && w.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'))
                .then(|| w.to_string())
        })
        .collect()
}

#[test]
fn firmware_understands_everything_the_app_sends() {
    let ctx = app();
    set_cfg(
        &ctx,
        json!({"message_mode": "rotate", "rotate_date": true, "grid_enable": true,
                         "icon_mode": "system", "logo": true}),
    );
    std::mem::forget(clock_request(&ctx, json!({"offline_text": "HELLO WORLD"})));
    {
        let mut up = ctx.uploads.lock().unwrap();
        up.push_back(UploadJob::Frames {
            frames: vec![[1; 7]],
            fps: 8,
            frame: false,
        });
        up.push_back(UploadJob::Anim {
            name: "spin".into(),
            fps: 8,
            frame: false,
        });
    }
    let s = Sim::new(&ctx, 30.0).event(10.0, |s| s.ctx.sync_now.store(true, Ordering::Relaxed));
    s.run();
    let known = quoted_after(&main_cpp(), "cmd == \"");
    let unknown: Vec<String> = s
        .commands(None)
        .into_iter()
        .filter(|c| !known.contains(c))
        .collect();
    assert!(
        unknown.is_empty(),
        "app sends commands the firmware doesn't handle: {unknown:?}"
    );
}

/// Text written once should read the same on the clock (app off) and from the app.
#[test]
fn app_expands_every_firmware_placeholder() {
    let src = main_cpp();
    let found: BTreeSet<String> = src
        .match_indices("s.replace(\"{")
        .filter_map(|(i, m)| {
            let rest = &src[i + m.len()..];
            rest.find('}').map(|e| rest[..e].to_string())
        })
        .collect();
    assert!(
        ["time", "time12", "date", "day", "temp"]
            .iter()
            .all(|n| found.contains(*n)),
        "{found:?}"
    );
    let ctx = app();
    let text: Vec<String> = found.iter().map(|n| format!("{{{n}}}")).collect();
    set_cfg(
        &ctx,
        json!({"message_mode": "text", "custom_text": text.join(" ")}),
    );
    Sim::new(&ctx, 1.0).run();
    let msg = ctx.state().message.clone();
    assert!(!msg.contains('{'), "{msg}");
}
