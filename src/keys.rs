//! Platform-neutral key chords parsed from xdotool-style specifications.
//!
//! ```
//! use computer_use::keys::{Key, KeyChord, Modifiers};
//!
//! let chord: KeyChord = "super+shift+t".parse()?;
//! assert_eq!(chord.modifiers, Modifiers::META | Modifiers::SHIFT);
//! assert_eq!(chord.key, Key::Char('t'));
//! # Ok::<(), computer_use::Error>(())
//! ```

use std::fmt;
use std::str::FromStr;

use crate::error::Error;

bitflags::bitflags! {
    /// Modifier keys held during a key press.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
    pub struct Modifiers: u8 {
        /// Shift.
        const SHIFT = 1 << 0;
        /// Control.
        const CONTROL = 1 << 1;
        /// Alt / Option.
        const ALT = 1 << 2;
        /// Command on macOS, the Windows key on Windows, Super on Linux.
        const META = 1 << 3;
        /// Fn (macOS only; ignored elsewhere).
        const FUNCTION = 1 << 4;
    }
}

/// A key on a numeric keypad.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum KeypadKey {
    /// Digits 0–9.
    Digit(u8),
    /// Keypad Enter.
    Enter,
    /// `=`.
    Equal,
    /// `*`.
    Multiply,
    /// `+`.
    Add,
    /// `-`.
    Subtract,
    /// `.`.
    Decimal,
    /// `/`.
    Divide,
}

/// A single physical or logical key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Key {
    /// A printable character (letters are stored lowercase).
    Char(char),
    /// Return / Enter.
    Return,
    /// Tab.
    Tab,
    /// Space bar.
    Space,
    /// Escape.
    Escape,
    /// Backspace (macOS "delete").
    Backspace,
    /// Forward delete.
    Delete,
    /// Insert (Help on macOS).
    Insert,
    /// Arrow up.
    Up,
    /// Arrow down.
    Down,
    /// Arrow left.
    Left,
    /// Arrow right.
    Right,
    /// Home.
    Home,
    /// End.
    End,
    /// Page up.
    PageUp,
    /// Page down.
    PageDown,
    /// Caps lock.
    CapsLock,
    /// Context menu key.
    Menu,
    /// Function keys F1–F24.
    F(u8),
    /// Numeric keypad keys.
    Keypad(KeypadKey),
    /// A modifier pressed on its own.
    Modifier(Modifiers),
}

/// A key plus the modifiers held while pressing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyChord {
    /// Held modifiers.
    pub modifiers: Modifiers,
    /// The key pressed.
    pub key: Key,
}

impl KeyChord {
    /// A chord without modifiers.
    #[must_use]
    pub const fn new(key: Key) -> Self {
        Self { modifiers: Modifiers::empty(), key }
    }

    /// Adds modifiers to the chord.
    #[must_use]
    pub const fn with(mut self, modifiers: Modifiers) -> Self {
        self.modifiers = self.modifiers.union(modifiers);
        self
    }
}

impl FromStr for KeyChord {
    type Err = Error;

    fn from_str(spec: &str) -> Result<Self, Self::Err> {
        parse_chord(spec)
    }
}

impl fmt::Display for KeyChord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (flag, name) in [
            (Modifiers::CONTROL, "ctrl"),
            (Modifiers::ALT, "alt"),
            (Modifiers::SHIFT, "shift"),
            (Modifiers::META, "super"),
            (Modifiers::FUNCTION, "fn"),
        ] {
            if self.modifiers.contains(flag) {
                write!(f, "{name}+")?;
            }
        }
        match self.key {
            Key::Char(c) => write!(f, "{c}"),
            Key::F(n) => write!(f, "F{n}"),
            Key::Keypad(KeypadKey::Digit(d)) => write!(f, "KP_{d}"),
            Key::Keypad(k) => write!(f, "KP_{k:?}"),
            Key::Modifier(m) => write!(f, "{m:?}"),
            other => write!(f, "{other:?}"),
        }
    }
}

