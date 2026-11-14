# WFD Control (Python, retired)

> **Retired.** The app to use is the Rust app in [`../../wfd-app/`](../../wfd-app/), which was
> ported from this one and is now the source of truth. This Python app is frozen: no fixes or
> features (it never got the web password setting, the upload log or the forecast), and its
> tests no longer run in CI. It lived in `wfd-control/` until it was moved here. Don't run both:
> they share the COM port and port 8737.

The original PC app for the custom WFD clock firmware: a local dashboard at
http://127.0.0.1:8737 plus the background services that feed the clock (time sync, audio
spectrum, CPU/GPU/RAM bars, weather, message line, grid animations).

```
uv run app.py              # or double-click run.bat
uv run app.py --no-browser
```

Dependencies (`requirements.txt`, also inline in `app.py`): `pyserial`, `numpy`,
`PyAudioWPatch` (WASAPI loopback), `psutil`. Settings live in `wfd_config.json` next to
`app.py` (see `wfd_config.example.json`).

`wfd_control.vbs` starts it hidden at login with the venv in `..\..\wfd-firmware\.venv`
(put a shortcut to it in `shell:startup`). The Rust app's `install.ps1` turns such a login
script off and copies this app's `wfd_config.json` on a first install.

Tests: `test.bat` (or `uv run --with-requirements requirements.txt --with pytest pytest`). They
run the app against a fake serial port, a simulated WiFi clock and its HTTP API, and cross-check
it against the firmware source. `WFD_UPDATE_GOLDEN=1` regenerates `tests/golden/clocklink.log`.
The firmware's own tests moved to [`../../wfd-firmware/tests/`](../../wfd-firmware/tests/).

`firmware/patch_fw.py` patches the stock vendor firmware image (from the reverse-engineering days).

App docs (written for the Rust app; this one behaves the same apart from the Rust-only features):
[../../docs/APP.md](../../docs/APP.md). The serial commands are in
[../../docs/FIRMWARE.md](../../docs/FIRMWARE.md).
