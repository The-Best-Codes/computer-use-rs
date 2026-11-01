use super::ax::{self, AxElement};
use crate::keys::{Key, KeyChord, Modifiers};

const SHIFT: i64 = 1 << 0;
const OPTION: i64 = 1 << 1;
const CONTROL: i64 = 1 << 2;
const NO_COMMAND: i64 = 1 << 3;

const MAX_DEPTH: usize = 6;

pub(super) fn shortcut(chord: &KeyChord) -> Option<(String, i64)> {
    if !chord.modifiers.contains(Modifiers::META) {
        return None;
    }
    let Key::Char(c) = chord.key else { return None };
    let mut mask = 0;
    for (flag, bit) in [(Modifiers::SHIFT, SHIFT), (Modifiers::ALT, OPTION), (Modifiers::CONTROL, CONTROL)] {
        if chord.modifiers.contains(flag) {
            mask |= bit;
        }
    }
    Some((c.to_uppercase().collect(), mask))
}

pub(super) fn find_item(app: &AxElement, wanted: &(String, i64)) -> Option<AxElement> {
    find(&ax::element(app, "AXMenuBar")?, wanted, 0)
}

fn find(element: &AxElement, wanted: &(String, i64), depth: usize) -> Option<AxElement> {
    if depth >= MAX_DEPTH {
        return None;
    }
    for child in ax::elements(element, "AXChildren") {
        if ax::string(&child, "AXRole").as_deref() == Some("AXMenuItem") && matches(&child, wanted) {
            return Some(child);
        }
        if let Some(found) = find(&child, wanted, depth + 1) {
            return Some(found);
        }
    }
    None
}

fn matches(item: &AxElement, (key, mask): &(String, i64)) -> bool {
    let Some(cmd_char) = ax::string(item, "AXMenuItemCmdChar") else { return false };
    if cmd_char.to_uppercase() != *key || ax::boolean(item, "AXEnabled") == Some(false) {
        return false;
    }
    let modifiers = ax::attribute(item, "AXMenuItemCmdModifiers").as_deref().and_then(ax::as_number);
    #[allow(clippy::cast_possible_truncation)] // a small bitmask
    let modifiers = modifiers.map_or(0, |m| m as i64);
    modifiers & NO_COMMAND == 0 && modifiers == *mask
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_shortcuts() -> crate::Result<()> {
        assert_eq!(shortcut(&"super+shift+s".parse()?), Some(("S".to_owned(), SHIFT)));
        assert_eq!(shortcut(&"cmd+a".parse()?), Some(("A".to_owned(), 0)));
        assert_eq!(shortcut(&"ctrl+a".parse()?), None);
        assert_eq!(shortcut(&"super+Return".parse()?), None);
        Ok(())
    }
}
