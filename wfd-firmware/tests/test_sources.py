"""Generated firmware sources must match what they're generated from.

These two checks used to live in the Python app's test_protocol.py (now legacy/python-app/tests);
they only look at the firmware, so they run here.
"""
import gzip
import re
from pathlib import Path

import font14  # oracle/font14.py

ROOT = Path(__file__).resolve().parents[2]
SRC = ROOT / "wfd-firmware" / "clock" / "src"
WEB = ROOT / "wfd-firmware" / "clock" / "web"


def test_embedded_pages_match_their_source():
    """src/web_pages.h must hold the gzip of every web/*.html (embed_web.py runs on pio builds)."""
    header = (SRC / "web_pages.h").read_text(encoding="utf-8")
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


def test_firmware_font_table_is_generated_from_font14():
    assert (SRC / "font14_table.h").read_text(encoding="utf-8") == font14.c_header(), \
        "font14_table.h is stale: run `python wfd-firmware/tests/oracle/font14.py`"
