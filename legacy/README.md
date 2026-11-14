# Legacy

Superseded code, kept for reference. Nothing here is used by the current firmware or app.

- `python-app/`: **WFD Control in Python** (`app.py`), the original PC app, formerly
  `wfd-control/`. The Rust app in [`../wfd-app/`](../wfd-app/) was ported from it and replaced it;
  for a while it was kept as the parity reference, and it is now retired. It is **frozen** (no
  fixes or features) and its tests are **no longer run in CI**, so it may drift out of step with
  the firmware. It still runs from here: `uv run app.py` (or `run.bat`), tests with `test.bat`.
  Don't run it alongside the Rust app: they share the COM port and port 8737. The firmware's
  own tests that used to live in its suite moved to
  [`../wfd-firmware/tests/`](../wfd-firmware/tests/).
- `vendor-protocol-app/`: the first web front end (`wfd.py` + `static/`). It speaks the
  **stock** EleksWFD firmware's packet protocol ([../PROTOCOL.md](../PROTOCOL.md)), so it only
  helps on a clock that still runs the vendor firmware. Run: `uv run wfd.py`.
- `daemon/`: the headless helper (time sync + mic spectrum) that came before
  `python-app/app.py`. Its `.vbs` launcher has absolute paths from the original checkout.
