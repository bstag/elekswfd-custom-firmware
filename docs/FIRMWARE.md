# Firmware

PlatformIO project in `wfd-firmware/clock/` (Arduino‑ESP32 framework). Sources in
`src/` (see [Source layout](#source-layout)) plus generated headers `clock_panel.h` (LED
map), `font14_table.h` and `web_pages.h`.

## Architecture

The main loop, ~every 250 ms (and on every serial command), rebuilds the 42‑byte
TM1680 RAM from scratch and flushes it. Each panel region is drawn only if it's being
driven; stale regions clear. Priorities and sources:

- **Time / weekday** — always, from the DS3231 (kept correct by NTP and/or the app).
- **Live regions** (spectrum, bars, message, grid, icons, logo, weather) — shown while a
  corresponding command arrived recently (≈1.5 s window). These "last seen" times start
  as *never* (`Stamp` in `clock_state.h`), so nothing counts as live right after boot.
- **"App off" fallback** — `appTime` records the last serial command; if none for >2 s
  the PC app is considered off, and the clock plays its own **grid animation** and runs
  its own **message line** (see below). NTP and on‑device weather run regardless.

WiFi/NTP/weather/web run from the same loop (`wifiLoop()`); the HTTPS weather fetch
blocks briefly every 15 min. `WiFi.setSleep(false)` is required for a stable link.
Rejoining the saved network is left to the core's auto‑reconnect; `wifiLoop()` only starts
an attempt itself when the WiFi task has reported nothing for 20 s while the clock is off
the network (auto‑reconnect gave up, e.g. a failed first join, or DHCP stalled), so it never
cuts into an attempt that is still running.

## Source layout

All in `wfd-firmware/clock/src/`. Each `.cpp` has a header of the same name with what the
other files call. State used by more than one file is grouped into structs by what owns it,
one instance each: `disp` (the panel and its settings), `live` (the frame the app sends),
`standalone` (what runs when the app is off) and `net` (WiFi, the UDP stream, the web password)
in `clock_state.h`; `wx` (weather and forecast) in `weather_fetch.h`, `alerts` (alarms, timer,
alert) in `buttons_alerts.h`, `micState` (what the mic task hands over; `volatile`) in
`microphone.h`, `otaLog` in `ota_update.h` and the `web` server in `web_routes.h`, so a file
sees only the groups whose headers it includes.

| File | Holds |
|---|---|
| `main.cpp` | `setup()`, `loop()`, `WFD_PROTOCOL` and the `FW_BUILD` stamp |
| `clock_state.h/.cpp` | the shared groups above, `Stamp` |
| `nvs_store.h/.cpp` | every value saved in flash: one table of NVS key, type and default, used to load them all at boot (`prefsLoad`) and to save one (`prefSave`) |
| `clock_display.h/.cpp` | TM1680 and DS3231 over I2C, `draw()`, 14‑seg text rendering, brightness/style, the info notice |
| `serial_cmds.h/.cpp` | the serial command set, `handle()` |
| `clock_settings.h/.cpp` | the settings table (`setting`/`applySettings`), `configJson`, `stateJson`, `CFG`/`CFGSET` |
| `standalone.h/.cpp` | "app off": grid animation choice, stored drawing, the message line engine |
| `web_routes.h/.cpp` | web pages (sent in pieces by `HtmlOut`) and the JSON API routes |
| `wifi_link.h/.cpp` | WiFi station / setup AP, NTP, mDNS, the UDP display stream (`netPoll`) |
| `weather_fetch.h/.cpp` | Open‑Meteo fetch task and the ZIP lookup (ArduinoJson) |
| `microphone.h/.cpp` | mic sampling task and the `MIC`/`MICLOG` diagnostics |
| `ota_update.h/.cpp` | `POST /update` upload handler and its log |
| `buttons_alerts.h/.cpp` | buttons, alarm/timer alerts |
| `extras.h`, `grid_engine.h`, `mic_dsp.h` | plain C++ logic, also compiled by the host tests |
| `clock_panel.h`, `font14_table.h`, `web_pages.h` | generated (see below) |

`build_stamp.py` (a pre‑build script) makes `main.cpp` recompile whenever another source
file changes, so `FW_BUILD` (its `__DATE__ __TIME__`) always dates the build.

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
| `GANIM <name> [fps] [frame]` | Play a built‑in animation when the app is off: `spin bounce rain wave pulse sparkle scan life heart audio mouth weather`, or `custom` for the uploaded frames |
| `GCLR` / `GADD r0..r6` / `GCOMMIT fps frame` | Upload drawn frames (≤ 80) to flash and play them (`custom`) |
| `GAUTO 0/1` | Play the grid animation when the app is off |
| `GINFO` | Print the grid settings and the current frame's row bytes |
| `OMTEXT <text>` | Message text for the on-device message line (≤ 96 chars, placeholders allowed) |
| `MSGCFG mode rot dwell scroll` | Message line: mode `0`=temp `1`=text `2`=date `3`=rotate; `rot` bitmask (1 temp, 2 text, 4 date); seconds per item; ms per scroll step |
| `MSGINFO` | Print the message line's settings and what it's showing now |
| `BYE` | The app is quitting: the clock takes over at once instead of after 2 s (USB or WiFi) |
| `CFG` | Print every setting as JSON: `CFG {...}` (the same keys as `/api/status` config) |
| `CFGSET {json}` | Change settings with the same checks as `POST /api/set`: replies `CFGOK {settings}` or `CFGERR {"error": {...}}` |
| `MIC [0-3]` | Mic diagnostics: one 2048‑sample capture at 10 kHz with stats (optional ADC attenuation 0/2.5/6/11 dB for this capture) |
| `MICLOG <s>` | Mic level bar every 100 ms |
| `MICTRACE <s>` | Per processed frame: `T ms rms low high level% bass treble` (for tuning; see below) |
| `OMAUTO 0/1` | Run the message line when the app is off (`0` = temperature only) |
| `ICON mask` | Top‑row icons (13‑bit bitmask) |
| `LOGO 0/1` | ELEKSMAKER logo |
| `GEO lat lon unit` | Weather location for on‑device fetch (persisted) |
| `TZ <posix-tz>` | Timezone string, e.g. `CST6CDT,M3.2.0,M11.1.0` (persisted) |
| `WIFISET ssid⇥pass` | Save WiFi creds (tab‑separated) and reboot |
| `WIFISSID name` | Change just the network name, keep the password |
| `WIFISCAN` / `WIFIINFO` / `WIFIRESET` | Scan 2.4 GHz nets / status / clear creds |
| `WEBPASS <pass>` | Web password (user `admin`) for the settings pages and API, `/update`, `/wifi` and the WiFi display stream; `WEBPASS` alone removes it. Up to 64 printable characters, no spaces. Replies `PWOK {"set":…}` / `PWERR {"error":…}`. **USB only**: it isn't a setting (`/api/set`) and isn't accepted over UDP, so changing it needs physical access |

## Web endpoints (port 80)

| Path | Purpose |
|---|---|
| `GET /` | Dashboard (setup portal in AP mode) |
| `GET /status` | Status page (time, NTP, WiFi, weather) |
| `GET/POST /control` | Phone control panel (brightness, weather ZIP/units, timezone, message line, standalone toggles) |
| `GET /wifi`, `POST /save` | WiFi picker: scanned networks by signal, or type a hidden one (captive portal in AP mode) |
| `GET/POST /update` | OTA firmware upload (logged, see *Upload log* below) |
| `GET /rescan` | Re‑scan WiFi networks |
| `GET /api/status` | JSON: live state + every setting (see below) |
| `GET /draw` | Grid animator (the same page the PC app serves): draw, preview live, save |
| `GET /api/frames` | JSON: the stored drawing `{frames, fps, frame}` |
| `POST /api/preview` | JSON: show one grid frame now for 1.5 s `{"rows": [7 bytes], "frame": 0/1}` |
| `POST /api/set` | JSON: change settings |
| `POST /api/frames` | JSON: store drawn grid frames `{"frames": [[r0..r6], ...], "fps": 8, "frame": false}` (≤ 80) and play them when the app is off |

Anyone on your LAN can reach these pages. Set `WEBPASS` over USB to put every page and API
call behind HTTP basic auth (user `admin`): the pages, `/update`, `/wifi`, `POST /api/set`,
`/api/preview`, and also the reads `GET /api/status` and `GET /api/frames` (the status names
your WiFi network, ZIP and alarms). Without the password they answer `401`. The dashboard and
the animator are served behind the same login, so the browser sends it on their API calls.
With no password set, and always in the first-run setup AP, everything is open.

In AP (setup) mode the clock is `192.168.4.1` and a DNS captive portal redirects to the
setup page.

## JSON API

Same shape as the PC app's API, so one dashboard can talk to either.

`GET /api/status` returns `{"state": {...}, "config": {...}}`. `state` has `fw`, `uptime_s`,
`heap_free`, `time`, `ntp`, `wifi {ssid, ip, rssi}`, `weather {fresh, temp, unit, cond}`,
`app_connected`, `message` (what the message line shows), `stored_frames`, `grid_rows`
(the current grid frame, 7 row bytes), `mic {active, level, bass, treble}` and
`net {host, udp_port, udp_packets, udp_rejected, key_required}`. When `WEBPASS` is set (and
the clock isn't in setup-AP mode) it needs the web password like every other call, and
answers `401` without it; so does `GET /api/frames`.

`POST /api/set` takes a JSON object of settings. It needs the header `X-WFD: 1`, which stops
other web pages posting to the clock from your browser, plus the web password if `WEBPASS`
is set. Every key is checked first; if any is bad nothing changes, and you get a 400 with
`{"error": {key: reason}}`. Success is `{"ok": true}`, with `"warning"` if a ZIP lookup failed.

| Key | Type | Notes |
|---|---|---|
| `brightness` | int 1–16 | |
| `clock_24h` | bool | |
| `style` | int 1–5 | spectrum style |
| `units` | `fahrenheit` / `celsius` | |
| `tz` | string | POSIX TZ, e.g. `CST6CDT,M3.2.0,M11.1.0` |
| `location` | 5‑digit US ZIP | looked up on the clock |
| `offline_text` | string ≤ 96 | message text; placeholders allowed |
| `msg_standalone` | bool | run the message line when the app is off |
| `standalone_msg_mode` | `weather` / `text` / `date` / `forecast` / `rotate` | |
| `standalone_rotate` | list of `weather` `text` `date` `forecast` | what rotate cycles through |
| `standalone_dwell` | int 2–600 | seconds per item |
| `standalone_scroll_ms` | int 50–5000 | ms per scroll step |
| `mic_standalone` | bool | spectrum columns from the clock's mic when the app is off |
| `mic_sampling` | `timer` \| `spin` | how the mic is sampled: `timer` (default) sleeps between samples; `spin` is the older busy-wait, kept as a fallback (NVS `micspin`) |
| `grid_standalone` | bool | play a grid animation when the app is off |
| `standalone_anim` | name | `spin bounce rain wave pulse sparkle scan life heart audio mouth weather`, or `custom` (needs drawn frames) |
| `standalone_fps` | int 1–30 | |
| `standalone_frame` | bool | light the border |
| `weather_scene` | `auto` or a scene | what `weather` draws: `auto` follows the weather; or `clear partly cloudy fog rain snow storm night night_partly` |
| `alarms` | list of up to 4 `{"on": bool, "time": "HH:MM", "days": ["mon", …]}` | days are `mon tue wed thu fri sat sun`; reported as 4 entries |
| `timer` | seconds 0–86399 | starts a countdown (0 cancels); reads back the seconds left |
| `alert` | bool | `false` stops an alarm or timer alert; `true` starts a test alert; reads back whether one is going off |

```bash
curl http://<clock-ip>/api/status            # with a WEBPASS: curl -u admin:<pass> ...
curl -X POST -H "X-WFD: 1" -H "Content-Type: application/json" -d '{"standalone_anim":"rain","standalone_fps":12}' http://<clock-ip>/api/set
```

Send `Content-Type: application/json`. Without it the clock's web server treats the body as
form fields and you get a 400. The phone page's form goes through the same checks, so both always agree.

## The app over WiFi

The PC app can drive the clock over WiFi instead of USB (see [APP.md](APP.md#connection)).

- **Finding the clock:** it announces itself over mDNS as **`wfd-clock.local`**, with services
  `_http._tcp` (80) and `_wfd._udp` (4210). Windows doesn't always resolve `.local` names, so the
  app also asks over mDNS itself.
- **Live display:** UDP port **4210**, one packet per frame (~30 a second). The first line is
  `WFD1`, followed by the web password if one is set (`WFD1 <password>`). Each further line is
  one command. Only display commands are accepted: `SPEC BARS MSG GRID ICON LOGO WX SET BYE`.
  Packets with a wrong header or password are dropped and counted (`net.udp_rejected` in
  `/api/status`).

  ```
  WFD1
  SPEC 4 2
  ICON 4096
  GRID 0 0 28 20 28 0 0 1
  ```
- **Everything else** (settings, uploads, health) goes through the HTTP API: `/api/status`,
  `/api/set`, `/api/frames`.

## Dashboard

`/` serves a dashboard built on this API. It's **the same page the PC app serves**: it checks
`/api/status` (the app's has a `link` field) and, when the app serves it, also shows the PC sections
(audio, CPU/GPU/RAM bars, the live message line and grid, icons, connection and firmware). On the
clock you get a live preview (time, message line, the 7×7 grid as
it is right now, weather/WiFi/NTP) and every setting, saved as you change it. The source is
`clock/web/dashboard.html`. `embed_web.py` runs before every `pio run`, gzips every page in `web/`
(dashboard, grid animator) into `src/web_pages.h` (about 6 + 4 KB), and one OTA update ships the page and firmware together.
`/control` stays as a basic form for browsers without JavaScript; the old status page is at
`/status`.

## Message line (when the app is off)

The clock renders the 6‑character message line itself, so it needs no PC:

| Mode | Shows |
|---|---|
| Temperature | Current temperature (blank until weather has loaded) |
| Text | `omtext`, scrolling if longer than 6 chars |
| Date | e.g. `TUE OCT 13` |
| Forecast | `HI 78F`, `LO 55F` (today) and `THU 81` (tomorrow's high), 3 s each; the temperature until a forecast has loaded |
| Rotate | Cycles through the ticked sources every *dwell* seconds; temperature and forecast are skipped while they haven't loaded |

The forecast comes with the clock's own weather fetch (Open‑Meteo `daily`, 4 days); `/api/status`
shows it as `state.weather.forecast` (`[{day, hi, lo, scene}]`, today first).

## Alarms and timer

Up to 4 alarms, each with a time and weekdays, and a countdown timer: settings `alarms` and
`timer`, so they're set from the dashboard, `/api/set` or the app, and they run on the clock with
the PC off (alarms are saved in NVS; a timer doesn't survive a restart). The clock has no
speaker, so an alarm or a finished timer is an **alert**: the whole panel flashes (all LEDs, then
`ALARM` / `TIMER` on the message line) until any of the three buttons is pressed, the `alert`
setting is set to `false`, or 5 minutes pass. Each alarm fires once in its minute, by the RTC's
local time. A running timer counts down on the message line (`  4:59`, ` 1H05M`) over whatever
it would show. `/api/status` reports `state.timer` (seconds left) and `state.alert` (`alarm`,
`timer` or empty); serial prints `ALERT alarm|timer|off`. The logic (when an alarm is due, the
texts, reading the forecast) is `src/extras.h`, compiled and tested on the PC like the other
engines.

## Buttons

A short press changes the spectrum style (left: previous, middle/right: next), or stops an
alert. Holding any button for 2 s scrolls the clock's address (`IP 192.168.0.152`, 20 s) when
it's on WiFi; when it isn't, it opens the `WFD-Setup` network and picker again (see
[FLASHING.md](FLASHING.md#wifi-setup-one-time-no-usb)).

Text placeholders expand live from the RTC and weather: `{time}` (`17:05`), `{time12}`
(`5:05PM`), `{date}` (`TUE OCT 13`), `{day}` (`TUESDAY`), `{temp}` (`91F`). The PC app
expands the same set, so a text reads the same whether the app is running or not.

Set it from the phone page (`/control`) or over serial (`OMTEXT`, `MSGCFG`). While the app
is connected it drives the message line instead. Any serial command counts as the app being
connected, so the line pauses for 2 s after each one.

## Grid animations (when the app is off)

`grid_engine.h` is a C++ port of the original Python app's `grid_anim.py` for the nine animations
that need no live data. It's plain C++, so the [host tests](#host-tests) compile it on the PC
(MSVC, g++ or clang++) and check the deterministic ones frame by frame against that Python engine
(kept as a test oracle, `wfd-firmware/tests/oracle/grid_anim.py`). Pick
one on the phone page or with `GANIM`; the app's *Save to clock* sends `GANIM` for these, and
uploads frames only for drawings (`custom`). Audio, CPU and mouth need the PC and stay app‑only.

**`weather`** draws a scene for the current weather; the Rust app draws the same frames
(`wfd-app/src/grid_anim.rs`; both are tested against `wfd-app/tests/fixtures/weather_frames.json`):

| Scene | When | Picture |
|---|---|---|
| `clear` | WMO 0–1, day | sun, rays turning |
| `partly` | WMO 2, day | sun upper left, a cloud drifting past |
| `cloudy` | WMO 3 (day or night) | two layers of clouds scrolling |
| `fog` | 45, 48 | dashed bands sliding against each other |
| `rain` | drizzle, rain, showers (51–67, 80–82) | cloud and falling drops |
| `snow` | 71–77, 85, 86 | flakes falling slowly, drifting |
| `storm` | 95–99 | cloud, lightning bolt, rain, a full-grid flash |
| `night` | WMO 0–1, night | crescent moon, twinkling stars |
| `night_partly` | WMO 2, night | moon and a cloud drifting past |

The clock asks Open‑Meteo for `weather_code` and `is_day` and picks the scene
(`grid::sceneFromWmo`); `state.weather.scene` in `/api/status` says what it shows now. Without its
own recent fetch it falls back to the 3‑way code (sunny/cloudy/rain) and the local hour (day
06–20). The `weather_scene` setting pins one scene (the dashboard's *Grid scene*).

## Microphone (when the app is off)

The clock has its own mic (GPIO32 / ADC1_CH4, behind an op‑amp). A task on core 0 samples it
at 8 kHz in 32 ms frames while the app is off and something uses it, and `mic_dsp.h` turns each
frame into the numbers below. The sample clock is hardware timer group 0 / timer 0: its alarm
wakes the task every 125 µs for one `analogRead`, and the task sleeps in between instead of
spinning on `micros()`, so core 0 stays free for WiFi and the idle task. (The I2S driver's
built‑in‑ADC DMA mode would avoid even the per‑sample wake‑ups, but whether its readings match
`analogRead`'s scale, polarity and sample order, which `mic_dsp.h` is tuned on, depends on code
that ships only precompiled; it needs checking on the hardware first.) Each frame gives:

- **loudness**, in dB above an adaptive noise floor. The floor settles near the room's quiet
  level (it rises 0.3 dB/s and falls 1.5 dB/s). Nothing shows until a sound is 12 dB above it,
  and 28 dB more fills the column. The rise is instant and the fall smooth, like the PC app.
- **7 bands** from an FFT (60 Hz – 4 kHz), as a shape relative to the strongest band.

The left spectrum column shows the bass share (bands 0–2) and the right one the treble share.
The grid's `audio` (7 bars) and `mouth` animations use the bands.

Measured on the hardware: the op‑amp output rests near 0 V and swings upward with sound. At
0 dB attenuation loud sounds saturate the ADC; at 11 dB (used) its ~0.1 V dead zone hides much
of the background hum. A real room spans ~40 dB, from RMS ~15 quiet to 1000+ for music and
claps, so a linear meter scaled to recent peaks either lights up in silence or goes dark after
one loud sound. Hence the dB scale.

The thresholds were tuned on a recording from the clock (`MICTRACE`), saved as
`wfd-firmware/tests/host/mictrace_room.txt`. The tests replay it through the C++ code (quiet must
stay dark, music and claps must fill the column) and feed it simulated tones (a low tone lights
the bass column and band 1, a high tone the treble column and band 5). To retune for another
room, record a new trace and adjust `Loudness` in `mic_dsp.h` until the tests pass.

## Persistent storage (NVS namespace `wfd`)

All of them are in one table in `src/nvs_store.cpp` (key, type, default, and the range a
value is clamped to when loaded); clocks in the field depend on these, so they don't change:

| Key | Type | Default | Key | Type | Default |
|---|---|---|---|---|---|
| `style` | int | 4 | `micauto` | int | 1 |
| `bright` | int | 6 | `omauto` | int | 0 |
| `clk24` | int | 1 | `mmode` | int | 1 (text), clamped 0–4 |
| `gn` | int | 0, clamped 0–80 | `mrot` | int | 7 |
| `gfps` | int | 8 | `mdwell` | int | 8 |
| `gflag` | int | 0 | `mscroll` | int | 350 |
| `gauto` | int | 0 | `omtext` | string | `WFD CLOCK` |
| `ganim` | bytes | `gn` × 7 (`gn` → 0 if short) | `tz` | string | `CST6CDT,M3.2.0,M11.1.0` |
| `gaid` | int | `custom` if frames loaded, else `spin`; clamped | `lat`, `lon` | string | empty |
| `webpass` | string | empty | `wunit` | int | 0 |
| `wxsc` | int | −1 (auto), clamped | `zip` | string | empty |
| `alarms` | bytes | 4 × 4 (all off if short) | `ssid`, `pass` | string | empty |
| `otalog` | bytes | the last upload's log | | | |

`zip`, `ssid` and `pass` are read from flash when needed; the rest are loaded once at boot.
Settings survive OTA updates, `wfd-flash.exe` and `flash_usb.py` (the `nvs` partition offset
is unchanged and they leave it alone on a clock that already runs this firmware). Writing the
whole `wfd-clock-merged.bin` at 0x0 by hand (esptool, or `flash_usb.py --wipe-settings`)
erases them, because the image fills `nvs` with 0xFF.

## Partition table (`partitions_wfd.csv`, 4 MB, dual‑OTA)

```
nvs      data nvs     0x9000   0x5000
otadata  data ota     0xe000   0x2000
app0     app  ota_0   0x10000  0x1E0000
app1     app  ota_1   0x1F0000 0x1E0000
spiffs   data spiffs  0x3D0000 0x30000
```

Two app slots are mandatory for OTA. The app is ~1 MB, well within a 1.875 MB slot.

## Host tests

`wfd-firmware/tests/` checks the firmware on the PC, no clock needed. Run `test.bat` there
(extra arguments go to pytest), or:

```bash
cd wfd-firmware/tests && uv run --with-requirements requirements.txt pytest
```

- **`test_grid_engine.py`**, **`test_mic_dsp.py`**, **`test_extras.py`** compile `grid_engine.h`,
  `mic_dsp.h` and `extras.h` with the drivers in `tests/host/` (MSVC found with vswhere, or g++ /
  clang++; skipped if there's no compiler). The grid engine is compared frame by frame with
  `tests/oracle/grid_anim.py` and, for `weather`, with the Rust app's frames
  (`wfd-app/tests/fixtures/weather_frames.json`); the mic code is fed simulated tones and the
  recording in `tests/host/mictrace_room.txt`.
- **`test_sources.py`**: `src/web_pages.h` holds the current `web/*.html`, `font14_table.h` is
  generated from `tests/oracle/font14.py`, and `releases/version.json` has the protocol in
  `main.cpp`.
- **`test_flash_usb.py`**: `flash_usb.py` keeps a clock's settings when it re-flashes.
- **`test_device_api.py`** / **`test_device_net.py`** (opt-in) check the JSON API, UDP display
  packets, frame uploads and mDNS on a real clock, and put back everything they change:
  `set WFD_CLOCK=<clock ip> && wfd-firmware\tests\test.bat -m device` (and `WFD_CLOCK_PASS` if
  it has a web password).

`tests/oracle/` holds frozen copies of modules from the retired Python app
(`legacy/python-app/`): test oracles only, not app code. CI runs this suite on Windows.

## Regenerating the generated headers

Both headers are committed, so a fresh clone builds as-is.

- `font14_table.h` (C font): run `python wfd-firmware/tests/oracle/font14.py` after changing a
  glyph there. The Rust app has the same font (`wfd-app/src/font14.rs`, checked against
  `wfd-app/tests/fixtures/font14_cases.json`); change it there too.
- `clock_panel.h` (LED/segment map): hand-assembled from the mapping results in
  `wfd-firmware/tools/` (`panel_map.json`, `segmap.json`). It only changes if you re‑map a
  different panel revision.

## Making the release binaries

After `pio run` in `wfd-firmware/clock`, refresh `wfd-firmware/releases/`:

```bash
python wfd-firmware/make_release.py
```

It writes `wfd-clock-firmware.bin` (the OTA image), `wfd-clock-merged.bin` (bootloader, partitions,
boot_app0 and the app, for the first USB flash at 0x0) and `version.json` (build stamp and
protocol, which the PC app reads to offer updates). It merges with `--flash_freq 80m`; without
that, esptool rewrites the bootloader header to 40 MHz.

## Upload log

Each OTA upload is recorded in NVS (`otalog`), so the record survives the restart after a good
update and is still there after an aborted one. `/api/status` shows it as `state.ota`:

| Field | Meaning |
|---|---|
| `result` | `ok`, `no auth`, `begin failed: …`, `write failed: …`, `end failed: …` or `aborted` (connection dropped or stalled) |
| `bytes`, `chunks`, `ms` | received, pieces, start to end |
| `max_gap_ms`, `gap_at` | the longest pause between pieces and how many bytes in it came |
| `rssi_start`, `rssi_end` | WiFi signal (dBm) |
| `heap_start`, `heap_end` | free memory |

The same appears on serial as one line: `OTA ok bytes=… chunks=… ms=… maxgap=…ms@… rssi=…..… heap=…..…`.
A healthy upload of the ~1.1 MB image is ~790 pieces in ~16 s with pauses under half a second.

## Versions

`/api/status` reports `state.fw` (the build stamp) and `state.protocol` (`WFD_PROTOCOL` in
`main.cpp`); `CFG` reports them as `_fw` / `_protocol`, plus the clock's WiFi address as `_ip` (empty when it isn't on WiFi) whether a web password is set as `_pw` and whether it has internet time as `_ntp`, so the app sees them over USB too. Bump
`WFD_PROTOCOL` together with `PROTOCOL` in `wfd-app/src/config.rs` whenever commands, settings
or the API change in a way the app depends on. A host test fails if `releases/version.json`
disagrees with `main.cpp`; `wfd-app/tests/firmware_sync.rs` fails if the app's constant differs
from the firmware's, so bump both together.
Protocol 4 added the `weather` animation and the `weather_scene` setting; protocol 5 the
`alarms`, `timer` and `alert` settings and the forecast; protocol 6 the `mic_sampling` setting,
and `GET /api/status` and `GET /api/frames` asking for the web password when one is set.
