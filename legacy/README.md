# Legacy

Superseded code, kept for reference. Nothing here is used by the current firmware or app.

- `vendor-protocol-app/`: the first web front end (`wfd.py` + `static/`). It speaks the
  **stock** EleksWFD firmware's packet protocol ([../PROTOCOL.md](../PROTOCOL.md)), so it only
  helps on a clock that still runs the vendor firmware. Run: `uv run wfd.py`.
- `daemon/`: the headless helper (time sync + mic spectrum) that came before
  `wfd-control/app.py`. Its `.vbs` launcher has absolute paths from the original checkout.
