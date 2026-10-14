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
import base64
import ctypes
import datetime
import json
import os
import re
import socket
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
import mdns  # noqa: E402

HERE = Path(__file__).parent
CONFIG = HERE / "wfd_config.json"
RELEASES = HERE.parent / "wfd-firmware" / "releases"   # firmware this app ships (make_release.py)
WEB = HERE.parent / "wfd-firmware" / "clock" / "web"    # pages shared with the clock (dashboard, draw)
PROTOCOL = 3                                           # firmware WFD_PROTOCOL this app speaks
HTTP_ADDR = ("127.0.0.1", 8737)
CH340_VID, CH340_PID = 0x1A86, 0x7523    # the clock's USB-serial chip
BAUD = 115200
NLEV = 12                                # spectrum column height

# Settings the app owns (wfd_config.json). Everything about the clock itself lives on the clock
# (CLOCK_KEYS): the app reads it with CFG and changes it with CFGSET, so the clock, its dashboard
# and this app always agree.
DEFAULTS = {
    "time_sync": True,
    "spectrum": True, "gain_out": 1.0, "gain_mic": 1.0,
    "bars": True,
    "weather": True,
    "message_mode": "weather",        # weather | window | text | date | rotate
    "custom_text": "HELLO",
    "scroll_ms": 350,
    "rotate_weather": True, "rotate_window": True, "rotate_text": False, "rotate_date": False,
    "rotate_dwell": 8,                 # seconds per item in rotate mode
    "grid_enable": False, "grid_anim": "spin", "grid_frame": False, "grid_fps": 12,
    "grid_custom": [],                 # list of frames, each = [r0..r6] row bytes
    "icon_mode": "off",                # off | audio | system | all
    "link": "auto",                    # auto (USB if plugged in, else WiFi) | usb | wifi
    "clock_host": "",                  # WiFi: name or IP; blank = wfd-clock.local
    "clock_pass": "",                  # the clock's web password (WEBPASS), if it has one
    "logo": False,                     # ELEKSMAKER logo on/off
}

# Owned by the clock (firmware configJson / applySettings). The clock validates them.
CLOCK_KEYS = {
    "brightness", "clock_24h", "style", "units", "tz", "location", "offline_text", "msg_standalone",
    "standalone_msg_mode", "standalone_rotate", "standalone_dwell", "standalone_scroll_ms",
    "mic_standalone", "grid_standalone", "standalone_anim", "standalone_fps", "standalone_frame",
}

_RANGES = {"gain_out": (0.0, 10.0), "gain_mic": (0.0, 10.0),
           "scroll_ms": (50, 5000), "rotate_dwell": (2, 600), "grid_fps": (2, 30)}
_CHOICES = {"message_mode": {"weather", "window", "text", "date", "rotate"},
            "icon_mode": {"off", "audio", "system", "all"},
            "link": {"auto", "usb", "wifi"}}


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

# Animations the clock runs itself (firmware grid_engine.h; audio/mouth from its own mic);
# "custom" = frames drawn in /draw. Only "cpu" needs live PC data and can't play standalone.
BUILTIN_GRID = ["spin", "bounce", "rain", "wave", "pulse", "sparkle", "scan", "life", "heart", "audio", "mouth"]
REPEATABLE = BUILTIN_GRID + ["custom"]


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
        # only the app's own settings: older files also hold clock settings, which the clock owns now
        cfg.update({k: v for k, v in json.loads(CONFIG.read_text()).items() if k in DEFAULTS})
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
    "clock": None,                     # the clock's own settings (CFG), None until read
    "link": None,                      # "usb" | "wifi" while connected
    "clock_ip": None,                  # last address the clock answered on over WiFi
    "clock_fw": None, "clock_protocol": None,          # what the clock runs (None until read)
    "release": None,                   # releases/version.json: the firmware this app ships
    "update": {"state": "idle", "message": ""},        # "Update clock": idle|uploading|done|failed
    "grid_rows": None,                 # the grid frame the app last sent (dashboard preview), None = none
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
clock_requests = []                    # CFGSET requests for the link thread: [patch, Event, result]
clock_requests_lock = threading.Lock()


