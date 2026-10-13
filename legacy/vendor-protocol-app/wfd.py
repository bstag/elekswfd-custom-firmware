# /// script
# requires-python = ">=3.10"
# dependencies = ["pyserial>=3.5", "psutil>=5.9"]
# ///
"""WFD Control: a local web front end for the EleksWFD clock.

Run:  uv run wfd.py        (or: pip install pyserial psutil && python wfd.py)
Then open http://127.0.0.1:8737 (opened automatically).
"""
from __future__ import annotations

import argparse
import json
import shutil
import subprocess
import threading
import time
import webbrowser
from http import HTTPStatus
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import psutil
import serial
from serial.tools import list_ports

CH340_VID, CH340_PID = 0x1A86, 0x7523
STATIC = Path(__file__).with_name("static")


# --------------------------------------------------------------------------
# Device protocol (see ../PROTOCOL.md)
# --------------------------------------------------------------------------
def pack_frame(cells: list) -> bytes:
    """49 cells, row-major top-left first -> 7 bytes, one per row, leftmost cell = bit 6."""
    if len(cells) != 49:
        raise ValueError("frame needs 49 cells")
    out = bytearray(7)
    for r in range(7):
        for c in range(7):
            if cells[r * 7 + c]:
                out[r] |= 1 << (6 - c)
    return bytes(out)


