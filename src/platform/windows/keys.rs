use windows::Win32::UI::Input::KeyboardAndMouse::{
    VIRTUAL_KEY, VK_ADD, VK_APPS, VK_BACK, VK_CAPITAL, VK_CONTROL, VK_DECIMAL, VK_DELETE, VK_DIVIDE, VK_DOWN, VK_END,
    VK_ESCAPE, VK_F1, VK_HOME, VK_INSERT, VK_LEFT, VK_LWIN, VK_MENU, VK_MULTIPLY, VK_NEXT, VK_NUMPAD0, VK_PRIOR,
    VK_RETURN, VK_RIGHT, VK_SHIFT, VK_SPACE, VK_SUBTRACT, VK_TAB, VK_UP, VkKeyScanW,
};

use crate::error::{Error, Result};
use crate::keys::{Key, KeyChord, KeypadKey, Modifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct VirtualKey {
    pub(super) vk: u16,
    pub(super) extended: bool,
}

impl VirtualKey {
    const fn plain(vk: VIRTUAL_KEY) -> Self {
        Self { vk: vk.0, extended: false }
    }

    const fn extended(vk: VIRTUAL_KEY) -> Self {
        Self { vk: vk.0, extended: true }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Stroke {
    Keys { modifiers: Vec<VirtualKey>, key: VirtualKey, text: Option<char> },
    Unicode(char),
}

pub(super) fn resolve(chord: &KeyChord) -> Result<Stroke> {
    let mut modifiers = modifier_keys(chord.modifiers);
    let key = match chord.key {
        Key::Modifier(only) => {
            let mut keys = modifier_keys(only);
            let key = keys.pop().ok_or_else(|| Error::invalid("empty modifier key"))?;
            for extra in keys {
                if !modifiers.contains(&extra) {
                    modifiers.push(extra);
                }
            }
            return Ok(Stroke::Keys { modifiers, key, text: None });
        }
        Key::Keypad(KeypadKey::Equal) => Key::Char('='),
        other => other,
    };
    if let Some(vk) = named_key(key) {
        let text = match key {
            Key::Char(c) if chord.modifiers.is_empty() => Some(c),
            _ => None,
        };
        return Ok(Stroke::Keys { modifiers, key: vk, text });
    }
    let Key::Char(c) = key else {
        return Err(Error::Unsupported(format!("{chord} has no Windows key mapping")));
    };
    match layout_key(c) {
        Some((vk, implied)) => {
            for extra in modifier_keys(implied) {
                if !modifiers.contains(&extra) {
                    modifiers.push(extra);
                }
            }
            let text = chord.modifiers.is_empty().then_some(c);
            Ok(Stroke::Keys { modifiers, key: VirtualKey::plain(VIRTUAL_KEY(vk)), text })
        }
        None if chord.modifiers.is_empty() => Ok(Stroke::Unicode(c)),
        None => Err(Error::Unsupported(format!("{c:?} has no key on the current keyboard layout"))),
    }
}

fn layout_key(c: char) -> Option<(u16, Modifiers)> {
    let mut units = [0u16; 2];
    let [unit] = c.encode_utf16(&mut units) else {
        return None;
    };
    // SAFETY: plain FFI call taking a UTF-16 code unit by value.
    let scan = unsafe { VkKeyScanW(*unit) };
    decode_vk_scan(scan)
}

fn decode_vk_scan(scan: i16) -> Option<(u16, Modifiers)> {
    if scan == -1 {
        return None;
    }
    let [vk, shift_state] = scan.to_le_bytes();
    let mut modifiers = Modifiers::empty();
    modifiers.set(Modifiers::SHIFT, shift_state & 1 != 0);
    modifiers.set(Modifiers::CONTROL, shift_state & 2 != 0);
    modifiers.set(Modifiers::ALT, shift_state & 4 != 0);
    Some((u16::from(vk), modifiers))
}

pub(super) fn modifier_keys(modifiers: Modifiers) -> Vec<VirtualKey> {
    [
        (Modifiers::CONTROL, VirtualKey::plain(VK_CONTROL)),
        (Modifiers::ALT, VirtualKey::plain(VK_MENU)),
        (Modifiers::SHIFT, VirtualKey::plain(VK_SHIFT)),
        (Modifiers::META, VirtualKey::extended(VK_LWIN)),
    ]
    .into_iter()
    .filter(|(flag, _)| modifiers.contains(*flag))
    .map(|(_, key)| key)
    .collect()
}

pub(super) fn named_key(key: Key) -> Option<VirtualKey> {
    let plain = VirtualKey::plain;
    let extended = VirtualKey::extended;
    Some(match key {
        Key::Char(c @ ('a'..='z' | '0'..='9')) => {
            VirtualKey { vk: u16::from(u8::try_from(c.to_ascii_uppercase()).ok()?), extended: false }
        }
        Key::Char(' ') | Key::Space => plain(VK_SPACE),
        Key::Return => plain(VK_RETURN),
        Key::Tab => plain(VK_TAB),
        Key::Escape => plain(VK_ESCAPE),
        Key::Backspace => plain(VK_BACK),
        Key::Delete => extended(VK_DELETE),
        Key::Insert => extended(VK_INSERT),
        Key::Up => extended(VK_UP),
        Key::Down => extended(VK_DOWN),
        Key::Left => extended(VK_LEFT),
        Key::Right => extended(VK_RIGHT),
        Key::Home => extended(VK_HOME),
        Key::End => extended(VK_END),
        Key::PageUp => extended(VK_PRIOR),
        Key::PageDown => extended(VK_NEXT),
        Key::CapsLock => plain(VK_CAPITAL),
        Key::Menu => extended(VK_APPS),
        Key::F(n @ 1..=24) => VirtualKey { vk: VK_F1.0 + u16::from(n - 1), extended: false },
        Key::Char(_) | Key::Modifier(_) | Key::F(_) => return None,
        Key::Keypad(pad) => match pad {
            KeypadKey::Digit(d @ 0..=9) => VirtualKey { vk: VK_NUMPAD0.0 + u16::from(d), extended: false },
            KeypadKey::Digit(_) | KeypadKey::Equal => return None,
            KeypadKey::Enter => extended(VK_RETURN),
            KeypadKey::Multiply => plain(VK_MULTIPLY),
            KeypadKey::Add => plain(VK_ADD),
            KeypadKey::Subtract => plain(VK_SUBTRACT),
            KeypadKey::Decimal => plain(VK_DECIMAL),
            KeypadKey::Divide => extended(VK_DIVIDE),
        },
    })
}

#[cfg(test)]
mod tests {
    use windows::Win32::UI::Input::KeyboardAndMouse::{VK_F24, VK_NUMPAD7};

    use super::*;

    #[test]
    fn maps_named_keys() {
        assert_eq!(named_key(Key::Char('a')), Some(VirtualKey { vk: u16::from(b'A'), extended: false }));
        assert_eq!(named_key(Key::Char('7')), Some(VirtualKey { vk: u16::from(b'7'), extended: false }));
        assert_eq!(named_key(Key::Return), Some(VirtualKey::plain(VK_RETURN)));
        assert_eq!(named_key(Key::Left), Some(VirtualKey::extended(VK_LEFT)));
        assert_eq!(named_key(Key::F(24)), Some(VirtualKey::plain(VK_F24)));
        assert_eq!(named_key(Key::F(25)), None);
        assert_eq!(named_key(Key::Keypad(KeypadKey::Digit(7))), Some(VirtualKey::plain(VK_NUMPAD7)));
        assert_eq!(named_key(Key::Keypad(KeypadKey::Enter)), Some(VirtualKey::extended(VK_RETURN)));
        assert_eq!(named_key(Key::Char(';')), None);
    }

    #[test]
    fn orders_modifiers() {
        let keys = modifier_keys(Modifiers::SHIFT | Modifiers::CONTROL | Modifiers::FUNCTION);
        assert_eq!(keys, vec![VirtualKey::plain(VK_CONTROL), VirtualKey::plain(VK_SHIFT)]);
    }

    #[test]
    fn decodes_layout_lookups() {
        assert_eq!(decode_vk_scan(-1), None);
        assert_eq!(decode_vk_scan(0x0131), Some((0x31, Modifiers::SHIFT)));
        // AltGr combinations report Ctrl+Alt.
        assert_eq!(decode_vk_scan(0x0651), Some((0x51, Modifiers::CONTROL | Modifiers::ALT)));
    }

    #[test]
    fn resolves_chords() -> Result<()> {
        let chord: KeyChord = "ctrl+shift+t".parse()?;
        let Stroke::Keys { modifiers, key, text } = resolve(&chord)? else {
            panic!("expected keys");
        };
        assert_eq!(modifiers, vec![VirtualKey::plain(VK_CONTROL), VirtualKey::plain(VK_SHIFT)]);
        assert_eq!(key.vk, u16::from(b'T'));
        assert_eq!(text, None);

        let alone = resolve(&KeyChord::new(Key::Modifier(Modifiers::SHIFT)))?;
        assert_eq!(alone, Stroke::Keys { modifiers: vec![], key: VirtualKey::plain(VK_SHIFT), text: None });

        let plain = resolve(&KeyChord::new(Key::Char('q')))?;
        assert_eq!(
            plain,
            Stroke::Keys {
                modifiers: vec![],
                key: VirtualKey { vk: u16::from(b'Q'), extended: false },
                text: Some('q')
            }
        );
        Ok(())
    }
}
