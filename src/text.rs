//! Locating a piece of text inside an element's value for `select_text`.

use std::ops::Range;

use crate::error::{Error, Result};

/// What `select_text` should do with the matched text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum SelectionMode {
    /// Select the matched text.
    #[default]
    Text,
    /// Place the caret immediately before it.
    CursorBefore,
    /// Place the caret immediately after it.
    CursorAfter,
}

/// A request to select (or place the caret around) some text inside an element.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TextSelection {
    /// The text to find, exactly as it appears in the element's value.
    pub text: String,
    /// Text that must appear immediately before the match, to disambiguate repeats.
    pub prefix: Option<String>,
    /// Text that must appear immediately after the match, to disambiguate repeats.
    pub suffix: Option<String>,
    /// What to do with the match.
    pub mode: SelectionMode,
}

impl TextSelection {
    /// Selects `text`.
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into(), ..Self::default() }
    }

    /// Requires `prefix` immediately before the match.
    #[must_use]
    pub fn prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = Some(prefix.into());
        self
    }

    /// Requires `suffix` immediately after the match.
    #[must_use]
    pub fn suffix(mut self, suffix: impl Into<String>) -> Self {
        self.suffix = Some(suffix.into());
        self
    }

    /// Sets the selection mode.
    #[must_use]
    pub const fn mode(mut self, mode: SelectionMode) -> Self {
        self.mode = mode;
        self
    }

    /// Resolves the request against `value`, returning a byte range to select.
    ///
    /// A collapsed range (`start == end`) places the caret.
    ///
    /// # Errors
    ///
    /// Fails when the text is empty, absent, or occurs more than once after applying the
    /// prefix and suffix constraints.
    pub fn resolve(&self, value: &str) -> Result<Range<usize>> {
        if self.text.is_empty() {
            return Err(Error::invalid("text to select must not be empty"));
        }
        let prefix = self.prefix.as_deref().unwrap_or("");
        let suffix = self.suffix.as_deref().unwrap_or("");
        let mut matches = value.match_indices(&self.text).filter_map(|(start, m)| {
            let end = start + m.len();
            (value[..start].ends_with(prefix) && value[end..].starts_with(suffix)).then_some(start..end)
        });
        let found = matches
            .next()
            .ok_or_else(|| Error::invalid(format!("text {:?} was not found in the element's value", self.text)))?;
        if matches.next().is_some() {
            return Err(Error::invalid(format!(
                "text {:?} occurs more than once; add a prefix or suffix to disambiguate",
                self.text
            )));
        }
        Ok(match self.mode {
            SelectionMode::Text => found,
            SelectionMode::CursorBefore => found.start..found.start,
            SelectionMode::CursorAfter => found.end..found.end,
        })
    }
}

/// Converts a byte offset in `value` to a UTF-16 code-unit offset.
#[must_use]
pub fn utf16_offset(value: &str, byte_offset: usize) -> usize {
    value[..byte_offset].encode_utf16().count()
}

/// Converts a byte offset in `value` to a `char` (Unicode scalar) offset.
#[must_use]
pub fn char_offset(value: &str, byte_offset: usize) -> usize {
    value[..byte_offset].chars().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_unique_match() -> Result<()> {
        assert_eq!(TextSelection::new("world").resolve("hello world")?, 6..11);
        let before = TextSelection::new("world").mode(SelectionMode::CursorBefore);
        assert_eq!(before.resolve("hello world")?, 6..6);
        let after = TextSelection::new("hello").mode(SelectionMode::CursorAfter);
        assert_eq!(after.resolve("hello world")?, 5..5);
        Ok(())
    }

    #[test]
    fn disambiguates_with_prefix_and_suffix() -> Result<()> {
        let value = "cat dog cat bird";
        assert!(TextSelection::new("cat").resolve(value).is_err());
        assert_eq!(TextSelection::new("cat").prefix("dog ").resolve(value)?, 8..11);
        assert_eq!(TextSelection::new("cat").suffix(" dog").resolve(value)?, 0..3);
        Ok(())
    }

    #[test]
    fn reports_missing_text() {
        assert!(TextSelection::new("zebra").resolve("cat").is_err());
        assert!(TextSelection::new("").resolve("cat").is_err());
    }

    #[test]
    fn converts_offsets() {
        let value = "a😀b";
        assert_eq!(utf16_offset(value, 5), 3);
        assert_eq!(char_offset(value, 5), 2);
    }
}
