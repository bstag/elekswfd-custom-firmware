//! Minimal mDNS lookup: find the clock's IP from its .local name.
//!
//! Windows can't always resolve .local names itself (it depends on the network profile and
//! policy), so the app asks the network directly: one multicast query, the first A record wins.
//! Port of legacy/python-app/mdns.py.

use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::time::{Duration, Instant};

const MDNS_ADDR: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::new(224, 0, 0, 251), 5353);

/// A-record question for `name`, asking for a unicast reply (QU bit).
///
/// Labels longer than 255 bytes can't be encoded (Python raises); they're cut to 255 here,
/// and [`resolve`] refuses such names.
pub fn build_query(name: &str) -> Vec<u8> {
    // header: id 0, flags 0, 1 question, no records
    let mut q = vec![0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0];
    for label in name.trim_end_matches('.').split('.') {
        let b = &label.as_bytes()[..label.len().min(255)];
        q.push(b.len() as u8);
        q.extend_from_slice(b);
    }
    q.push(0);
    q.extend_from_slice(&[0, 1, 0x80, 0x01]); // QTYPE A, QCLASS IN | unicast-response bit
    q
}

fn u16_at(data: &[u8], i: usize) -> Option<u16> {
    Some(u16::from_be_bytes(data.get(i..i + 2)?.try_into().ok()?))
}

/// Index just past the (possibly compressed) name starting at `i`.
fn skip_name(data: &[u8], mut i: usize) -> Option<usize> {
    loop {
        let n = *data.get(i)?;
        if n >= 0xC0 {
            return Some(i + 2); // compression pointer: 2 bytes, ends the name
        }
        if n == 0 {
            return Some(i + 1);
        }
        i += 1 + n as usize;
    }
}

fn parse(data: &[u8]) -> Option<Vec<Ipv4Addr>> {
    let (qd, an) = (u16_at(data, 4)?, u16_at(data, 6)?);
    let mut i = 12;
    for _ in 0..qd {
        i = skip_name(data, i)? + 4; // QTYPE + QCLASS (not checked, as in Python)
    }
    let mut out = Vec::new();
    for _ in 0..an {
        i = skip_name(data, i)?;
        let head = data.get(i..i + 10)?;
        let rtype = u16::from_be_bytes([head[0], head[1]]);
        let rdlen = u16::from_be_bytes([head[8], head[9]]) as usize;
        i += 10;
        if rtype == 1 && rdlen == 4 {
            let a: [u8; 4] = data.get(i..i + 4)?.try_into().ok()?;
            out.push(Ipv4Addr::from(a));
        }
        i += rdlen;
    }
    Some(out)
}

/// IPv4 addresses in the answer section of a DNS/mDNS message. Anyone on the LAN can send us
/// packets: anything malformed or cut short gives an empty list (never a partial one).
pub fn parse_a_records(data: &[u8]) -> Vec<Ipv4Addr> {
    parse(data).unwrap_or_default()
}

/// This PC's own IPv4 addresses, one per network adapter (no loopback).
fn local_ipv4s() -> Vec<Ipv4Addr> {
    let mut out = Vec::new();
    for (_, data) in &sysinfo::Networks::new_with_refreshed_list() {
        for net in data.ip_networks() {
            if let std::net::IpAddr::V4(a) = net.addr
                && !a.is_loopback()
                && !a.is_unspecified()
                && !out.contains(&a)
            {
                out.push(a);
            }
        }
    }
    out
}

/// When the query goes out again (multicast UDP gets lost; the clock can be busy for a moment).
const RESEND_AT: [Duration; 3] = [
    Duration::ZERO,
    Duration::from_millis(500),
    Duration::from_millis(1300),
];

/// IP of `name` (e.g. "wfd-clock.local") via multicast mDNS, or None.
///
/// The query goes out on every network adapter, not just the default one: on a PC with a VPN
/// or virtual adapters (Tailscale, Hyper-V, WSL) Windows would otherwise send it there and the
/// clock on the LAN never hears it. It is repeated twice; the first A record wins.
pub fn resolve(name: &str, timeout: Duration) -> Option<Ipv4Addr> {
    if name.split('.').any(|l| l.len() > 255) {
        return None;
    }
    let mut from = local_ipv4s();
    from.push(Ipv4Addr::UNSPECIFIED); // and wherever Windows routes it by default
    let socks: Vec<UdpSocket> = from
        .into_iter()
        .filter_map(|a| {
            let s = UdpSocket::bind((a, 0)).ok()?;
            s.set_multicast_ttl_v4(255).ok()?;
            s.set_nonblocking(true).ok()?;
            Some(s)
        })
        .collect();
    if socks.is_empty() {
        return None;
    }
    let query = build_query(name);
    let start = Instant::now();
    let mut sent = 0;
    let mut buf = [0u8; 1500];
    while start.elapsed() < timeout {
        if sent < RESEND_AT.len() && start.elapsed() >= RESEND_AT[sent] {
            sent += 1;
            for s in &socks {
                let _ = s.send_to(&query, MDNS_ADDR); // an adapter that can't send it: skip
            }
        }
        for s in &socks {
            // drain what arrived; any error (nothing yet, an ICMP "unreachable") ends this socket's turn
            while let Ok((n, _)) = s.recv_from(&mut buf) {
                if let Some(&ip) = parse_a_records(&buf[..n]).first() {
                    return Some(ip);
                }
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    None
}
