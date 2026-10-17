WFD Control - the PC app for the WFD clock's custom firmware
=============================================================

This zip holds:

  wfd-flash.exe  puts the custom firmware on the clock over USB (backs it up first)
  wfd-app.exe    the PC app (one file; the dashboard pages and the matching clock firmware are built in)
  install.ps1    installs the app for your user and starts it at login

First time: flash the clock
---------------------------
Plug the clock into a USB 2.0 port (USB 3 ports can't put it into flashing mode) and
double-click wfd-flash.exe. It:

  1. finds the clock (and tells you if Windows is missing its USB driver)
  2. backs up the whole flash (4 MB, about 2 minutes) to Documents\WFD Clock backups,
     checked against the clock's own checksum; if that fails, nothing is written
  3. asks, then writes the firmware and restarts the clock

On a clock that already runs this firmware it keeps your WiFi and settings. To go back to
the stock firmware later: wfd-flash.exe --restore "<your backup file>"

Then on your phone, join the clock's "WFD-Setup" WiFi and pick your network (2.4 GHz).

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

Updates
-------
After the first flash, "Update clock" in the dashboard keeps the clock on the firmware
this app ships, over WiFi. The firmware files are also attached to the release:

  wfd-clock-merged.bin     full image for a USB flash at 0x0 (erases the clock's settings)
  wfd-clock-firmware.bin   app image for an OTA update (http://<clock>/update)

Project, docs and source: https://github.com/bstag/elekswfd-custom-firmware
