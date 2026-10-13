# /// script
# requires-python = ">=3.10"
# dependencies = ["pyserial>=3.5", "numpy>=1.24", "psutil>=5.9", "PyAudioWPatch>=0.2.12"]
# ///
"""WFD Control - unified app for the custom EleksWFD firmware.

Runs the background services (time sync, audio spectrum, CPU/GPU/RAM bars, weather)
and serves a local control UI at http://127.0.0.1:8737 (HTTP_ADDR).

    uv run app.py            (or: python app.py)

Settings live in wfd_config.json next to this file and persist across restarts.
Pair with the wfd-clock firmware (serial: SET GET SPEC STYLE BARS WX BRIGHT).
"""
import ctypes
import datetime
import json
import os
import shutil
import subprocess
import sys
import threading
import time
import traceback
import urllib.parse
import urllib.request
import webbrowser
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import numpy as np
import psutil
import pyaudiowpatch as pa
import serial
from serial.tools import list_ports

sys.path.insert(0, str(Path(__file__).parent))
import font14  # noqa: E402
import grid_anim  # noqa: E402

HERE = Path(__file__).parent
CONFIG = HERE / "wfd_config.json"
HTTP_ADDR = ("127.0.0.1", 8737)
CH340_VID, CH340_PID = 0x1A86, 0x7523    # the clock's USB-serial chip
BAUD = 115200
NLEV = 12                                # spectrum column height

DEFAULTS = {
    "time_sync": True, "clock_24h": True,
    "spectrum": True, "style": 4, "gain_out": 1.0, "gain_mic": 1.0,
    "bars": True,
    "weather": True, "location": "", "units": "fahrenheit",
    "brightness": 6,
    "message_mode": "weather",        # weather | window | text | date | rotate
    "custom_text": "HELLO",
    "scroll_ms": 350,
    "rotate_weather": True, "rotate_window": True, "rotate_text": False, "rotate_date": False,
    "rotate_dwell": 8,                 # seconds per item in rotate mode
    "grid_enable": False, "grid_anim": "spin", "grid_frame": False, "grid_fps": 12,
    "grid_custom": [],                 # list of frames, each = [r0..r6] row bytes
    "grid_standalone": True,           # clock plays the stored grid animation when the app is off
    "offline_text": "WFD CLOCK",       # message shown on the clock when the app is off
    "msg_standalone": True,
    "icon_mode": "off",                # off | audio | system | all
    "logo": False,                     # ELEKSMAKER logo on/off
}

_RANGES = {"style": (1, 5), "brightness": (1, 16), "gain_out": (0.0, 10.0), "gain_mic": (0.0, 10.0),
           "scroll_ms": (50, 5000), "rotate_dwell": (2, 600), "grid_fps": (2, 30)}
_CHOICES = {"message_mode": {"weather", "window", "text", "date", "rotate"},
            "icon_mode": {"off", "audio", "system", "all"},
            "units": {"fahrenheit", "celsius"}}


def coerce(key, v):
    """Check a /api/set value against the type of DEFAULTS[key]; returns it normalized or raises ValueError."""
    d = DEFAULTS[key]
    if isinstance(d, bool):
        if not isinstance(v, bool):
            raise ValueError("expected true/false")
    elif isinstance(d, (int, float)):
        if isinstance(v, bool) or not isinstance(v, (int, float, str)):
            raise ValueError("expected a number")
        v = type(d)(float(v))
        lo, hi = _RANGES.get(key, (v, v))
        v = min(max(v, lo), hi)
    elif isinstance(d, str):
        if not isinstance(v, str):
            raise ValueError("expected a string")
        if key in _CHOICES and v not in _CHOICES[key]:
            raise ValueError(f"expected one of {sorted(_CHOICES[key])}")
    elif isinstance(d, list):                       # grid_custom: frames of 7 row bytes
        if not isinstance(v, list) or not all(isinstance(f, list) for f in v):
            raise ValueError("expected a list of frames")
        v = [[int(x) & 0x7F for x in (f + [0] * 7)[:7]] for f in v[:80]]
    return v


