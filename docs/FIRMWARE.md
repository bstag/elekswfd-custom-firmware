# Firmware

PlatformIO project in `wfd-firmware/clock/` (Arduino‑ESP32 framework). Single
`src/main.cpp` plus generated headers `clock_panel.h` (LED map) and `font14_table.h`.

## Architecture

The main loop, ~every 250 ms (and on every serial command), rebuilds the 42‑byte
TM1680 RAM from scratch and flushes it. Each panel region is drawn only if it's being
driven; stale regions clear. Priorities and sources:

- **Time / weekday** — always, from the DS3231 (kept correct by NTP and/or the app).
- **Live regions** (spectrum, bars, message, grid, icons, logo, weather) — shown while a
  corresponding command arrived recently (≈1.5 s window).
- **"App off" fallback** — `appTime` records the last serial command; if none for >2 s
  the PC app is considered off, and the clock plays its **stored grid animation** and
  **stored offline message** from flash. NTP and on‑device weather run regardless.

WiFi/NTP/weather/web run from the same loop (`wifiLoop()`); the HTTPS weather fetch
blocks briefly every 15 min. `WiFi.setSleep(false)` is required for a stable link.

## Serial protocol (115200 8N1)

Driven by the PC app; also handy for debugging over a serial monitor.

| Command | Meaning |
|---|---|
| `SET yy mo dd dow hh mi ss` | Set RTC (dow 1=Mon..7=Sun), clears the lost‑power flag |
| `GET` | Print current RTC time |
| `SPEC lo hi` | Spectrum columns, each 0–12 |
| `STYLE n` | Spectrum style 1–5 (persisted) |
| `BARS cpu gpu ram` | CPU/GPU/RAM bars, each 0–100 % |
| `WX code temp unit` | Weather: code 1=sunny 2=cloud 3=rain, unit 0=F 1=C |
| `BRIGHT 1..16` | Brightness (persisted) |
| `CLK24 0/1` | 24‑hour (1) or 12‑hour (0) clock display (persisted) |
| `MSG b0..b11` | 6 message chars, raw glyph bytes (lo,hi ×6) |
| `GRID r0..r6 frame` | 7×7 grid rows (bit c = column c) + border‑frame flag |
| `GCLR` / `GADD r0..r6` / `GCOMMIT fps frame` | Upload a stored grid animation to flash |
| `GAUTO 0/1` | Play the stored grid animation when the app is off |
| `OMCLR` / `OMADD b0..b11` / `OMCOMMIT fps` | Upload a stored offline message to flash |
| `OMAUTO 0/1` | Show the stored offline message when the app is off |
| `ICON mask` | Top‑row icons (13‑bit bitmask) |
| `LOGO 0/1` | ELEKSMAKER logo |
| `GEO lat lon unit` | Weather location for on‑device fetch (persisted) |
| `TZ <posix-tz>` | Timezone string, e.g. `CST6CDT,M3.2.0,M11.1.0` (persisted) |
| `WIFISET ssid⇥pass` | Save WiFi creds (tab‑separated) and reboot |
| `WIFISSID name` | Change just the network name, keep the password |
| `WIFISCAN` / `WIFIINFO` / `WIFIRESET` | Scan 2.4 GHz nets / status / clear creds |
| `WEBPASS <pass>` | Password for `/control`, `/update`, `/wifi` (user `admin`); `WEBPASS` alone clears it |

## Web endpoints (port 80)

| Path | Purpose |
|---|---|
| `GET /` | Status page (time, NTP, WiFi, weather) |
| `GET/POST /control` | Phone control panel (brightness, weather ZIP/units, timezone, offline message, standalone toggles) |
| `GET /wifi`, `POST /save` | Change WiFi network (captive portal in AP mode) |
| `GET/POST /update` | OTA firmware upload |
| `GET /rescan` | Re‑scan WiFi networks |

Anyone on your LAN can reach these pages. Set `WEBPASS` over USB to put `/control`,
`/update` and `/wifi` behind HTTP basic auth. The first-run setup AP is always open.

In AP (setup) mode the clock is `192.168.4.1` and a DNS captive portal redirects to the
setup page.

## Persistent storage (NVS namespace `wfd`)

`style`, `bright`, `clk24`, `webpass`; grid animation `gn/gfps/gflag/gauto` + `ganim` (blob); offline message
`omn/omfps/omauto` + `om` (blob) + `omtext`; WiFi/location `ssid/pass/tz/lat/lon/wunit/zip`.
Settings survive OTA and USB flashes (the `nvs` partition offset is unchanged).

## Partition table (`partitions_wfd.csv`, 4 MB, dual‑OTA)

```
nvs      data nvs     0x9000   0x5000
otadata  data ota     0xe000   0x2000
app0     app  ota_0   0x10000  0x1E0000
app1     app  ota_1   0x1F0000 0x1E0000
spiffs   data spiffs  0x3D0000 0x30000
```

Two app slots are mandatory for OTA. The app is ~1 MB, well within a 1.875 MB slot.

## Regenerating the generated headers

Both headers are committed, so a fresh clone builds as-is.

- `font14_table.h` (C font): run `python wfd-control/font14.py` after changing a glyph there.
  The Python app and the firmware then share one font.
- `clock_panel.h` (LED/segment map): hand-assembled from the mapping results in
  `wfd-firmware/tools/` (`panel_map.json`, `segmap.json`). It only changes if you re‑map a
  different panel revision.

## Making the release binaries

After `pio run` in `wfd-firmware/clock`, refresh `wfd-firmware/releases/`:

```bash
cd wfd-firmware
B=clock/.pio/build/wfd
cp $B/firmware.bin releases/wfd-clock-firmware.bin          # OTA image
uvx --from "esptool>=4.7,<5" esptool --chip esp32 merge_bin \
  -o releases/wfd-clock-merged.bin --flash_mode dio --flash_freq 80m --flash_size 4MB \
  0x1000 $B/bootloader.bin 0x8000 $B/partitions.bin \
  0xe000 ~/.platformio/packages/framework-arduinoespressif32/tools/partitions/boot_app0.bin \
  0x10000 $B/firmware.bin                                    # USB image (flash at 0x0)
```

Keep `--flash_freq 80m`. Without it, esptool rewrites the bootloader header to 40 MHz.