def set_clock(patch, timeout=3.0):
    """Change clock-owned settings through the link (CFGSET). Returns (ok, errors_or_None)."""
    if not state["connected"]:
        return False, {k: "clock not connected" for k in patch}
    req = [patch, threading.Event(), None]
    with clock_requests_lock:
        clock_requests.append(req)
    if not req[1].wait(timeout):
        return False, {k: "clock didn't answer" for k in patch}
    return req[2]


def clock_setting(key, default):
    c = state["clock"]
    return c.get(key, default) if c else default
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
            loc = clock_setting("location", "").strip()       # the clock's ZIP; blank = by IP
            units = clock_setting("units", "fahrenheit").lower()
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


class LinkLost(Exception):
    """The clock stopped answering over WiFi."""


class UsbLink:
    """The clock on USB serial: commands go out as they're made; settings via CFG / CFGSET."""
    kind = "usb"

    def __init__(self, s):
        self.s, self.name = s, s.port
        self.clock_ntp = False             # unknown over USB: keep sending the PC's time
        self.fw = self.protocol = None

    @property
    def alive(self):
        return self.s.is_open

    def send(self, line):
        self.s.write((line + "\n").encode())

    def flush(self):
        pass

    def request(self, line, prefixes, timeout=1.5):
        """Send a command and wait for a reply line starting with one of prefixes -> (prefix, json)."""
        self.send(line)
        deadline = time.time() + timeout
        while time.time() < deadline:
            raw = self.s.readline()
            if not raw:
                continue
            text = raw.decode(errors="replace").strip()
            for pre in prefixes:           # other replies (OK ...) from earlier commands are skipped
                if text.startswith(pre + " "):
                    return pre, json.loads(text[len(pre) + 1:])
        return None, None

    def get_config(self):
        pre, c = self.request("CFG", ["CFG"])
        if not pre:
            return None
        self.fw, self.protocol = c.pop("_fw", None), c.pop("_protocol", None)
        return c

    def set_config(self, patch):
        pre, body = self.request("CFGSET " + json.dumps(patch, separators=(",", ":")), ["CFGOK", "CFGERR"])
        if pre == "CFGOK":
            return True, body.pop("_warning", None)
        if pre == "CFGERR":
            err = body.get("error")
            return False, err if isinstance(err, dict) else {k: str(err) for k in patch}
        return False, {k: "clock didn't answer" for k in patch}

    def store_anim(self, name, fps, frame):
        self.send(f"GANIM {name} {fps} {1 if frame else 0}")

    def store_frames(self, frames, fps, frame):
        self.send("GCLR")
        time.sleep(0.03)
        for fr in frames[:80]:
            self.send(_frame_cmd("GADD", fr))
            time.sleep(0.015)
        self.send(f"GCOMMIT {fps} {1 if frame else 0}")

    def after_time(self):
        time.sleep(0.15)
        self.s.reset_input_buffer()

    def close(self):
        try:
            self.s.close()
        except Exception:
            pass


