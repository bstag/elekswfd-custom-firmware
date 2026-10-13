"""The clock's own JSON API, against a real clock on your network. Opt-in:

    set WFD_CLOCK=192.168.0.152 && test.bat -m device

Every setting a test changes is put back afterwards. If the clock has a WEBPASS, also set
WFD_CLOCK_PASS. Skipped entirely when WFD_CLOCK isn't set.
"""
import base64
import json
import os
import urllib.error
import urllib.request

import pytest

CLOCK = os.environ.get("WFD_CLOCK")
pytestmark = [pytest.mark.device,
              pytest.mark.skipif(not CLOCK, reason="set WFD_CLOCK=<clock ip> to run device tests")]

EXPECTED_CONFIG = {
    "brightness", "clock_24h", "style", "units", "tz", "location", "offline_text", "msg_standalone",
    "standalone_msg_mode", "standalone_rotate", "standalone_dwell", "standalone_scroll_ms",
    "grid_standalone", "standalone_anim", "standalone_fps", "standalone_frame",
}


def call(path, body=None, header=True):
    data = None if body is None else json.dumps(body).encode()
    headers = {"Content-Type": "application/json"}
    if header:
        headers["X-WFD"] = "1"
    if os.environ.get("WFD_CLOCK_PASS"):
        tok = base64.b64encode(f"admin:{os.environ['WFD_CLOCK_PASS']}".encode()).decode()
        headers["Authorization"] = "Basic " + tok
    req = urllib.request.Request(f"http://{CLOCK}{path}", data, headers)
    try:
        with urllib.request.urlopen(req, timeout=10) as r:
            return r.status, json.loads(r.read())
    except urllib.error.HTTPError as e:
        return e.code, json.loads(e.read() or b"null")


def config():
    status, body = call("/api/status")
    assert status == 200
    return body["config"]


@pytest.fixture
def restore():
    """Snapshot the clock's settings and put them back after the test."""
    before = config()
    yield before
    keep = {k: v for k, v in before.items() if k not in ("location", "tz")}   # never touched; avoid a ZIP lookup
    status, body = call("/api/set", keep)
    assert status == 200, body
    assert config() == before


def test_status_shape():
    status, body = call("/api/status")
    assert status == 200
    assert set(body["config"]) == EXPECTED_CONFIG
    s = body["state"]
    for key in ("fw", "uptime_s", "time", "ntp", "wifi", "weather", "app_connected", "message", "grid_rows"):
        assert key in s, key
    assert len(s["grid_rows"]) == 7 and all(0 <= r < 128 for r in s["grid_rows"])


def test_set_round_trips(restore):
    new = {
        "brightness": 3 if restore["brightness"] != 3 else 4,
        "standalone_msg_mode": "rotate",
        "standalone_rotate": ["text", "date"],
        "standalone_dwell": 5,
        "standalone_scroll_ms": 200,
        "offline_text": "API TEST {day}",
        "standalone_anim": "heart",
        "standalone_fps": 9,
        "standalone_frame": False,
    }
    status, body = call("/api/set", new)
    assert (status, body) == (200, {"ok": True})
    got = config()
    for k, v in new.items():
        assert got[k] == v, k


def test_bad_request_changes_nothing(restore):
    status, body = call("/api/set", {"brightness": 5, "style": 99, "standalone_anim": "nope",
                                     "standalone_rotate": ["weather", "x"], "units": "kelvin", "bogus": 1})
    assert status == 400
    assert set(body["error"]) == {"style", "standalone_anim", "standalone_rotate", "units", "bogus"}
    assert config() == restore


@pytest.mark.parametrize("key, value", [
    ("brightness", 0), ("brightness", "6"), ("brightness", True), ("clock_24h", 1),
    ("location", "7670"), ("location", "ABCDE"), ("offline_text", "x" * 97),
    ("standalone_dwell", 1), ("standalone_scroll_ms", 6000), ("standalone_msg_mode", "window"),
])
def test_rejects(restore, key, value):
    status, body = call("/api/set", {key: value})
    assert status == 400 and key in body["error"], body


def test_post_needs_header():
    status, body = call("/api/set", {"brightness": 5}, header=False)
    assert status == 403


def test_not_json():
    req = urllib.request.Request(f"http://{CLOCK}/api/set", b"brightness=5", {"X-WFD": "1"})
    with pytest.raises(urllib.error.HTTPError) as e:
        urllib.request.urlopen(req, timeout=10)
    assert e.value.code == 400
