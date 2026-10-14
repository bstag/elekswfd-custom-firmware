//! window: the foreground window's title (`foreground_title` in app.py), for the message line.

/// Title of the foreground window; non-ASCII characters become spaces, "" when there is none.
#[cfg(windows)]
pub fn foreground_title() -> String {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindowTextLengthW, GetWindowTextW,
    };
    // SAFETY: plain Win32 calls; the buffer is sized for the length asked for, plus the NUL.
    let utf16 = unsafe {
        let h = GetForegroundWindow();
        if h.is_null() {
            return String::new();
        }
        let n = GetWindowTextLengthW(h).max(0);
        let mut buf = vec![0u16; n as usize + 1];
        let got = GetWindowTextW(h, buf.as_mut_ptr(), n + 1).max(0) as usize;
        buf.truncate(got.min(n as usize));
        buf
    };
    ascii_only(&utf16)
}

#[cfg(not(windows))]
pub fn foreground_title() -> String {
    String::new()
}

/// One space per non-ASCII code point (a surrogate pair is one code point, as in Python).
fn ascii_only(utf16: &[u16]) -> String {
    char::decode_utf16(utf16.iter().copied())
        .map(|c| match c {
            Ok(c) if c.is_ascii() => c,
            _ => ' ',
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_ascii_becomes_space() {
        let s: Vec<u16> = "Caf\u{e9} \u{1F600}!".encode_utf16().collect();
        assert_eq!(ascii_only(&s), "Caf   !");
    }

    #[test]
    fn foreground_title_does_not_panic() {
        let _ = foreground_title();
    }
}
