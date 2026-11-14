"""The clock's network side, against a real clock (opt-in like test_device_api.py):

    set WFD_CLOCK=192.168.0.152 && test.bat -m device

Sends a few harmless UDP display packets and checks the counters; checks /api/frames rejects
bad input. Never stores frames (that would replace your drawing). If the clock has a WEBPASS,
also set WFD_CLOCK_PASS: every request sends it (and the reads are checked to need it).
"""
import base64
import json
import os
import socket
import time
import urllib.error
import urllib.request

import pytest

import mdns

CLOCK = os.environ.get("WFD_CLOCK")
PASS = os.environ.get("WFD_CLOCK_PASS", "")
pytestmark = [pytest.mark.device,
              pytest.mark.skipif(not CLOCK, reason="set WFD_CLOCK=<clock ip> to run device tests")]


def auth():
    """The web password (WFD_CLOCK_PASS), if the clock has one: every page and API call needs it."""
    if not PASS:
        return {}
    return {"Authorization": "Basic " + base64.b64encode(f"admin:{PASS}".encode()).decode()}


def get(path):
    return urllib.request.urlopen(urllib.request.Request(f"http://{CLOCK}{path}", headers=auth()), timeout=5)


def net():
    return json.load(get("/api/status"))["state"]["net"]


def send(payload: bytes, n=1):
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    for _ in range(n):
        s.sendto(payload, (CLOCK, 4210))
        time.sleep(0.02)
    s.close()
    time.sleep(0.4)


def our_address_towards_the_clock():
    """This PC's address on the clock's network (a VPN adapter may hold the default route)."""
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        s.connect((CLOCK, 9))      # no packet is sent: it only picks the route
        return s.getsockname()[0]
    finally:
        s.close()


def test_clock_answers_mdns():
    found = mdns.resolve("wfd-clock.local", timeout=3, source=our_address_towards_the_clock())
    assert found == socket.gethostbyname(CLOCK)


def test_good_packets_are_counted():
    before = net()["udp_packets"]
    head = ("WFD1 " + PASS) if PASS else "WFD1"
    send(f"{head}\nICON 0".encode(), 5)
    assert net()["udp_packets"] - before >= 5


def test_packets_without_the_header_are_rejected():
    state = json.load(get("/api/status"))["state"]
    if state["app_connected"]:
        pytest.skip("the PC app is streaming to the clock over WiFi (its packets are counted too): stop it first")
    before = net()
    send(b"HELLO\nSPEC 12 12", 2)
    after = net()
    assert after["udp_rejected"] - before["udp_rejected"] == 2
    assert after["udp_packets"] == before["udp_packets"]


def test_frames_upload_rejects_bad_input():
    def post(body, header=True):
        req = urllib.request.Request(f"http://{CLOCK}/api/frames", json.dumps(body).encode(),
                                     {"Content-Type": "application/json", **({"X-WFD": "1"} if header else {}), **auth()})
        try:
            with urllib.request.urlopen(req, timeout=5) as r:
                return r.status
        except urllib.error.HTTPError as e:
            return e.code
    assert post({"frames": [[1] * 7]}, header=False) == 403
    assert post({"frames": []}) == 400
    assert post({"frames": [[1] * 8]}) == 400
    assert post({"nope": 1}) == 400


def test_stored_frames_can_be_read_back():
    d = json.load(get("/api/frames"))
    assert set(d) == {"frames", "fps", "frame"}
    assert all(len(f) == 7 and all(0 <= r < 128 for r in f) for f in d["frames"])


def test_live_preview_shows_a_frame_and_rejects_junk():
    def post(body):
        req = urllib.request.Request(f"http://{CLOCK}/api/preview", json.dumps(body).encode(),
                                     {"Content-Type": "application/json", "X-WFD": "1", **auth()})
        try:
            with urllib.request.urlopen(req, timeout=5) as r:
                return r.status
        except urllib.error.HTTPError as e:
            return e.code
    assert post({"rows": [127, 65, 65, 65, 65, 65, 127], "frame": 1}) == 200    # shows for 1.5 s
    assert post({"nope": 1}) == 400


def test_the_drawing_tool_is_served():
    import gzip
    with get("/draw") as r:   # sent gzipped, like a browser wants
        assert r.status == 200 and r.headers["Content-Encoding"] == "gzip"
        assert b"WFD Grid Animator" in gzip.decompress(r.read())


@pytest.mark.skipif(not PASS, reason="set WFD_CLOCK_PASS (the clock's web password) to check it's required")
def test_reads_need_the_password():
    for path in ("/api/status", "/api/frames"):
        with pytest.raises(urllib.error.HTTPError) as e:
            urllib.request.urlopen(f"http://{CLOCK}{path}", timeout=5)
        assert e.value.code == 401, path