# icon bit order must match the firmware ICON_* arrays
ICON = {n: i for i, n in enumerate(
    ["speaker", "mic", "headphones", "vinyl", "mixer", "monitor", "tower", "server",
     "clock", "hourglass", "battery", "calendar", "wifi"])}


def compute_icons(mode, levels, metrics, online, battery):
    on = set()
    if mode == "all":
        return 0x1FFF
    if mode == "audio":
        o, mi = levels["out"], levels["mic"]
        if o >= 1:  on.add("speaker")
        if o >= 4:  on.add("headphones")
        if o >= 7:  on.add("vinyl")
        if o >= 10: on.add("mixer")
        if mi >= 2: on.add("mic")
    elif mode == "system":
        if metrics["cpu"] > 50: on.add("tower")
        if metrics["gpu"] > 30: on.add("monitor")
        if metrics["ram"] > 75: on.add("server")
        if online: on.add("wifi")
        if int(time.time()) % 2: on.add("clock")          # blink each second
        if battery is not None and battery < 30: on.add("battery")
    return sum(1 << ICON[n] for n in on)

REPEATABLE = ["spin", "bounce", "rain", "wave", "pulse", "sparkle", "scan", "life", "heart", "custom"]
_PERIOD = {"spin": 24, "scan": 7, "pulse": 8, "wave": 18, "heart": 16}


def render_grid_frames(anim, custom):
    if anim == "custom":
        return [list(f)[:7] for f in (custom or [])] or [[0] * 7]
    p = _PERIOD.get(anim)
    n = p * max(1, round(36 / p)) if p else 48
    eng = grid_anim.GridEngine()
    return [eng.frame(anim, {}) for _ in range(n)]


def scroll_frame(text, pos):
    """12 message bytes for text scrolled to pos (wraps with a 3-space gap)."""
    if len(text) <= 6:
        return font14.render6(text.ljust(6), 0)
    padded = text + "   "
    return font14.render6(padded + padded, pos % len(padded))


OMTEXT_MAX = 96                          # firmware MSG_TEXT_MAX


def clean_omtext(text):
    """Offline-message text as the clock stores it: one line of printable ASCII, capped."""
    text = "".join(ch if 32 <= ord(ch) < 127 else " " for ch in (text or ""))
    return text.strip()[:OMTEXT_MAX]


def foreground_title():
    try:
        u = ctypes.windll.user32
        h = u.GetForegroundWindow()
        n = u.GetWindowTextLengthW(h)
        buf = ctypes.create_unicode_buffer(n + 1)
        u.GetWindowTextW(h, buf, n + 1)
        return "".join(ch if ord(ch) < 128 else " " for ch in buf.value)
    except Exception:
        return ""


def load_config():
    cfg = dict(DEFAULTS)
    try:
        cfg.update({k: v for k, v in json.loads(CONFIG.read_text()).items() if not k.startswith("_")})
    except Exception:
        pass
    return cfg


def save_config(cfg):
    CONFIG.write_text(json.dumps({k: cfg[k] for k in DEFAULTS if k in cfg}, indent=2))


# ============================================================ shared state
state = {
    "connected": False, "port": None,
    "time": None,
    "metrics": {"cpu": 0, "gpu": 0, "ram": 0},
    "weather": {"code": 0, "temp": 0, "unit": 0, "label": "", "cond": ""},
    "levels": {"out": 0, "mic": 0},
    "audio": {"out": "", "mic": ""},
    "message": "",
    "saved": "",
    "online": False,
    "battery": None,
    "geo": None,
}
state_lock = threading.Lock()           # writers replace whole values via set_state(); readers of a
                                        # single key need no lock, /api/status serializes under it


def set_state(**kv):
    with state_lock:
        state.update(kv)


