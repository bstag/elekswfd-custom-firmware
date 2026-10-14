"""Shared fixtures: a fresh app module per test, a fake serial port and a simulated clock.

Nothing here touches real hardware, the network, or your wfd_config.json.
"""
import datetime as _dt
import json
import random
import sys
import threading
import types
import urllib.error
import urllib.request
from pathlib import Path

import pytest
import serial

HERE = Path(__file__).parent
APP_DIR = HERE.parent
sys.path.insert(0, str(APP_DIR))

T0 = 1_760_000_000.0                       # simulated epoch for every run
START = _dt.datetime(2026, 10, 13, 16, 0, 0)


@pytest.fixture
def app(tmp_path):
    """A freshly imported app.py with default config that never writes to disk."""
    random.seed(0)                         # grid_anim seeds some animations from `random`
    sys.modules.pop("app", None)
    import app as mod
    mod.CONFIG = tmp_path / "wfd_config.json"
    mod.save_config = lambda cfg: None
    mod.cfg.clear()
    mod.cfg.update(mod.DEFAULTS)
    yield mod
    mod.stop.set()
    sys.modules.pop("app", None)


CLOCK_DEFAULTS = {                          # what a real clock reports for CFG
    "brightness": 6, "clock_24h": True, "style": 4, "units": "fahrenheit",
    "tz": "CST6CDT,M3.2.0,M11.1.0", "location": "76708", "offline_text": "HELLO",
    "msg_standalone": True, "standalone_msg_mode": "text", "standalone_rotate": ["weather", "text", "date"],
    "standalone_dwell": 8, "standalone_scroll_ms": 350, "mic_standalone": True, "grid_standalone": True,
    "standalone_anim": "spin", "standalone_fps": 8, "standalone_frame": False,
}


class FakeSerial:
    """Records every line written; answers CFG / CFGSET like the firmware. Can simulate an unplug
    after N writes on its first connection."""

    def __init__(self, sim, n):
        self.sim, self.port, self.is_open, self.writes, self.n = sim, f"COM{n}", True, 0, n
        self.replies = []

    def write(self, b):
        self.writes += 1
        if self.n == 1 and self.writes == self.sim.unplug_after:
            self.is_open = False
            raise serial.SerialException("unplugged")
        line = b.decode().rstrip()
        self.sim.lines.append(f"{self.sim.now():8.3f} {self.port} {line}")
        cmd, _, arg = line.partition(" ")
        if cmd == "CFG":
            self.replies.append("CFG " + json.dumps(self.sim.clock_cfg))
        elif cmd == "CFGSET":
            patch = json.loads(arg)
            bad = {k: "unknown setting" for k in patch if k not in self.sim.clock_cfg}
            if "brightness" in patch and not (isinstance(patch["brightness"], int) and 1 <= patch["brightness"] <= 16):
                bad["brightness"] = "expected an integer 1..16"
            if bad:
                self.replies.append("CFGERR " + json.dumps({"error": bad}))
            else:
                self.sim.clock_cfg.update(patch)
                self.replies.append("CFGOK " + json.dumps(self.sim.clock_cfg))
        elif cmd in ("GANIM", "GCOMMIT"):
            self.replies.append("OK")               # unrelated replies the app must skip

    def readline(self):
        if self.replies:
            return (self.replies.pop(0) + "\r\n").encode()
        self.sim.sleep(0.05)                       # like the real port's read timeout
        return b""

    def reset_input_buffer(self):
        self.replies.clear()


class FakeUdp:
    """The clock's UDP port: logs each command of each packet like FakeSerial logs lines."""

    def __init__(self, sim):
        self.sim = sim

    def sendto(self, data, addr):
        head, *lines = data.decode().split("\n")
        self.sim.packets.append((head, lines))
        for line in lines:
            self.sim.lines.append(f"{self.sim.now():8.3f} WIFI {line}")

    def close(self):
        pass


