//! UsbLink / WifiLink details and the helpers they share (wire formats of app.py).

use chrono::NaiveDateTime;
use serde_json::{Map, Value, json};
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::io;
use std::rc::Rc;
use std::time::Instant;
use wfd_app::link::{
    Clock, Datagram, HttpApi, Link, LinkLost, SerialIo, UsbLink, WifiLink, base64, basic_auth,
    frame_cmd, py_json, truthy,
};

struct FakeClock(Cell<f64>);

impl Clock for FakeClock {
    fn now(&self) -> f64 {
        self.0.get()
    }
    fn sleep(&self, secs: f64) {
        self.0.set(self.0.get() + secs);
    }
    fn epoch_of(&self, _at: Instant) -> f64 {
        0.0
    }
    fn local_now(&self) -> NaiveDateTime {
        NaiveDateTime::default()
    }
}

/// A serial port with scripted reply lines; every write is recorded.
#[derive(Default)]
struct Script {
    written: Rc<RefCell<Vec<String>>>,
    replies: VecDeque<&'static str>,
    reads: Rc<Cell<usize>>,
    clock: Option<Rc<FakeClock>>,
}

impl SerialIo for Script {
    fn port_name(&self) -> String {
        "COM7".into()
    }
    fn is_open(&self) -> bool {
        true
    }
    fn write(&mut self, data: &[u8]) -> io::Result<()> {
        self.written
            .borrow_mut()
            .push(String::from_utf8_lossy(data).into_owned());
        Ok(())
    }
    fn readline(&mut self) -> io::Result<Vec<u8>> {
        self.reads.set(self.reads.get() + 1);
        match self.replies.pop_front() {
            Some(r) => Ok(format!("{r}\r\n").into_bytes()),
            None => {
                self.clock.as_ref().unwrap().sleep(0.25); // the read timeout
                Ok(Vec::new())
            }
        }
    }
    fn reset_input_buffer(&mut self) -> io::Result<()> {
        self.replies.clear();
        Ok(())
    }
    fn close(&mut self) {}
}

type Written = Rc<RefCell<Vec<String>>>;
type Packets = Rc<RefCell<Vec<(String, String, u16)>>>;