class Clock:
    def __init__(self) -> None:
        self.lock = threading.RLock()
        self.port: serial.Serial | None = None
        self.port_name: str | None = None
        self.last_error: str | None = None

    # -- connection -------------------------------------------------------
    @staticmethod
    def ports() -> list[dict]:
        res = []
        for p in list_ports.comports():
            res.append({
                "device": p.device,
                "description": p.description,
                "is_clock": p.vid == CH340_VID and p.pid == CH340_PID,
            })
        return sorted(res, key=lambda p: not p["is_clock"])

    def connect(self, name: str | None = None) -> str:
        with self.lock:
            self.close()
            if not name:
                found = [p for p in self.ports() if p["is_clock"]]
                if not found:
                    raise RuntimeError("No CH340 device found. Is the clock plugged in?")
                name = found[0]["device"]
            s = serial.Serial()
            s.port, s.baudrate, s.timeout, s.write_timeout = name, 115200, 0.1, 2
            # Leave DTR/RTS untouched-ish: both asserted is a no-op on the ESP32 auto-reset circuit.
            s.dtr = True
            s.rts = True
            s.open()
            self.port, self.port_name, self.last_error = s, name, None
            return name

    def close(self) -> None:
        with self.lock:
            if self.port:
                try:
                    self.port.close()
                except Exception:
                    pass
            self.port = None

    @property
    def connected(self) -> bool:
        return bool(self.port and self.port.is_open)

    def send(self, data: bytes, pause: float = 0.0) -> None:
        with self.lock:
            if not self.connected:
                self.connect(self.port_name)
            try:
                self.port.write(data)
                self.port.flush()
                self.port.reset_input_buffer()  # device chatter is ignored, like the vendor app
            except serial.SerialException as e:
                self.last_error = str(e)
                self.close()
                raise
            if pause:
                time.sleep(pause)

    # -- commands ---------------------------------------------------------
    def text(self, msg: str) -> None:
        self.send(b"FA" + msg.encode("latin-1", errors="replace"))

    def stats(self, cpu: float, gpu: float, ram: float) -> None:
        bar = lambda v: max(0, min(20, int(v) // 5))
        self.send(bytes([0xEB, 0xAA, bar(cpu), bar(gpu), bar(ram), 0, 0, 0, 0]))

    def stats_off(self) -> None:
        with self.lock:
            self.send(b"CL", 0.05)
            self.send(bytes([0xEB, 0x00]) + bytes(7), 0.05)
            self.send(bytes([0xEF, 0x04]))

    def live_frame(self, cells: list) -> None:
        self.send(bytes([0xEC, 0xAA]) + pack_frame(cells))

    def exit_live(self) -> None:
        with self.lock:
            self.send(bytes([0xEC, 0x00]) + bytes(7), 0.02)
            self.send(bytes([0xEB, 0x00]) + bytes(7), 0.05)
            self.send(bytes([0xEF, 0x04]))

    def upload(self, frames: list, speed: int, progress=None) -> None:
        if not 1 <= len(frames) <= 255:
            raise ValueError("1-255 frames")
        speed = max(1, min(10, int(speed)))
        packed = [pack_frame(f) for f in frames]
        with self.lock:
            self.send(bytes([0xED, len(packed), speed]), 0.05)
            for i, fr in enumerate(packed, 1):
                self.send(b"\xEE" + fr, 0.1)
                if progress:
                    progress(i, len(packed))

    def reset_animation(self) -> None:
        self.send(b"\xEA")


# --------------------------------------------------------------------------
# System stats
# --------------------------------------------------------------------------
class Stats:
    def __init__(self) -> None:
        self.nvsmi = shutil.which("nvidia-smi")
        psutil.cpu_percent(None)

    def gpu(self) -> float:
        if not self.nvsmi:
            return 0.0
        try:
            out = subprocess.run(
                [self.nvsmi, "--query-gpu=utilization.gpu", "--format=csv,noheader,nounits"],
                capture_output=True, text=True, timeout=3,
                creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
            ).stdout
            vals = [float(x) for x in out.split() if x.strip()]
            return max(vals) if vals else 0.0
        except Exception:
            return 0.0

    def read(self) -> dict:
        return {
            "cpu": psutil.cpu_percent(None),
            "gpu": self.gpu(),
            "ram": psutil.virtual_memory().percent,
        }


class StatsSync(threading.Thread):
    def __init__(self, clock: Clock, stats: Stats) -> None:
        super().__init__(daemon=True)
        self.clock, self.stats = clock, stats
        self.enabled = threading.Event()
        self.latest = {"cpu": 0, "gpu": 0, "ram": 0}
        self.interval = 1.0

    def run(self) -> None:
        while True:
            self.latest = self.stats.read()
            if self.enabled.is_set():
                try:
                    self.clock.stats(**self.latest)
                except Exception as e:
                    self.clock.last_error = str(e)
            time.sleep(self.interval)


class TextRepeater(threading.Thread):
    """Optionally re-sends a text template every N seconds ({time}, {date}, {cpu}... are filled in)."""

    def __init__(self, clock: Clock, sync: StatsSync) -> None:
        super().__init__(daemon=True)
        self.clock, self.sync = clock, sync
        self.template: str | None = None
        self.every = 60
        self.wake = threading.Event()

    def render(self, tpl: str) -> str:
        now = time.localtime()
        s = self.sync.latest
        fields = {
            "time": time.strftime("%H:%M", now),
            "time12": time.strftime("%I:%M %p", now).lstrip("0"),
            "date": time.strftime("%a %b %d", now),
            "cpu": f"{s['cpu']:.0f}%", "gpu": f"{s['gpu']:.0f}%", "ram": f"{s['ram']:.0f}%",
        }
        out = tpl
        for k, v in fields.items():
            out = out.replace("{" + k + "}", v)
        return out

    def run(self) -> None:
        while True:
            tpl = self.template
            if tpl:
                try:
                    self.clock.text(self.render(tpl))
                except Exception as e:
                    self.clock.last_error = str(e)
            self.wake.wait(self.every if tpl else 3600)
            self.wake.clear()


# --------------------------------------------------------------------------
# HTTP API
# --------------------------------------------------------------------------
clock = Clock()
sync = StatsSync(clock, Stats())
repeater = TextRepeater(clock, sync)
upload_state = {"busy": False, "done": 0, "total": 0, "error": None}


def api(path: str, body: dict) -> dict:
    if path == "/api/status":
        return {
            "connected": clock.connected, "port": clock.port_name, "error": clock.last_error,
            "stats_sync": sync.enabled.is_set(), "stats": sync.latest,
            "repeat": {"template": repeater.template, "every": repeater.every},
            "upload": upload_state,
        }
    if path == "/api/ports":
        return {"ports": Clock.ports()}
    if path == "/api/connect":
        return {"port": clock.connect(body.get("port") or None)}
    if path == "/api/disconnect":
        clock.close()
        return {}
    if path == "/api/text":
        text = str(body.get("text", ""))
        if body.get("repeat"):
            repeater.template = text
            repeater.every = max(5, int(body.get("every", 60)))
            repeater.wake.set()
            return {"sent": repeater.render(text)}
        repeater.template = None
        msg = repeater.render(text) if "{" in text else text
        clock.text(msg)
        return {"sent": msg}
    if path == "/api/text/stop":
        repeater.template = None
        return {}
    if path == "/api/stats":
        if body.get("enabled"):
            sync.enabled.set()
        else:
            sync.enabled.clear()
            clock.stats_off()
        return {"stats_sync": sync.enabled.is_set()}
    if path == "/api/live":
        clock.live_frame(body["cells"])
        return {}
    if path == "/api/live/exit":
        clock.exit_live()
        return {}
    if path == "/api/upload":
        if upload_state["busy"]:
            raise RuntimeError("Upload already running")
        frames, speed = body["frames"], int(body.get("speed", 5))

        def work():
            was_syncing = sync.enabled.is_set()
            sync.enabled.clear()
            upload_state.update(busy=True, done=0, total=len(frames), error=None)
            try:
                clock.upload(frames, speed, lambda d, t: upload_state.update(done=d))
            except Exception as e:
                upload_state["error"] = str(e)
            finally:
                upload_state["busy"] = False
                if was_syncing:
                    sync.enabled.set()

        threading.Thread(target=work, daemon=True).start()
        return {"started": True}
    if path == "/api/reset":
        clock.reset_animation()
        return {}
    if path == "/api/raw":
        data = bytes.fromhex(str(body.get("hex", "")).replace(",", " "))
        if not data:
            raise ValueError("empty packet")
        clock.send(data)
        return {"sent": data.hex(" ")}
    raise KeyError(path)


class Handler(BaseHTTPRequestHandler):
    server_version = "WFDControl/1.0"

    def log_message(self, *a):  # quiet
        pass

    def _send(self, code: int, payload: bytes, ctype: str) -> None:
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(payload)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(payload)

    def do_GET(self):
        if self.path.startswith("/api/"):
            return self._api({})
        name = "index.html" if self.path in ("/", "/index.html") else None
        if not name:
            return self._send(404, b"not found", "text/plain")
        self._send(200, (STATIC / name).read_bytes(), "text/html; charset=utf-8")

    def do_POST(self):
        # Custom header forces a CORS preflight, so other websites can't drive the clock.
        if self.headers.get("X-WFD") != "1":
            return self._send(403, b"forbidden", "text/plain")
        n = int(self.headers.get("Content-Length") or 0)
        try:
            body = json.loads(self.rfile.read(n) or b"{}")
        except json.JSONDecodeError:
            return self._send(400, b'{"error":"bad json"}', "application/json")
        self._api(body)

    def _api(self, body: dict):
        try:
            res, code = api(self.path.split("?")[0], body), 200
        except KeyError:
            res, code = {"error": "unknown endpoint"}, 404
        except Exception as e:
            res, code = {"error": str(e)}, 400
        self._send(code, json.dumps(res).encode(), "application/json")


def main() -> None:
    ap = argparse.ArgumentParser(description="EleksWFD web control")
    ap.add_argument("--port", help="COM port (default: auto-detect CH340)")
    ap.add_argument("--http", type=int, default=8737)
    ap.add_argument("--no-browser", action="store_true")
    ap.add_argument("--stats", action="store_true", help="start with system-info sync on")
    args = ap.parse_args()

    try:
        print("Clock on", clock.connect(args.port))
    except Exception as e:
        print("Not connected yet:", e)
    sync.start()
    repeater.start()
    if args.stats:
        sync.enabled.set()

    url = f"http://127.0.0.1:{args.http}"
    srv = ThreadingHTTPServer(("127.0.0.1", args.http), Handler)
    print("WFD Control running at", url, "(Ctrl+C to quit)")
    if not args.no_browser:
        threading.Timer(0.5, webbrowser.open, (url,)).start()
    try:
        srv.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        clock.close()


if __name__ == "__main__":
    main()
