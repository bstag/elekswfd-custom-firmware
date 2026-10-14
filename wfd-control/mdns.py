"""Minimal mDNS lookup: find the clock's IP from its .local name.

Windows can't always resolve .local names itself (it depends on the network profile and
policy), so the app asks the network directly: one multicast query, the first A record wins.
"""
import socket
import struct
import time

MDNS_ADDR = ("224.0.0.251", 5353)


def build_query(name: str) -> bytes:
    """A-record question for name, asking for a unicast reply (QU bit)."""
    labels = b"".join(bytes([len(p)]) + p.encode() for p in name.rstrip(".").split("."))
    return struct.pack(">HHHHHH", 0, 0, 1, 0, 0, 0) + labels + b"\0" + struct.pack(">HH", 1, 0x8001)


def _skip_name(data: bytes, i: int) -> int:
    while True:
        n = data[i]
        if n >= 0xC0:                 # compression pointer: 2 bytes, ends the name
            return i + 2
        if n == 0:
            return i + 1
        i += 1 + n


def parse_a_records(data: bytes) -> list[str]:
    """IPv4 addresses in the answer section of a DNS/mDNS message (malformed -> [])."""
    try:
        qd, an = struct.unpack(">HH", data[4:8])
        i = 12
        for _ in range(qd):
            i = _skip_name(data, i) + 4
        out = []
        for _ in range(an):
            i = _skip_name(data, i)
            rtype, _, _, rdlen = struct.unpack(">HHIH", data[i:i + 10])
            i += 10
            if rtype == 1 and rdlen == 4:
                out.append(socket.inet_ntoa(data[i:i + 4]))   # OSError if the packet is cut short
            i += rdlen
        return out
    except (IndexError, struct.error, OSError):           # anyone on the LAN can send us packets
        return []


def resolve(name: str, timeout: float = 1.5) -> str | None:
    """IP of name.local via mDNS, or None."""
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM, socket.IPPROTO_UDP)
    try:
        s.setsockopt(socket.IPPROTO_IP, socket.IP_MULTICAST_TTL, 255)
        s.settimeout(0.3)
        s.bind(("", 0))
        deadline = time.monotonic() + timeout
        s.sendto(build_query(name), MDNS_ADDR)
        while time.monotonic() < deadline:
            try:
                data, _ = s.recvfrom(1500)
            except socket.timeout:
                continue
            ips = parse_a_records(data)
            if ips:
                return ips[0]
        return None
    except OSError:
        return None
    finally:
        s.close()