class WifiLink:
    """The clock over WiFi: each frame's commands go out as one UDP packet; settings, uploads and
    the health check use its HTTP API."""
    kind = "wifi"
    PORT = 4210

    def __init__(self, ip, password="", sock=None, http=None):
        self.ip, self.password, self.name = ip, password, f"WiFi {ip}"
        self.sock = sock or socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        self.http = http or self._http
        self.buf = []
        self.clock_ntp = False
        self.fw = self.protocol = None
        self.ok_at = time.time()

    @property
    def alive(self):
        return time.time() - self.ok_at < 15        # no good HTTP answer for 15 s: gone

    def _http(self, method, path, body=None):
        headers = {"X-WFD": "1"}
        data = None
        if body is not None:
            data = json.dumps(body).encode()
            headers["Content-Type"] = "application/json"
        if self.password:
            headers["Authorization"] = "Basic " + base64.b64encode(f"admin:{self.password}".encode()).decode()
        req = urllib.request.Request(f"http://{self.ip}{path}", data, headers, method=method)
        try:
            with urllib.request.urlopen(req, timeout=3) as r:
                return r.status, json.loads(r.read() or b"null")
        except urllib.error.HTTPError as e:
            try:
                return e.code, json.loads(e.read() or b"null")
            except ValueError:
                return e.code, None
        except (OSError, ValueError) as e:
            raise LinkLost(str(e)) from e

    def send(self, line):
        self.buf.append(line)

    def flush(self):
        if not self.buf:
            return
        head = "WFD1 " + self.password if self.password else "WFD1"
        self.sock.sendto("\n".join([head] + self.buf).encode(), (self.ip, self.PORT))
        self.buf.clear()

    def get_config(self):
        code, body = self.http("GET", "/api/status")
        if code != 200 or not body:
            raise LinkLost(f"status: HTTP {code}")
        self.ok_at = time.time()
        self.clock_ntp = bool(body["state"].get("ntp"))
        self.fw, self.protocol = body["state"].get("fw"), body["state"].get("protocol")
        return body["config"]

    def set_config(self, patch):
        code, body = self.http("POST", "/api/set", patch)
        if code == 200:
            return True, (body or {}).get("warning")
        err = "wrong clock password (clock_pass)" if code == 401 else (body or {}).get("error") or f"HTTP {code}"
        return False, err if isinstance(err, dict) else {k: str(err) for k in patch}

    def store_anim(self, name, fps, frame):
        self.set_config({"standalone_anim": name, "standalone_fps": fps, "standalone_frame": bool(frame),
                         "grid_standalone": True})

    def store_frames(self, frames, fps, frame):
        self.http("POST", "/api/frames", {"frames": frames[:80], "fps": fps, "frame": bool(frame)})

    def after_time(self):
        pass

    def close(self):
        try:
            self.flush()
            self.sock.close()
        except OSError:
            pass


def load_release():
    """releases/version.json -> {"fw", "protocol"} (None if missing)."""
    try:
        info = json.loads((RELEASES / "version.json").read_text(encoding="utf-8"))
        return {"fw": str(info["fw"]), "protocol": int(info["protocol"])}
    except (OSError, ValueError, KeyError, TypeError):
        return None


def firmware_status(st):
    """What the dashboard shows about the clock's firmware vs this app."""
    fw, proto, rel = st["clock_fw"], st["clock_protocol"], st["release"]
    if not st["connected"] or (fw is None and st["clock"] is None):
        compat = "unknown"
    elif proto is None or proto < PROTOCOL:
        compat = "older"                   # (builds before protocol 3 didn't report one)
    elif proto > PROTOCOL:
        compat = "newer"
    else:
        compat = "ok"
    available = bool(rel and st["connected"] and fw != rel["fw"] and rel["protocol"] >= (proto or 0))
    return {"compat": compat, "update_available": available}


def multipart(field, filename, data, boundary="wfd-clock-ota-7f3a"):
    head = (f"--{boundary}\r\nContent-Disposition: form-data; name=\"{field}\"; filename=\"{filename}\"\r\n"
            "Content-Type: application/octet-stream\r\n\r\n").encode()
    return head + data + f"\r\n--{boundary}--\r\n".encode(), f"multipart/form-data; boundary={boundary}"


def upload_firmware(ip, data, password=""):
    """POST the app image to the clock's /update (same as its web page); raises on failure."""
    body, ctype = multipart("fw", "wfd-clock-firmware.bin", data)
    headers = {"Content-Type": ctype}
    if password:
        headers["Authorization"] = "Basic " + base64.b64encode(f"admin:{password}".encode()).decode()
    req = urllib.request.Request(f"http://{ip}/update", body, headers, method="POST")
    try:
        with urllib.request.urlopen(req, timeout=180) as r:
            if r.status != 200:
                raise RuntimeError(f"clock answered HTTP {r.status}")
    except urllib.error.HTTPError as e:
        raise RuntimeError("wrong clock password" if e.code == 401 else f"clock refused the update (HTTP {e.code})") from e
    except ConnectionResetError:
        pass                               # older firmware reboots before closing: the upload got there


