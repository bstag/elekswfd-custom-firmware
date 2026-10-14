//! Is the clock plugged in but missing its USB driver? Then it has no COM port, so `find_port`
//! sees nothing and the app would only say "not connected". Windows normally installs the CH340
//! driver itself (Windows Update); when it hasn't, the device sits in Device Manager with a
//! problem code (28 = no driver installed). `check()` spots that so the dashboard can say so.
//!
//! The decision (`diagnose`) is plain logic over a device list; only listing the present USB
//! devices (`usb_devices`) talks to Windows (SetupAPI + the configuration manager).

/// WCH's download page for the CH340/CH341 Windows driver (CH341SER.EXE).
pub const DRIVER_URL: &str = "https://www.wch-ic.com/downloads/CH341SER_EXE.html";

/// One present USB device: its instance ID (`USB\VID_1A86&PID_7523\...`) and Windows' view of it.
#[derive(Clone, Debug, PartialEq)]
pub struct UsbDevice {
    pub instance_id: String,
    pub problem: u32,         // CM_PROB_* (0 = none, 28 = no driver installed)
    pub driver_loaded: bool,  // DN_DRIVER_LOADED
}

const CH340_ID: &str = "VID_1A86&PID_7523";
const CM_PROB_FAILED_INSTALL: u32 = 28;

/// What's wrong with a plugged-in CH340, if anything (None: not plugged in, or working).
pub fn diagnose(devices: &[UsbDevice]) -> Option<String> {
    let dev = devices
        .iter()
        .find(|d| d.instance_id.to_ascii_uppercase().contains(CH340_ID))?;
    match dev.problem {
        CM_PROB_FAILED_INSTALL => Some("the clock is plugged in, but Windows has no driver for it (CH340)".into()),
        0 if dev.driver_loaded => None,
        0 => Some("the clock is plugged in, but its USB driver (CH340) isn't loaded".into()),
        code => Some(format!("the clock is plugged in, but Windows reports a problem with its USB driver (CH340, code {code})")),
    }
}

/// `diagnose` for the devices plugged in right now.
pub fn check() -> Option<String> {
    diagnose(&usb_devices())
}

#[cfg(windows)]
pub fn usb_devices() -> Vec<UsbDevice> {
    use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
        CM_Get_DevNode_Status, CR_SUCCESS, DIGCF_ALLCLASSES, DIGCF_PRESENT, DN_DRIVER_LOADED, SP_DEVINFO_DATA,
        SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInfo, SetupDiGetClassDevsW, SetupDiGetDeviceInstanceIdW,
    };
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;

    let enumerator: Vec<u16> = "USB\0".encode_utf16().collect();
    let mut out = Vec::new();
    // SAFETY: plain SetupAPI enumeration; every buffer is sized and owned here, and the list is
    // destroyed before returning.
    unsafe {
        let set = SetupDiGetClassDevsW(
            std::ptr::null(),
            enumerator.as_ptr(),
            std::ptr::null_mut(),
            DIGCF_ALLCLASSES | DIGCF_PRESENT,
        );
        if set as isize == INVALID_HANDLE_VALUE as isize {
            return out;
        }
        let mut i = 0;
        loop {
            let mut info: SP_DEVINFO_DATA = std::mem::zeroed();
            info.cbSize = std::mem::size_of::<SP_DEVINFO_DATA>() as u32;
            if SetupDiEnumDeviceInfo(set, i, &mut info) == 0 {
                break;
            }
            i += 1;
            let mut buf = [0u16; 512];
            let mut len = 0u32;
            if SetupDiGetDeviceInstanceIdW(set, &info, buf.as_mut_ptr(), buf.len() as u32, &mut len) == 0 {
                continue;
            }
            let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            let (mut status, mut problem) = (0u32, 0u32);
            if CM_Get_DevNode_Status(&mut status, &mut problem, info.DevInst, 0) != CR_SUCCESS {
                continue;
            }
            out.push(UsbDevice {
                instance_id: String::from_utf16_lossy(&buf[..end]),
                problem,
                driver_loaded: status & DN_DRIVER_LOADED != 0,
            });
        }
        SetupDiDestroyDeviceInfoList(set);
    }
    out
}

#[cfg(not(windows))]
pub fn usb_devices() -> Vec<UsbDevice> {
    Vec::new()
}