cfg = load_config()
cfg_lock = threading.Lock()
stop = threading.Event()
_pa_lock = threading.Lock()
energy = {"out": 0.0, "mic": 0.0}
gbands = [0.0] * 7                      # 7-band spectrum of the audio output (for the grid audio anim)
_band_peak = [1e-4] * 7
_BAND_EDGES = [60, 160, 320, 640, 1280, 2560, 5120, 12000]
grid_engine = grid_anim.GridEngine()
grid_preview = {"rows": [0] * 7, "frame": 0, "at": 0.0}   # live frame from the drawing tool
sync_now = threading.Event()           # set by /api/sync: push the PC time to the clock now
uploads = []                           # queued serial uploads for the worker to perform
uploads_lock = threading.Lock()


# ============================================================ audio capture
def audio_reader(kind):
    try:
        ctypes.windll.ole32.CoInitializeEx(None, 0)
    except Exception:
        pass
    while not stop.is_set():
        p = st = None
        try:
            with _pa_lock:
                p = pa.PyAudio()
                wasapi = p.get_host_api_info_by_type(pa.paWASAPI)
                if kind == "out":
                    d = p.get_device_info_by_index(wasapi["defaultOutputDevice"])
                    dev = None
                    for lb in p.get_loopback_device_info_generator():
                        if d["name"] in lb["name"]:
                            dev = lb
                            break
                    dev = dev or next(p.get_loopback_device_info_generator())
                else:
                    dev = p.get_device_info_by_index(wasapi["defaultInputDevice"])
                ch = max(1, int(dev["maxInputChannels"]))
                sr = int(dev["defaultSampleRate"])
                st = p.open(format=pa.paInt16, channels=ch, rate=sr, input=True,
                            input_device_index=dev["index"], frames_per_buffer=1024)
            with state_lock:
                state["audio"] = {**state["audio"], kind: dev["name"]}
            while not stop.is_set():
                raw = st.read(1024, exception_on_overflow=False)
                x = np.frombuffer(raw, np.int16).astype(np.float32) / 32768.0
                if ch > 1:
                    x = x.reshape(-1, ch).mean(axis=1)
                energy[kind] = float(np.sqrt(np.mean(x * x)) + 1e-9)
                if kind == "out":                 # 7-band spectrum for the grid audio animation
                    mag = np.abs(np.fft.rfft(x * np.hanning(len(x))))
                    freqs = np.fft.rfftfreq(len(x), 1 / sr)
                    for bi in range(7):
                        sel = (freqs >= _BAND_EDGES[bi]) & (freqs < _BAND_EDGES[bi + 1])
                        e = float(np.sqrt(np.mean(mag[sel] ** 2))) if sel.any() else 0.0
                        _band_peak[bi] = max(e, _band_peak[bi] * 0.99)
                        gbands[bi] = 0.0 if _band_peak[bi] < 1e-5 else min(1.0, e / _band_peak[bi])
        except Exception:
            energy[kind] = 0.0
            time.sleep(1.5)
        finally:
            try:
                st and st.close()
            except Exception:
                pass
            try:
                p and p.terminate()
            except Exception:
                pass


# ============================================================ system metrics
_nvsmi = shutil.which("nvidia-smi")


def metrics_reader():
    psutil.cpu_percent(None)
    while not stop.is_set():
        gpu = 0
        if _nvsmi:
            try:
                out = subprocess.run(
                    [_nvsmi, "--query-gpu=utilization.gpu", "--format=csv,noheader,nounits"],
                    capture_output=True, text=True, timeout=3,
                    creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0)).stdout
                vals = [float(x) for x in out.split() if x.strip()]
                gpu = int(max(vals)) if vals else 0
            except Exception:
                gpu = 0
        try:
            b = psutil.sensors_battery()
            battery = int(b.percent) if b else None
        except Exception:
            battery = None
        set_state(metrics={"cpu": int(psutil.cpu_percent(None)), "gpu": gpu,
                           "ram": int(psutil.virtual_memory().percent)}, battery=battery)
        time.sleep(1.0)