def wait_for_build(ip, fw, timeout=60):
    """Poll the clock's /api/status until it reports build fw (True) or time runs out (False)."""
    end = time.time() + timeout
    while time.time() < end:
        try:
            with urllib.request.urlopen(f"http://{ip}/api/status", timeout=3) as r:
                if json.load(r)["state"].get("fw") == fw:
                    return True
        except (OSError, ValueError, KeyError):
            pass                           # restarting / rejoining WiFi
        time.sleep(2)
    return False


def update_clock():
    """Background job for "Update clock": find the clock on the network and upload the release."""
    def job():
        try:
            ip = find_clock_ip()
            if not ip:
                raise RuntimeError("clock not found on WiFi (updates go over the network)")
            data = (RELEASES / "wfd-clock-firmware.bin").read_bytes()
            set_state(update={"state": "uploading", "message": f"sending {len(data) // 1024} KB to {ip}…"})
            with cfg_lock:
                pw = cfg.get("clock_pass", "")
            want = state["release"]["fw"]
            for attempt in (1, 2):         # a stalled upload (busy WiFi) is worth one more try
                if attempt == 2:
                    set_state(update={"state": "uploading", "message": "no new firmware yet; sending it again…"})
                try:
                    upload_firmware(ip, data, pw)
                except (TimeoutError, socket.timeout, urllib.error.URLError):
                    pass                   # the reply can get lost in the restart: check the result instead
                set_state(update={"state": "uploading", "message": "waiting for the clock to restart…"})
                if wait_for_build(ip, want, timeout=45):
                    break
            else:
                raise RuntimeError("the clock didn't come back with the new firmware (tried twice)")
            set_state(update={"state": "done", "message": f"the clock now runs {want.replace('wfd-clock ', '')}"})
        except Exception as e:
            set_state(update={"state": "failed", "message": str(e)})
    if state["update"]["state"] == "uploading":
        return False
    set_state(update={"state": "uploading", "message": "looking for the clock…"})
    threading.Thread(target=job, daemon=True).start()
    return True


def find_clock_ip():
    """The clock's IP for WiFi: the configured host (name or IP), else wfd-clock.local via mDNS,
    else the last address it answered on."""
    with cfg_lock:
        host = cfg.get("clock_host", "").strip() or "wfd-clock.local"
    if re.fullmatch(r"\d+\.\d+\.\d+\.\d+", host):
        return host
    try:
        return socket.gethostbyname(host)       # normal DNS, or .local where Windows resolves it
    except OSError:
        pass
    ip = mdns.resolve(host) if host.endswith(".local") else None
    return ip or state["clock_ip"]


def open_wifi():
    ip = find_clock_ip()
    if not ip:
        return None
    with cfg_lock:
        pw = cfg.get("clock_pass", "")
    link = WifiLink(ip, pw)
    try:
        link.get_config()
    except LinkLost:
        link.close()
        return None
    set_state(clock_ip=ip)
    return link