/// A UsbLink over a scripted port: (link, lines written, clock, readline count).
fn usb(replies: &[&'static str]) -> (UsbLink, Written, Rc<FakeClock>, Rc<Cell<usize>>) {
    let clock = Rc::new(FakeClock(Cell::new(100.0)));
    let s = Script {
        replies: replies.iter().copied().collect(),
        clock: Some(Rc::clone(&clock)),
        ..Default::default()
    };
    let (w, r) = (Rc::clone(&s.written), Rc::clone(&s.reads));
    (UsbLink::new(Box::new(s), clock.clone()), w, clock, r)
}

fn obj(v: Value) -> Map<String, Value> {
    v.as_object().unwrap().clone()
}

#[test]
fn json_is_compact_ascii_with_python_floats() {
    assert_eq!(
        py_json(&json!({"a": 1, "b": [1.0, 0.5, true, null], "c": "x\"\u{e9}\u{1F600}\n"})),
        [
            r#"{"a":1,"b":[1.0,0.5,true,null],"c":"x\""#,
            "\\",
            "u00e9",
            "\\",
            "ud83d",
            "\\",
            "ude00",
            r#"\n"}"#
        ]
        .concat()
    );
}

#[test]
fn base64_and_basic_auth() {
    assert_eq!(base64(b""), "");
    assert_eq!(base64(b"f"), "Zg==");
    assert_eq!(base64(b"fo"), "Zm8=");
    assert_eq!(base64(b"foo"), "Zm9v");
    assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    assert_eq!(basic_auth("hunter2"), "Basic YWRtaW46aHVudGVyMg==");
}

#[test]
fn frame_commands_and_truthiness() {
    assert_eq!(frame_cmd("GRID", [1u8, 2, 3]), "GRID 1 2 3");
    for (v, t) in [
        (json!(null), false),
        (json!(0), false),
        (json!(0.0), false),
        (json!(""), false),
        (json!([]), false),
        (json!({}), false),
        (json!(2), true),
        (json!("x"), true),
        (json!(true), true),
    ] {
        assert_eq!(truthy(Some(&v)), t, "{v}");
    }
    assert!(!truthy(None));
}

#[test]
fn usb_writes_lines_at_once() {
    let (mut l, w, _, _) = usb(&[]);
    l.send("SPEC 1 2").unwrap();
    l.send("LOGO 1").unwrap();
    l.flush().unwrap();
    assert_eq!(*w.borrow(), ["SPEC 1 2\n", "LOGO 1\n"]);
    assert_eq!(
        (l.kind(), l.name().as_str(), l.alive(), l.clock_ntp()),
        ("usb", "COM7", true, false)
    );
}

#[test]
fn usb_config_skips_other_lines_and_strips_the_version() {
    let (mut l, w, _, _) = usb(&[
        "OK",
        "CFGOK {}",
        r#"CFG {"brightness":6,"_fw":"B","_protocol":3}"#,
    ]);
    let c = l.get_config().unwrap().unwrap();
    assert_eq!(Value::Object(c), json!({"brightness": 6}));
    assert_eq!(l.version(), (Some("B".into()), Some(3)));
    assert_eq!(*w.borrow(), ["CFG\n"]);
}

#[test]
fn usb_config_gives_up_after_1_5_s() {
    let (mut l, _, clock, reads) = usb(&[]);
    assert_eq!(l.get_config().unwrap(), None);
    assert_eq!(reads.get(), 6, "1.5 s of 0.25 s reads");
    assert_eq!(clock.now(), 101.5);
}

#[test]
fn usb_set_config_replies() {
    let patch = obj(json!({"brightness": 9, "tz": "x"}));
    let (mut l, w, _, _) = usb(&[r#"CFGOK {"brightness":9,"_warning":{"tz":"odd"}}"#]);
    assert_eq!(
        l.set_config(&patch).unwrap(),
        Ok(Some(json!({"tz": "odd"})))
    );
    assert_eq!(*w.borrow(), ["CFGSET {\"brightness\":9,\"tz\":\"x\"}\n"]);

    let (mut l, _, _, _) = usb(&[r#"CFGERR {"error":{"tz":"bad"}}"#]);
    assert_eq!(l.set_config(&patch).unwrap(), Err(json!({"tz": "bad"})));

    let (mut l, _, _, _) = usb(&[r#"CFGERR {"error":"busy"}"#]);
    assert_eq!(
        l.set_config(&patch).unwrap(),
        Err(json!({"brightness": "busy", "tz": "busy"}))
    );

    let (mut l, _, _, _) = usb(&[]);
    assert_eq!(
        l.set_config(&patch).unwrap(),
        Err(json!({"brightness": "clock didn't answer", "tz": "clock didn't answer"}))
    );
}

#[test]
fn usb_uploads_are_paced() {
    let (mut l, w, clock, _) = usb(&[]);
    l.store_frames(&vec![[3; 7]; 90], 7, false).unwrap();
    let w = w.borrow();
    assert_eq!(w.len(), 82, "GCLR + 80 frames at most + GCOMMIT");
    assert_eq!(
        (w[0].as_str(), w[1].as_str(), w[81].as_str()),
        ("GCLR\n", "GADD 3 3 3 3 3 3 3\n", "GCOMMIT 7 0\n")
    );
    assert!((clock.now() - (100.0 + 0.03 + 80.0 * 0.015)).abs() < 1e-9);
}

// ------------------------------------------------------------------ WifiLink with fakes

struct Udp(Packets);

impl Datagram for Udp {
    fn send_to(&mut self, data: &[u8], ip: &str, port: u16) -> io::Result<()> {
        self.0
            .borrow_mut()
            .push((String::from_utf8_lossy(data).into_owned(), ip.into(), port));
        Ok(())
    }
}

struct Http(VecDeque<Result<(u16, Value), LinkLost>>);

impl HttpApi for Http {
    fn call(&mut self, _m: &str, _p: &str, _b: Option<&Value>) -> Result<(u16, Value), LinkLost> {
        self.0.pop_front().unwrap()
    }
}

fn wifi(
    pw: &str,
    replies: Vec<Result<(u16, Value), LinkLost>>,
) -> (WifiLink, Packets, Rc<FakeClock>) {
    let clock = Rc::new(FakeClock(Cell::new(0.0)));
    let sent = Rc::new(RefCell::new(Vec::new()));
    let l = WifiLink::with(
        "10.0.0.9",
        pw,
        Box::new(Udp(Rc::clone(&sent))),
        Box::new(Http(replies.into())),
        clock.clone(),
    );
    (l, sent, clock)
}

#[test]
fn wifi_batches_a_frame_into_one_packet() {
    let (mut l, sent, _) = wifi("pw", vec![]);
    l.flush().unwrap();
    assert!(sent.borrow().is_empty(), "nothing to send, no packet");
    l.send("SPEC 1 2").unwrap();
    l.send("BYE").unwrap();
    l.flush().unwrap();
    assert_eq!(
        *sent.borrow(),
        [(
            "WFD1 pw\nSPEC 1 2\nBYE".to_string(),
            "10.0.0.9".to_string(),
            4210
        )]
    );
}

#[test]
fn wifi_health_and_settings() {
    let status =
        json!({"state": {"ntp": true, "fw": "A", "protocol": 3}, "config": {"brightness": 4}});
    let (mut l, _, clock) = wifi(
        "",
        vec![
            Ok((200, status)),
            Ok((401, Value::Null)),
            Ok((400, json!({"error": "nope"}))),
            Ok((500, Value::Null)),
            Ok((200, json!({"ok": true, "warning": "w"}))),
            Ok((503, Value::Null)),
        ],
    );
    assert_eq!(l.get_config().unwrap(), Some(obj(json!({"brightness": 4}))));
    assert!(l.clock_ntp());
    assert_eq!(l.version(), (Some("A".into()), Some(3)));
    let p = obj(json!({"brightness": 1}));
    assert_eq!(
        l.set_config(&p).unwrap(),
        Err(json!({"brightness": "wrong clock password (clock_pass)"}))
    );
    assert_eq!(
        l.set_config(&p).unwrap(),
        Err(json!({"brightness": "nope"}))
    );
    assert_eq!(
        l.set_config(&p).unwrap(),
        Err(json!({"brightness": "HTTP 500"}))
    );
    assert_eq!(l.set_config(&p).unwrap(), Ok(Some(json!("w"))));
    assert!(l.get_config().is_err(), "a bad status is a lost link");
    assert!(l.alive());
    clock.sleep(15.0);
    assert!(!l.alive(), "no good answer for 15 s: gone");
}
