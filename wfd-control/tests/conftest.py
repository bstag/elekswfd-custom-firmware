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


class FakeSerial:
    """Records every line written. Can simulate an unplug after N writes on its first connection."""

    def __init__(self, sim, n):
        self.sim, self.port, self.is_open, self.writes, self.n = sim, f"COM{n}", True, 0, n

    def write(self, b):
        self.writes += 1
        if self.n == 1 and self.writes == self.sim.unplug_after:
            self.is_open = False
            raise serial.SerialException("unplugged")
        self.sim.lines.append(f"{self.sim.now():8.3f} {self.port} {b.decode().rstrip()}")

    def reset_input_buffer(self):
        pass


class Sim:
    """Drives ClockLink on simulated time. `events` maps a simulated second to a callable."""

    def __init__(self, app, seconds, unplug_after=None, events=None):
        self.app, self.seconds, self.unplug_after = app, seconds, unplug_after
        self.events = sorted((events or {}).items())
        self.t = T0
        self.lines = []
        self.opens = 0
        sim = self

        class FakeDT(_dt.datetime):
            @classmethod
            def now(cls, tz=None):
                return START + _dt.timedelta(seconds=sim.t - T0)

        app.time = types.SimpleNamespace(time=lambda: sim.t, sleep=self.sleep)
        app.datetime = types.SimpleNamespace(datetime=FakeDT)
        app.foreground_title = lambda: "Visual Studio Code - app.py"
        app.open_serial = self.open_serial

    def now(self):
        return self.t - T0

    def open_serial(self):
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
