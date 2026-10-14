"""Back up the clock's entire 4 MB flash over USB, so you can revert later.

    pip install esptool pyserial
    python backup_usb.py

Writes firmware-backup/clock-backup-<timestamp>.bin. Keep it safe — it's the only way
to restore YOUR clock exactly (backups from other clocks usually don't work, the stock
firmware is tied to the chip's MAC). Restore with:

    esptool --chip esp32 --port COMx write-flash 0x0 firmware-backup/clock-backup-<...>.bin

Needs a USB 2.0 port (see docs/FLASHING.md). Reading 4 MB takes ~1–2 minutes.
"""
import datetime
import json
import subprocess
import sys
from pathlib import Path

from serial.tools import list_ports

OUT_DIR = Path(__file__).parent.parent / "firmware-backup"
CH340_VID, CH340_PID = 0x1A86, 0x7523    # the clock's USB-serial chip


DRIVER_URL = "https://www.wch-ic.com/downloads/CH341SER_EXE.html"


def find_port():
    for p in list_ports.comports():
        if p.vid == CH340_VID and p.pid == CH340_PID:
            return p.device
    return None


def not_found_message():
    """Why there's no COM port for the clock: not plugged in, or plugged in without its driver."""
    msg = "CH340 clock not found. Plug it into a USB 2.0 port."
    if sys.platform != "win32":
        return msg
    try:   # present USB devices with the CH340's IDs, and Windows' problem code for each
        ps = ("Get-PnpDevice -PresentOnly | Where-Object InstanceId -match 'VID_1A86&PID_7523' | "
              "Select-Object InstanceId, ConfigManagerErrorCode | ConvertTo-Json -Compress")
        out = subprocess.run(["powershell", "-NoProfile", "-Command", ps],
                             capture_output=True, text=True, timeout=20).stdout.strip()
        devices = json.loads(out) if out else []
    except (OSError, ValueError, subprocess.SubprocessError):
        return msg
    if isinstance(devices, dict):
        devices = [devices]
    if not devices:
        return msg
    return ("The clock is plugged in, but Windows has no working driver for its USB chip (CH340)."
            " Windows normally installs it by itself; if it hasn't, get CH341SER.EXE from\n"
            f"  {DRIVER_URL}\n"
            "run it, then unplug and replug the clock.")


def backup(port=None):
    port = port or find_port()
    if not port:
        sys.exit(not_found_message())
    OUT_DIR.mkdir(exist_ok=True)
    out = OUT_DIR / f"clock-backup-{datetime.datetime.now():%Y%m%d-%H%M%S}.bin"
    print(f"Backing up full flash from {port} -> {out.name} (takes ~1-2 min)...")
    rc = subprocess.run([sys.executable, "-m", "esptool", "--chip", "esp32", "--port", port,
                         "--baud", "460800", "read-flash", "0", "0x400000", str(out)]).returncode
    if rc == 0:
        print(f"\nBackup saved: {out}")
        return out
    print("\nBackup failed — use a USB 2.0 port and retry. See docs/FLASHING.md.")
    sys.exit(rc)


if __name__ == "__main__":
    backup()
