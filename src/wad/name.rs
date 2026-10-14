//! Eight-character `Doom` lump names, stored uppercase and `NUL`-padded.
//!
//! The WAD directory keeps every lump name in exactly eight bytes, `NUL`
//! padded after the last character, and engines compare names
//! case-insensitively by uppercasing the `ASCII` letters. [`LumpName`]
//! holds those rules as an invariant, so derived equality, ordering and
//! hashing match what `Doom` does. [`LumpName::from_raw`] is the lenient
//! path for bytes read from files; [`LumpName::parse`] is the strict path
//! for user input.

use core::fmt::{self, Debug, Display, Formatter, Write as _};

use serde::{Serialize, Serializer};

use crate::error::CyberError;

/// Bytes a lump name occupies in the WAD directory.
pub(crate) const NAME_LEN: usize = 8;
/// Longest a lump name may be, in characters: the width of a directory
/// name entry, published for callers that sanitize user text into names.
pub const MAX_NAME_LEN: usize = NAME_LEN;
/// First `ASCII` graphic byte (`!`), the lowest character `parse` accepts.
const MIN_GRAPHIC: u8 = 0x21;
/// Last `ASCII` graphic byte (`~`), the highest character `parse` accepts.
const MAX_GRAPHIC: u8 = 0x7E;

/// A `Doom` lump name: up to eight bytes, always stored uppercase with
/// `NUL` padding after the last character.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LumpName([u8; NAME_LEN]);

impl LumpName {
    /// Builds a name from the eight raw directory bytes, leniently: bytes
    /// up to the first `NUL` are kept with `ASCII` letters uppercased, and
    /// everything from the first `NUL` on is zeroed. Any non-`NUL` byte is
    /// accepted because real WADs contain odd names.
    #[must_use]
    #[inline]
    pub fn from_raw(raw: [u8; NAME_LEN]) -> Self {
        let end = raw.iter().position(|&byte| byte == 0).unwrap_or(NAME_LEN);
        let mut name = [0_u8; NAME_LEN];
        for (slot, byte) in name
            .iter_mut()
            .zip(raw.iter().map(u8::to_ascii_uppercase))
            .take(end)
        {
            *slot = byte;
        }
        Self(name)
    }

    /// Parses user input into a name, strictly: 1 to 8 `ASCII` graphic
    /// characters (`0x21..=0x7E`), lowercase accepted and uppercased.
    ///
    /// # Errors
    ///
    /// [`CyberError::InvalidArgument`] naming the rejected input when
    /// `text` is empty, longer than eight characters, or holds a space,
    /// control or non-`ASCII` character.
    #[inline]
    pub fn parse(text: &str) -> Result<Self, CyberError> {
        let bytes = text.as_bytes();
        let count = bytes.len();
        if count == 0 {
            return Err(CyberError::InvalidArgument(format!(
                "lump name must hold 1 to {NAME_LEN} characters, {text:?} is empty"
            )));
        }
        if count > NAME_LEN {
            return Err(CyberError::InvalidArgument(format!(
                "lump name {text:?} is {count} characters, the limit is {NAME_LEN}"
            )));
        }
        if let Some(bad) = bytes
            .iter()
            .copied()
            .find(|&byte| !(MIN_GRAPHIC..=MAX_GRAPHIC).contains(&byte))
        {
            return Err(CyberError::InvalidArgument(format!(
                "lump name {text:?} contains byte {bad:#04X}, which is not printable ASCII (0x21 to 0x7E)"
            )));
        }
        let mut name = [0_u8; NAME_LEN];
        for (slot, byte) in name
            .iter_mut()
            .zip(bytes.iter().map(u8::to_ascii_uppercase))
        {
            *slot = byte;
        }
        Ok(Self(name))
    }

    /// Whether `ch` may appear in a lump name *derived* from user text,
    /// such as a filename: `ASCII` letters, digits and `_`. Stricter than
    /// [`LumpName::parse`], which accepts every printable `ASCII` byte;
    /// import tools filter candidate names through this before parsing.
    #[must_use]
    #[inline]
    pub const fn is_valid_char(ch: char) -> bool {
        ch.is_ascii_alphanumeric() || ch == '_'
    }

