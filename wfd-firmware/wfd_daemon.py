"""WFD background helper: keeps the clock's time correct and drives the two spectrum
columns from PC audio.  Column 1 (left) = system audio OUTPUT (what's playing),
Column 2 (right) = default MICROPHONE input.  Runs forever, reconnecting as needed.

Pair with the wfd-clock firmware. Start at login via wfd_daemon.vbs.
"""
import datetime
import json
import os
import shutil
import subprocess
import threading
import time
import urllib.parse
import urllib.request

import numpy as np
import psutil
import pyaudiowpatch as pa
import serial
from serial.tools import list_ports

NLEV = 12
FPS = 30.0
RESYNC_SEC = 600
GAIN_OUT = 1.0
GAIN_MIC = 1.0
LOG = os.path.join(os.path.dirname(os.path.abspath(__file__)), "wfd_daemon.log")
_logged = set()


def log(*a, once=None):
    if once is not None:
        if once in _logged:
            return
        _logged.add(once)
    try:
        with open(LOG, "a") as f:
            f.write(time.strftime("%H:%M:%S ") + " ".join(str(x) for x in a) + "\n")
    except OSError:
        pass


# ---------- clock serial ----------
def find_port():
    for p in list_ports.comports():
        if p.vid == 0x1A86 and p.pid == 0x7523:
            return p.device
    return None


def open_serial():
    port = find_port()
    if not port:
        return None
    try:
        s = serial.Serial()
        s.port, s.baudrate, s.timeout = port, 115200, 1
        s.dtr = s.rts = False
        s.open()
        time.sleep(0.2)
        return s
    except serial.SerialException as e:
        log("serial open failed", e)
        return None


def set_time(s):
    now = datetime.datetime.now()
    s.write(f"SET {now.year%100} {now.month} {now.day} {now.isoweekday()} "
            f"{now.hour} {now.minute} {now.second}\n".encode())
    time.sleep(0.2)
    s.reset_input_buffer()
    log("time set", now.isoformat())


# ---------- audio capture (two reader threads keep a latest RMS each) ----------
energy = {"out": 0.0, "mic": 0.0}
stop = threading.Event()
_pa_lock = threading.Lock()          # serialize PortAudio init across threads

# ---------- system metrics (CPU / GPU / RAM percent) ----------
_nvsmi = shutil.which("nvidia-smi")
sysmetrics = {"cpu": 0, "gpu": 0, "ram": 0}


# ---------- weather (word + temperature) ----------
CONFIG = os.path.join(os.path.dirname(os.path.abspath(__file__)), "wfd_config.json")
weather = {"code": 0, "temp": 0, "unit": 0}


def read_config():
    try:
        with open(CONFIG) as f:
            c = json.load(f)
        return str(c.get("location", "")).strip(), str(c.get("units", "fahrenheit")).strip().lower()
    except Exception:
        return "", "fahrenheit"


def _get(url):
    req = urllib.request.Request(url, headers={"User-Agent": "wfd-clock"})
    with urllib.request.urlopen(req, timeout=8) as r:
        return json.load(r)


def resolve_location(loc):
    """Return (lat, lon, label) for '' (IP), a US ZIP, or a 'Town, State' string."""
    if not loc:
        d = _get("http://ip-api.com/json/?fields=lat,lon,city")
        return d["lat"], d["lon"], d.get("city", "IP")
    if loc.isdigit() and len(loc) == 5:
        p = _get(f"https://api.zippopotam.us/us/{loc}")["places"][0]
        return float(p["latitude"]), float(p["longitude"]), f'{p["place name"]} {p["state abbreviation"]}'
    g = _get(f"https://geocoding-api.open-meteo.com/v1/search?name={urllib.parse.quote(loc)}&count=1")
    r = g["results"][0]
    return r["latitude"], r["longitude"], r["name"]


def _wmo_to_word(c):
    if c in (0, 1):
        return 1                       # sunny / mainly clear
    if c in (2, 3, 45, 48):
        return 2                       # cloudy / fog
    return 3                           # 51+ drizzle/rain/snow/showers/thunderstorm


def weather_reader():
    cur_loc = None
    lat = lon = None
    while not stop.is_set():
        try:
            loc, units = read_config()
            if loc != cur_loc or lat is None:          # location changed -> re-resolve
                lat, lon, label = resolve_location(loc)
                cur_loc = loc
                log("weather location", repr(loc or "(auto)"), "->", label, lat, lon)
            unit_name = "celsius" if units.startswith("c") else "fahrenheit"
            weather["unit"] = 1 if unit_name == "celsius" else 0
            url = (f"https://api.open-meteo.com/v1/forecast?latitude={lat}&longitude={lon}"
                   f"&current=temperature_2m,weather_code&temperature_unit={unit_name}")
            c = _get(url)["current"]
            weather["code"] = _wmo_to_word(int(c["weather_code"]))
            weather["temp"] = int(round(c["temperature_2m"]))
            log("weather", weather)
        except Exception as e:
            log("weather error", e)
        stop.wait(900)                 # refresh every 15 min


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
        sysmetrics["cpu"] = int(psutil.cpu_percent(None))
        sysmetrics["gpu"] = gpu
        sysmetrics["ram"] = int(psutil.virtual_memory().percent)
        time.sleep(1.0)


