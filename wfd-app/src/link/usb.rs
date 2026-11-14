//! The USB transport: [`UsbLink`] over a [`SerialIo`], and the real serial port.

use super::{BAUD, Clock, Link, LinkLost, SerialIo, error_text, find_port, frame_cmd, is_slow_patch, per_key};
use crate::ctx::ClockReply;
use serde_json::{Map, Value};
use std::io::{self, Read, Write};
use std::rc::Rc;
use std::time::{Duration, Instant};

/// How long a USB settings request waits for its reply (app.py `request(timeout=1.5)`).
const REQUEST_TIMEOUT: f64 = 1.5;
/// ...and for a change the clock has to look up on the internet first (a ZIP: up to 6 s there).
const SLOW_REQUEST_TIMEOUT: f64 = 8.0;

/// The clock on USB serial: commands go out as they're made; settings via CFG / CFGSET.
pub struct UsbLink {
    s: Box<dyn SerialIo>,
    clock: Rc<dyn Clock>,
    name: String,
    fw: Option<String>,
    protocol: Option<u32>,
    ip: Option<String>,
    pw_set: Option<bool>,
    /// the clock said it has internet time (CFG `_ntp`; older firmware doesn't say: false)
    ntp: bool,
}

impl UsbLink {
    pub fn new(s: Box<dyn SerialIo>, clock: Rc<dyn Clock>) -> Self {
        let name = s.port_name();
        UsbLink {
            s,
            clock,
            name,
            fw: None,
            protocol: None,
            ip: None,
            pw_set: None,
            ntp: false,
        }
    }

    /// Send a command and wait for a reply line starting with one of `prefixes` + " ".
    /// Other lines (e.g. `OK` from earlier commands) are skipped.
    fn request(
        &mut self,
        line: &str,
        prefixes: &[&'static str],
        timeout: f64,
    ) -> Result<Option<(&'static str, Value)>, LinkLost> {
        self.send(line)?;
        let deadline = self.clock.now() + timeout;
        while self.clock.now() < deadline {
            let raw = self.s.readline()?;
            if raw.is_empty() {
                continue;
            }
            let text = String::from_utf8_lossy(&raw);
            let text = text.trim();
            for &pre in prefixes {
                if let Some(rest) = text.strip_prefix(pre).and_then(|r| r.strip_prefix(' ')) {
                    // app.py lets a garbled reply raise (logged, link kept); here it's skipped.
                    if let Ok(v) = serde_json::from_str(rest) {
                        return Ok(Some((pre, v)));
                    }
                }
            }
        }
        Ok(None)
    }
}

impl Link for UsbLink {
    fn kind(&self) -> &'static str {
        "usb"
    }

    fn name(&self) -> String {
        self.name.clone()
    }

    fn alive(&self) -> bool {
        self.s.is_open()
    }

    /// A clock with internet time keeps its own (in its own time zone setting): sending the
    /// PC's time as well made the two take turns when the zones differed.
    fn clock_ntp(&self) -> bool {
        self.ntp
    }

    fn version(&self) -> (Option<String>, Option<u32>) {
        (self.fw.clone(), self.protocol)
    }

    fn clock_ip(&self) -> Option<String> {
        self.ip.clone()
    }

    fn web_password_set(&self) -> Option<bool> {
        self.pw_set
    }

    fn set_web_password(&mut self, password: &str) -> Result<Result<bool, String>, LinkLost> {
        Ok(match self.request(&format!("WEBPASS {password}"), &["PWOK", "PWERR"], REQUEST_TIMEOUT)? {
            Some(("PWOK", body)) => {
                let set = body.get("set").and_then(Value::as_bool).unwrap_or(!password.is_empty());
                self.pw_set = Some(set);
                Ok(set)
            }
            Some((_, body)) => Err(body
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("the clock refused the password")
                .to_string()),
            None => Err("the clock didn't answer (older firmware? update the clock first)".into()),
        })
    }

    fn send(&mut self, line: &str) -> Result<(), LinkLost> {
        self.s.write(format!("{line}\n").as_bytes())?;
        Ok(())
    }

    fn flush(&mut self) -> Result<(), LinkLost> {
        Ok(())
    }

    fn get_config(&mut self) -> Result<Option<Map<String, Value>>, LinkLost> {
        let Some((_, Value::Object(mut c))) = self.request("CFG", &["CFG"], REQUEST_TIMEOUT)? else {
            return Ok(None);
        };
        // the build info travels with the settings but isn't one
        self.fw = c.remove("_fw").and_then(|v| v.as_str().map(str::to_owned));
        self.protocol = c
            .remove("_protocol")
            .and_then(|v| v.as_u64())
            .and_then(|p| u32::try_from(p).ok());
        self.ip = c
            .remove("_ip")
            .and_then(|v| v.as_str().map(str::to_owned))
            .filter(|ip| !ip.is_empty()); // "" = the clock isn't on WiFi
        self.pw_set = c.remove("_pw").and_then(|v| v.as_bool());
        self.ntp = c.remove("_ntp").and_then(|v| v.as_bool()).unwrap_or(false);
        Ok(Some(c))
    }

