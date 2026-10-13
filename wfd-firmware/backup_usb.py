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
import subprocess
import sys
from pathlib import Path

from serial.tools import list_ports

OUT_DIR = Path(__file__).parent.parent / "firmware-backup"
CH340_VID, CH340_PID = 0x1A86, 0x7523    # the clock's USB-serial chip


def find_port():
    for p in list_ports.comports():
        if p.vid == CH340_VID and p.pid == CH340_PID:
            return p.device
    return None


def backup(port=None):
    port = port or find_port()
    if not port:
        sys.exit("CH340 clock not found. Plug into a USB 2.0 port.")
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
