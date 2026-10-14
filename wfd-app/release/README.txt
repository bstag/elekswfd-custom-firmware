WFD Control - the PC app for the WFD clock's custom firmware
=============================================================

This zip holds:

  wfd-app.exe   the app (one file; the dashboard pages and the matching clock firmware are built in)
  install.ps1   installs it for your user and starts it at login

Install
-------
Unzip anywhere, then in that folder run:

    powershell -ExecutionPolicy Bypass -File install.ps1

It copies wfd-app.exe to %LOCALAPPDATA%\WFD Clock\, adds a "WFD Clock" shortcut to your
Startup folder and starts the app. The dashboard is at http://127.0.0.1:8737
Settings are kept in %LOCALAPPDATA%\WFD Clock\wfd_config.json.

To update, unzip a newer release and run install.ps1 again; your settings stay.
To remove, delete the "WFD Clock" shortcut from shell:startup and the folder above.

Or just double-click wfd-app.exe to try it without installing (settings then go next to it).

The clock
---------
The clock needs the custom firmware first (one USB flash, see the project README).
After that, "Update clock" in the dashboard keeps it on the firmware this app ships.
The firmware files are also attached to the release:

  wfd-clock-merged.bin     full image for a USB flash at 0x0
  wfd-clock-firmware.bin   app image for an OTA update (http://<clock>/update)

Project, docs and source: https://github.com/bstag/elekswfd-custom-firmware
