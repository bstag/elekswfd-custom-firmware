"""14-segment font for the EleksWFD message characters.

Per-char segment bits (confirmed from the segment map):
  low byte : a=0x10 b=0x20 c=0x40 d=0x80 e=0x01 f=0x02 g=0x04   (g = full middle)
  high byte: h=0x10 i=0x20 j=0x40 k=0x80 l=0x01 m=0x02
Geometry: a top, b upper-right, c lower-right, d bottom, e lower-left, f upper-left,
g middle, h TL->C diag, i top-center vert, j TR->C diag, k BL->C diag, l C->bottom vert, m BR->C diag.
"""
SEG_LOW = {"a": 0x10, "b": 0x20, "c": 0x40, "d": 0x80, "e": 0x01, "f": 0x02, "g": 0x04}
SEG_HIGH = {"h": 0x10, "i": 0x20, "j": 0x40, "k": 0x80, "l": 0x01, "m": 0x02}

# segments per glyph
_F = {
    " ": "", "-": "g", "_": "d", "=": "gd", "'": "f", '"': "fb", "/": "jk", "\\": "hm",
    "0": "abcdef", "1": "bc", "2": "abged", "3": "abgcd", "4": "fgbc", "5": "afgcd",
    "6": "afgecd", "7": "abc", "8": "abcdefg", "9": "abcdfg",
    "A": "abcefg", "B": "abcdgil", "C": "adef", "D": "abcdil", "E": "adefg", "F": "aefg",
    "G": "acdef", "H": "bcefg", "I": "adil", "J": "bcde", "K": "efgjm", "L": "def",
    "M": "bcefhj", "N": "bcefhm", "O": "abcdef", "P": "abefg", "Q": "abcdefm", "R": "abefgm",
    "S": "afgcd", "T": "ail", "U": "bcdef", "V": "efjk", "W": "bcefkm", "X": "hjkm",
    "Y": "hjl", "Z": "adjk",
    ".": "l", ":": "il", "!": "bl", "?": "abil", "+": "gil", "*": "ghijklm",
    "(": "adjk", ")": "adhm", "[": "adef", "]": "abcd", "<": "jk", ">": "hm", "%": "fcjk",
}


def glyph(ch):
    """Return (low_byte, high_byte) for a character; unknown -> blank."""
    segs = _F.get(ch.upper() if ch.upper() in _F else ch, "")
    lo = hi = 0
    for s in segs:
        lo |= SEG_LOW.get(s, 0)
        hi |= SEG_HIGH.get(s, 0)
    return lo, hi


def render6(text, start=0):
    """12 bytes for the 6 message chars showing text[start:start+6] (space-padded)."""
    out = []
    for i in range(6):
        ch = text[start + i] if 0 <= start + i < len(text) else " "
        lo, hi = glyph(ch)
        out += [lo, hi]
    return out
