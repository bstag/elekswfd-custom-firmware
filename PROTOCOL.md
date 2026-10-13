# EleksWFD serial protocol (reverse-engineered from EleksWFD_EN.exe v3.1)

The findings come from disassembling `EleksWFD_EN.exe`. The clock was not watched while the commands were sent, so each command's effect on screen still needs checking.

## Hardware / link
- MCU: **ESP32**. The vendor tool flashes it with `esptool --chip esp32 --flash_mode dio --flash_freq 80m`.
- USB bridge: **CH340**, USB\VID_1A86&PID_7523. It shows up as a normal COM port (COM4 on this PC).
- Serial: **115200 baud, 8N1, no flow control**.
- The app opens the port, writes, waits a few ms and closes it again for every action.
- **The protocol is one-way.** The app reads what the device sends back but only writes it to its debug log. The device sent nothing back in testing.
- Boot banner on UART: `Load Success / Authorization Success / SPIFFS文件系统挂载成功！/ Initiate Success`.

## Commands (PC -> clock)

| Bytes | Meaning (from app code path) |
|---|---|
| `EA` | Reset the stored animation to factory default ("Clean EleksWFD GIF to Default") |
| `ED nn ss` | Start an animation upload: `nn` = frame count, `ss` = play speed. Wait ~50 ms after it. |
| `EE b0..b6` | One animation frame (7 bytes = 49 bits = **7x7 grid**). Send `nn` of these, ~100 ms apart. |
| `EC AA b0..b6` | "Link mode": show one 7x7 frame live (the editor preview). Total 9 bytes. |
| `EC 00` + 7x`00` | Exit link mode |
| `EB AA c g r 00 00 00 00` | System-info bars: each value is 0-20 (the app sends percent / 5). Three bars of 20 LEDs. Byte order looks like CPU, GPU, RAM. |
| `EB 00` + 7x`00` | Stop system-info display |
| `EF 04` | Return to the normal display mode. The app only ever sends `04`. `EF 01`-`03` were tested on the device and do nothing visible. |
| ASCII `CL` | Clear, sent when leaving sync mode, followed by `EB 00` and `EF 04` |
| ASCII `FA<text>` | Scrolling text, Latin-1, no terminator. The app used it for `BILIBILI <n> TIKTOK <m>` follower counts. |

Frame encoding, from the vendor editor code: byte *k* = grid row *k*, top row first. Within a byte, the leftmost cell is bit 6 (`0x40`) and the rightmost is bit 0. The vendor UI limits play speed to 1-10 and frame count to 1-255. The speed's actual timing on the clock is unknown.

Confirmed on the device:
- scrolling text (`FA`)
- bar order (top bar = CPU, then GPU, then RAM)
- frame orientation (byte 0 = top row, bit 6 = left column)

## .WFD animation file
- 10000 bytes.
- Frame *i* (1-based) is 7 bytes at offset `7*i`.
- Byte 9998 = frame count, byte 9999 = play speed.

## Device firmware (found inside the vendor package)
The vendor app hides the clock firmware as fake translation files:
- `translations/qt_ru.qm` is the ESP32 app image, flashed at 0x10000.
- `translations/qt_uk.qm` is the partition table, flashed at 0x8000.

The partition table:

| Partition | Offset | Size |
|---|---|---|
| nvs | 0x9000 | 0x5000 |
| otadata | 0xe000 | 0x2000 |
| app0 | 0x10000 | 2 MB |
| spiffs | 0x210000 | 0x1f0000 |

About the image:
- Arduino-ESP32 on ESP-IDF 3.3.5, built 26 March 2021.
- The animation is stored as `/movie.dat` on SPIFFS.
- The firmware has a MAC-based "Authorization" check: a key table and EEPROM MAC compare.
- FreeRTOS tasks: `PUSH_RAM`, `ONOFF`, `FLASH_PROGRESSBAR1`, `FLASH_GRID`, `PICK_SOUND`, `SHOW_DATE`.
- A button held at boot starts `CONNECT_WIFI` and `SETTING`. Despite its name, the `CONNECT_WIFI` task contains no Wi-Fi calls; there is no NTP or SNTP code.
- Buttons are on GPIO25, GPIO26 and GPIO27.
- The display controller is initialised with the command bytes `80 A4 98 81 83 <brightness> 88`.