# ============================================================ weather
_COND = {1: "Sunny", 2: "Cloudy", 3: "Rain"}


def _get(url):
    req = urllib.request.Request(url, headers={"User-Agent": "wfd-clock"})
    with urllib.request.urlopen(req, timeout=8) as r:
        return json.load(r)


def _wmo(c):
    if c in (0, 1):
        return 1
    if c in (2, 3, 45, 48):
        return 2
    return 3


def resolve_location(loc):
    if not loc:
        d = _get("http://ip-api.com/json/?fields=lat,lon,city")
        return d["lat"], d["lon"], d.get("city", "Auto (IP)")
    if loc.isdigit() and len(loc) == 5:
        p = _get(f"https://api.zippopotam.us/us/{loc}")["places"][0]
        return float(p["latitude"]), float(p["longitude"]), f'{p["place name"]} {p["state abbreviation"]}'
    g = _get(f"https://geocoding-api.open-meteo.com/v1/search?name={urllib.parse.quote(loc)}&count=1")
    r = g["results"][0]
    return r["latitude"], r["longitude"], r["name"]


def weather_reader():
    cur_loc = None
    lat = lon = label = None
    while not stop.is_set():
        try:
            with cfg_lock:
                loc, units = cfg["location"].strip(), cfg["units"].lower()
            if loc != cur_loc or lat is None:
                lat, lon, label = resolve_location(loc)
                cur_loc = loc
                set_state(geo={"lat": round(float(lat), 4), "lon": round(float(lon), 4),
                               "unit": 1 if units.startswith("c") else 0})
            uname = "celsius" if units.startswith("c") else "fahrenheit"
            c = _get(f"https://api.open-meteo.com/v1/forecast?latitude={lat}&longitude={lon}"
                     f"&current=temperature_2m,weather_code&temperature_unit={uname}")["current"]
            code = _wmo(int(c["weather_code"]))
            set_state(weather={"code": code, "temp": int(round(c["temperature_2m"])),
                               "unit": 1 if uname == "celsius" else 0, "label": label, "cond": _COND[code]},
                      online=True)
        except Exception:
            set_state(online=False)
        stop.wait(900)


# ============================================================ serial link to the clock
def find_port():
    for p in list_ports.comports():
        if p.vid == CH340_VID and p.pid == CH340_PID:
            return p.device
    return None


def open_serial():
    port = find_port()
    if not port:
        return None
    try:
        s = serial.Serial()
        s.port, s.baudrate, s.timeout = port, BAUD, 1
        s.dtr = s.rts = False              # don't reset the ESP32 on open
        s.open()
        time.sleep(0.2)
        return s
    except serial.SerialException:
        return None


def _frame_cmd(cmd, values):
    return cmd + " " + " ".join(map(str, values))


