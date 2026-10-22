"""USB flasher for the WFD clock (first-time install, or a re-flash).

    pip install esptool pyserial
    python flash_usb.py [--no-backup] [--wipe-settings]

Backs up the clock's whole flash first (see backup_usb.py), then flashes
releases/wfd-clock-merged.bin. After a first flash, set up WiFi (join the clock's
"WFD-Setup" network) and all future updates go over WiFi — no USB needed again.

The merged image fills the settings partition (nvs: WiFi and every setting) with 0xFF,
so writing it whole erases them. A clock that already runs this firmware keeps them:
the script asks it first (CFG) and then writes everything except nvs. --wipe-settings
writes the whole image anyway. (wfd-app's wfd-flash.exe does the same without Python.)

IMPORTANT: the CH340 only enters flashing mode reliably on a USB 2.0 port.
If you get "Failed to connect" / "Wrong boot mode", move the cable to a USB 2.0
port (often the black ones, or on the back of the PC) and try again.
"""
import json
import struct
import subprocess
import sys
import tempfile
import time
from pathlib import Path

import backup_usb

MERGED = Path(__file__).parent / "releases" / "wfd-clock-merged.bin"
PARTITION_TABLE = 0x8000


def partitions(image):
    """The partition table inside a merged image: [(name, type, subtype, offset, size)]."""
    out, at = [], PARTITION_TABLE
    while at + 32 <= len(image) and image[at:at + 2] == b"\xaa\x50":
        kind, sub, off, size = struct.unpack_from("<BBII", image, at + 2)
        out.append((image[at + 12:at + 28].split(b"\0")[0].decode(), kind, sub, off, size))
        at += 32
    return out


def plan(image, keep_settings):
    """What to write: [(address, bytes)]. Keeping settings skips the nvs (data/nvs) partition."""
    if not keep_settings:
        return [(0, image)]
    nvs = next(((off, size) for _, kind, sub, off, size in partitions(image) if kind == 1 and sub == 2), None)
    if nvs is None or nvs[0] + nvs[1] > len(image):
        raise ValueError("the image has no settings (nvs) partition to skip")
    start, end = nvs[0], nvs[0] + nvs[1]
    return [(0, image[:start]), (end, image[end:])]


def our_firmware(reply):
    """Our firmware answers CFG with `CFG {json}`: its build, or None for anything else."""
    for line in reply.splitlines():
        line = line.strip()
        if line.startswith("CFG "):
            try:
                cfg = json.loads(line[4:])
            except ValueError:
                return None
            return cfg.get("_fw") or "an older build"
    return None


def ask_cfg(port, seconds=5):
    """Ask the clock over serial what it runs (it may be restarting from the port opening)."""
    import serial
    s = serial.Serial()
    s.port, s.baudrate, s.timeout = port, 115200, 0.2
    s.dtr = s.rts = False              # don't reset the ESP32 on open
    try:
        s.open()
    except serial.SerialException:
        return None
    got, end, asked = "", time.time() + seconds, 0.0
    try:
        while time.time() < end:
            if time.time() - asked > 0.7:
                s.write(b"CFG\n")
                asked = time.time()
            got += s.read(512).decode("utf-8", "replace")
            fw = our_firmware(got)
            if fw:
                s.write(b"BYE\n")
                return fw
    finally:
        s.close()
    return None


def main():
    if not MERGED.exists():
        sys.exit(f"missing {MERGED} — build it with PlatformIO or get it from releases/")
    port = backup_usb.find_port()
    if not port:
        sys.exit(backup_usb.not_found_message())
    ours = None if "--wipe-settings" in sys.argv else ask_cfg(port)
    if ours:
        print(f"The clock runs this firmware ({ours}): its WiFi and settings will be kept.")
    if "--no-backup" not in sys.argv:
        print("Backing up your current firmware first (so you can revert)...")
        backup_usb.backup(port)        # read full flash before we overwrite it
    segments = plan(MERGED.read_bytes(), keep_settings=bool(ours))
    print(f"\nFlashing {MERGED.name} to {port} (use a USB 2.0 port if this fails)...")
    with tempfile.TemporaryDirectory() as tmp:
        args = []
        for i, (addr, data) in enumerate(segments):
            part = Path(tmp) / f"part{i}.bin"
            part.write_bytes(data)
            args += [hex(addr), str(part)]
        cmd = [sys.executable, "-m", "esptool", "--chip", "esp32", "--port", port,
               "--baud", "460800", "--before", "default-reset", "--after", "hard-reset",
               "write-flash", *args]
        rc = subprocess.run(cmd).returncode
    if rc == 0 and ours:
        print("\nDone. The clock is restarting on the new firmware with your settings.")
    elif rc == 0:
        print("\nDone. Now join the clock's 'WFD-Setup' WiFi on your phone and enter your "
              "2.4GHz network credentials. From then on, update over WiFi — no USB.")
    else:
        print("\nFlash failed. Most common fix: use a USB 2.0 port (not USB 3). "
              "See docs/FLASHING.md.")
    sys.exit(rc)


if __name__ == "__main__":
    main()
