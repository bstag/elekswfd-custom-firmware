//! The app's own mDNS lookup (port of test_mdns.py, plus parity with mdns.py on
//! fuzzed packets from tests/fixtures/mdns_*).

use std::collections::BTreeMap;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::time::Duration;
use wfd_app::mdns::{build_query, parse_a_records, resolve};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn labels(name: &str) -> Vec<u8> {
    let mut out: Vec<u8> = name
        .split('.')
        .flat_map(|p| [&[p.len() as u8][..], p.as_bytes()].concat())
        .collect();
    out.push(0);
    out
}

/// An mDNS response like the clock's: optional echoed question, then one A record.
fn answer(name: &str, ip: [u8; 4], compressed: bool, with_question: bool) -> Vec<u8> {
    let mut m = vec![0, 0, 0x84, 0, 0, with_question as u8, 0, 1, 0, 0, 0, 0];
    if with_question {
        m.extend(labels(name));
        m.extend([0, 1, 0, 1]);
    }
    if compressed && with_question {
        m.extend([0xC0, 0x0C]);
    } else {
        m.extend(labels(name));
    }
    m.extend([0, 1, 0x80, 0x01, 0, 0, 0, 120, 0, 4]);
    m.extend(ip);
    m
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

#[test]
fn query_asks_for_an_a_record_with_unicast_reply() {
    let q = build_query("wfd-clock.local");
    assert_eq!(q[12..12 + 17], labels("wfd-clock.local")[..]);
    assert_eq!(q[q.len() - 4..], [0, 1, 0x80, 0x01]);
    assert_eq!(q[4..6], [0, 1]);
}

#[test]
fn query_matches_python_bytes() {
    let want: BTreeMap<String, String> =
        serde_json::from_slice(&std::fs::read(fixture("mdns_queries.json")).unwrap()).unwrap();
    for (name, hex) in want {
        assert_eq!(build_query(&name), unhex(&hex), "{name:?}");
    }
}

#[test]
fn reads_plain_and_compressed_answers() {
    let ip = |a, b, c, d| vec![Ipv4Addr::new(a, b, c, d)];
    assert_eq!(
        parse_a_records(&answer("wfd-clock.local", [192, 168, 0, 152], false, false)),
        ip(192, 168, 0, 152)
    );
    assert_eq!(
        parse_a_records(&answer("wfd-clock.local", [10, 1, 2, 3], true, true)),
        ip(10, 1, 2, 3)
    );
}

#[test]
fn garbage_is_not_an_answer() {
    assert!(parse_a_records(b"").is_empty());
    let a = answer("wfd-clock.local", [1, 2, 3, 4], false, false);
    assert!(parse_a_records(&a[..a.len() - 3]).is_empty());
}

#[test]
fn every_truncation_and_random_packet_is_safe() {
    let a = answer("wfd-clock.local", [10, 1, 2, 3], true, true);
    for i in 0..a.len() {
        assert!(parse_a_records(&a[..i]).is_empty(), "cut at {i}");
    }
    // xorshift: deterministic random packets and bit flips; must never panic
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    for _ in 0..20_000 {
        let len = (next() % 80) as usize;
        let junk: Vec<u8> = (0..len).map(|_| next() as u8).collect();
        let _ = parse_a_records(&junk);
        let mut m = a.clone();
        for _ in 0..1 + next() % 4 {
            let i = (next() as usize) % m.len();
            m[i] = next() as u8;
        }
        let _ = parse_a_records(&m);
    }
}

/// Same answers as mdns.parse_a_records on ~1200 truncated / mutated / random packets.
#[test]
fn parse_matches_python_on_fuzzed_packets() {
    let cases: Vec<(String, Vec<String>)> =
        serde_json::from_slice(&std::fs::read(fixture("mdns_parse_cases.json")).unwrap()).unwrap();
    assert!(cases.len() > 1000);
    let mut found = 0;
    for (hex, want) in cases {
        let got: Vec<String> = parse_a_records(&unhex(&hex))
            .iter()
            .map(Ipv4Addr::to_string)
            .collect();
        found += !want.is_empty() as usize;
        assert_eq!(got, want, "{hex}");
    }
    assert!(found > 10, "fixture should include real answers");
}

/// Asks the real network for the user's clock (192.168.0.152). Run with `cargo test -- --ignored`.
#[test]
#[ignore]
fn resolves_the_real_clock() {
    let ip = resolve("wfd-clock.local", Duration::from_millis(1500));
    println!("wfd-clock.local -> {ip:?}");
    assert_eq!(ip, Some(Ipv4Addr::new(192, 168, 0, 152)));
}

#[test]
fn unresolvable_name_times_out() {
    let t = std::time::Instant::now();
    assert_eq!(
        resolve("no-such-host-wfd-test.local", Duration::from_millis(400)),
        None
    );
    assert!(t.elapsed() < Duration::from_secs(2));
}
