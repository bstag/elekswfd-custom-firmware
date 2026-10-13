# WFD Control

A local web front end for the EleksWFD clock. It replaces EleksWFD_EN.exe for everything the clock can do over USB.

## Run
Double-click `run.bat`, or:

```
uv run wfd.py              # auto-detects the CH340 port, opens http://127.0.0.1:8737
uv run wfd.py --stats      # start with CPU/GPU/RAM bars on
uv run wfd.py --port COM4 --no-browser
```

Without uv: `pip install pyserial psutil` and then `python wfd.py`.

## Features
- **Scrolling text**, with live placeholders (`{time}`, `{date}`, `{cpu}`…) and optional auto-repeat.
- **System info sync**: the CPU / GPU / RAM bars, updated every second. GPU needs an NVIDIA card (`nvidia-smi`).
- **Animation editor**: 7x7 grid with click-drag drawing, frames, shift/mirror/invert tools, browser preview, live mirroring on the clock, upload, restore factory animation, and open/save of the vendor's `.WFD` files.
- **Raw packet console** for experimenting.

## Notes
- The server only listens on 127.0.0.1.
- It keeps the COM port open, so close it before using the vendor app.
- The protocol is in `../PROTOCOL.md`. The clock's own time cannot be set over USB; the PC app never sent it. Use a `{time}` text repeat as a stopgap.