class FakeHttp:
    """The clock's HTTP API over the same simulated settings as FakeSerial."""

    def __init__(self, sim):
        self.sim = sim

    def __call__(self, method, path, body=None):
        sim = self.sim
        sim.http_calls.append((round(sim.now(), 3), method, path, body))
        if sim.wifi_down:
            raise sim.app.LinkLost("timed out")
        if (method, path) == ("GET", "/api/status"):
            return 200, {"state": {"ntp": sim.clock_ntp}, "config": json.loads(json.dumps(sim.clock_cfg))}
        if (method, path) == ("POST", "/api/set"):
            bad = {k: "unknown setting" for k in body if k not in sim.clock_cfg}
            if "brightness" in body and not (isinstance(body["brightness"], int) and 1 <= body["brightness"] <= 16):
                bad["brightness"] = "expected an integer 1..16"
            if bad:
                return 400, {"error": bad}
            sim.clock_cfg.update(body)
            return 200, {"ok": True}
        if (method, path) == ("POST", "/api/frames"):
            return 200, {"ok": True, "frames": len(body["frames"])}
        return 404, {"error": "not found"}


class Sim:
    """Drives ClockLink on simulated time. `events` maps a simulated second to a callable.
    wifi=True: no USB (until usb_present), the clock answers over FakeUdp / FakeHttp."""

    def __init__(self, app, seconds, unplug_after=None, events=None, wifi=False, password=""):
        self.app, self.seconds, self.unplug_after = app, seconds, unplug_after
        self.packets, self.http_calls = [], []
        self.wifi_down, self.clock_ntp, self.usb_present = False, True, not wifi
        self.wifi_opens = 0
        self.events = sorted((events or {}).items())
        self.t = T0
        self.lines = []
        self.opens = 0
        self.clock_cfg = json.loads(json.dumps(CLOCK_DEFAULTS))
        sim = self

        class FakeDT(_dt.datetime):
            @classmethod
            def now(cls, tz=None):
                return START + _dt.timedelta(seconds=sim.t - T0)

        app.time = types.SimpleNamespace(time=lambda: sim.t, sleep=self.sleep)
        app.datetime = types.SimpleNamespace(datetime=FakeDT)
        app.foreground_title = lambda: "Visual Studio Code - app.py"
        app.open_serial = self.open_serial
        app.find_port = lambda: "COM9" if self.usb_present else None
        if wifi:
            def open_wifi():
                if self.wifi_down:
                    return None
                self.wifi_opens += 1
                return app.WifiLink("10.0.0.9", password, sock=FakeUdp(self), http=FakeHttp(self))
            app.open_wifi = open_wifi
        else:
            app.open_wifi = lambda: None           # tests never touch the real network

    def now(self):
        return self.t - T0

    def open_serial(self):
        if not self.usb_present:
            return None
        self.opens += 1
        return FakeSerial(self, self.opens)

    def sleep(self, dt):
        self.t += dt
        while self.events and self.events[0][0] <= self.now():
            self.events.pop(0)[1]()
        if self.now() > self.seconds:
            self.app.stop.set()

    def run(self):
        self.app.ClockLink().run()
        return self.lines

    def commands(self, port=None):
        """The command word of every line sent (optionally only on one port)."""
        out = []
        for line in self.lines:
            _, p, cmd = line.split(maxsplit=2)
            if port is None or p == port:
                out.append(cmd.split()[0])
        return out


@pytest.fixture
def sim(app):
    """Factory: sim(seconds, unplug_after=None, events=None) -> Sim, with live-ish inputs set."""
    app.energy.update({"out": 0.02, "mic": 0.004})
    app.state.update(geo={"lat": 31.5765, "lon": -97.1786, "unit": 0},
                     weather={"code": 1, "temp": 92, "unit": 0, "label": "Waco", "cond": "Sunny"},
                     metrics={"cpu": 37, "gpu": 12, "ram": 61}, online=True)
    return lambda *a, **kw: Sim(app, *a, **kw)


@pytest.fixture
def api(app):
    """Run the HTTP handler on a free port. api(path, body=None, header=True) -> (status, json)."""
    srv = app.ThreadingHTTPServer(("127.0.0.1", 0), app.H)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    base = "http://127.0.0.1:%d" % srv.server_address[1]

    def call(path, body=None, header=True):
        data = None if body is None else json.dumps(body).encode()
        req = urllib.request.Request(base + path, data, {"X-WFD": "1"} if header else {})
        try:
            with urllib.request.urlopen(req) as r:
                return r.status, json.loads(r.read() or b"null")
        except urllib.error.HTTPError as e:
            raw = e.read()
            try:
                return e.code, json.loads(raw)
            except ValueError:
                return e.code, raw.decode()

    yield call
    srv.shutdown()
