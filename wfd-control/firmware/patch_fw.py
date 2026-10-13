"""Patch the EleksWFD ESP32 app image (vendor file translations/qt_ru.qm).

Each patch names a virtual address (as seen in a disassembly), the bytes expected there,
and the replacement bytes. The tool refuses to patch if the expected bytes don't match,
then recomputes the image checksum and appended SHA-256 so the bootloader accepts it.

    python patch_fw.py <in.bin> <out.bin> <patchset>
    python patch_fw.py --check <image.bin>
"""
from __future__ import annotations

import hashlib
import struct
import sys
from pathlib import Path

HEADER_LEN = 24  # 8-byte image header + 16-byte extended header


class Image:
    def __init__(self, data: bytes):
        if data[0] != 0xE9:
            raise ValueError("not an ESP32 app image (magic byte != 0xE9)")
        self.data = bytearray(data)
        self.segments = []  # (load_addr, file_offset_of_data, length)
        off = HEADER_LEN
        for _ in range(data[1]):
            addr, length = struct.unpack_from("<II", data, off)
            self.segments.append((addr, off + 8, length))
            off += 8 + length
        self.end_of_segments = off
        self.hash_appended = data[23] == 1

    def va_to_offset(self, va: int) -> int:
        for addr, foff, length in self.segments:
            if addr <= va < addr + length:
                return foff + (va - addr)
        raise ValueError(f"address {va:#x} is not inside any image segment")

    def checksum_offset(self) -> int:
        # zero padding so that (end + 1) is 16-byte aligned, then the checksum byte
        return self.end_of_segments + (15 - (self.end_of_segments % 16))

    def compute_checksum(self) -> int:
        c = 0xEF
        for _, foff, length in self.segments:
            for b in self.data[foff:foff + length]:
                c ^= b
        return c

    def fix(self) -> bytes:
        co = self.checksum_offset()
        self.data[co] = self.compute_checksum()
        body = bytes(self.data[:co + 1])
        if self.hash_appended:
            body += hashlib.sha256(body).digest()
        return body + bytes(self.data[len(body):])  # keep any trailing bytes untouched

    def verify(self) -> list[str]:
        problems = []
        co = self.checksum_offset()
        if self.data[co] != self.compute_checksum():
            problems.append("checksum mismatch")
        if self.hash_appended:
            if hashlib.sha256(self.data[:co + 1]).digest() != bytes(self.data[co + 1:co + 33]):
                problems.append("sha256 mismatch")
        return problems


def apply(img: Image, patches: list[tuple[int, bytes, bytes, str]]) -> None:
    for va, old, new, note in patches:
        if len(old) != len(new):
            raise ValueError(f"{note}: replacement must be the same length")
        off = img.va_to_offset(va)
        cur = bytes(img.data[off:off + len(old)])
        if cur != old:
            raise ValueError(f"{note}: expected {old.hex(' ')} at {va:#x}, found {cur.hex(' ')}")
        img.data[off:off + len(new)] = new
        print(f"  patched {va:#010x} (+{off:#x}): {old.hex(' ')} -> {new.hex(' ')}   {note}")


# --------------------------------------------------------------------------
# Patch sets. Addresses come from disassembling vendor firmware
# (Arduino-ESP32 / IDF 3.3.5, built Mar 26 2021). Each patch checks the original bytes first.
# --------------------------------------------------------------------------
PATCHSETS: dict[str, list[tuple[int, bytes, bytes, str]]] = {
    # Zero-risk pipeline test: only changes the text printed on the serial port at boot.
    "banner-test": [
        (0x3F400326, b"Initiate Success", b"WFD patch test 1", "boot banner text (serial only)"),
    ],
}


def main(argv: list[str]) -> int:
    if len(argv) == 3 and argv[1] == "--check":
        img = Image(Path(argv[2]).read_bytes())
        probs = img.verify()
        print("OK" if not probs else "BAD: " + ", ".join(probs))
        return 1 if probs else 0
    if len(argv) != 4 or argv[3] not in PATCHSETS:
        print(__doc__)
        print("patch sets:", ", ".join(PATCHSETS))
        return 2
    src, dst, name = Path(argv[1]), Path(argv[2]), argv[3]
    img = Image(src.read_bytes())
    if img.verify():
        print("input image does not verify:", img.verify())
        return 1
    print(f"applying '{name}' to {src.name}")
    apply(img, PATCHSETS[name])
    out = img.fix()
    check = Image(out).verify()
    if check:
        print("internal error, output does not verify:", check)
        return 1
    dst.write_bytes(out)
    print(f"wrote {dst} ({len(out)} bytes), checksum + sha256 recomputed")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
