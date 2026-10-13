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
  nine built‑ins itself (or stores your drawing); the **grid animator** (`/draw`) is a 7×7 frame editor with live preview
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

All UI settings persist in `wfd-control/wfd_config.json` (location, units, style, gains,
brightness, message mode, grid animation, icon mode, logo, offline text, standalone
toggles, …). Edit the file directly or use the UI; changes apply live.

## Auto‑start at login (Windows)

`wfd-control/wfd_control.vbs` launches the app hidden with `pythonw.exe`. Put a copy in
your Startup folder (`shell:startup`) — e.g. named "WFD Clock Helper.vbs". To disable,
delete it from Startup. Point its paths at a **persistent** Python env (not a temp venv).

## Phone vs app — who wins

Brightness, weather location, and the offline message can be set from **either** the
clock's phone page **or** this app, so last‑writer‑wins for those three. Everything else
is cleanly split: the phone page owns standalone behavior; the app owns the live,
PC‑fed features (spectrum, bars, window title, generated animations).

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
- **`test_device_api.py`** (opt‑in) tests the clock's JSON API on a real clock and restores
  everything it changes: `set WFD_CLOCK=<clock ip> && test.bat -m device`.

If you change what the app sends on purpose, the golden test fails with a diff. Regenerate
the log, then review its diff in git before committing:

```bash
cd wfd-control && WFD_UPDATE_GOLDEN=1 uv run --with-requirements requirements.txt --with pytest pytest tests/test_clocklink.py
```
