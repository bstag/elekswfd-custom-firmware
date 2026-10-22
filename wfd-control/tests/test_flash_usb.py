"""wfd-firmware/flash_usb.py: keeps a clock's settings when re-flashing our own firmware."""
import sys
from pathlib import Path

import pytest

FW_DIR = Path(__file__).resolve().parents[2] / "wfd-firmware"
sys.path.insert(0, str(FW_DIR))
import flash_usb  # noqa: E402

MERGED = (FW_DIR / "releases" / "wfd-clock-merged.bin").read_bytes()


def test_reads_the_partition_table_from_the_image():
    names = [p[0] for p in flash_usb.partitions(MERGED)]
    assert names == ["nvs", "otadata", "app0", "app1", "spiffs"]
    nvs = next(p for p in flash_usb.partitions(MERGED) if p[0] == "nvs")
    assert (nvs[3], nvs[4]) == (0x9000, 0x5000)


def test_the_whole_image_would_erase_the_settings():
    assert set(MERGED[0x9000:0xE000]) == {0xFF}


def test_a_stock_clock_gets_the_whole_image():
    assert flash_usb.plan(MERGED, keep_settings=False) == [(0, MERGED)]


def test_our_clock_keeps_its_settings():
    (a0, d0), (a1, d1) = flash_usb.plan(MERGED, keep_settings=True)
    assert (a0, len(d0)) == (0, 0x9000)
    assert a1 == 0xE000 and d1 == MERGED[0xE000:]


def test_an_image_without_a_partition_table_is_refused():
    with pytest.raises(ValueError):
        flash_usb.plan(b"\xff" * 0x100, keep_settings=True)


@pytest.mark.parametrize("reply, fw", [
    ('noise\r\nCFG {"_fw":"wfd-clock Oct 14 2026 14:36:22","_protocol":4}\r\n', "wfd-clock Oct 14 2026 14:36:22"),
    ('CFG {"brightness":7}\n', "an older build"),
    ("ets Jun  8 2016 00:22:57\nrst:0x1\n", None),           # the stock firmware
    ("CFG {broken\n", None),
])
def test_recognises_our_firmware(reply, fw):
    assert flash_usb.our_firmware(reply) == fw
