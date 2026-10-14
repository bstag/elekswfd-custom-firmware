# WFD Control (PC app)

A local web app (`wfd-control/app.py`) that serves a dashboard **and** runs the
background services that feed the clock the live, PC‑only data over USB serial.

## Running

```bash
cd wfd-control
uv run app.py              # uv reads the inline deps; or use a venv:
# python -m venv .venv && .venv/Scripts/pip install pyserial numpy pyaudiowpatch psutil
# .venv/Scripts/python app.py
```
Dashboard: **http://127.0.0.1:8737**. Add `--no-browser` to not auto‑open.

Dependencies: `pyserial`, `numpy`, `pyaudiowpatch` (WASAPI loopback), `psutil`. The
drawing tool and fonts use `font14.py` / `grid_anim.py` (in the same folder).

## What it provides

- **Clock** — time sync (DST‑aware), **12/24‑hour** format, brightness
- **Spectrum** — PC audio output (WASAPI loopback) on the left column, default mic on the
  right; five styles; gain sliders
- **System bars** — live CPU (psutil) / GPU (`nvidia-smi`) / RAM
- **Weather** — resolves a ZIP or "Town, State" and pushes it; shows condition + temp
- **Message line** — weather temp / window title / custom text / **date** / rotation; custom text supports `{date} {time} {time12} {day} {temp}` placeholders
- **Grid** — 13 animations incl. audio/CPU‑reactive; **Save to clock** makes the clock play the
  built‑ins itself, audio and mouth from its own mic (or stores your drawing); the **grid animator** (`/draw`) is a 7×7 frame editor with live preview
- **Top row** — icon modes (audio meter / system status) and the logo toggle
- **Offline message** + standalone toggles — what the clock shows when the app is off. *Save to clock*
  sends the text (placeholders and all); the clock's phone page picks the mode, rotation and speed

## How it drives the clock

A worker thread connects to the CH340 port (auto‑found by VID/PID `1A86:7523`), then at
~30 fps sends `SPEC`, and as needed `BARS`, `WX`, `MSG`, `GRID`, `ICON`, `LOGO`,
`STYLE`, `BRIGHT`, `GEO`, plus `SET` every 10 min. Uploads (`Save to clock`) go through
`GANIM` (built‑in grid animation, by name), `GCLR/GADD/GCOMMIT` (drawn grid frames) and `OMTEXT`
(offline message text). It reconnects automatically if the clock
is unplugged.

**It holds the serial port.** Stop the app (end `pythonw.exe` / `app.py`) before flashing
over USB or running the old vendor tool. (OTA over WiFi does **not** need the port free.)

## Settings

Every setting has one owner:

- **The clock** owns everything about itself: brightness, 12/24‑hour, spectrum style, units,
  time zone, weather ZIP, the offline message, and all "when the app is off" behaviour. They
  live in the clock's flash. The app reads them over USB (`CFG`) when it connects and every
  5 s, and changes them with `CFGSET`. The clock checks them exactly as its own dashboard does.
  A change made on the clock's dashboard or phone page therefore shows up in the app, and the
  app never overwrites it.
- **The app** owns what needs the PC: PC audio spectrum and gains, CPU/GPU/RAM bars, the live
  message line (window title, custom text, rotation), live grid animations, icons and logo.
  These persist in `wfd-control/wfd_config.json`.

The app's UI shows both together. Saving a clock setting while the clock is unplugged fails
with "clock not connected", and the clock's own error messages (e.g. a bad ZIP) are shown
as they are. The app's weather uses the clock's ZIP and units. It sends a location to the clock
(`GEO`) only if the clock has none.

## Auto‑start at login (Windows)

`wfd-control/wfd_control.vbs` launches the app hidden with `pythonw.exe`. Put a copy in
your Startup folder (`shell:startup`) — e.g. named "WFD Clock Helper.vbs". To disable,
delete it from Startup. Point its paths at a **persistent** Python env (not a temp venv).

## Connection

The app talks to the clock over **USB** or **WiFi** (setting `link`, in the *Connection* section):

- **Auto** (default): USB when the clock is plugged in, otherwise WiFi. While on WiFi it checks
  for USB every 10 s and switches over.
- **USB only** / **WiFi only**.

Over WiFi, each frame's display commands go to the clock as one UDP packet, and settings and
uploads use the clock's web API. The clock is found as `wfd-clock.local`: the app asks over mDNS
itself, because Windows doesn't always resolve `.local` names. You can also enter an IP
(`clock_host`). The app remembers the last address that worked. If the clock has a web password
(serial `WEBPASS`), enter it as the clock password (`clock_pass`). The app never shows it back.
A clock with internet time (NTP) keeps its own time over WiFi; the app sends the PC's time only
if the clock hasn't synced.

The clock still needs power, so a USB charger is enough once the app uses WiFi.

When the app quits normally it sends `BYE`, so the clock takes over at once instead of after 2 s.

## App and clock together

While the app is connected it drives the live display (spectrum, bars, message line, grid).
When it stops, the clock carries on with its own settings after 2 s. Settings never fight:
see *Settings* above.

## Note on the older headless helper

`legacy/daemon/wfd_daemon.py` is a superseded headless version (time + mic spectrum only).
`wfd-control/app.py` replaces it with the full UI and feature set.

## Tests

```bash
wfd-control/test.bat            # or: cd wfd-control && uv run --with-requirements requirements.txt --with pytest pytest
```

No clock, network or audio device is needed, and your `wfd_config.json` is never read or
written. The suite covers:

- **`test_clocklink.py`** runs the serial worker against a fake port on simulated time.
  It compares every line sent in a busy 45 s scenario to `tests/golden/clocklink.log`, and
  checks specific behaviour (reconnect re-sends settings, uploads, sync, spectrum off).
- **`test_api.py`** covers the local HTTP API: header check, validation, status, uploads.
- **`test_rendering.py`** covers settings validation, message scrolling and icons.
- **`test_protocol.py`** cross-checks against the firmware source: every command the app
  sends is handled in `main.cpp`, `font14_table.h` matches `font14.py`, and the icon count
  matches `clock_panel.h`.

- **`test_grid_engine.py`** compiles the firmware's `grid_engine.h` on the PC and checks it
  against `grid_anim.py` (skipped if there's no C++ compiler).
- **`test_mic_dsp.py`** compiles the firmware's `mic_dsp.h` and checks it on simulated tones and
  on a real recording from the clock (`tests/host/mictrace_room.txt`).
- **`test_wifi_link.py`** drives the app's WiFi link against a simulated clock (UDP packets, HTTP
  settings and uploads, reconnects, switching to USB); **`test_mdns.py`** checks the lookup.
- **`test_device_api.py`** / **`test_device_net.py`** (opt‑in) tests the clock's JSON API on a real clock and restores
  everything it changes: `set WFD_CLOCK=<clock ip> && test.bat -m device`.

If you change what the app sends on purpose, the golden test fails with a diff. Regenerate
the log, then review its diff in git before committing:

```bash
cd wfd-control && WFD_UPDATE_GOLDEN=1 uv run --with-requirements requirements.txt --with pytest pytest tests/test_clocklink.py
```
