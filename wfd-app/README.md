# WFD Control (Rust)

The PC app for the custom WFD clock firmware: one `wfd-app.exe` that drives the clock over USB
or WiFi, feeds it what only the PC knows (PC audio spectrum, CPU/GPU/RAM, the active window
title, weather) and serves the dashboard at <http://127.0.0.1:8737>.

It was ported from the original Python app, now retired to
[`../legacy/python-app/`](../legacy/python-app/); this app is the source of truth. It still reads
that app's `wfd_config.json` format. The dashboard and grid animator come from `../wfd-firmware/clock/web/`, the same files the
clock itself serves. The release exe has those pages **and the clock's current firmware**
(`../wfd-firmware/releases/`) built in, so it can update the clock with no other files.

## Install (current user, starts at login)

Without building: download `wfd-app-…-windows-x64.zip` from the [releases](https://github.com/bstag/elekswfd-custom-firmware/releases/latest),
unzip, and run `install.ps1` from there. From the repo:

```powershell
powershell -ExecutionPolicy Bypass -File wfd-app\install.ps1
```

This builds the release exe, copies it to `%LOCALAPPDATA%\WFD Clock\` and adds a **WFD Clock**
shortcut to your Startup folder. If you're upgrading from the retired Python app, it copies that
app's settings on the first install (`legacy\python-app\wfd_config.json`, or
`wfd-control\wfd_config.json` from before the move) and turns off its login script (renamed to
`.vbs.disabled`). Then it starts the app. Run it again after pulling changes to update. To undo, delete the shortcut from
`shell:startup` and rename the `.vbs.disabled` back.

## wfd-flash

`src/bin/wfd-flash.rs` is a second exe: the first USB flash for people without Python. It backs
up all 4 MB (MD5-checked), then writes the merged image built into it, keeping the settings on a
clock that already runs our firmware. `--backup-only`, `--restore FILE`, `--yes`. See
[docs/FLASHING.md](../docs/FLASHING.md).

## Releases

Bump `version` in `Cargo.toml`, commit, then push a matching tag:

```
git tag v1.0.1 && git push origin v1.0.1
```

`.github/workflows/release.yml` runs the tests, then `release/package.ps1` (also runnable
locally; output in `dist/`) and publishes the zip (exe + `install.ps1` + `release/README.txt`)
and the clock firmware from `wfd-firmware/releases/`. The exe links the C runtime statically
(`.cargo/config.toml`), so it needs nothing installed. `wfd-app --version` prints the app
version and the firmware it ships.

## Run by hand

```
cargo run --release -- [--no-browser] [--config <path>] [--addr <host:port>]
```

Settings default to `wfd_config.json` next to the exe. Release builds have no console window;
started from a terminal they still print `--help` and errors there. Only one app can talk to the
clock at a time (it holds the COM port and port 8737), so stop any other copy (or the retired
Python app) first.

## Layout

| Module | What it does | Ported from (`legacy/python-app/`) |
|---|---|---|
| `config` | app settings, validation, which settings the clock owns | `DEFAULTS`, `coerce`, `CLOCK_KEYS` |
| `audio`, `dsp` | PC output (WASAPI loopback) + mic, levels, 7 FFT bands | `audio_reader`, `audio_levels` |
| `metrics`, `weather`, `window` | CPU/GPU/RAM/battery, weather, window title | `metrics_reader`, `weather_reader`, `foreground_title` |
| `font14`, `grid_anim`, `icons` | message glyphs, grid animations, top-row icons | `font14.py`, `grid_anim.py`, `compute_icons` |
| `link`, `mdns` | USB and WiFi transports, finding `wfd-clock.local` | `UsbLink`, `WifiLink`, `mdns.py` |
| `clocklink` | the ~30 Hz loop that drives the clock | `ClockLink` |
| `ota`, `release` | "Update clock", version check | `update_clock`, `firmware_status` |
| `usbdriver` | spots the clock plugged in without its USB driver (`state.usb_problem`) | (Rust only) |
| `flashplan` | what `wfd-flash` writes: the whole image, or all but `nvs` to keep settings | (Rust only) |
| `web` | HTTP server + API | `class H` |
| `state`, `ctx` | shared state (`/api/status`) and the context threads share | module globals |

## Tests

```
cargo test
```

About 200 tests, no clock or network needed. Many compare against fixtures in `tests/fixtures/`:
regression snapshots of this app's own behaviour. They were first generated from the Python app
it was ported from (the generator scripts are next to them, and still run against
`legacy/python-app/`), but this app is now the source of truth. The main ones:

- `tests/clocklink.rs` runs the clock link on simulated time against a fake clock and must
  reproduce the golden log `tests/fixtures/clocklink_golden.log` **byte for byte** (4,645
  lines). `WFD_UPDATE_GOLDEN=1 cargo test --test clocklink` writes the new output next to it
  (`.log.new`) when a change is on purpose.
- Settings validation is checked on 1,534 cases, icons on 6,000, every non-random grid
  animation frame for frame, and audio levels bit for bit.

The live tests are opt-in: `cargo test -- --ignored` (mDNS lookup of the clock, live weather),
`cargo run --example audio_probe`. The firmware's device tests
(`wfd-firmware\tests\test.bat -m device`, see [docs/FIRMWARE.md](../docs/FIRMWARE.md#host-tests))
talk to the clock itself.

New in the Rust app only: the **weather** grid animation (scenes for the current weather,
the same frames as the clock's: `tests/weather_anim.rs`), setting the clock's web password over USB (`POST /api/clock_password`,
Connection & firmware on the dashboard), and a log of every firmware upload (`ota-log.jsonl`
next to the settings, plus the clock's own report; see [docs/APP.md](../docs/APP.md)).

Known differences from the Python app:

- **Random grid animations** (rain, sparkle, life) use a different random generator, so their
  frames differ; tests check the behaviour instead.
- **Bad input:** the app returns clear errors where Python crashed.
- **Weather** follows a ZIP/units change within 30 s instead of 15 min.
- **Silence:** the meters drop to 0 when nothing plays (WASAPI loopback sends no data then).