def reader(kind):
    """kind 'out' = default-output loopback, 'mic' = default input."""
    import ctypes
    try:
        ctypes.windll.ole32.CoInitializeEx(None, 0)   # WASAPI needs COM in this thread
    except Exception as e:
        log(kind, "CoInitialize failed", e)
    while not stop.is_set():
        p = None
        st = None
        try:
            with _pa_lock:                      # one PortAudio init at a time
                p = pa.PyAudio()
                wasapi = p.get_host_api_info_by_type(pa.paWASAPI)
                if kind == "out":
                    dev = p.get_device_info_by_index(wasapi["defaultOutputDevice"])
                    lb = None
                    for d in p.get_loopback_device_info_generator():
                        if dev["name"] in d["name"]:
                            lb = d
                            break
                    dev = lb or next(p.get_loopback_device_info_generator())
                else:
                    dev = p.get_device_info_by_index(wasapi["defaultInputDevice"])
                idx = dev["index"]
                ch = max(1, int(dev["maxInputChannels"]))
                sr = int(dev["defaultSampleRate"])
                st = p.open(format=pa.paInt16, channels=ch, rate=sr, input=True,
                            input_device_index=idx, frames_per_buffer=1024)
            log(kind, "capturing", dev["name"], f"{ch}ch {sr}Hz", once=kind + dev["name"])
            while not stop.is_set():
                raw = st.read(1024, exception_on_overflow=False)
                x = np.frombuffer(raw, np.int16).astype(np.float32) / 32768.0
                if ch > 1:
                    x = x.reshape(-1, ch).mean(axis=1)
                energy[kind] = float(np.sqrt(np.mean(x * x)) + 1e-9)
        except Exception as e:
            log(kind, "capture error, retrying:", e)
            energy[kind] = 0.0
            time.sleep(1.5)
        finally:
            try:
                if st:
                    st.close()
            except Exception:
                pass
            try:
                if p:
                    p.terminate()
            except Exception:
                pass


def main():
    threading.Thread(target=reader, args=("out",), daemon=True).start()
    time.sleep(0.5)                       # stagger: concurrent PortAudio init crashes
    threading.Thread(target=reader, args=("mic",), daemon=True).start()
    threading.Thread(target=metrics_reader, daemon=True).start()
    threading.Thread(target=weather_reader, daemon=True).start()

    peak = {"out": 1e-5, "mic": 1e-5}
    disp = {"out": 0.0, "mic": 0.0}
    gain = {"out": GAIN_OUT, "mic": GAIN_MIC}
    s = None
    last_sync = 0.0
    last_bars = 0.0
    last_wx = 0.0
    period = 1.0 / FPS

    while True:
        if s is None or not s.is_open:
            s = open_serial()
            if s:
                set_time(s)
                last_sync = time.time()
            else:
                time.sleep(2)
                continue
        if time.time() - last_sync > RESYNC_SEC:
            try:
                set_time(s)
                last_sync = time.time()
            except serial.SerialException:
                s = None
                continue

        out_lvl = 0
        vals = {}
        for k in ("out", "mic"):
            e = energy[k] * gain[k]
            peak[k] = max(e, peak[k] * 0.995)
            frac = 0.0 if peak[k] < 1e-5 else e / peak[k]
            target = NLEV * max(0.0, (frac - 0.12) / 0.88)
            disp[k] = target if target >= disp[k] else max(target, disp[k] - 0.8)
            vals[k] = int(round(min(NLEV, disp[k])))
        try:
            s.write(f"SPEC {vals['out']} {vals['mic']}\n".encode())
            if time.time() - last_bars > 1.0:          # CPU/GPU/RAM once a second
                s.write(f"BARS {sysmetrics['cpu']} {sysmetrics['gpu']} {sysmetrics['ram']}\n".encode())
                last_bars = time.time()
            if time.time() - last_wx > 60.0:            # weather once a minute
                s.write(f"WX {weather['code']} {weather['temp']} {weather['unit']}\n".encode())
                last_wx = time.time()
        except serial.SerialException:
            s = None
        time.sleep(period)


if __name__ == "__main__":
    main()
