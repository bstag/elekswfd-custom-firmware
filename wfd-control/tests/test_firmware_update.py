"""Firmware version check and "Update clock" (OTA from the app)."""
import email.parser
import json
import time

import pytest


def test_version_is_read_over_usb_and_kept_out_of_settings(app, sim):
    s = sim(1)
    s.run()
    assert app.state["clock_fw"] == "wfd-clock Oct 13 2026 19:57:32"
    assert app.state["clock_protocol"] == 3
    assert not {"_fw", "_protocol"} & set(app.state["clock"])


@pytest.mark.parametrize("clock_fw, clock_proto, release, compat, available", [
    ("A", 3, {"fw": "A", "protocol": 3}, "ok", False),
    ("A", 3, {"fw": "B", "protocol": 3}, "ok", True),          # newer build, same protocol
    ("old", None, {"fw": "B", "protocol": 3}, "older", True),   # before versions were reported
    ("A", 2, {"fw": "B", "protocol": 3}, "older", True),
    ("A", 4, {"fw": "B", "protocol": 3}, "newer", False),       # never "update" to an older protocol
    ("A", 3, None, "ok", False),                                # no release shipped
])
def test_firmware_status(app, clock_fw, clock_proto, release, compat, available):
    st = {**app.state, "connected": True, "clock": {}, "clock_fw": clock_fw,
          "clock_protocol": clock_proto, "release": release}
    assert app.firmware_status(st) == {"compat": compat, "update_available": available}


def test_disconnected_clock_is_unknown(app):
    st = {**app.state, "connected": False, "release": {"fw": "B", "protocol": 3}}
    assert app.firmware_status(st) == {"compat": "unknown", "update_available": False}


def test_multipart_body_is_what_the_clock_expects(app):
    data = bytes(range(256)) * 4
    body, ctype = app.multipart("fw", "wfd-clock-firmware.bin", data)
    msg = email.parser.BytesParser().parsebytes(b"Content-Type: " + ctype.encode() + b"\r\n\r\n" + body)
    (part,) = msg.get_payload()
    assert part.get_param("name", header="content-disposition") == "fw"
    assert part.get_filename() == "wfd-clock-firmware.bin"
    assert part.get_payload(decode=True) == data


@pytest.fixture
def release(app, tmp_path):
    (tmp_path / "wfd-clock-firmware.bin").write_bytes(b"\xe9firmware")
    (tmp_path / "version.json").write_text(json.dumps({"fw": "wfd-clock NEW", "protocol": 3}))
    app.RELEASES = tmp_path
    app.set_state(release=app.load_release())
    return tmp_path


def wait_update(app, timeout=3):
    end = time.time() + timeout
    while app.state["update"]["state"] == "uploading" and time.time() < end:
        time.sleep(0.02)
    return app.state["update"]


def test_update_uploads_the_shipped_firmware(app, api, release, monkeypatch):
    calls = []
    monkeypatch.setattr(app, "find_clock_ip", lambda: "10.0.0.9")
    monkeypatch.setattr(app, "upload_firmware", lambda ip, data, pw="": calls.append((ip, data, pw)))
    monkeypatch.setattr(app, "wait_for_build", lambda ip, fw, timeout=60: (ip, fw) == ("10.0.0.9", "wfd-clock NEW"))
    app.cfg["clock_pass"] = "pw"
    assert api("/api/update_clock", {}) == (202, {"ok": True})
    up = wait_update(app)
    assert up["state"] == "done" and "NEW" in up["message"]
    assert calls == [("10.0.0.9", b"\xe9firmware", "pw")]


def test_lost_reply_still_counts_if_the_clock_runs_the_new_build(app, api, release, monkeypatch):
    def upload(*a, **k):
        raise TimeoutError("timed out")          # the clock restarted before its reply arrived
    monkeypatch.setattr(app, "find_clock_ip", lambda: "10.0.0.9")
    monkeypatch.setattr(app, "upload_firmware", upload)
    monkeypatch.setattr(app, "wait_for_build", lambda ip, fw, timeout=60: True)
    api("/api/update_clock", {})
    assert wait_update(app)["state"] == "done"


def test_update_fails_if_the_new_build_never_shows_up(app, api, release, monkeypatch):
    monkeypatch.setattr(app, "find_clock_ip", lambda: "10.0.0.9")
    monkeypatch.setattr(app, "upload_firmware", lambda *a, **k: None)
    monkeypatch.setattr(app, "wait_for_build", lambda ip, fw, timeout=60: False)
    api("/api/update_clock", {})
    up = wait_update(app)
    assert up["state"] == "failed" and "didn't come back" in up["message"]


def test_update_reports_failures(app, api, release, monkeypatch):
    monkeypatch.setattr(app, "find_clock_ip", lambda: None)
    api("/api/update_clock", {})
    up = wait_update(app)
    assert up["state"] == "failed" and "not found on WiFi" in up["message"]


def test_one_update_at_a_time(app, api, release, monkeypatch):
    monkeypatch.setattr(app, "find_clock_ip", lambda: (time.sleep(0.5), "10.0.0.9")[1])
    monkeypatch.setattr(app, "upload_firmware", lambda *a, **k: None)
    monkeypatch.setattr(app, "wait_for_build", lambda *a, **k: True)
    assert api("/api/update_clock", {})[0] == 202
    assert api("/api/update_clock", {})[0] == 409
    wait_update(app)


def test_no_release_no_update(app, api, tmp_path):
    app.RELEASES = tmp_path
    app.set_state(release=app.load_release())
    assert app.state["release"] is None
    assert api("/api/update_clock", {})[0] == 400
