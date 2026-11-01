use objc2_core_graphics::CGEventFlags;

use crate::keys::{Key, KeypadKey, Modifiers};

pub(super) struct Mapped {
    pub(super) code: u16,
    pub(super) implied: Modifiers,
}

const fn plain(code: u16) -> Mapped {
    Mapped { code, implied: Modifiers::empty() }
}

const fn shifted(code: u16) -> Mapped {
    Mapped { code, implied: Modifiers::SHIFT }
}

pub(super) const COMMAND: u16 = 0x37;
pub(super) const SHIFT: u16 = 0x38;
pub(super) const OPTION: u16 = 0x3A;
pub(super) const CONTROL: u16 = 0x3B;
pub(super) const FUNCTION: u16 = 0x3F;
pub(super) const RETURN: u16 = 0x24;
pub(super) const TAB: u16 = 0x30;

pub(super) const MODIFIER_KEYS: [(Modifiers, u16, CGEventFlags); 5] = [
    (Modifiers::CONTROL, CONTROL, CGEventFlags::MaskControl),
    (Modifiers::ALT, OPTION, CGEventFlags::MaskAlternate),
    (Modifiers::SHIFT, SHIFT, CGEventFlags::MaskShift),
    (Modifiers::META, COMMAND, CGEventFlags::MaskCommand),
    (Modifiers::FUNCTION, FUNCTION, CGEventFlags::MaskSecondaryFn),
];

pub(super) fn flags(modifiers: Modifiers) -> CGEventFlags {
    MODIFIER_KEYS
        .iter()
        .filter(|(m, _, _)| modifiers.contains(*m))
        .fold(CGEventFlags::empty(), |acc, (_, _, f)| acc | *f)
}

pub(super) fn map(key: Key) -> Option<Mapped> {
    Some(match key {
        Key::Char(c) => map_char(c)?,
        Key::Return => plain(RETURN),
        Key::Tab => plain(TAB),
        Key::Space => plain(0x31),
        Key::Escape => plain(0x35),
        Key::Backspace => plain(0x33),
        Key::Delete => plain(0x75),
        Key::Insert => plain(0x72),
        Key::Up => plain(0x7E),
        Key::Down => plain(0x7D),
        Key::Left => plain(0x7B),
        Key::Right => plain(0x7C),
        Key::Home => plain(0x73),
        Key::End => plain(0x77),
        Key::PageUp => plain(0x74),
        Key::PageDown => plain(0x79),
        Key::CapsLock => plain(0x39),
        Key::Menu => plain(0x6E),
        Key::F(n) => plain(function_key(n)?),
        Key::Keypad(k) => plain(keypad(k)),
        Key::Modifier(m) => plain(MODIFIER_KEYS.iter().find(|(flag, _, _)| m.contains(*flag))?.1),
    })
}

const fn function_key(n: u8) -> Option<u16> {
    Some(match n {
        1 => 0x7A,
        2 => 0x78,
        3 => 0x63,
        4 => 0x76,
        5 => 0x60,
        6 => 0x61,
        7 => 0x62,
        8 => 0x64,
        9 => 0x65,
        10 => 0x6D,
        11 => 0x67,
        12 => 0x6F,
        13 => 0x69,
        14 => 0x6B,
        15 => 0x71,
        16 => 0x6A,
        17 => 0x40,
        18 => 0x4F,
        19 => 0x50,
        20 => 0x5A,
        _ => return None,
    })
}

const fn keypad(key: KeypadKey) -> u16 {
    match key {
        KeypadKey::Digit(d) => match d {
            0 => 0x52,
            1 => 0x53,
            2 => 0x54,
            3 => 0x55,
            4 => 0x56,
            5 => 0x57,
            6 => 0x58,
            7 => 0x59,
            8 => 0x5B,
            _ => 0x5C,
        },
        KeypadKey::Enter => 0x4C,
        KeypadKey::Equal => 0x51,
        KeypadKey::Multiply => 0x43,
        KeypadKey::Add => 0x45,
        KeypadKey::Subtract => 0x4E,
        KeypadKey::Decimal => 0x41,
        KeypadKey::Divide => 0x4B,
    }
}

const fn map_char(c: char) -> Option<Mapped> {
    Some(match c {
        'a' => plain(0x00),
        's' => plain(0x01),
        'd' => plain(0x02),
        'f' => plain(0x03),
        'h' => plain(0x04),
        'g' => plain(0x05),
        'z' => plain(0x06),
        'x' => plain(0x07),
        'c' => plain(0x08),
        'v' => plain(0x09),
        'b' => plain(0x0B),
        'q' => plain(0x0C),
        'w' => plain(0x0D),
        'e' => plain(0x0E),
        'r' => plain(0x0F),
        'y' => plain(0x10),
        't' => plain(0x11),
        'o' => plain(0x1F),
        'u' => plain(0x20),
        'i' => plain(0x22),
        'p' => plain(0x23),
        'l' => plain(0x25),
        'j' => plain(0x26),
        'k' => plain(0x28),
        'n' => plain(0x2D),
        'm' => plain(0x2E),
        '1' => plain(0x12),
        '2' => plain(0x13),
        '3' => plain(0x14),
        '4' => plain(0x15),
        '5' => plain(0x17),
        '6' => plain(0x16),
        '7' => plain(0x1A),
        '8' => plain(0x1C),
        '9' => plain(0x19),
        '0' => plain(0x1D),
        '=' => plain(0x18),
        '-' => plain(0x1B),
        ']' => plain(0x1E),
        '[' => plain(0x21),
        '\'' => plain(0x27),
        ';' => plain(0x29),
        '\\' => plain(0x2A),
        ',' => plain(0x2B),
        '/' => plain(0x2C),
        '.' => plain(0x2F),
        '`' => plain(0x32),
        ' ' => plain(0x31),
        '!' => shifted(0x12),
        '@' => shifted(0x13),
        '#' => shifted(0x14),
        '$' => shifted(0x15),
        '%' => shifted(0x17),
        '^' => shifted(0x16),
        '&' => shifted(0x1A),
        '*' => shifted(0x1C),
        '(' => shifted(0x19),
        ')' => shifted(0x1D),
        '+' => shifted(0x18),
        '_' => shifted(0x1B),
        '}' => shifted(0x1E),
        '{' => shifted(0x21),
        '"' => shifted(0x27),
        ':' => shifted(0x29),
        '|' => shifted(0x2A),
        '<' => shifted(0x2B),
        '?' => shifted(0x2C),
        '>' => shifted(0x2F),
        '~' => shifted(0x32),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::KeyChord;

    fn code(spec: &str) -> Option<u16> {
        let chord: KeyChord = spec.parse().ok()?;
        map(chord.key).map(|m| m.code)
    }

    #[test]
    fn maps_common_keys() {
        assert_eq!(code("Return"), Some(RETURN));
        assert_eq!(code("super+c"), Some(0x08));
        assert_eq!(code("KP_0"), Some(0x52));
        assert_eq!(code("F12"), Some(0x6F));
        assert_eq!(code("F24"), None);
        assert!(map(Key::Char('+')).is_some_and(|m| m.implied == Modifiers::SHIFT));
        assert!(map(Key::Char('é')).is_none());
    }

    #[test]
    fn builds_flags() {
        assert_eq!(flags(Modifiers::META | Modifiers::SHIFT), CGEventFlags::MaskCommand | CGEventFlags::MaskShift);
    }
}
