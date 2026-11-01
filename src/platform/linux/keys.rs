use crate::error::{Error, Result};
use crate::keys::{Key, KeyChord, KeypadKey, Modifiers};

pub(crate) const XK_RETURN: u32 = 0xff0d;
pub(crate) const XK_TAB: u32 = 0xff09;

pub(crate) fn keysym(key: Key) -> Result<u32> {
    Ok(match key {
        Key::Char(c) => char_keysym(c)?,
        Key::Return => XK_RETURN,
        Key::Tab => XK_TAB,
        Key::Space => 0x0020,
        Key::Escape => 0xff1b,
        Key::Backspace => 0xff08,
        Key::Delete => 0xffff,
        Key::Insert => 0xff63,
        Key::Up => 0xff52,
        Key::Down => 0xff54,
        Key::Left => 0xff51,
        Key::Right => 0xff53,
        Key::Home => 0xff50,
        Key::End => 0xff57,
        Key::PageUp => 0xff55,
        Key::PageDown => 0xff56,
        Key::CapsLock => 0xffe5,
        Key::Menu => 0xff67,
        Key::F(n @ 1..=35) => 0xffbe + u32::from(n - 1),
        Key::F(n) => return Err(Error::Unsupported(format!("function key F{n} has no X11 keysym"))),
        Key::Keypad(pad) => keypad_keysym(pad)?,
        Key::Modifier(modifiers) => modifier_keysym(modifiers)?,
    })
}

pub(crate) fn char_keysym(c: char) -> Result<u32> {
    let code = u32::from(c);
    match code {
        0x20..=0x7e | 0xa0..=0xff => Ok(code),
        _ if c.is_control() => Err(Error::Unsupported(format!("control character {c:?} has no X11 keysym"))),
        _ => Ok(0x0100_0000 | code),
    }
}

fn keypad_keysym(key: KeypadKey) -> Result<u32> {
    Ok(match key {
        KeypadKey::Digit(d @ 0..=9) => 0xffb0 + u32::from(d),
        KeypadKey::Digit(d) => return Err(Error::Unsupported(format!("keypad digit {d} does not exist"))),
        KeypadKey::Enter => 0xff8d,
        KeypadKey::Equal => 0xffbd,
        KeypadKey::Multiply => 0xffaa,
        KeypadKey::Add => 0xffab,
        KeypadKey::Subtract => 0xffad,
        KeypadKey::Decimal => 0xffae,
        KeypadKey::Divide => 0xffaf,
    })
}

fn modifier_keysym(modifiers: Modifiers) -> Result<u32> {
    const TABLE: [(Modifiers, u32); 4] =
        [(Modifiers::SHIFT, 0xffe1), (Modifiers::CONTROL, 0xffe3), (Modifiers::ALT, 0xffe9), (Modifiers::META, 0xffeb)];
    TABLE
        .iter()
        .find(|(flag, _)| modifiers.contains(*flag))
        .map(|(_, sym)| *sym)
        .ok_or_else(|| Error::Unsupported(format!("modifier {modifiers:?} has no X11 keysym")))
}

pub(crate) fn modifier_mask(modifiers: Modifiers) -> i32 {
    const TABLE: [(Modifiers, i32); 4] =
        [(Modifiers::SHIFT, 1 << 0), (Modifiers::CONTROL, 1 << 2), (Modifiers::ALT, 1 << 3), (Modifiers::META, 1 << 6)];
    TABLE.iter().filter(|(flag, _)| modifiers.contains(*flag)).fold(0, |mask, (_, bit)| mask | bit)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum KeyStroke {
    Text(String),
    Sym(u32),
}

pub(crate) fn resolve(chord: &KeyChord) -> Result<(i32, KeyStroke)> {
    let mask = modifier_mask(chord.modifiers);
    let stroke = match chord.key {
        Key::Char(c) if mask == 0 => {
            char_keysym(c)?;
            KeyStroke::Text(c.to_string())
        }
        key => KeyStroke::Sym(keysym(key)?),
    };
    Ok((mask, stroke))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_named_keys() -> Result<()> {
        assert_eq!(keysym(Key::Return)?, 0xff0d);
        assert_eq!(keysym(Key::Backspace)?, 0xff08);
        assert_eq!(keysym(Key::PageUp)?, 0xff55);
        assert_eq!(keysym(Key::PageDown)?, 0xff56);
        assert_eq!(keysym(Key::F(1))?, 0xffbe);
        assert_eq!(keysym(Key::F(12))?, 0xffc9);
        assert_eq!(keysym(Key::Keypad(KeypadKey::Digit(7)))?, 0xffb7);
        assert_eq!(keysym(Key::Modifier(Modifiers::META))?, 0xffeb);
        assert!(keysym(Key::F(0)).is_err());
        Ok(())
    }

    #[test]
    fn maps_characters() -> Result<()> {
        assert_eq!(char_keysym('a')?, 0x61);
        assert_eq!(char_keysym('/')?, 0x2f);
        assert_eq!(char_keysym('é')?, 0xe9);
        assert_eq!(char_keysym('€')?, 0x0100_20ac);
        assert!(char_keysym('\u{7}').is_err());
        Ok(())
    }

    #[test]
    fn resolves_chords() -> Result<()> {
        let chord: KeyChord = "ctrl+shift+t".parse()?;
        assert_eq!(resolve(&chord)?, (0b101, KeyStroke::Sym(0x74)));
        let chord: KeyChord = "super+Return".parse()?;
        assert_eq!(resolve(&chord)?, (1 << 6, KeyStroke::Sym(0xff0d)));
        let chord: KeyChord = "x".parse()?;
        assert_eq!(resolve(&chord)?, (0, KeyStroke::Text("x".into())));
        Ok(())
    }
}
