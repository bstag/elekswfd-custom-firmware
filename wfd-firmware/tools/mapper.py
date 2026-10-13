"""LED mapper: lights one TM1680 RAM bit at a time; you click what lit in the browser.

Run with the wfd-bridge firmware on the clock:   python mapper.py   -> http://127.0.0.1:8740
Progress is saved to panel_map.json after every click (resumable).
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
MAP_FILE = HERE / "panel_map.json"
NBITS = 384  # 48 bytes

bridge = Bridge()
lock = threading.Lock()
bridge.tm_init(6)


def show_bit(i: int | None) -> None:
    ram = [0] * 48
    if i is not None and 0 <= i < NBITS:
        ram[i // 8] = 1 << (i % 8)
    with lock:
        bridge.tm_write(0x00, ram)


def load() -> dict:
    if MAP_FILE.exists():
        return json.loads(MAP_FILE.read_text())
    return {"bits": {}}  # bit index (str) -> element id, or "" for nothing


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
            return self._send(200, (HERE / "mapper.html").read_bytes(), "text/html; charset=utf-8")
        if self.path == "/api/map":
            return self._send(200, load())
        self._send(404, {"error": "not found"})

    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = json.loads(self.rfile.read(n) or b"{}")
        if self.path == "/api/show":
            show_bit(body.get("bit"))
            return self._send(200, {})
        if self.path == "/api/save":
            m = load()
            if body.get("element") is None:
                m["bits"].pop(str(body["bit"]), None)
            else:
                m["bits"][str(body["bit"])] = body["element"]
            MAP_FILE.write_text(json.dumps(m, indent=1, sort_keys=True))
            return self._send(200, {"count": len(m["bits"])})
        if self.path == "/api/all":
            with lock:
                bridge.tm_write(0x00, [0xFF if body.get("on") else 0] * 48)
            return self._send(200, {})
        self._send(404, {"error": "not found"})


if __name__ == "__main__":
    srv = ThreadingHTTPServer(("127.0.0.1", 8740), H)
    print("mapper on http://127.0.0.1:8740")
    if "--no-browser" not in sys.argv:
        threading.Timer(0.5, webbrowser.open, ("http://127.0.0.1:8740",)).start()
    srv.serve_forever()
