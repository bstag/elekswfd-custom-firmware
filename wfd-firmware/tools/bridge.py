"""Talk to the wfd-bridge test firmware: python bridge.py "<cmd>" ["<cmd>" ...]

Also importable: Bridge().cmd("scan"), Bridge().tm_init(), Bridge().tm_write(bytes).
"""
import sys
import time

import serial
from serial.tools import list_ports

TM, RTC = 0x73, 0x68
TM_INIT = [0x80, 0xA4, 0x98, 0x81, 0x83, 0xB4, 0x88]  # sys off, COM opt, RC master, sys on, LED on, PWM, blink off


def find_port() -> str:
    for p in list_ports.comports():
        if p.vid == 0x1A86 and p.pid == 0x7523:
            return p.device
    raise SystemExit("clock not found (CH340 1A86:7523)")


class Bridge:
    def __init__(self, port: str | None = None):
        self.s = serial.Serial()
        self.s.port, self.s.baudrate, self.s.timeout = port or find_port(), 115200, 2
        self.s.dtr = self.s.rts = False  # don't reset the board on open
        self.s.open()
        time.sleep(0.2)
        self.s.reset_input_buffer()

    def cmd(self, line: str) -> str:
        self.s.write((line + "\n").encode())
        while True:
            r = self.s.readline().decode(errors="replace").strip()
            if not r:
                raise TimeoutError(f"no reply to {line!r}")
            if r.startswith(("ok", "err")):
                return r

    def w(self, addr: int, data) -> str:
        return self.cmd(f"w {addr:x} " + " ".join(f"{b:02x}" for b in data))

    def tm_init(self, pwm: int = 4):
        for c in TM_INIT[:-2] + [0xB0 | (pwm & 0xF), 0x88]:
            r = self.w(TM, [c])
            if not r.startswith("ok"):
                return r
        return "ok"

    def tm_write(self, start: int, data) -> str:
        return self.w(TM, [start] + list(data))


if __name__ == "__main__":
    b = Bridge()
    for c in sys.argv[1:]:
        print(f"> {c}\n{b.cmd(c)}")
