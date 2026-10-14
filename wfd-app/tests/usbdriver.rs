//! Spotting a clock that's plugged in without a working USB driver (src/usbdriver.rs).
use wfd_app::usbdriver::{UsbDevice, check, diagnose, usb_devices};

fn dev(id: &str, problem: u32, driver_loaded: bool) -> UsbDevice {
    UsbDevice { instance_id: id.into(), problem, driver_loaded }
}

const CH340: &str = r"USB\VID_1A86&PID_7523\9&577993C&0&3";
const HUB: &str = r"USB\ROOT_HUB30\4&1A2B3C&0&0";

#[test]
fn nothing_plugged_in_is_not_a_problem() {
    assert_eq!(diagnose(&[]), None);
    assert_eq!(diagnose(&[dev(HUB, 0, true)]), None);
}

#[test]
fn a_working_clock_is_not_a_problem() {
    assert_eq!(diagnose(&[dev(HUB, 0, true), dev(CH340, 0, true)]), None);
}

#[test]
fn a_clock_without_a_driver_says_so() {
    let msg = diagnose(&[dev(HUB, 0, true), dev(CH340, 28, false)]).unwrap();
    assert!(msg.contains("no driver"), "{msg}");
}

#[test]
fn other_driver_problems_give_the_code() {
    let msg = diagnose(&[dev(CH340, 10, false)]).unwrap();
    assert!(msg.contains("code 10"), "{msg}");
    let msg = diagnose(&[dev(CH340, 0, false)]).unwrap();
    assert!(msg.contains("isn't loaded"), "{msg}");
}

#[test]
fn ids_match_whatever_the_case() {
    assert!(diagnose(&[dev(&CH340.to_lowercase(), 28, false)]).is_some());
}

#[test]
fn other_problem_devices_are_ignored() {
    assert_eq!(diagnose(&[dev(r"USB\VID_046D&PID_C52B\5&1", 28, false)]), None);
}

/// The real list (Windows): runs anywhere; with the clock plugged in and its driver working
/// (as on the dev PC) it must find it and see no problem.
#[test]
fn the_live_device_list_works() {
    let devices = usb_devices();
    if cfg!(windows)
        && devices.iter().any(|d| d.instance_id.contains("VID_1A86&PID_7523"))
        && wfd_app::link::find_port().is_some()
    {
        assert_eq!(check(), None, "clock has a COM port, so its driver works");
    }
}