fn parse_chord(spec: &str) -> Result<KeyChord, Error> {
    let spec = spec.trim();
    if spec.is_empty() {
        return Err(Error::invalid("key specification is empty"));
    }
    let (body, trailing_plus) = match spec.strip_suffix("++") {
        Some(rest) => (rest, true),
        None if spec == "+" => ("", true),
        None => (spec, false),
    };
    let mut tokens: Vec<&str> = body.split('+').map(str::trim).filter(|t| !t.is_empty()).collect();
    let key_token = if trailing_plus {
        "+"
    } else {
        tokens.pop().ok_or_else(|| Error::invalid(format!("unsupported key '{spec}'")))?
    };

    let mut modifiers = Modifiers::empty();
    for token in tokens {
        modifiers |= parse_modifier(token)
            .ok_or_else(|| Error::invalid(format!("unsupported modifier '{token}' in '{spec}'")))?;
    }
    let key = parse_key(key_token).ok_or_else(|| Error::invalid(format!("unsupported key '{spec}'")))?;
    if let Key::Char(c) = key
        && key_token.chars().next().is_some_and(char::is_uppercase)
        && key_token.chars().count() == 1
    {
        return Ok(KeyChord { modifiers: modifiers | Modifiers::SHIFT, key: Key::Char(c) });
    }
    Ok(KeyChord { modifiers, key })
}

fn parse_modifier(token: &str) -> Option<Modifiers> {
    Some(match token.to_ascii_lowercase().as_str() {
        "shift" | "shift_l" | "shift_r" => Modifiers::SHIFT,
        "ctrl" | "control" | "control_l" | "control_r" | "ctrl_l" | "ctrl_r" => Modifiers::CONTROL,
        "alt" | "option" | "opt" | "alt_l" | "alt_r" | "mod1" => Modifiers::ALT,
        "super" | "cmd" | "command" | "meta" | "win" | "windows" | "super_l" | "super_r" | "meta_l" | "meta_r"
        | "mod4" => Modifiers::META,
        "fn" | "function" => Modifiers::FUNCTION,
        _ => return None,
    })
}

fn parse_key(token: &str) -> Option<Key> {
    let mut chars = token.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        return Some(match c {
            ' ' => Key::Space,
            '\n' | '\r' => Key::Return,
            '\t' => Key::Tab,
            c => Key::Char(c.to_lowercase().next().unwrap_or(c)),
        });
    }
    if let Some(m) = parse_modifier(token) {
        return Some(Key::Modifier(m));
    }
    let lower = token.to_ascii_lowercase().replace([' ', '-'], "_");
    if let Some(n) = lower.strip_prefix('f').and_then(|n| n.parse::<u8>().ok())
        && (1..=24).contains(&n)
    {
        return Some(Key::F(n));
    }
    if let Some(rest) = lower.strip_prefix("kp_") {
        return parse_keypad(rest);
    }
    Some(match lower.as_str() {
        "return" | "enter" => Key::Return,
        "tab" | "iso_left_tab" => Key::Tab,
        "space" | "spacebar" => Key::Space,
        "escape" | "esc" => Key::Escape,
        "backspace" => Key::Backspace,
        "delete" | "del" | "forwarddelete" | "forward_delete" => Key::Delete,
        "insert" | "ins" | "help" => Key::Insert,
        "up" | "arrowup" | "arrow_up" | "uparrow" => Key::Up,
        "down" | "arrowdown" | "arrow_down" | "downarrow" => Key::Down,
        "left" | "arrowleft" | "arrow_left" | "leftarrow" => Key::Left,
        "right" | "arrowright" | "arrow_right" | "rightarrow" => Key::Right,
        "home" => Key::Home,
        "end" => Key::End,
        "page_up" | "pageup" | "prior" => Key::PageUp,
        "page_down" | "pagedown" | "next" => Key::PageDown,
        "caps_lock" | "capslock" => Key::CapsLock,
        "menu" | "apps" | "contextmenu" => Key::Menu,
        _ => Key::Char(keysym_char(&lower)?),
    })
}

