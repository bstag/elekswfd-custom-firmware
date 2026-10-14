"""Regenerate the font14 / grid_anim / icons parity fixtures from the Python reference.

Run from the repo root (app.py needs its dependencies for compute_icons/scroll_frame):
    cd wfd-control && uv run -q --with-requirements requirements.txt python -I \
        ../wfd-app/tests/fixtures/gen_font14_grid_icons.py
Writes font14_cases.json, grid_frames.json and icons_cases.json next to this script.
"""
import json
import math
import struct
import sys
import types
from pathlib import Path

HERE = Path(__file__).resolve().parent
WFD_CONTROL = HERE.parent.parent.parent / "wfd-control"
sys.path.insert(0, str(WFD_CONTROL))

import font14  # noqa: E402
import grid_anim  # noqa: E402
import app  # noqa: E402


def dump(name, obj):
    (HERE / name).write_text(json.dumps(obj, separators=(",", ":")) + "\n", encoding="utf-8", newline="\n")
    print("wrote", name)


def f64_bits(x):
    return struct.unpack("<Q", struct.pack("<d", float(x)))[0]


# ---------------------------------------------------------------- font14
TEXTS = ["", "A", "hello", "HELLO!", "Now playing: Song Title - Artist", "abc/def\\ghi",
         "12:34", "x" * 6, "seven77", "50% off <3>", "été ß", "tab\there"]
glyphs = [[chr(c), list(font14.glyph(chr(c)))] for c in range(32, 127)]
glyphs += [[ch, list(font14.glyph(ch))] for ch in ["é", "ß", "ÿ", "\t", "\x7f"]]
render6 = []
for t in TEXTS:
    for start in (-7, -2, -1, 0, 1, 3, 5, 6, 10, 40):
        render6.append({"text": t, "start": start, "out": font14.render6(t, start)})
scroll = []
for t in TEXTS:
    for pos in (0, 1, 2, 5, 6, 9, 13, 30, 34, 35, 36, 37, 100, 1001):
        scroll.append({"text": t, "pos": pos, "out": app.scroll_frame(t, pos)})
OM = ["", "  hi  ", "café ☃ ok", "line1\nline2\r\n", "\ttabbed\t", "a" * 200,
      " " + "b" * 120 + " ", "\x00\x1f\x7f\x80 ctrl ~", " nbsp ", "x" * 95 + "éyz",
      "   " + "q" * 96, "Hello, World! {}[]|~`"]
omtext = [{"text": t, "out": app.clean_omtext(t)} for t in OM]
dump("font14_cases.json", {"glyphs": glyphs, "render6": render6, "scroll": scroll,
                           "omtext": omtext, "c_header": font14.c_header()})

# ---------------------------------------------------------------- grid_anim
N = 400


def bands_at(i):
    k = 0 if i % 23 < 4 else (i % 23) / 22
    return [(math.sin(i * 0.31 + c * 0.9) + 1) / 2 * k for c in range(7)]


CUSTOM = [[0x7F, 0, 0x41, 0x22, 0x14, 0x08, 0xFF], [1, 2, 4, 8, 16, 32, 64],
          [0, 0, 0, 0, 0, 0, 0], [0x55, 0x2A, 0x55, 0x2A, 0x55, 0x2A, 0x55]]

anims = {}
for name in ["spin", "bounce", "wave", "pulse", "scan", "heart", "nope"]:
    eng = grid_anim.GridEngine()
    anims[name] = {"frames": [eng.frame(name, {}) for _ in range(N)]}
for name in ["audio", "mouth"]:
    eng = grid_anim.GridEngine()
    bands = [bands_at(i) for i in range(N)]
    frames = [eng.frame(name, {"bands": b}) for b in bands]
    anims[name] = {"frames": frames, "bands_bits": [[f64_bits(x) for x in b] for b in bands]}
eng = grid_anim.GridEngine()
anims["custom"] = {"frames": [eng.frame("custom", {"custom": CUSTOM}) for _ in range(N)],
                   "custom": CUSTOM}
eng = grid_anim.GridEngine()
anims["custom_empty"] = {"frames": [eng.frame("custom", {"custom": []}) for _ in range(10)]}
# cpu at 100% (full, no flicker) and 0% (blank) is deterministic
for pct in (0, 100):
    eng = grid_anim.GridEngine()
    anims[f"cpu{pct}"] = {"frames": [eng.frame("cpu", {"cpu": pct}) for _ in range(20)]}

# switching animations resets t and state
eng = grid_anim.GridEngine()
seq = []
for name, n in [("spin", 9), ("bounce", 13), ("spin", 5), ("wave", 4), ("bounce", 7), ("pulse", 3)]:
    for _ in range(n):
        seq.append([name, eng.frame(name, {})])
dump("grid_frames.json", {"animations": grid_anim.ANIMATIONS, "anims": anims, "switch": seq})

# ---------------------------------------------------------------- icons
icon_names = sorted(app.ICON, key=app.ICON.get)
cases = []
real_time = app.time


def icons_at(now, *args):
    app.time = types.SimpleNamespace(time=lambda: now + 0.25)
    try:
        return app.compute_icons(*args)
    finally:
        app.time = real_time


for mode in ["all", "audio", "system", "off", "", "weird"]:
    for out_l, mic_l in [(0, 0), (1, 1), (2, 2), (3, 3), (4, 0), (6, 5), (7, 1), (9, 12), (10, 0), (12, 12)]:
        for cpu, gpu, ram in [(0, 0, 0), (50, 30, 75), (50.5, 30.1, 75.01), (99, 10, 80), (20, 95.5, 10)]:
            for online in (False, True):
                for battery in (None, 0, 29, 30, 100):
                    for now in (1_700_000_000, 1_700_000_001):
                        mask = icons_at(now, mode, {"out": out_l, "mic": mic_l},
                                        {"cpu": cpu, "gpu": gpu, "ram": ram}, online, battery)
                        cases.append({"mode": mode, "out": out_l, "mic": mic_l, "cpu": cpu,
                                      "gpu": gpu, "ram": ram, "online": online,
                                      "battery": battery, "now": now, "mask": mask})
dump("icons_cases.json", {"names": icon_names, "cases": cases})
