"""Refresh releases/ from the last `pio run` in clock/:

    python make_release.py

- releases/wfd-clock-firmware.bin  the app image, for OTA (browser, curl, or the PC app's
                                   "Update clock" button)
- releases/wfd-clock-merged.bin    bootloader + partitions + boot_app0 + app, for the first USB
                                   flash at 0x0 (flash_usb.py)
- releases/version.json            build stamp + protocol, read by the PC app to offer updates

Needs esptool (run via `uvx --from "esptool>=4.7,<5" esptool` if it isn't installed).
"""
import json
import re
import shutil
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
BUILD = HERE / "clock" / ".pio" / "build" / "wfd"
OUT = HERE / "releases"
BOOT_APP0 = Path.home() / ".platformio/packages/framework-arduinoespressif32/tools/partitions/boot_app0.bin"


def build_info(app_bin: Path, main_cpp: Path) -> dict:
    stamp = re.search(rb"wfd-clock [A-Z][a-z]{2} [ 0-9]{2} \d{4} [0-9:]{8}", app_bin.read_bytes())
    proto = re.search(r"#define WFD_PROTOCOL (\d+)", main_cpp.read_text(encoding="utf-8"))
    if not stamp or not proto:
        sys.exit("can't find the build stamp / WFD_PROTOCOL; build with `pio run` first")
    return {"fw": stamp.group().decode(), "protocol": int(proto.group(1))}


def merge(out: Path):
    esptool = [shutil.which("esptool")] if shutil.which("esptool") else ["uvx", "-q", "--from", "esptool>=4.7,<5", "esptool"]
    subprocess.run(esptool + [
        "--chip", "esp32", "merge_bin", "-o", str(out),
        "--flash_mode", "dio", "--flash_freq", "80m", "--flash_size", "4MB",   # 80m: keep the bootloader header
        "0x1000", str(BUILD / "bootloader.bin"), "0x8000", str(BUILD / "partitions.bin"),
        "0xe000", str(BOOT_APP0), "0x10000", str(BUILD / "firmware.bin")], check=True)


def main():
    app_bin = BUILD / "firmware.bin"
    if not app_bin.exists():
        sys.exit(f"missing {app_bin}; run `pio run` in clock/ first")
    info = build_info(app_bin, HERE / "clock" / "src" / "main.cpp")
    shutil.copyfile(app_bin, OUT / "wfd-clock-firmware.bin")
    merge(OUT / "wfd-clock-merged.bin")
    (OUT / "version.json").write_text(json.dumps(info, indent=2) + "\n", encoding="utf-8")
    print(f"releases/: {info['fw']} (protocol {info['protocol']})")


if __name__ == "__main__":
    main()
