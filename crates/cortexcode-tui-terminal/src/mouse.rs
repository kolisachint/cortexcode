//! Mouse reporting: turning it on, and reading what comes back. Port of
//! `mouse.ts`.
//!
//! The wheel is captured so scrolling is something the app decides, not a
//! race between the app's writes and the terminal's scrollback. While it is
//! on, a plain drag is delivered here instead of selecting text (every
//! terminal keeps a shift bypass); `CORTEX_MOUSE=0` turns it off.
//!
//! `?1000h` (press/release, which carries the wheel) and `?1006h` (SGR
//! coordinates) only; drag and any-motion tracking are deliberately not
//! enabled.

/// Enable button + wheel reporting, preferring SGR coordinates.
pub const MOUSE_ENABLE: &str = "\x1b[?1000h\x1b[?1006h";

/// Disable in the reverse order it was enabled.
pub const MOUSE_DISABLE: &str = "\x1b[?1006l\x1b[?1000l";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseEventKind {
    WheelUp,
    WheelDown,
    WheelLeft,
    WheelRight,
    Press,
    Release,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MouseEvent {
    pub kind: MouseEventKind,
    /// 0 left, 1 middle, 2 right; -1 for wheel events and X10 releases.
    pub button: i32,
    /// 1-based, as the terminal reports it.
    pub column: i64,
    /// 1-based, as the terminal reports it.
    pub row: i64,
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
}

const X10_PREFIX: &str = "\x1b[M";

/// `^\x1b\[<(\d+);(\d+);(\d+)([Mm])`: the numbers, the final byte and the
/// match length.
fn match_sgr(data: &str) -> Option<(&str, &str, &str, char, usize)> {
    let rest = data.strip_prefix("\x1b[<")?;
    let mut fields = [""; 3];
    let mut pos = 0;
    for (i, field) in fields.iter_mut().enumerate() {
        let digits = rest[pos..].bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            return None;
        }
        *field = &rest[pos..pos + digits];
        pos += digits;
        let sep = if i < 2 { b';' } else { 0 };
        if i < 2 {
            if rest.as_bytes().get(pos) != Some(&sep) {
                return None;
            }
            pos += 1;
        }
    }
    let final_byte = match rest.as_bytes().get(pos) {
        Some(b'M') => 'M',
        Some(b'm') => 'm',
        _ => return None,
    };
    Some((fields[0], fields[1], fields[2], final_byte, 3 + pos + 1))
}

/// The X10 payload bytes (JS `charCodeAt`: UTF-16 units, here chars).
fn x10_payload(data: &str) -> Option<[u32; 3]> {
    let rest = data.strip_prefix(X10_PREFIX)?;
    let mut chars = rest.chars();
    Some([
        chars.next()? as u32,
        chars.next()? as u32,
        chars.next()? as u32,
    ])
}

/// Whether `data` starts with a mouse report.
pub fn is_mouse_sequence(data: &str) -> bool {
    match_sgr(data).is_some() || x10_payload(data).is_some()
}

/// How many bytes the leading mouse report occupies, or 0 if there is none.
pub fn mouse_sequence_length(data: &str) -> usize {
    if let Some((.., len)) = match_sgr(data) {
        return len;
    }
    if let Some(rest) = data.strip_prefix(X10_PREFIX) {
        let payload: usize = rest.chars().take(3).map(char::len_utf8).sum();
        if rest.chars().take(3).count() == 3 {
            return X10_PREFIX.len() + payload;
        }
    }
    0
}

fn decode(flags: i64, column: i64, row: i64, pressed: bool) -> MouseEvent {
    let shift = flags & 4 != 0;
    let alt = flags & 8 != 0;
    let ctrl = flags & 16 != 0;
    // Bit 6 marks the wheel; the low two bits then give the direction.
    if flags & 64 != 0 {
        let kind = match flags & 3 {
            0 => MouseEventKind::WheelUp,
            1 => MouseEventKind::WheelDown,
            2 => MouseEventKind::WheelLeft,
            _ => MouseEventKind::WheelRight,
        };
        return MouseEvent {
            kind,
            button: -1,
            column,
            row,
            shift,
            alt,
            ctrl,
        };
    }
    let button = (flags & 3) as i32;
    // X10 has no per-button release: button 3 *is* the release.
    if !pressed || button == 3 {
        return MouseEvent {
            kind: MouseEventKind::Release,
            button: if button == 3 { -1 } else { button },
            column,
            row,
            shift,
            alt,
            ctrl,
        };
    }
    MouseEvent {
        kind: MouseEventKind::Press,
        button,
        column,
        row,
        shift,
        alt,
        ctrl,
    }
}

/// Read the mouse report at the start of `data`. Coordinates stay 1-based.
pub fn parse_mouse_event(data: &str) -> Option<MouseEvent> {
    if let Some((flags, column, row, final_byte, _)) = match_sgr(data) {
        return Some(decode(
            flags.parse().ok()?,
            column.parse().ok()?,
            row.parse().ok()?,
            final_byte == 'M',
        ));
    }
    let [flags, column, row] = x10_payload(data)?;
    let (flags, column, row) = (flags as i64 - 32, column as i64 - 32, row as i64 - 32);
    if flags < 0 || column < 0 || row < 0 {
        return None;
    }
    Some(decode(flags, column, row, true))
}
