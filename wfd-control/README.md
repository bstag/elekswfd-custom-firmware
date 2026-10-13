# WFD Control

The PC app for the custom WFD clock firmware: a local dashboard at http://127.0.0.1:8737
plus the background services that feed the clock (time sync, audio spectrum, CPU/GPU/RAM
bars, weather, message line, grid animations).

```
uv run app.py              # or double-click run.bat
uv run app.py --no-browser
```

Run the tests with `test.bat` (see [Tests](../docs/APP.md#tests)).

`wfd_control.vbs` starts it hidden at login (put a shortcut to it in `shell:startup`).

Full docs: [../docs/APP.md](../docs/APP.md). The serial commands it speaks are in
[../docs/FIRMWARE.md](../docs/FIRMWARE.md).