    /// The name's bytes without the `NUL` padding.
    #[must_use]
    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        let end = self
            .0
            .iter()
            .position(|&byte| byte == 0)
            .unwrap_or(NAME_LEN);
        self.0.get(..end).unwrap_or(&self.0)
    }

    /// The eight directory bytes exactly as stored: uppercase, `NUL`-padded.
    #[must_use]
    #[inline]
    pub const fn raw(self) -> [u8; NAME_LEN] {
        self.0
    }

    /// Character count, without the `NUL` padding.
    #[must_use]
    #[inline]
    pub fn len(self) -> usize {
        self.as_bytes().len()
    }

    /// Whether the name holds no characters (an all-`NUL` entry).
    #[must_use]
    #[inline]
    pub fn is_empty(self) -> bool {
        self.as_bytes().is_empty()
    }

    /// Matches the name against a case-insensitive glob: `*` stands for
    /// any run of characters, `?` for exactly one. The pattern is compared
    /// byte-wise against the name without its `NUL` padding.
    #[must_use]
    #[inline]
    pub fn matches_glob(self, pattern: &str) -> bool {
        glob(self.as_bytes(), pattern.as_bytes())
    }
}

/// The recursive step of [`LumpName::matches_glob`]: whether `name` is
/// matched in full by `pattern`.
fn glob(name: &[u8], pattern: &[u8]) -> bool {
    let Some((first, rest)) = pattern.split_first() else {
        return name.is_empty();
    };
    if *first == b'*' {
        return glob(name, rest)
            || name
                .split_first()
                .is_some_and(|(_, tail)| glob(tail, pattern));
    }
    name.split_first().is_some_and(|(head, tail)| {
        (*first == b'?' || first.eq_ignore_ascii_case(head)) && glob(tail, rest)
    })
}

/// Matches raw name bytes against a case-insensitive glob, the same rule
/// [`LumpName::matches_glob`] applies to a parsed name. For listings whose
/// rows hold plain strings rather than [`LumpName`]s.
#[must_use]
pub(crate) fn glob_match(name: &[u8], pattern: &str) -> bool {
    glob(name, pattern.as_bytes())
}

/// Writes `bytes` to `formatter` unchanged where printable, escaping every
/// other byte as `\xNN` with uppercase hex. Shared by the `Display` and
/// `Debug` implementations.
fn write_escaped(bytes: &[u8], formatter: &mut Formatter<'_>) -> fmt::Result {
    for &byte in bytes {
        if byte.is_ascii_graphic() || byte == b' ' {
            formatter.write_char(char::from(byte))?;
        } else {
            write!(formatter, "\\x{byte:02X}")?;
        }
    }
    Ok(())
}

impl Display for LumpName {
    /// The name without padding: printable `ASCII` as-is, every other byte
    /// escaped as `\xNN` with uppercase hex.
    #[inline]
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write_escaped(self.as_bytes(), f)
    }
}

impl Debug for LumpName {
    /// `LumpName("...")` with the same escaping as `Display`.
    #[inline]
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str("LumpName(\"")?;
        write_escaped(self.as_bytes(), f)?;
        f.write_str("\")")
    }
}

impl Serialize for LumpName {
    /// Serializes as the display string.
    #[inline]
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_str(self)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use serde_json::json;

    use super::{LumpName, MAX_NAME_LEN, NAME_LEN, glob_match};
    use crate::error::CyberError;

    #[test]
    fn from_raw_truncates_at_the_first_nul_and_zeroes_the_rest() {
        let name = LumpName::from_raw(*b"E1M1\0\0\0\0");
        assert_eq!(name.as_bytes(), b"E1M1", "kept bytes");
        assert_eq!(name.raw(), *b"E1M1\0\0\0\0", "raw bytes are exact");
        assert_eq!(name.len(), 4, "character count");
        assert!(!name.is_empty(), "a four-character name is not empty");
        let garbage = LumpName::from_raw(*b"AB\0Z\0\0\0\0");
        assert_eq!(
            garbage.raw(),
            *b"AB\0\0\0\0\0\0",
            "bytes after the NUL are zeroed"
        );
        let full = LumpName::from_raw(*b"SSTART00");
        assert_eq!(
            full.as_bytes(),
            b"SSTART00",
            "a full eight-byte name is kept"
        );
        let empty = LumpName::from_raw([0_u8; NAME_LEN]);
        assert!(empty.is_empty(), "an all-NUL entry is empty");
        assert_eq!(empty.len(), 0, "empty length");
    }

