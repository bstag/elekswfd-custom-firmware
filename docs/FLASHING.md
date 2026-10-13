# Flashing & updating

## The one rule: USB 2.0 for the first flash

The clock uses a **CH340** USB‑serial bridge. On this hardware the automatic
"enter flashing mode" only works on a **USB 2.0** port. On USB 3 ports (and through
many hubs) it fails with *"Failed to connect to ESP32: Timed out waiting for packet
header"* or *"Wrong boot mode detected (0x13)"*.

- Use a **USB 2.0** port — often the black ones, frequently on the back of a desktop.
- A powered **USB 2.0 hub** also works.
- This is a timing quirk of CH340 + USB 3 host controllers, not a broken clock.

(The stock vendor tool has the exact same limitation; it just doesn't tell you.)

## Back up first (so you can revert)

**Do this before your first flash.** A backup lets you return to the original stock
firmware. Backups are per‑device (the stock firmware is tied to the chip's MAC), so you
must back up **your own** clock — someone else's dump won't restore yours.

```bash
pip install esptool pyserial
python wfd-firmware/backup_usb.py      # -> firmware-backup/clock-backup-<timestamp>.bin
```
Keep that file somewhere safe. Restore any time with:
```bash
esptool --chip esp32 --port COMx write-flash 0x0 firmware-backup/clock-backup-<...>.bin
```

## First flash (USB)

### Easy way — prebuilt binary
```bash
pip install esptool pyserial
python wfd-firmware/flash_usb.py
```
This **automatically backs up your current firmware first** (pass `--no-backup` to skip),
then flashes `wfd-firmware/releases/wfd-clock-merged.bin` (bootloader + partitions + app)
to offset `0x0`. If it can't connect, move to a USB 2.0 port.

Manual equivalent:
```bash
esptool --chip esp32 --port COMx --baud 460800 write-flash 0x0 wfd-firmware/releases/wfd-clock-merged.bin
```

### Developer way — build from source
```bash
# Install PlatformIO Core: pip install platformio  (or: uv tool install platformio)
cd wfd-firmware/clock
pio run              # build
pio run -t upload    # build + flash over USB
```
First build downloads the ESP32 toolchain (~400–600 MB), cached afterward.

## WiFi setup (one time, no USB)

On first boot with no saved WiFi, the clock hosts an access point **"WFD-Setup"**.

1. Join **WFD-Setup** from a phone/laptop (no password).
2. A captive‑portal page opens; if not, browse to **http://192.168.4.1**.
3. Pick your network and enter the password. **It must be a 2.4 GHz network** — the
   ESP32 has no 5 GHz radio, so 5 GHz‑only SSIDs won't appear. SSIDs are
   **case‑sensitive** (e.g. `bluey` ≠ `Bluey`).
4. Save. The clock restarts, joins your WiFi, and syncs NTP time + weather.

Find the clock's IP from your router, or it's shown on the status page. Serial
fallbacks (see [FIRMWARE.md](FIRMWARE.md)): `WIFISCAN`, `WIFIINFO`, `WIFIRESET`.

## OTA updates (after the first flash — no USB ever again)

1. Build/obtain `wfd-clock-firmware.bin` (`pio run` puts it in
   `wfd-firmware/clock/.pio/build/wfd/firmware.bin`; a copy is in `releases/`).
2. Open the clock's web page `http://<clock-ip>/` → **Firmware update**, choose the
   `.bin`, upload. It flashes the inactive OTA slot and reboots.

Command line:
```bash
curl -F "fw=@wfd-firmware/releases/wfd-clock-firmware.bin" http://<clock-ip>/update
# with a WEBPASS set:
curl -u admin:<pass> -F "fw=@wfd-firmware/releases/wfd-clock-firmware.bin" http://<clock-ip>/update
```

OTA requires the **dual‑OTA partition table** (already set in
`wfd-firmware/clock/partitions_wfd.csv`: `app0`/`app1` each 0x1E0000). Your settings
and WiFi credentials live in the `nvs` partition and survive both OTA and USB flashes.

## Restore the stock firmware

```bash
# stop the PC app first (frees the serial port), then on USB 2.0:
esptool --chip esp32 --port COMx write-flash 0x10000 translations/qt_ru.qm
```
`translations/qt_ru.qm` is the stock app image (the vendor hid it as a fake translation
file); `translations/qt_uk.qm` is its partition table. A full 4 MB dump of the original
device is in `firmware-backup/elekswfd_flash_backup.bin` (write at `0x0`).
