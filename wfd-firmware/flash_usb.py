"""One-shot USB flasher for the WFD clock (first-time install).

    pip install esptool pyserial
    python flash_usb.py

Flashes releases/wfd-clock-merged.bin to the clock over USB. After this, set up
WiFi (join the clock's "WFD-Setup" network) and all future updates go over WiFi
via the clock's web page (http://<clock-ip>/update) — no USB needed again.

IMPORTANT: the CH340 only enters flashing mode reliably on a USB 2.0 port.
If you get "Failed to connect" / "Wrong boot mode", move the cable to a USB 2.0
port (often the black ones, or on the back of the PC) and try again.
"""
import subprocess
import sys
from pathlib import Path

import backup_usb

MERGED = Path(__file__).parent / "releases" / "wfd-clock-merged.bin"


def main():
    if not MERGED.exists():
        sys.exit(f"missing {MERGED} — build it with PlatformIO or get it from releases/")
    port = backup_usb.find_port()
    if not port:
        sys.exit("CH340 clock not found. Plug it into a USB 2.0 port and install the CH340 driver.")
    if "--no-backup" not in sys.argv:
        print("Backing up your current firmware first (so you can revert)...")
        backup_usb.backup(port)        # read full flash before we overwrite it
    print(f"\nFlashing {MERGED.name} to {port} (use a USB 2.0 port if this fails)...")
    cmd = [sys.executable, "-m", "esptool", "--chip", "esp32", "--port", port,
           "--baud", "460800", "--before", "default-reset", "--after", "hard-reset",
           "write-flash", "0x0", str(MERGED)]
    rc = subprocess.run(cmd).returncode
    if rc == 0:
        print("\nDone. Now join the clock's 'WFD-Setup' WiFi on your phone and enter your "
              "2.4GHz network credentials. From then on, update over WiFi — no USB.")
    else:
        print("\nFlash failed. Most common fix: use a USB 2.0 port (not USB 3). "
              "See docs/FLASHING.md.")
    sys.exit(rc)


if __name__ == "__main__":
    main()