    #[test]
    fn from_raw_uppercases_and_keeps_odd_bytes() {
        let lower = LumpName::from_raw(*b"map01\0\0\0");
        assert_eq!(lower.as_bytes(), b"MAP01", "ASCII letters are uppercased");
        let odd = LumpName::from_raw(*b"\xFF\x01a_\0\0\0\0");
        assert_eq!(
            &odd.raw()[..4],
            &[0xFF, 0x01, b'A', b'_'],
            "high and control bytes are kept as they are, ASCII letters uppercased"
        );
    }

    #[test]
    fn parse_accepts_valid_names_and_uppercases() {
        let name = LumpName::parse("E1M1").expect("E1M1 is a valid name");
        assert_eq!(name.as_bytes(), b"E1M1", "accepted verbatim");
        let lower = LumpName::parse("map01").expect("map01 is a valid name");
        assert_eq!(
            lower,
            LumpName::parse("MAP01").expect("MAP01 is a valid name"),
            "lowercase input is stored uppercase"
        );
        let symbols = LumpName::parse("A[1]-_^").expect("graphic punctuation is valid");
        assert_eq!(symbols.as_bytes(), b"A[1]-_^", "punctuation survives");
        let eight = LumpName::parse("TOOLONG8").expect("exactly eight is valid");
        assert_eq!(eight.len(), NAME_LEN, "the length limit is inclusive");
        assert_eq!(
            LumpName::from_raw(name.raw()),
            name,
            "raw bytes round-trip through from_raw"
        );
    }

    #[test]
    fn parse_rejects_bad_input_with_invalid_argument() {
        let CyberError::InvalidArgument(message) =
            LumpName::parse("").expect_err("the empty name must fail")
        else {
            panic!("expected InvalidArgument for an empty name");
        };
        assert!(message.contains("empty"), "empty message: {message}");
        let long = LumpName::parse("TOOLONGNAME").expect_err("nine characters must fail");
        assert_eq!(long.code(), "invalid_argument", "too-long code");
        assert!(
            long.to_string().contains("TOOLONGNAME"),
            "too-long message names the input: {long}"
        );
        let space = LumpName::parse("A B").expect_err("a space must fail");
        assert_eq!(space.code(), "invalid_argument", "space code");
        assert!(
            space.to_string().contains("0x20"),
            "space message names the byte: {space}"
        );
        let accent = LumpName::parse("\u{00C9}").expect_err("non-ASCII must fail");
        assert_eq!(accent.code(), "invalid_argument", "non-ASCII code");
        assert!(
            accent.to_string().contains("0xC3"),
            "non-ASCII message names the first UTF-8 byte: {accent}"
        );
        let tab = LumpName::parse("\t").expect_err("a control character must fail");
        assert_eq!(tab.code(), "invalid_argument", "control code");
        assert!(
            tab.to_string().contains("0x09"),
            "control message names the byte: {tab}"
        );
    }

    #[test]
    fn max_name_len_is_the_directory_width() {
        assert_eq!(MAX_NAME_LEN, NAME_LEN, "the public limit is the raw width");
    }

    #[test]
    fn is_valid_char_covers_the_derivation_alphabet() {
        for ch in ['A', 'Z', 'a', 'z', '0', '9', '_'] {
            assert!(LumpName::is_valid_char(ch), "{ch} is a lump character");
        }
        for ch in [' ', '.', '/', '-', '!', '\0', '\u{e9}'] {
            assert!(!LumpName::is_valid_char(ch), "{ch} is not");
        }
    }

    #[test]
    fn display_and_debug_escape_non_printable_bytes() {
        let name = LumpName::from_raw(*b"T\x01\x7F\x80\0\0\0\0");
        assert_eq!(
            format!("{name}"),
            r"T\x01\x7F\x80",
            "control and high bytes become uppercase-hex escapes"
        );
        let spaced = LumpName::from_raw(*b"A B\0\0\0\0\0");
        assert_eq!(format!("{spaced}"), "A B", "a raw space prints as-is");
        assert_eq!(
            format!("{name:?}"),
            r#"LumpName("T\x01\x7F\x80")"#,
            "Debug wraps the same escaped text"
        );
        assert_eq!(
            format!("{:?}", LumpName::parse("E1M1").expect("valid name")),
            r#"LumpName("E1M1")"#,
            "Debug of a plain name"
        );
    }

