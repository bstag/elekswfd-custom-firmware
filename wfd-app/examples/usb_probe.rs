//! List the present USB devices as the app sees them, and what it makes of the clock's driver.
//!     cargo run --example usb_probe
use wfd_app::usbdriver::{check, usb_devices};

fn main() {
    for d in usb_devices() {
        println!("{:<60} problem={:<3} driver_loaded={}", d.instance_id, d.problem, d.driver_loaded);
    }
    println!("clock port: {:?}", wfd_app::link::find_port());
    println!("driver check: {}", check().unwrap_or_else(|| "ok (no problem found)".into()));
}
