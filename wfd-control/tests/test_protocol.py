"""Cross-checks between the app and the firmware source, so the two can't drift apart."""
import json
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
    app.clock_requests.append([{"offline_text": "HELLO WORLD"}, __import__("threading").Event(), None])
    app.uploads.append({"type": "grid", "frames": [[1] * 7], "fps": 8, "frame": False})
    app.uploads.append({"type": "ganim", "name": "spin", "fps": 8, "frame": False})
    s = sim(30, events={10: app.sync_now.set})
    s.run()
    sent = set(s.commands())
    unknown = sent - firmware_commands()
    assert not unknown, f"app sends commands the firmware doesn't handle: {sorted(unknown)}"


def test_app_expands_every_firmware_placeholder(app, sim):
    """Text written once should read the same on the clock (app off) and from the app."""
    names = set(re.findall(r's\.replace\("\{(\w+)\}"', MAIN_CPP.read_text(encoding="utf-8")))
    assert {"time", "time12", "date", "day", "temp"} <= names
    app.cfg.update(message_mode="text", custom_text=" ".join("{%s}" % n for n in sorted(names)))
    sim(1).run()
    assert "{" not in app.state["message"], app.state["message"]


def test_app_builtin_grid_list_matches_firmware(app):
    header = (ROOT / "wfd-firmware" / "clock" / "src" / "grid_engine.h").read_text(encoding="utf-8")
    names = re.findall(r'"(\w+)"', re.search(r"NAMES\[N_ANIM\]\s*=\s*\{([^}]*)\}", header).group(1))
    assert names == ["custom"] + app.BUILTIN_GRID


WEB = ROOT / "wfd-firmware" / "clock" / "web"


def test_embedded_pages_match_their_source():
    """src/web_pages.h must hold the gzip of every web/*.html (embed_web.py runs on pio builds)."""
    import gzip
    header = (ROOT / "wfd-firmware" / "clock" / "src" / "web_pages.h").read_text(encoding="utf-8")
    pages = sorted(WEB.glob("*.html"))
    assert {p.name for p in pages} >= {"dashboard.html", "draw.html"}
    for page in pages:
        name = page.stem.upper() + "_GZ"
        block = re.search(name + r"\[\] PROGMEM = \{([^}]*)\}", header)
        assert block, f"{page.name} isn't embedded: run `python wfd-firmware/clock/embed_web.py`"
        data = bytes(int(b, 16) for b in re.findall(r"0x([0-9a-f]{2})", block.group(1)))
        assert int(re.search(name + r"_LEN = (\d+)", header).group(1)) == len(data)
        html = page.read_bytes().replace(b"\r\n", b"\n")
        assert gzip.decompress(data) == html, f"{page.name} changed: run `python wfd-firmware/clock/embed_web.py`"


def _sent_keys(html):
    """Setting names a page sends: set({ key: ... }), set({ [id]: ... }) loops over literal lists,
    and slider(..., 'key')."""
    sent = set(re.findall(r"set\(\{\s*(\w+):", html)) | set(re.findall(r"\bv => \(\{\s*(\w+):", html))   # seg() buttons
    for ids in re.findall(r"for \(const \w+ of \[([^\]]*)\]\)[^;]{0,160}?set\(\{ \[", html):
        sent |= set(re.findall(r"'(\w+)'", ids))
    sent |= set(re.findall(r"slider\('[^']+', '[^']+', '[^']*', '(\w+)'", html))
    return sent


def test_shared_pages_only_send_known_settings(app):
    """The dashboard and animator talk to the clock and to the app: every setting they send must
    have an owner (app DEFAULTS or the clock's CLOCK_KEYS)."""
    known = set(app.DEFAULTS) | app.CLOCK_KEYS
    for page in ("dashboard.html", "draw.html"):
        sent = _sent_keys((WEB / page).read_text(encoding="utf-8"))
        assert not sent - known, f"{page} sends settings nobody owns: {sorted(sent - known)}"
    assert len(_sent_keys((WEB / "dashboard.html").read_text(encoding="utf-8"))) >= 25


def test_every_clock_setting_is_on_the_dashboard(app):
    html = (WEB / "dashboard.html").read_text(encoding="utf-8")
    missing = {k for k in app.CLOCK_KEYS if not re.search(r"\b%s\b" % k, html)}
    assert not missing, f"clock settings with no control on the dashboard: {sorted(missing)}"


def test_clock_owned_settings_match_the_firmware(app):
    """The app forwards exactly the settings the clock reports (configJson), no more, no fewer."""
    fw = set(re.findall(r'\bc\["(\w+)"\]', MAIN_CPP.read_text(encoding="utf-8")))
    assert app.CLOCK_KEYS == fw
    assert not app.CLOCK_KEYS & set(app.DEFAULTS), "a setting has one owner"


def test_app_and_shipped_firmware_speak_the_same_protocol(app):
    fw = int(re.search(r"#define WFD_PROTOCOL (\d+)", MAIN_CPP.read_text(encoding="utf-8")).group(1))
    shipped = json.loads((ROOT / "wfd-firmware" / "releases" / "version.json").read_text(encoding="utf-8"))
    assert app.PROTOCOL == fw, "bump app.PROTOCOL together with WFD_PROTOCOL"
    assert shipped["protocol"] == fw, "releases/ is stale: run wfd-firmware/make_release.py"


def test_firmware_font_table_is_generated_from_font14():
    assert FONT_H.read_text(encoding="utf-8") == font14.c_header(), \
        "font14_table.h is stale: run `python wfd-control/font14.py`"


def test_icon_count_matches_firmware(app):
    panel = (ROOT / "wfd-firmware" / "clock" / "src" / "clock_panel.h").read_text(encoding="utf-8")
    m = re.search(r"N_ICONS\s*=?\s*(\d+)", panel) or re.search(r"#define\s+N_ICONS\s+(\d+)", panel)
    assert m, "N_ICONS not found in clock_panel.h"
    assert len(app.ICON) == int(m.group(1))
