//! Fixed-size Doom lump names.

use crate::error::CyberError;

/// Longest a lump name may be, in characters.
pub const MAX_NAME_LEN: usize = 8;

/// A Doom lump name: 1 to [`MAX_NAME_LEN`] characters from the lump
/// alphabet ([`LumpName::is_valid_char`]).
///
/// Names are stored exactly as parsed; uppercasing is the caller's choice
/// (classic WADs spell every name in uppercase, ports accept mixed case).
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct LumpName {
    /// The validated name characters.
    name: String,
}

impl LumpName {
    /// Parse a lump name, validating length and alphabet.
    ///
    /// # Errors
    ///
    /// [`CyberError::InvalidArgument`] when `name` is empty, longer than
    /// [`MAX_NAME_LEN`] characters, or holds a character outside the lump
    /// alphabet.
    #[inline]
    pub fn parse(name: &str) -> Result<Self, CyberError> {
        if name.is_empty() {
            return Err(CyberError::InvalidArgument(
                "lump name must not be empty".to_owned(),
            ));
        }
        if name.chars().count() > MAX_NAME_LEN {
            return Err(CyberError::InvalidArgument(format!(
                "lump name {name} is longer than {MAX_NAME_LEN} characters"
            )));
        }
        if let Some(invalid) = name.chars().find(|ch| !Self::is_valid_char(*ch)) {
            return Err(CyberError::InvalidArgument(format!(
                "lump name {name} contains {invalid}, which is outside the \
                 alphabet [A-Za-z0-9_]; rename the source or pass an explicit name"
            )));
        }
        Ok(Self {
            name: name.to_owned(),
        })
    }

    /// Whether `ch` may appear in a lump name: ASCII letters, digits, `_`.
    #[must_use]
    #[inline]
    pub const fn is_valid_char(ch: char) -> bool {
        ch.is_ascii_alphanumeric() || ch == '_'
    }

    /// The name as parsed.
    #[must_use]
    #[inline]
    pub fn as_str(&self) -> &str {
        &self.name
    }
}

#[cfg(test)]
mod tests {
    use super::{LumpName, MAX_NAME_LEN};

    #[test]
    fn parse_accepts_classic_names() {
        for name in ["E1M1", "TITLEPIC", "D_RUNNIN", "flat5", "f77", "PLAYPAL8"] {
            let parsed =
                LumpName::parse(name).unwrap_or_else(|error| panic!("{name} must parse: {error}"));
            assert_eq!(parsed.as_str(), name, "name kept verbatim");
        }
    }

    #[test]
    fn parse_accepts_the_maximum_length_and_rejects_longer() {
        let max = "a".repeat(MAX_NAME_LEN);
        assert!(LumpName::parse(&max).is_ok(), "{max} is exactly legal");
        let longer = format!("{max}b");
        let error = LumpName::parse(&longer).expect_err("nine characters must fail");
        assert_eq!(error.code(), "invalid_argument", "length error code");
    }

    #[test]
    fn parse_rejects_empty_and_invalid_characters() {
        let empty = LumpName::parse("").expect_err("empty names are rejected");
        assert_eq!(empty.code(), "invalid_argument", "empty error code");
        for name in ["HAS SPACE", "DOT.NAME", "SLASH/", "\u{dc}LTRA"] {
            let error = LumpName::parse(name).expect_err("name must be rejected");
            assert_eq!(error.code(), "invalid_argument", "alphabet error code");
        }
    }

    #[test]
    fn is_valid_char_covers_the_alphabet() {
        for ch in ['A', 'Z', 'a', 'z', '0', '9', '_'] {
            assert!(LumpName::is_valid_char(ch), "{ch} is a lump character");
        }
        for ch in [' ', '.', '/', '-', '!', '\0', '\u{e9}'] {
            assert!(!LumpName::is_valid_char(ch), "{ch} is not");
        }
    }
}