class ClockLink:
    """Owns the serial port and pushes everything the clock shows, ~30 times a second."""

    TICK = 1.0 / 30

    def __init__(self):
        self.s = None
        self.sent = {}                     # command -> last value sent; cleared on (re)connect
        self.last_err = None
        self.last_sync = self.last_bars = self.last_wx = self.last_scroll = self.last_grid = 0.0
        self.peak = {"out": 1e-5, "mic": 1e-5}     # audio level auto-gain
        self.disp = {"out": 0.0, "mic": 0.0}       # displayed level (fast rise, slow fall)
        self.cur_src = None                # message text currently scrolling
        self.scroll_pos = 0
        self.rot_idx = -1
        self.rot_until = 0.0

    def run(self):
        while not stop.is_set():
            if not self.connect():
                time.sleep(2)
                continue
            with cfg_lock:
                c = dict(cfg)
            try:
                self.tick(c)
            except serial.SerialException:
                self.s = None
                set_state(connected=False)
            except Exception as e:         # never let a transient error kill the link, but say what it was
                if repr(e) != self.last_err:
                    self.last_err = repr(e)
                    traceback.print_exc()
            time.sleep(self.TICK)

    # ---------------------------------------------------------------- plumbing
    def connect(self):
        """Ensure the port is open; on a fresh connection, sync time and forget what was sent."""
        if self.s is not None and self.s.is_open:
            return True
        self.s = open_serial()
        set_state(connected=bool(self.s), port=self.s.port if self.s else None)
        if not self.s:
            return False
        with cfg_lock:
            if cfg["time_sync"]:
                self.set_time()
        self.last_sync = time.time()
        self.sent.clear()                  # a fresh (or power-cycled) clock needs everything again
        return True

    def send(self, line):
        self.s.write((line + "\n").encode())

    def push(self, cmd, value):
        """Send 'CMD value' only when value differs from what the clock last got."""
        if self.sent.get(cmd) != value:
            self.send(f"{cmd} {value}")
            self.sent[cmd] = value

    def set_time(self):
        now = datetime.datetime.now()
        self.send(f"SET {now.year%100} {now.month} {now.day} {now.isoweekday()} "
                  f"{now.hour} {now.minute} {now.second}")
        time.sleep(0.15)
        self.s.reset_input_buffer()
        set_state(time=now.strftime("%a %H:%M:%S"))

    # ---------------------------------------------------------------- one frame
    def tick(self, c):
        if (c["time_sync"] and time.time() - self.last_sync > 600) or sync_now.is_set():
            self.set_time()
            self.last_sync = time.time()
            sync_now.clear()
        self.push_settings(c)
        self.run_upload()
        levels = self.audio_levels(c)
        vals = levels if c["spectrum"] else {"out": 0, "mic": 0}
        self.send(f"SPEC {vals['out']} {vals['mic']}")
        self.send(f"ICON {compute_icons(c['icon_mode'], levels, state['metrics'], state['online'], state['battery'])}")
        self.send(f"LOGO {1 if c['logo'] else 0}")
        now = datetime.datetime.now()
        set_state(time=now.strftime("%a %H:%M:%S"))
        self.send_bars_weather(c)
        self.send_message(c, now)
        self.send_grid(c)

    def push_settings(self, c):
        self.push("STYLE", c["style"])
        self.push("BRIGHT", c["brightness"])
        self.push("CLK24", int(c["clock_24h"]))
        self.push("GAUTO", int(c["grid_standalone"]))
        self.push("OMAUTO", int(c["msg_standalone"]))
        g = state["geo"]
        if g:                                          # location for on-device weather
            self.push("GEO", f"{g['lat']} {g['lon']} {g['unit']}")

    def run_upload(self):
        """Perform one queued upload to the clock's flash (grid animation / offline text), if any."""
        with uploads_lock:
            job = uploads.pop(0) if uploads else None
        if not job:
            return
        if job["type"] == "omtext":        # the clock renders and scrolls the text itself
            self.send(f"OMTEXT {job['text']}")
            set_state(saved=f"message: {job['text']!r}")
            return
        self.send("GCLR")
        time.sleep(0.03)
        for fr in job["frames"][:80]:
            self.send(_frame_cmd("GADD", fr))
            time.sleep(0.015)
        self.send(f"GCOMMIT {job['fps']} {1 if job['frame'] else 0}")
        set_state(saved=f"grid: {len(job['frames'])} frames")
        self.sent.pop("GAUTO", None)       # a commit turns auto on; re-assert the setting

    def audio_levels(self, c):
        """0..NLEV levels for output/mic (always computed: used by the spectrum and the icon row)."""
        levels = {}
        for k, g in (("out", c["gain_out"]), ("mic", c["gain_mic"])):
            e = energy[k] * g
            self.peak[k] = max(e, self.peak[k] * 0.995)
            frac = 0.0 if self.peak[k] < 1e-5 else e / self.peak[k]
            target = NLEV * max(0.0, (frac - 0.12) / 0.88)
            d = self.disp[k]
            self.disp[k] = target if target >= d else max(target, d - 0.8)
            levels[k] = int(round(min(NLEV, self.disp[k])))
        set_state(levels=levels)
        return levels

    def send_bars_weather(self, c):
        if c["bars"] and time.time() - self.last_bars > 1.0:
            m = state["metrics"]
            self.send(f"BARS {m['cpu']} {m['gpu']} {m['ram']}")
            self.last_bars = time.time()
        w = state["weather"]
        if c["weather"] and w["label"] and time.time() - self.last_wx > 60.0:
            self.send(f"WX {w['code']} {w['temp']} {w['unit']}")   # only once real weather has loaded
            self.last_wx = time.time()

    def message_source(self, c):
        """Which source the message line shows now, or None to let the firmware show the temp."""
        mode = c["message_mode"]
        if mode in ("window", "text", "date"):
            return mode
        if mode != "rotate":
            return None                    # "weather": firmware draws the temperature itself
        srcs = [x for x in ("weather", "window", "text", "date") if c.get("rotate_" + x)]
        if not srcs:
            return None
        if time.time() >= self.rot_until:
            self.rot_idx = (self.rot_idx + 1) % len(srcs)
            self.rot_until = time.time() + max(2, c["rotate_dwell"])
            self.cur_src = None
            self.scroll_pos = 0
        return srcs[self.rot_idx % len(srcs)]

    def send_message(self, c, now):
        cur = self.message_source(c)
        if not cur or time.time() - self.last_scroll <= c["scroll_ms"] / 1000.0:
            return
        if cur == "weather":               # rotate's weather slot: temp glyphs from the PC
            w = state["weather"]
            txt = f"{w['temp']}{'C' if w['unit'] else 'F'}"
            frame = font14.render6(txt.rjust(6), 0)
            set_state(message="temp " + txt)
        else:
            fill = {"date": now.strftime("%a %b %d").upper(),
                    "time": now.strftime("%H:%M"),
                    "time12": now.strftime("%I:%M").lstrip("0") + now.strftime("%p"),
                    "day": now.strftime("%A").upper(),
                    "temp": f"{state['weather']['temp']}{'C' if state['weather']['unit'] else 'F'}"
                            if state["weather"]["label"] else "--"}
            if cur == "window":
                txt = foreground_title()
            elif cur == "date":
                txt = fill["date"]
            else:                          # custom text, with {date}/{time}/{time12}/{day}/{temp} placeholders
                txt = c["custom_text"]
                for k, v in fill.items():
                    txt = txt.replace("{" + k + "}", v)
            txt = txt.strip() or " "
            if txt != self.cur_src:
                self.cur_src = txt
                self.scroll_pos = 0
            frame = scroll_frame(txt, self.scroll_pos)
            self.scroll_pos += 1
            set_state(message=txt)
        self.send(_frame_cmd("MSG", frame))
        self.last_scroll = time.time()

    def send_grid(self, c):
        since = time.time() - self.last_grid
        if time.time() - grid_preview["at"] < 2.0:          # live drawing-tool preview wins
            if since > 1.0 / 20:
                self.send(_frame_cmd("GRID", grid_preview["rows"] + [grid_preview["frame"]]))
                self.last_grid = time.time()
        elif (c["grid_enable"] or c["grid_frame"]) and since > 1.0 / max(2, c["grid_fps"]):
            if c["grid_enable"]:
                m = state["metrics"]
                extra = {"bands": gbands, "cpu": m["cpu"], "gpu": m["gpu"], "ram": m["ram"],
                         "custom": c.get("grid_custom") or []}
                rows = grid_engine.frame(c["grid_anim"], extra)
            else:
                rows = [0] * 7
            self.send(_frame_cmd("GRID", list(rows) + [1 if c["grid_frame"] else 0]))
            self.last_grid = time.time()