    #[test]
    fn equality_hashing_and_ordering_ignore_case() {
        assert_eq!(
            LumpName::parse("e1m1").expect("valid name"),
            LumpName::parse("E1M1").expect("valid name"),
            "parsed names compare case-insensitively"
        );
        assert_eq!(
            LumpName::from_raw(*b"map01\0\0\0"),
            LumpName::parse("MAP01").expect("valid name"),
            "raw and parsed names meet after uppercasing"
        );
        assert!(
            LumpName::parse("A").expect("valid name") < LumpName::parse("B").expect("valid name"),
            "ordering follows the uppercase bytes"
        );
        let mut seen = HashSet::new();
        seen.insert(LumpName::parse("e1m1").expect("valid name"));
        seen.insert(LumpName::parse("E1M1").expect("valid name"));
        assert_eq!(seen.len(), 1, "case-folded names hash alike");
    }

    #[test]
    fn serialize_as_the_display_string() {
        let name = LumpName::parse("E1M1").expect("valid name");
        assert_eq!(
            serde_json::to_value(name).expect("a string serializes"),
            json!("E1M1"),
            "plain name"
        );
        let odd = LumpName::from_raw(*b"T\x01\0\0\0\0\0\0");
        assert_eq!(
            serde_json::to_value(odd).expect("an escaped name serializes"),
            json!(format!("{odd}")),
            "escaped name serializes as its display text"
        );
    }

    #[test]
    fn glob_matches_star_question_and_literals() {
        let map = LumpName::parse("MAP01").expect("valid name");
        assert!(map.matches_glob("*"), "star matches everything");
        assert!(map.matches_glob("map*"), "the pattern is case-insensitive");
        assert!(map.matches_glob("MAP*"), "prefix glob");
        assert!(map.matches_glob("MAP?1"), "question matches one character");
        assert!(
            !map.matches_glob("MAP??1"),
            "two questions cannot span one byte"
        );
        assert!(!map.matches_glob("TAB*"), "literal mismatch");
        let e1m1 = LumpName::parse("E1M1").expect("valid name");
        assert!(e1m1.matches_glob("*1"), "suffix glob");
        assert!(!e1m1.matches_glob("*2"), "suffix glob mismatch");
        let doom = LumpName::parse("D_E1M1").expect("valid name");
        assert!(doom.matches_glob("D_*"), "inner glob");
        assert!(doom.matches_glob("d_*"), "inner glob case-insensitively");
        assert!(!doom.matches_glob("D_??"), "length still has to fit");
        assert!(!doom.matches_glob("DOOM*"), "underscore is a literal");
        assert!(e1m1.matches_glob("**"), "consecutive stars collapse");
        assert!(
            !e1m1.matches_glob(""),
            "an empty pattern matches a non-empty name"
        );
    }

    #[test]
    fn glob_treats_the_empty_name_consistently() {
        let empty = LumpName::from_raw([0_u8; NAME_LEN]);
        assert!(
            empty.matches_glob(""),
            "empty pattern matches the empty name"
        );
        assert!(empty.matches_glob("*"), "star matches the empty name");
        assert!(!empty.matches_glob("?"), "question needs one character");
        assert!(!empty.matches_glob("MAP*"), "a literal still fails");
    }

    #[test]
    fn glob_match_applies_the_same_rules_to_raw_bytes() {
        assert!(glob_match(b"TEXTURE1", "*"), "star matches a raw name");
        assert!(glob_match(b"TEXTURE1", "tex*"), "case-insensitive prefix");
        assert!(
            glob_match(b"TEXTURE1", "TEXTUR?1"),
            "question matches one byte"
        );
        assert!(!glob_match(b"TEXTURE1", "PNAMES*"), "a literal mismatch");
        assert!(
            !glob_match(b"", "TEXTURE1"),
            "an empty name holds no pattern"
        );
    }
}