class ClockLink:
    """Owns the connection to the clock (USB or WiFi) and pushes everything it shows, ~30 times a
    second."""

    TICK = 1.0 / 30

    def __init__(self):
        self.link = None
        self.sent = {}                     # command -> last value sent; cleared on (re)connect
        self.last_err = None
        self.last_sync = self.last_bars = self.last_wx = self.last_scroll = self.last_grid = 0.0
        self.peak = {"out": 1e-5, "mic": 1e-5}     # audio level auto-gain
        self.disp = {"out": 0.0, "mic": 0.0}       # displayed level (fast rise, slow fall)
        self.cur_src = None                # message text currently scrolling
        self.scroll_pos = 0
        self.rot_idx = -1
        self.rot_until = 0.0
        self.last_cfg = 0.0
        self.last_usb_check = 0.0
        self.link_cfg = None               # (link, clock_host, clock_pass) the link was made with

    def run(self):
        while not stop.is_set():
            if not self.connect():
                time.sleep(2)
                continue
            with cfg_lock:
                c = dict(cfg)
            try:
                self.tick(c)
                self.link.flush()
            except (serial.SerialException, LinkLost, OSError):
                self.drop()
            except Exception as e:         # never let a transient error kill the link, but say what it was
                if repr(e) != self.last_err:
                    self.last_err = repr(e)
                    traceback.print_exc()
            time.sleep(self.TICK)
        self.say_bye()

    def say_bye(self):
        """App quitting: tell the clock, so it takes over at once instead of after 2 s."""
        if self.link and self.link.alive:
            try:
                self.link.send("BYE")
                self.link.flush()
            except (serial.SerialException, OSError):
                pass

    def drop(self):
        if self.link:
            self.link.close()
        self.link = None
        set_state(connected=False, link=None)

    # ---------------------------------------------------------------- plumbing
    def connect(self):
        """Ensure a link (USB first unless set to WiFi); on a fresh one, sync time, forget what was
        sent and read the clock's settings."""
        if self.link is not None and self.link.alive:
            return True
        if self.link is not None:
            self.drop()
        with cfg_lock:
            mode = cfg.get("link", "auto")
        s = open_serial() if mode in ("auto", "usb") else None
        self.link = UsbLink(s) if s else (open_wifi() if mode in ("auto", "wifi") else None)
        set_state(connected=bool(self.link), port=self.link.name if self.link else None,
                  link=self.link.kind if self.link else None)
        if not self.link:
            return False
        with cfg_lock:
            self.link_cfg = (cfg.get("link"), cfg.get("clock_host"), cfg.get("clock_pass"))
        self.sent.clear()                  # a fresh (or power-cycled) clock needs everything again
        self.read_clock_config()           # first: it also tells us whether the clock has NTP time
        with cfg_lock:
            if cfg["time_sync"] and not self.link.clock_ntp:
                self.set_time()
        self.last_sync = time.time()
        return True

    def read_clock_config(self):
        c = self.link.get_config()
        if c is not None:
            set_state(clock=c, clock_fw=self.link.fw, clock_protocol=self.link.protocol)
        self.last_cfg = time.time()

    def run_clock_requests(self):
        with clock_requests_lock:
            reqs = clock_requests[:]
            clock_requests.clear()
        for req in reqs:
            try:
                req[2] = self.link.set_config(req[0])
                if req[2][0]:
                    self.read_clock_config()
            except LinkLost:
                req[2] = (False, {k: "clock didn't answer" for k in req[0]})
                raise
            finally:
                req[1].set()

    def send(self, line):
        self.link.send(line)

    def push(self, cmd, value):
        """Send 'CMD value' only when value differs from what the clock last got."""
        if self.sent.get(cmd) != value:
            self.send(f"{cmd} {value}")
            self.sent[cmd] = value

    def set_time(self):
        now = datetime.datetime.now()
        self.send(f"SET {now.year%100} {now.month} {now.day} {now.isoweekday()} "
                  f"{now.hour} {now.minute} {now.second}")
        self.link.after_time()
        set_state(time=now.strftime("%a %H:%M:%S"))

    # ---------------------------------------------------------------- one frame
    def tick(self, c):
        due = c["time_sync"] and time.time() - self.last_sync > 600 and not self.link.clock_ntp
        if due or sync_now.is_set():               # a WiFi clock keeps its own time (NTP)
            self.set_time()
            self.last_sync = time.time()
            sync_now.clear()
        want = (c.get("link"), c.get("clock_host"), c.get("clock_pass"))
        if want != self.link_cfg:                  # connection settings changed: reconnect
            wrong_kind = (want[0] == "usb" and self.link.kind == "wifi") or (want[0] == "wifi" and self.link.kind == "usb")
            if wrong_kind or (self.link.kind == "wifi" and want[1:] != self.link_cfg[1:]):
                self.drop()
                return
            self.link_cfg = want
        if self.link.kind == "wifi" and c.get("link") == "auto" and time.time() - self.last_usb_check > 10:
            self.last_usb_check = time.time()      # plugged in meanwhile? USB is the better link
            if find_port():
                self.drop()
                return
        self.run_clock_requests()
        if time.time() - self.last_cfg > 5:            # pick up changes made on the clock itself
            self.read_clock_config()
        self.push_location()
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

    def push_location(self):
        """Give the clock our weather location only if it has none of its own (no ZIP set)."""
        g, clk = state["geo"], state["clock"]
        if self.link.kind == "usb" and g and clk is not None and not clk.get("location"):
            self.push("GEO", f"{g['lat']} {g['lon']} {g['unit']}")

    def run_upload(self):
        """Perform one queued upload to the clock's flash (grid animation / offline text), if any."""
        with uploads_lock:
            job = uploads.pop(0) if uploads else None
        if not job:
            return
        if job["type"] == "ganim":         # the clock runs the animation itself
            self.link.store_anim(job["name"], job["fps"], job["frame"])
            set_state(saved=f"grid: {job['name']}")
            self.last_cfg = 0              # changes the clock's settings: re-read them
            return
        self.link.store_frames(job["frames"], job["fps"], job["frame"])
        set_state(saved=f"grid: {len(job['frames'])} frames")
        self.last_cfg = 0                  # GCOMMIT changes the clock's settings: re-read them

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
                set_state(grid_rows=list(grid_preview["rows"]))
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
            set_state(grid_rows=list(rows))
            self.last_grid = time.time()
        elif not (c["grid_enable"] or c["grid_frame"]) and state["grid_rows"] is not None:
            set_state(grid_rows=None)                     # app grid off: the clock shows its own


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
            return self._send(200, (WEB / "dashboard.html").read_bytes(), "text/html; charset=utf-8")
        if self.path in ("/draw", "/draw.html"):
            return self._send(200, (WEB / "draw.html").read_bytes(), "text/html; charset=utf-8")
        if self.path == "/api/status":            # config = app settings + the clock's own
            with cfg_lock, state_lock:
                shown = {**cfg, **(state["clock"] or {}), "clock_pass": ""}
                st = {**state, "clock_pass_set": bool(cfg.get("clock_pass")), "app_protocol": PROTOCOL,
                      **firmware_status(state)}
                body = json.dumps({"state": st, "config": shown}).encode()
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
            clock = {k: v for k, v in body.items() if k in CLOCK_KEYS}
            for k, v in body.items():
                if k in CLOCK_KEYS:
                    continue
                if k not in DEFAULTS:
                    errors[k] = "unknown setting"
                    continue
                try:
                    clean[k] = coerce(k, v)
                except (TypeError, ValueError) as e:
                    errors[k] = str(e)
            if errors:                             # nothing reaches the clock unless the app part is valid
                return self._send(400, {"error": errors})
            warning = None
            if clock:                              # the clock checks its own; all-or-nothing there too
                ok, info = set_clock(clock)
                if not ok:
                    return self._send(400, {"error": info})
                warning = info
            with cfg_lock:
                cfg.update(clean)
                save_config(cfg)
            return self._send(200, {"ok": True, "warning": warning} if warning else {"ok": True})
        if self.path == "/api/update_clock":     # upload the shipped firmware to the clock (OTA)
            if not state["release"]:
                return self._send(400, {"error": "no firmware in wfd-firmware/releases"})
            if not update_clock():
                return self._send(409, {"error": "an update is already running"})
            return self._send(202, {"ok": True})
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
            if body.get("frames") or anim == "custom":
                if not custom:
                    return self._send(400, {"error": "no frames drawn yet"})
                job = {"type": "grid", "frames": custom, "fps": fps, "frame": frame}
                queued = len(custom)
            elif anim in BUILTIN_GRID:
                job = {"type": "ganim", "name": anim, "fps": fps, "frame": frame}
                queued = 1
            else:
                return self._send(400, {"error": f"'{anim}' needs live data; can't store it standalone"})
            with uploads_lock:
                uploads.append(job)
            return self._send(200, {"queued": queued})
        if self.path == "/api/om_save":              # kept for older pages: same as setting offline_text
            text = body.get("text", clock_setting("offline_text", ""))
            if not isinstance(text, str):
                return self._send(400, {"error": "text: expected a string"})
            text = clean_omtext(text)
            ok, info = set_clock({"offline_text": text})
            if not ok:
                return self._send(400, {"error": info})
            return self._send(200, {"queued": 1, "text": text})
        self._send(404, {"error": "not found"})


def main():
    if not CONFIG.exists():
        save_config(cfg)
    set_state(release=load_release())
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