# ============================================================ web server
class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def _send(self, code, body, ctype="application/json"):
        b = body if isinstance(body, bytes) else json.dumps(body).encode()
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(b)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(b)

    def do_GET(self):
        if self.path in ("/", "/index.html"):
            return self._send(200, (HERE / "control.html").read_bytes(), "text/html; charset=utf-8")
        if self.path in ("/draw", "/draw.html"):
            return self._send(200, (HERE / "draw.html").read_bytes(), "text/html; charset=utf-8")
        if self.path == "/api/status":
            with cfg_lock, state_lock:
                body = json.dumps({"state": state, "config": cfg}).encode()
            return self._send(200, body)
        self._send(404, {"error": "not found"})

    def do_POST(self):
        if self.headers.get("X-WFD") != "1":
            return self._send(403, b"forbidden", "text/plain")
        n = int(self.headers.get("Content-Length") or 0)
        try:
            body = json.loads(self.rfile.read(n) or b"{}")
        except json.JSONDecodeError:
            return self._send(400, {"error": "bad json"})
        if self.path == "/api/set":
            clean, errors = {}, {}
            for k, v in body.items():
                if k not in DEFAULTS:
                    errors[k] = "unknown setting"
                    continue
                try:
                    clean[k] = coerce(k, v)
                except (TypeError, ValueError) as e:
                    errors[k] = str(e)
            if errors:
                return self._send(400, {"error": errors})
            with cfg_lock:
                cfg.update(clean)
                save_config(cfg)
            return self._send(200, {"ok": True})
        if self.path == "/api/sync":
            sync_now.set()
            return self._send(200, {"ok": True})
        if self.path == "/api/grid_preview":
            rows = (body.get("rows") or [0] * 7)[:7]
            grid_preview.update(rows=[int(x) & 0x7F for x in (rows + [0] * 7)[:7]],
                                frame=1 if body.get("frame") else 0, at=time.time())
            return self._send(200, {"ok": True})
        if self.path == "/api/grid_save":
            if body.get("frames"):
                try:
                    body["frames"] = coerce("grid_custom", body["frames"])
                except (TypeError, ValueError) as e:
                    return self._send(400, {"error": f"frames: {e}"})
            with cfg_lock:
                anim = body.get("anim", cfg["grid_anim"])
                custom = body.get("frames") or cfg.get("grid_custom")
                fps = int(body.get("fps", cfg["grid_fps"]))
                frame = bool(body.get("frame", cfg["grid_frame"]))
            if anim not in REPEATABLE and not body.get("frames"):
                return self._send(400, {"error": f"'{anim}' needs live data; can't store it standalone"})
            frames = body.get("frames") or render_grid_frames(anim, custom)
            with uploads_lock:
                uploads.append({"type": "grid", "frames": frames, "fps": fps, "frame": frame})
            return self._send(200, {"queued": len(frames)})
        if self.path == "/api/om_save":
            with cfg_lock:
                text = body.get("text", cfg["offline_text"])
            if not isinstance(text, str):
                return self._send(400, {"error": "text: expected a string"})
            text = clean_omtext(text)
            with uploads_lock:
                uploads.append({"type": "omtext", "text": text})
            return self._send(200, {"queued": 1, "text": text})
        self._send(404, {"error": "not found"})


def main():
    if not CONFIG.exists():
        save_config(cfg)
    for fn in (lambda: audio_reader("out"), None, lambda: audio_reader("mic"),
               metrics_reader, weather_reader, ClockLink().run):
        if fn is None:
            time.sleep(0.5)                       # stagger audio inits
            continue
        threading.Thread(target=fn, daemon=True).start()
    srv = ThreadingHTTPServer(HTTP_ADDR, H)
    url = "http://%s:%d" % HTTP_ADDR
    print("WFD Control at", url)
    if "--no-browser" not in sys.argv:
        threading.Timer(0.6, webbrowser.open, (url,)).start()
    try:
        srv.serve_forever()
    except KeyboardInterrupt:
        stop.set()


if __name__ == "__main__":
    main()
