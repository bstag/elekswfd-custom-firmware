"""Segment selector: light one segment at a time on a reference digit / character,
you click which segment lit on a big drawing. Builds segmap.json.

Run with the wfd-bridge firmware on the clock:  python segmap.py  -> http://127.0.0.1:8741
"""
import json
import sys
import threading
import webbrowser
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from bridge import Bridge  # noqa: E402

HERE = Path(__file__).parent
OUT = HERE / "segmap.json"

# Reference positions, from the LED map:
#   digit_1 lives in RAM byte 12; its 7 segment bits are at offsets 0,1,2,4,5,6,7.
#   alnum_1 spans bytes 0,1; its 13 segment bits at the offsets below.
DIGIT_STEPS = [(12, o) for o in (0, 1, 2, 4, 5, 6, 7)]
CHAR_STEPS = [(0, o) for o in (0, 1, 2, 4, 5, 6, 7)] + [(1, o) for o in (0, 1, 4, 5, 6, 7)]

bridge = Bridge()
bridge.tm_init(6)
lock = threading.Lock()


def show(byte: int, offset: int) -> None:
    ram = [0] * 42
    if byte is not None:
        ram[byte] = 1 << offset
    with lock:
        bridge.tm_write(0x00, ram)


def load() -> dict:
    if OUT.exists():
        return json.loads(OUT.read_text())
    return {"digit": {}, "char": {}}


class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def _send(self, code, body, ctype="application/json"):
        b = body if isinstance(body, bytes) else json.dumps(body).encode()
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(b)))
        self.end_headers()
        self.wfile.write(b)

    def do_GET(self):
        if self.path == "/":
            return self._send(200, (HERE / "segmap.html").read_bytes(), "text/html; charset=utf-8")
        if self.path == "/api/map":
            return self._send(200, load())
        self._send(404, {"error": "not found"})

    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = json.loads(self.rfile.read(n) or b"{}")
        if self.path == "/api/show":
            steps = DIGIT_STEPS if body["mode"] == "digit" else CHAR_STEPS
            i = body["step"]
            if 0 <= i < len(steps):
                show(*steps[i])
            else:
                show(None, 0)
            return self._send(200, {})
        if self.path == "/api/save":
            m = load()
            key = str(body["step"])
            if body.get("segment") is None:
                m[body["mode"]].pop(key, None)
            else:
                m[body["mode"]][key] = body["segment"]
            OUT.write_text(json.dumps(m, indent=1))
            return self._send(200, {"saved": len(m[body["mode"]])})
        if self.path == "/api/off":
            show(None, 0)
            return self._send(200, {})
        self._send(404, {"error": "not found"})


if __name__ == "__main__":
    srv = ThreadingHTTPServer(("127.0.0.1", 8741), H)
    print("segmap on http://127.0.0.1:8741")
    if "--no-browser" not in sys.argv:
        threading.Timer(0.5, webbrowser.open, ("http://127.0.0.1:8741",)).start()
    srv.serve_forever()
