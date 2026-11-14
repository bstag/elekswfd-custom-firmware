//! Cross-checks between the app and the firmware source, so the two can't drift apart: the
//! protocol number, which settings the clock owns, the animation and icon lists, and what the
//! shared web pages send. (These used to run only in the retired Python app's test suite.)

use regex::Regex;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use wfd_app::config::{CLOCK_KEYS, PROTOCOL, Settings};
use wfd_app::icons::ICON_NAMES;
use wfd_app::web::BUILTIN_GRID;

fn firmware() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../wfd-firmware")
}

/// Every `*.cpp` and `*.h` of the firmware as one text (not the generated web pages).
fn firmware_source() -> String {
    let dir = firmware().join("clock/src");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            let ext = p.extension().and_then(|e| e.to_str());
            matches!(ext, Some("cpp" | "h")) && p.file_name().unwrap() != "web_pages.h"
        })
        .collect();
    files.sort();
    files
        .iter()
        .map(|p| std::fs::read_to_string(p).unwrap())
        .collect::<Vec<_>>()
        .join("\n")
}

fn page(name: &str) -> String {
    std::fs::read_to_string(firmware().join("clock/web").join(name)).unwrap()
}

fn all<'a>(re: &str, text: &'a str) -> BTreeSet<&'a str> {
    Regex::new(re)
        .unwrap()
        .captures_iter(text)
        .map(|c| c.get(1).unwrap().as_str())
        .collect()
}

#[test]
fn app_and_shipped_firmware_speak_the_same_protocol() {
    let src = firmware_source();
    let fw: u32 = all(r"#define WFD_PROTOCOL (\d+)", &src)
        .into_iter()
        .next()
        .expect("WFD_PROTOCOL in the firmware")
        .parse()
        .unwrap();
    assert_eq!(PROTOCOL, fw, "bump config::PROTOCOL together with WFD_PROTOCOL");
    let shipped: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(firmware().join("releases/version.json")).unwrap()).unwrap();
    assert_eq!(
        shipped["protocol"].as_u64(),
        Some(u64::from(fw)),
        "releases/ is stale: run wfd-firmware/make_release.py"
    );
}

#[test]
fn clock_owned_settings_match_the_firmware() {
    // the app forwards exactly the settings the clock reports (configJson), no more, no fewer
    let src = firmware_source();
    let fw = all(r#"\bc\["(\w+)"\]"#, &src);
    let app: BTreeSet<&str> = CLOCK_KEYS.into_iter().collect();
    assert_eq!(app, fw);
    let both: Vec<&&str> = CLOCK_KEYS.iter().filter(|k| Settings::NAMES.contains(k)).collect();
    assert!(both.is_empty(), "a setting has one owner: {both:?}");
}

#[test]
fn builtin_grid_list_matches_the_firmware() {
    let header = std::fs::read_to_string(firmware().join("clock/src/grid_engine.h")).unwrap();
    let list = Regex::new(r"NAMES\[N_ANIM\]\s*=\s*\{([^}]*)\}")
        .unwrap()
        .captures(&header)
        .expect("NAMES[N_ANIM] in grid_engine.h");
    let names: Vec<&str> = Regex::new(r#""(\w+)""#)
        .unwrap()
        .captures_iter(list.get(1).unwrap().as_str())
        .map(|c| c.get(1).unwrap().as_str())
        .collect();
    let mut want = vec!["custom"];
    want.extend(BUILTIN_GRID);
    assert_eq!(names, want);
}

#[test]
fn icon_count_matches_the_firmware() {
    let panel = std::fs::read_to_string(firmware().join("clock/src/clock_panel.h")).unwrap();
    let n: usize = all(r"#define\s+N_ICONS\s+(\d+)", &panel)
        .into_iter()
        .next()
        .expect("N_ICONS in clock_panel.h")
        .parse()
        .unwrap();
    assert_eq!(ICON_NAMES.len(), n);
}

/// Setting names a page sends: `set({ key: ... })`, `set({ [id]: ... })` loops over literal
/// lists, and `slider(..., 'key')`.
fn sent_keys(html: &str) -> BTreeSet<String> {
    let mut sent: BTreeSet<String> = all(r"set\(\{\s*(\w+):", html)
        .into_iter()
        .chain(all(r"\bv => \(\{\s*(\w+):", html)) // seg() buttons
        .chain(all(r"slider\('[^']+', '[^']+', '[^']*', '(\w+)'", html))
        .map(str::to_string)
        .collect();
    for ids in all(r"for \(const \w+ of \[([^\]]*)\]\)[^;]{0,160}?set\(\{ \[", html) {
        sent.extend(all(r"'(\w+)'", ids).into_iter().map(str::to_string));
    }
    sent
}

#[test]
fn shared_pages_only_send_known_settings() {
    // the dashboard and the animator talk to the clock and to the app: every setting they send
    // must have an owner (an app setting, or one of the clock's)
    let mut known: BTreeSet<String> = Settings::NAMES.iter().map(|k| k.to_string()).collect();
    known.extend(CLOCK_KEYS.iter().map(|k| k.to_string()));
    for name in ["dashboard.html", "draw.html"] {
        let sent = sent_keys(&page(name));
        let orphans: Vec<&String> = sent.difference(&known).collect();
        assert!(orphans.is_empty(), "{name} sends settings nobody owns: {orphans:?}");
    }
    assert!(sent_keys(&page("dashboard.html")).len() >= 25);
}

#[test]
fn every_clock_setting_is_on_the_dashboard() {
    let html = page("dashboard.html");
    let missing: Vec<&str> = CLOCK_KEYS
        .into_iter()
        .filter(|k| !Regex::new(&format!(r"\b{k}\b")).unwrap().is_match(&html))
        .collect();
    assert!(missing.is_empty(), "clock settings with no control on the dashboard: {missing:?}");
}
