//! 14-segment font for the six message characters, plus the scrolling helpers from app.py.
//!
//! Per-char segment bits:
//!   low byte : a=0x10 b=0x20 c=0x40 d=0x80 e=0x01 f=0x02 g=0x04   (g = full middle)
//!   high byte: h=0x10 i=0x20 j=0x40 k=0x80 l=0x01 m=0x02
//! Geometry: a top, b upper-right, c lower-right, d bottom, e lower-left, f upper-left,
//! g middle, h TL->C diag, i top-center vert, j TR->C diag, k BL->C diag, l C->bottom vert,
//! m BR->C diag.
//!
//! Port of wfd-control/font14.py (`glyph`, `render6`, `c_header`) and app.py
//! (`scroll_frame`, `clean_omtext`).

/// Segments lit for each known character (font14.py `_F`).
#[rustfmt::skip]
const FONT: &[(char, &str)] = &[
    (' ', ""), ('-', "g"), ('_', "d"), ('=', "gd"), ('\'', "f"), ('"', "fb"), ('/', "jk"),
    ('\\', "hm"),
    ('0', "abcdef"), ('1', "bc"), ('2', "abged"), ('3', "abgcd"), ('4', "fgbc"), ('5', "afgcd"),
    ('6', "afgecd"), ('7', "abc"), ('8', "abcdefg"), ('9', "abcdfg"),
    ('A', "abcefg"), ('B', "abcdgil"), ('C', "adef"), ('D', "abcdil"), ('E', "adefg"),
    ('F', "aefg"), ('G', "acdef"), ('H', "bcefg"), ('I', "adil"), ('J', "bcde"), ('K', "efgjm"),
    ('L', "def"), ('M', "bcefhj"), ('N', "bcefhm"), ('O', "abcdef"), ('P', "abefg"),
    ('Q', "abcdefm"), ('R', "abefgm"), ('S', "afgcd"), ('T', "ail"), ('U', "bcdef"),
    ('V', "efjk"), ('W', "bcefkm"), ('X', "hjkm"), ('Y', "hjl"), ('Z', "adjk"),
    ('.', "l"), (':', "il"), ('!', "bl"), ('?', "abil"), ('+', "gil"), ('*', "ghijklm"),
    ('(', "adjk"), (')', "adhm"), ('[', "adef"), (']', "abcd"), ('<', "jk"), ('>', "hm"),
    ('%', "fcjk"),
];

/// Firmware MSG_TEXT_MAX: longest offline-message text the clock stores.
pub const OMTEXT_MAX: usize = 96;

fn segments(ch: char) -> Option<&'static str> {
    FONT.iter().find(|(c, _)| *c == ch).map(|(_, s)| *s)
}

/// (low byte, high byte) for one segment letter.
fn segment_bits(s: char) -> (u8, u8) {
    match s {
        'a' => (0x10, 0),
        'b' => (0x20, 0),
        'c' => (0x40, 0),
        'd' => (0x80, 0),
        'e' => (0x01, 0),
        'f' => (0x02, 0),
        'g' => (0x04, 0),
        'h' => (0, 0x10),
        'i' => (0, 0x20),
        'j' => (0, 0x40),
        'k' => (0, 0x80),
        'l' => (0, 0x01),
        'm' => (0, 0x02),
        _ => (0, 0),
    }
}

/// (low, high) segment bytes for a character; unknown characters are blank.
pub fn glyph(ch: char) -> (u8, u8) {
    // Python: `ch.upper() if ch.upper() in _F else ch`. An upper-casing that expands to
    // several chars (e.g. 'ß' -> "SS") is never a table key, so only 1:1 mappings count.
    let mut up = ch.to_uppercase();
    let upper = match (up.next(), up.next()) {
        (Some(u), None) if segments(u).is_some() => u,
        _ => ch,
    };
    segments(upper)
        .unwrap_or("")
        .chars()
        .map(segment_bits)
        .fold((0, 0), |(lo, hi), (l, h)| (lo | l, hi | h))
}

/// 12 bytes ([lo, hi] x 6) for `text[start..start + 6]`; out-of-range positions are spaces.
pub fn render6(text: &str, start: isize) -> [u8; 12] {
    let chars: Vec<char> = text.chars().collect();
    let mut out = [0u8; 12];
    for i in 0..6 {
        let idx = start + i as isize;
        let ch = usize::try_from(idx)
            .ok()
            .and_then(|k| chars.get(k))
            .copied()
            .unwrap_or(' ');
        let (lo, hi) = glyph(ch);
        out[2 * i] = lo;
        out[2 * i + 1] = hi;
    }
    out
}

/// Message bytes for `text` scrolled to `pos`: up to six chars are shown left-aligned and
/// still; longer text wraps around with a three-space gap.
pub fn scroll_frame(text: &str, pos: usize) -> [u8; 12] {
    let len = text.chars().count();
    if len <= 6 {
        // Python ljust(6) then render from 0; render6 already pads with spaces.
        return render6(text, 0);
    }
    let padded = format!("{text}   ");
    let start = pos % (len + 3);
    render6(&padded.repeat(2), start as isize)
}

/// Offline-message text as the clock stores it: printable ASCII only (anything else becomes a
/// space), trimmed, at most `OMTEXT_MAX` chars.
pub fn clean_omtext(text: &str) -> String {
    let cleaned: String = text
        .chars()
        .map(|ch| if (' '..='~').contains(&ch) { ch } else { ' ' })
        .collect();
    // After cleaning, the only whitespace left is ' ', so this matches Python's str.strip().
    cleaned.trim_matches(' ').chars().take(OMTEXT_MAX).collect()
}

/// C tables FONT14_LO/HI for ASCII 32..127, byte-identical to font14.py `c_header()`.
pub fn c_header() -> String {
    let (los, his): (Vec<u8>, Vec<u8>) = (32u8..128).map(|c| glyph(char::from(c))).unzip();
    let row = |bs: &[u8]| {
        bs.iter()
            .map(|b| format!("0x{b:02x}"))
            .collect::<Vec<_>>()
            .join(",")
    };
    format!(
        "// 14-seg font for ASCII 32..127 (generated from font14.py)\n\
         static const uint8_t FONT14_LO[96]={{{}}};\n\
         static const uint8_t FONT14_HI[96]={{{}}};\n",
        row(&los),
        row(&his)
    )
}
