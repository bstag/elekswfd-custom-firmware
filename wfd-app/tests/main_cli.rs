//! The wfd-app command line. (The app itself isn't started here: it would open the clock's
//! COM port and the network.)

use std::path::PathBuf;
use std::process::Command;

fn exe() -> Command {
    Command::new(env!("CARGO_BIN_EXE_wfd-app"))
}

#[test]
fn help_lists_the_flags() {
    let out = exe().arg("--help").output().unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    for flag in ["--no-browser", "--config", "--addr", "127.0.0.1:8737"] {
        assert!(text.contains(flag), "{flag} missing from:\n{text}");
    }
}

#[test]
fn unknown_flags_are_refused() {
    let out = exe().arg("--bogus").output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("--bogus"));
}

#[test]
fn missing_flag_values_are_refused() {
    for flag in ["--config", "--addr"] {
        let out = exe().arg(flag).output().unwrap();
        assert_eq!(out.status.code(), Some(2), "{flag}");
    }
}

#[test]
fn the_shipped_release_is_readable() {
    // main embeds this file; it must parse the way app.py load_release reads it
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../wfd-firmware/releases");
    let rel = wfd_app::release::load_release(&dir).expect("version.json");
    assert!(rel.fw.starts_with("wfd-clock"), "{}", rel.fw);
    assert!(rel.protocol >= 3);
    assert!(dir.join("wfd-clock-firmware.bin").metadata().unwrap().len() > 100_000);
}
