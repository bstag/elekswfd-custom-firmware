"""Drive the clock's two spectrum columns from the PC microphone.

    python wfd_spectrum.py [--device N] [--gain 1.0] [--rate 30]

Captures the default input device, splits it into a low and a high band
(the two columns), maps each to 0..12 with auto-gain and VU-style decay,
and streams "SPEC lo hi" to the clock. Leaves the time display untouched.
"""
import argparse
import sys
import time

import numpy as np
import sounddevice as sd
import serial
from serial.tools import list_ports

SR = 16000
BLOCK = 512
NLEV = 12


def find_port() -> str:
    for p in list_ports.comports():
        if p.vid == 0x1A86 and p.pid == 0x7523:
            return p.device
    raise SystemExit("clock not found (CH340 1A86:7523)")


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--device", type=int, default=None, help="input device index (default: system default)")
    ap.add_argument("--gain", type=float, default=1.0)
    ap.add_argument("--rate", type=float, default=30.0, help="frames per second to the clock")
    ap.add_argument("--split", type=float, default=900.0, help="Hz boundary between the two columns")
    args = ap.parse_args()

    s = serial.Serial()
    s.port, s.baudrate, s.timeout = find_port(), 115200, 1
    s.dtr = s.rts = False
    s.open()
    time.sleep(0.2)

    freqs = np.fft.rfftfreq(BLOCK, 1 / SR)
    lo_band = freqs < args.split
    hi_band = ~lo_band
    win = np.hanning(BLOCK)

    peak = [1e-6, 1e-6]   # per-band adaptive maximum (auto-gain)
    disp = [0.0, 0.0]     # smoothed displayed level

    latest = {"lo": 0.0, "hi": 0.0}

    def cb(indata, frames, t, status):
        x = indata[:, 0] * win
        mag = np.abs(np.fft.rfft(x))
        latest["lo"] = float(np.sqrt(np.mean((mag[lo_band]) ** 2)))
        latest["hi"] = float(np.sqrt(np.mean((mag[hi_band]) ** 2)))

    dev = args.device
    name = sd.query_devices(dev if dev is not None else sd.default.device[0])["name"]
    print(f"listening on: {name}")
    print(f"driving clock on {s.port}.  Ctrl+C to stop.")

    with sd.InputStream(device=dev, channels=1, samplerate=SR, blocksize=BLOCK, callback=cb):
        period = 1.0 / args.rate
        try:
            while True:
                vals = []
                for i, key in enumerate(("lo", "hi")):
                    e = latest[key] * args.gain
                    peak[i] = max(e, peak[i] * 0.995)            # slow decay of the auto-gain ceiling
                    frac = 0.0 if peak[i] < 1e-5 else e / peak[i]
                    target = NLEV * max(0.0, (frac - 0.15) / 0.85)  # small noise gate
                    if target >= disp[i]:
                        disp[i] = target                         # attack: jump up
                    else:
                        disp[i] = max(target, disp[i] - 0.8)     # decay: fall ~0.8/frame
                    vals.append(int(round(min(NLEV, disp[i]))))
                s.write(f"SPEC {vals[0]} {vals[1]}\n".encode())
                time.sleep(period)
        except KeyboardInterrupt:
            s.write(b"SPEC 0 0\n")
            s.close()
            print("\nstopped")


if __name__ == "__main__":
    main()