    fn set_config(&mut self, patch: &Map<String, Value>) -> Result<ClockReply, LinkLost> {
        // compact JSON (no newline can occur in it: the line ends the command)
        let line = format!("CFGSET {}", Value::Object(patch.clone()));
        let timeout = if is_slow_patch(patch) { SLOW_REQUEST_TIMEOUT } else { REQUEST_TIMEOUT };
        Ok(match self.request(&line, &["CFGOK", "CFGERR"], timeout)? {
            Some(("CFGOK", mut body)) => Ok(body
                .as_object_mut()
                .and_then(|b| b.remove("_warning"))
                .filter(|w| !w.is_null())),
            Some((_, body)) => match body.get("error") {
                Some(err @ Value::Object(_)) => Err(err.clone()),
                other => Err(per_key(patch, &error_text(other.unwrap_or(&Value::Null)))),
            },
            None => Err(per_key(patch, "clock didn't answer")),
        })
    }

    fn store_anim(&mut self, name: &str, fps: u32, frame: bool) -> Result<(), LinkLost> {
        self.send(&format!("GANIM {name} {fps} {}", u8::from(frame)))
    }

    fn store_frames(&mut self, frames: &[[u8; 7]], fps: u32, frame: bool) -> Result<(), LinkLost> {
        // paced like app.py so the clock's serial buffer keeps up
        self.send("GCLR")?;
        self.clock.sleep(0.03);
        for fr in frames.iter().take(80) {
            self.send(&frame_cmd("GADD", fr))?;
            self.clock.sleep(0.015);
        }
        self.send(&format!("GCOMMIT {fps} {}", u8::from(frame)))
    }

    fn after_time(&mut self) -> Result<(), LinkLost> {
        self.clock.sleep(0.15);
        self.s.reset_input_buffer()?;
        Ok(())
    }

    fn close(&mut self) {
        self.s.close();
    }
}

/// A real serial port with pyserial-like `readline` (1 s timeout).
pub struct RealSerial {
    port: Option<Box<dyn serialport::SerialPort>>,
    name: String,
    buf: Vec<u8>,
}

const READ_TIMEOUT: Duration = Duration::from_secs(1);

impl SerialIo for RealSerial {
    fn port_name(&self) -> String {
        self.name.clone()
    }

    fn is_open(&self) -> bool {
        self.port.is_some()
    }

    fn write(&mut self, data: &[u8]) -> io::Result<()> {
        let port = self.port.as_mut().ok_or_else(closed)?;
        port.write_all(data)
    }

    fn readline(&mut self) -> io::Result<Vec<u8>> {
        let deadline = Instant::now() + READ_TIMEOUT;
        loop {
            if let Some(i) = self.buf.iter().position(|&b| b == b'\n') {
                return Ok(self.buf.drain(..=i).collect());
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(std::mem::take(&mut self.buf));
            }
            let port = self.port.as_mut().ok_or_else(closed)?;
            port.set_timeout(left)?;
            let mut chunk = [0u8; 256];
            match port.read(&mut chunk) {
                Ok(n) => self.buf.extend_from_slice(&chunk[..n]),
                Err(e) if e.kind() == io::ErrorKind::TimedOut => {
                    return Ok(std::mem::take(&mut self.buf));
                }
                Err(e) => return Err(e),
            }
        }
    }

    fn reset_input_buffer(&mut self) -> io::Result<()> {
        self.buf.clear();
        let port = self.port.as_mut().ok_or_else(closed)?;
        port.clear(serialport::ClearBuffer::Input)
            .map_err(io::Error::from)
    }

    fn close(&mut self) {
        self.port = None;
    }
}

fn closed() -> io::Error {
    io::Error::new(io::ErrorKind::NotConnected, "port closed")
}

/// Open the clock's serial port: 115200 baud, DTR/RTS low (so the ESP32 doesn't reset),
/// 1 s read timeout. None if it isn't plugged in or can't be opened.
pub fn open_serial() -> Option<RealSerial> {
    let name = find_port()?;
    let mut port = serialport::new(&name, BAUD)
        .timeout(READ_TIMEOUT)
        .dtr_on_open(false)
        .open()
        .ok()?;
    let _ = port.write_data_terminal_ready(false);
    let _ = port.write_request_to_send(false);
    std::thread::sleep(Duration::from_millis(200));
    Some(RealSerial {
        port: Some(port),
        name,
        buf: Vec::new(),
    })
}
