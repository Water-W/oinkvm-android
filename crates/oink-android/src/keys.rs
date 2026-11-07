//! HID keyboard usages to text and modifier state.
//!
//! Accessibility services cannot inject key events, so printable keys become
//! text edits on the focused field. The text follows the US layout; IME
//! composition on the source computer is not forwarded.

/// Modifier bits in HID order (E0..E7): LCtrl, LShift, LAlt, LMeta, RCtrl, RShift, RAlt, RMeta.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers(pub u8);

impl Modifiers {
    pub fn update(&mut self, usage: u16, pressed: bool) -> bool {
        if !(0xE0..=0xE7).contains(&usage) {
            return false;
        }
        let bit = 1u8 << (usage - 0xE0);
        if pressed {
            self.0 |= bit;
        } else {
            self.0 &= !bit;
        }
        true
    }
    pub fn shift(self) -> bool {
        self.0 & 0b0010_0010 != 0
    }
    /// Control, Alt or Meta: the key is a shortcut, not text.
    pub fn command(self) -> bool {
        self.0 & 0b1101_1101 != 0
    }
}

/// The character a US keyboard produces for `usage`, if it is printable.
pub fn us_text(usage: u16, shift: bool, caps: bool) -> Option<char> {
    let c = match usage {
        0x04..=0x1D => {
            let base = (b'a' + (usage - 0x04) as u8) as char;
            return Some(if shift ^ caps { base.to_ascii_uppercase() } else { base });
        }
        0x1E..=0x27 => {
            const PLAIN: &[u8; 10] = b"1234567890";
            const SHIFTED: &[u8; 10] = b"!@#$%^&*()";
            let i = (usage - 0x1E) as usize;
            return Some((if shift { SHIFTED[i] } else { PLAIN[i] }) as char);
        }
        0x2C => (' ', ' '),
        0x2D => ('-', '_'),
        0x2E => ('=', '+'),
        0x2F => ('[', '{'),
        0x30 => (']', '}'),
        0x31 | 0x32 => ('\\', '|'),
        0x33 => (';', ':'),
        0x34 => ('\'', '"'),
        0x35 => ('`', '~'),
        0x36 => (',', '<'),
        0x37 => ('.', '>'),
        0x38 => ('/', '?'),
        // Keypad
        0x54 => ('/', '/'),
        0x55 => ('*', '*'),
        0x56 => ('-', '-'),
        0x57 => ('+', '+'),
        0x59..=0x61 => {
            let d = (b'1' + (usage - 0x59) as u8) as char;
            (d, d)
        }
        0x62 => ('0', '0'),
        0x63 => ('.', '.'),
        _ => return None,
    };
    Some(if shift { c.1 } else { c.0 })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_digits_and_symbols_follow_us_layout() {
        assert_eq!(us_text(0x04, false, false), Some('a'));
        assert_eq!(us_text(0x04, true, false), Some('A'));
        assert_eq!(us_text(0x04, true, true), Some('a'));
        assert_eq!(us_text(0x1E, false, false), Some('1'));
        assert_eq!(us_text(0x1E, true, false), Some('!'));
        assert_eq!(us_text(0x27, false, false), Some('0'));
        assert_eq!(us_text(0x38, true, false), Some('?'));
        assert_eq!(us_text(0x28, false, false), None);
    }

    #[test]
    fn modifiers_track_hid_bits() {
        let mut m = Modifiers::default();
        assert!(m.update(0xE1, true));
        assert!(m.shift() && !m.command());
        assert!(m.update(0xE0, true));
        assert!(m.command());
        m.update(0xE0, false);
        m.update(0xE1, false);
        assert_eq!(m, Modifiers(0));
        assert!(!m.update(0x04, true));
    }
}
