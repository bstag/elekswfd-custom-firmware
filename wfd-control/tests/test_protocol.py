"""Cross-checks between the app and the firmware source, so the two can't drift apart."""
import re
from pathlib import Path

import font14

ROOT = Path(__file__).resolve().parents[2]
MAIN_CPP = ROOT / "wfd-firmware" / "clock" / "src" / "main.cpp"
FONT_H = ROOT / "wfd-firmware" / "clock" / "src" / "font14_table.h"


def firmware_commands():
    return set(re.findall(r'cmd == "(\w+)"', MAIN_CPP.read_text(encoding="utf-8")))


def test_firmware_understands_everything_the_app_sends(app, sim):
    app.cfg.update(message_mode="rotate", rotate_date=True, grid_enable=True, icon_mode="system", logo=True)
    app.uploads.append({"type": "om", "frames": app.render_om_frames("HELLO WORLD"), "fps": 3})
    app.uploads.append({"type": "grid", "frames": [[1] * 7], "fps": 8, "frame": False})
    s = sim(30, events={10: app.sync_now.set})
    s.run()
    sent = set(s.commands())
    unknown = sent - firmware_commands()
    assert not unknown, f"app sends commands the firmware doesn't handle: {sorted(unknown)}"


def test_firmware_font_table_is_generated_from_font14():
    assert FONT_H.read_text(encoding="utf-8") == font14.c_header(), \
        "font14_table.h is stale: run `python wfd-control/font14.py`"


def test_icon_count_matches_firmware(app):
    panel = (ROOT / "wfd-firmware" / "clock" / "src" / "clock_panel.h").read_text(encoding="utf-8")
    m = re.search(r"N_ICONS\s*=?\s*(\d+)", panel) or re.search(r"#define\s+N_ICONS\s+(\d+)", panel)
    assert m, "N_ICONS not found in clock_panel.h"
    assert len(app.ICON) == int(m.group(1))