## Microphone / spectrum mode (PICK_SOUND task @ 0x400d1ef8)
All of this happens on the device; the PC is not involved. The task loops every 10 ms:
1. Reads the mic with `analogRead(32)`, i.e. GPIO32 / ADC1 channel 4.
2. Tracks the peak. It ignores readings below 6 and halves the peak each pass.
3. Tracks an adaptive noise floor: the floor is the minimum sample seen, and it creeps up by 10 per pass.
4. If the sample is above 50, the level is `map(sample - floor, 0, 0.9*peak, 0, 12)`.
5. The display rises instantly to the level and falls back one step at a time. The fall speed is set by a variable.
6. It draws in one of 3 styles, selected by `settings[1]`. Each button press cycles the style 1 -> 2 -> 3 -> 1.

The task is skipped while the flag at `0x3ffc021c` is set (the other modes). No USB command changes the style or the sensitivity.

## Clock (time)
- There's no RTC chip: no I2C, and no DS1302 or similar commands.
- There's no network time.
- The time is kept in software on the ESP32 and set with the buttons in the `SETTING` screen. That screen blinks the time and date fields and is entered by holding a button at power-on.
- The serial parser has no set-time command.

## Time
**The PC app never sends the time.** QTime is used only for its delay() helper. The clock keeps its own time in firmware with no RTC chip and no network time (see Clock (time) above), so it can't be fixed over this protocol.

## Firmware update path (vendor)
`esptool write_flash -z` at 921600:
- `0x1000` bootloader_qio_80m.bin
- `0x8000` partitions
- `0xe000` boot_app0.bin
- `0x10000` app

The app and partition images do ship in this folder, disguised as `translations/qt_ru.qm` and `translations/qt_uk.qm`.

### Getting into download mode (tested 2026-10-13)
- **RTS -> EN works.** Every RTS pulse reboots the chip.
- **DTR behaves like the classic two-transistor auto-reset:** both lines asserted is a no-op. But no sequence pulls GPIO0 low at reset. Every attempt boots `0x13` (SPI_FAST_FLASH_BOOT).

What was tried:
- esptool v5 `default-reset`, with longer delays.
- Manual and swapped sequences.
- Switching both lines in a single `SetCommState` call.
- Raw ROM SYNC probes.
- The vendor's own `esptool.exe` v2.6 (`chip_id`, read-only). It fails too: "Timed out waiting for packet header".
- Holding the A, B or C buttons during reset. The buttons are GPIO25, 26 and 27, not GPIO0.

~~Conclusion: flashing needs GPIO0 held by hand.~~ **Solved: it was the USB port.** Behind a hub on the AMD USB 3.1 controller, auto-reset into download mode never worked: 41 attempts, 0 successes. On a **USB 2.0 port** it worked on the first try with every method, including the vendor's esptool. This matches the CH340 + USB 3.x timing issue noted by EleksTubeHAX.

Flash backup taken 2026-10-13 over USB 2.0:
- File: `firmware-backup/elekswfd_flash_backup.bin`, 4 MB.
- sha256 `06c10198…8148d8`.
- Chip ESP32-D0WDQ5 rev 1, MAC a8:42:e3:79:f9:24.
- The app, partitions, bootloader and boot_app0 in it are byte-identical to the vendor files.
- nvs (settings and licence) holds about 2 KB; spiffs holds the stored animation.

## Dead cloud features
- Bilibili API: closed, per the vendor's own 2023 release note.
- Douyin API.
- weather.com.cn.
- Update manifest: http://soft.eleksmaker.com/eleks.json (still up, last entry 2023).