fn parse_keypad(rest: &str) -> Option<Key> {
    if let Ok(d) = rest.parse::<u8>()
        && d <= 9
    {
        return Some(Key::Keypad(KeypadKey::Digit(d)));
    }
    Some(match rest {
        "enter" => Key::Keypad(KeypadKey::Enter),
        "equal" => Key::Keypad(KeypadKey::Equal),
        "multiply" => Key::Keypad(KeypadKey::Multiply),
        "add" => Key::Keypad(KeypadKey::Add),
        "subtract" => Key::Keypad(KeypadKey::Subtract),
        "decimal" | "delete" => Key::Keypad(KeypadKey::Decimal),
        "divide" => Key::Keypad(KeypadKey::Divide),
        "home" => Key::Home,
        "end" => Key::End,
        "left" => Key::Left,
        "right" => Key::Right,
        "up" => Key::Up,
        "down" => Key::Down,
        "prior" | "page_up" => Key::PageUp,
        "next" | "page_down" => Key::PageDown,
        "insert" => Key::Insert,
        _ => return None,
    })
}

fn keysym_char(name: &str) -> Option<char> {
    Some(match name {
        "minus" => '-',
        "equal" => '=',
        "plus" => '+',
        "bracketleft" => '[',
        "bracketright" => ']',
        "braceleft" => '{',
        "braceright" => '}',
        "backslash" => '\\',
        "bar" => '|',
        "semicolon" => ';',
        "colon" => ':',
        "apostrophe" | "quoteright" => '\'',
        "quotedbl" => '"',
        "grave" | "quoteleft" => '`',
        "asciitilde" => '~',
        "comma" => ',',
        "period" => '.',
        "slash" => '/',
        "question" => '?',
        "less" => '<',
        "greater" => '>',
        "exclam" => '!',
        "at" => '@',
        "numbersign" => '#',
        "dollar" => '$',
        "percent" => '%',
        "asciicircum" => '^',
        "ampersand" => '&',
        "asterisk" => '*',
        "parenleft" => '(',
        "parenright" => ')',
        "underscore" => '_',
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chord(spec: &str) -> KeyChord {
        spec.parse().unwrap_or_else(|e| panic!("{spec}: {e}"))
    }

    #[test]
    fn parses_xdotool_names() {
        assert_eq!(chord("Return").key, Key::Return);
        assert_eq!(chord("BackSpace").key, Key::Backspace);
        assert_eq!(chord("KP_0").key, Key::Keypad(KeypadKey::Digit(0)));
        assert_eq!(chord("Page_Up").key, Key::PageUp);
        assert_eq!(chord("Next").key, Key::PageDown);
        assert_eq!(chord("F12").key, Key::F(12));
        assert_eq!(chord("bracketleft").key, Key::Char('['));
        assert_eq!(chord("space").key, Key::Space);
    }

    #[test]
    fn parses_modifiers() {
        let c = chord("super+c");
        assert_eq!(c.modifiers, Modifiers::META);
        assert_eq!(c.key, Key::Char('c'));
        let c = chord("ctrl + shift + Tab");
        assert_eq!(c.modifiers, Modifiers::CONTROL | Modifiers::SHIFT);
        assert_eq!(c.key, Key::Tab);
        assert_eq!(chord("cmd+A").modifiers, Modifiers::META | Modifiers::SHIFT);
        assert_eq!(chord("shift").key, Key::Modifier(Modifiers::SHIFT));
    }

    #[test]
    fn parses_plus_key() {
        let c = chord("ctrl++");
        assert_eq!((c.modifiers, c.key), (Modifiers::CONTROL, Key::Char('+')));
        assert_eq!(chord("+").key, Key::Char('+'));
        assert_eq!(chord("ctrl+plus").key, Key::Char('+'));
    }

    #[test]
    fn rejects_garbage() {
        assert!("".parse::<KeyChord>().is_err());
        assert!("hyper+x".parse::<KeyChord>().is_err());
        assert!("NotAKey".parse::<KeyChord>().is_err());
        assert!("F25".parse::<KeyChord>().is_err());
    }

    #[test]
    fn display_round_trips() {
        for spec in ["ctrl+shift+t", "super+F5", "alt+KP_3", "Return"] {
            let c = chord(spec);
            assert_eq!(chord(&c.to_string()), c, "{spec} -> {c}");
        }
    }
}
