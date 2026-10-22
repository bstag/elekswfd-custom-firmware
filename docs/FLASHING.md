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

## The USB driver (CH340)

The clock needs Windows' driver for its CH340 chip to get a COM port. **Windows 10/11 normally
installs it by itself** (through Windows Update) the first time you plug the clock in; it then
shows in Device Manager under *Ports (COM & LPT)* as **USB-SERIAL CH340 (COMx)**.

If it hasn't (offline PC, Windows Update drivers turned off), the clock appears under *Other
devices* as **USB2.0-Serial** with a warning sign, and nothing finds a port. The app's dashboard
and `flash_usb.py` / `backup_usb.py` notice this and say so. Then:

1. Download **CH341SER.EXE** from WCH, the chip's maker: <https://www.wch-ic.com/downloads/CH341SER_EXE.html>
2. Run it and press *INSTALL*.
3. Unplug and replug the clock.

This repo doesn't include the driver: it's WCH's, and their page has the current signed version.
macOS and Linux have CH340 support built in.

## Easiest: `wfd-flash.exe` (Windows, nothing to install)

Download the zip from the [latest release](https://github.com/bstag/elekswfd-custom-firmware/releases/latest), unzip, plug the clock into a USB 2.0 port
and double-click **`wfd-flash.exe`**. It finds the clock (and says so if Windows is missing
its USB driver), **backs up the whole 4 MB flash first** to `Documents\WFD Clock backups`,
checked against the clock's own MD5 (if the backup fails, nothing is written), then asks and
flashes the firmware built into it.

- A **stock** clock gets the whole image. Its old settings are in the backup.
- A clock **already on this firmware** keeps its WiFi and settings: the tool asks it (`CFG`)
  first and then writes everything except the `nvs` partition.
- **Back to stock:** `wfd-flash.exe --restore "<backup file>"` (it backs up first too).
- `--backup-only` only takes a backup; `--yes` skips the questions.

If the WFD Control app is running it holds the port; the tool offers to stop it and starts
it again afterwards. The source is `wfd-app/src/bin/wfd-flash.rs` (built on
[espflash](https://github.com/esp-rs/espflash)).

The Python way below does the same from the repo.

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
to offset `0x0`. If it can't connect, move to a USB 2.0 port. The merged image fills the
settings partition with 0xFF, so writing it whole **erases WiFi and settings**. That's fine
for a first flash; on a clock that already runs this firmware the script notices (it asks
`CFG` first) and writes everything except `nvs`, so they're kept. `--wipe-settings` writes
the whole image anyway.

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

Easiest: the PC app's **Update clock** button (Connection & firmware). The app ships the
matching build, uploads it and waits for the clock to come back on it; see
[APP.md](APP.md#connection). By hand:

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
