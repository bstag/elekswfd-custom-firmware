"""The app's own mDNS lookup (mdns.py): building the question and reading the answer."""
import socket
import struct

import mdns


def labels(name):
    return b"".join(bytes([len(p)]) + p.encode() for p in name.split(".")) + b"\0"


def answer(name, ip, compressed=False, with_question=False):
    """An mDNS response like the clock's: optional echoed question, then one A record."""
    head = struct.pack(">HHHHHH", 0, 0x8400, 1 if with_question else 0, 1, 0, 0)
    q = labels(name) + struct.pack(">HH", 1, 1) if with_question else b""
    owner = b"\xc0\x0c" if compressed and with_question else labels(name)
    rr = owner + struct.pack(">HHIH", 1, 0x8001, 120, 4) + socket.inet_aton(ip)
    return head + q + rr


def test_query_asks_for_an_a_record_with_unicast_reply():
    q = mdns.build_query("wfd-clock.local")
    assert q[12:12 + 17] == labels("wfd-clock.local")
    assert struct.unpack(">HH", q[-4:]) == (1, 0x8001)
    assert struct.unpack(">H", q[4:6])[0] == 1


def test_reads_plain_and_compressed_answers():
    assert mdns.parse_a_records(answer("wfd-clock.local", "192.168.0.152")) == ["192.168.0.152"]
    assert mdns.parse_a_records(answer("wfd-clock.local", "10.1.2.3", compressed=True, with_question=True)) == ["10.1.2.3"]


def test_garbage_is_not_an_answer():
    assert mdns.parse_a_records(b"") == []
    assert mdns.parse_a_records(answer("wfd-clock.local", "1.2.3.4")[:-3]) == []
