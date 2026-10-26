//! The `Doom` sprite lump name grammar.
//!
//! A sprite lump inside the `S_START`/`S_END` region is named four
//! sprite characters, one frame letter (`A`-`Z`, then `[`, `\`, `]`: 29
//! frames) and one rotation digit `0`-`8`, e.g. `TROOA1`. Rotation `0`
//! serves every angle with the one image. An eight-character name appends
//! a second frame+rotation pair served by the same image horizontally
//! mirrored: `POSSA2A8` serves rotation 2 of frame `A` directly and
//! rotation 8 of frame `A` flipped.
//!
//! [`SpriteName::parse`] recognises exactly that grammar over uppercase
//! `ASCII` text (lump names are stored uppercase); everything else,
//! including lowercase input, is not a sprite name. [`SpriteName::format`]
//! writes the canonical name back, and the three part parsers
//! ([`parse_sprite_prefix`], [`parse_frame_letter`] and [`parse_rotation`])
//! validate the pieces a caller assembles a name from, so the grammar is
//! written once and every tool maps the shared [`SpriteNameError`] onto
//! its own refusal text.

/// Characters of the sprite prefix at the start of a name.
const SPRITE_LEN: usize = 4;
/// First rotation digit (`0`, the all-angles rotation).
const FIRST_ROTATION: u8 = b'0';
/// Last rotation digit (`8`, the number [`MAX_ROTATION`] names).
const LAST_ROTATION: u8 = b'8';

/// The all-angles rotation: one image serves every view.
pub const ALL_ANGLES: u8 = 0_u8;
/// The largest rotation number a sprite frame serves: the digit `8`.
pub const MAX_ROTATION: u8 = 8_u8;

/// Why some sprite name text fails the grammar.
///
/// The parts carry no payload: a caller already holds the rejected text
/// or number when it maps one of these onto its own
/// [`crate::error::CyberError::InvalidArgument`] instruction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SpriteNameError {
    /// The sprite prefix is not four `ASCII` letters or digits.
    Prefix,
    /// The frame is not one character of `A`-`Z`, `[`, `\` or `]`.
    Frame,
    /// The rotation number is not `0`-`8`.
    Rotation,
}

impl core::fmt::Display for SpriteNameError {
    #[inline]
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match *self {
            Self::Prefix => f.write_str("not four ASCII letters or digits"),
            Self::Frame => f.write_str("not one sprite frame letter A-Z, [, \\ or ]"),
            Self::Rotation => f.write_str("not a sprite rotation number 0 to 8"),
        }
    }
}

impl core::error::Error for SpriteNameError {}

/// A parsed sprite lump name: which frames and rotations one lump serves.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpriteName {
    /// The four-character sprite prefix, e.g. `"TROO"`.
    pub sprite: String,
    /// Frame letter served by the image itself.
    pub frame: char,
    /// Rotation `0`-`8` served by the image itself; `0` means every angle.
    pub rotation: u8,
    /// Frame letter and rotation served by the horizontally mirrored
    /// image; only the eight-character name form carries one.
    pub mirror: Option<(char, u8)>,
}

/// A frame letter plus rotation digit, and the text that follows the pair.
type Pair<'bytes> = (char, u8, &'bytes [u8]);

impl SpriteName {
    /// Parses a sprite lump name, uppercase `ASCII` only: a four-character
    /// sprite prefix, a frame+rotation pair, and optionally a second frame
    /// and rotation served by the same image mirrored.
    ///
    /// A mirrored name whose either rotation is `0` is rejected: the
    /// all-angles rotation has no mirror partner.
    #[must_use]
    #[inline]
    pub fn parse(text: &str) -> Option<Self> {
        let (prefix, rest) = split_prefix(text.as_bytes())?;
        let (frame, rotation, tail) = parse_pair(rest)?;
        let mirror = if tail.is_empty() {
            None
        } else {
            Some(parse_mirror(rotation, tail)?)
        };
        Some(Self {
            sprite: prefix.to_owned(),
            frame,
            rotation,
            mirror,
        })
    }

    /// The canonical lump name text this grammar reading carries: the
    /// prefix, the image's own frame+rotation pair and, for the
    /// eight-character form, the mirrored pair. It round-trips with
    /// [`Self::parse`].
    #[must_use]
    #[inline]
    pub fn format(&self) -> String {
        let sprite = self.sprite.as_str();
        let frame = self.frame;
        let rotation = self.rotation;
        match self.mirror {
            None => format!("{sprite}{frame}{rotation}"),
            Some((mirror_frame, mirror_rotation)) => {
                format!("{sprite}{frame}{rotation}{mirror_frame}{mirror_rotation}")
            }
        }
    }
}

/// The second frame+rotation pair of an eight-character name: both
/// rotations must be real (`0` has no mirror partner) and the pair must
/// run to the end of the name.
fn parse_mirror(rotation: u8, bytes: &[u8]) -> Option<(char, u8)> {
    if rotation == ALL_ANGLES {
        return None;
    }
    let (frame, mirror_rotation, rest) = parse_pair(bytes)?;
    (rest.is_empty() && mirror_rotation != ALL_ANGLES).then_some((frame, mirror_rotation))
}

/// The four sprite characters and the frame/rotation text that follows,
/// or `None` when the name starts with fewer than four uppercase `ASCII`
/// letters or digits.
fn split_prefix(bytes: &[u8]) -> Option<(&str, &[u8])> {
    let (prefix, rest) = bytes.split_at_checked(SPRITE_LEN)?;
    if !prefix
        .iter()
        .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
    {
        return None;
    }
    let text = core::str::from_utf8(prefix).ok()?;
    Some((text, rest))
}

/// One frame letter plus rotation digit at the head of `bytes`, and the
/// text that follows the pair.
fn parse_pair(bytes: &[u8]) -> Option<Pair<'_>> {
    let (frame, rest) = bytes.split_first()?;
    if !is_frame_letter(char::from(*frame)) {
        return None;
    }
    let (digit, tail) = rest.split_first()?;
    if !(FIRST_ROTATION..=LAST_ROTATION).contains(digit) {
        return None;
    }
    let rotation = digit.checked_sub(FIRST_ROTATION)?;
    Some((char::from(*frame), rotation, tail))
}

/// Whether `letter` is a sprite frame letter: `A`-`Z`, then the three
/// characters after `Z`, so one contiguous range covers all 29 frames.
const fn is_frame_letter(letter: char) -> bool {
    matches!(letter, 'A'..='Z' | '[' | '\\' | ']')
}

/// The four-character sprite prefix standing alone: `ASCII` letters or
/// digits, uppercased - what a caller filters or names by before the
/// frame and rotation are known.
///
/// # Errors
///
/// [`SpriteNameError::Prefix`] for any other text.
#[inline]
pub fn parse_sprite_prefix(text: &str) -> Result<String, SpriteNameError> {
    let upper = text.to_ascii_uppercase();
    let well_formed = upper.len() == SPRITE_LEN
        && upper
            .bytes()
            .all(|byte| byte.is_ascii_alphabetic() || byte.is_ascii_digit());
    if well_formed {
        return Ok(upper);
    }
    Err(SpriteNameError::Prefix)
}

/// A sprite frame letter from a caller's text: exactly one character of
/// `A`-`Z`, `[`, `\` or `]`, uppercased. Whether a missing frame defaults
/// to a particular letter is the caller's choice.
///
/// # Errors
///
/// [`SpriteNameError::Frame`] for any other text.
#[inline]
pub fn parse_frame_letter(text: &str) -> Result<char, SpriteNameError> {
    let upper = text.to_ascii_uppercase();
    let single = upper.chars().count() == 1;
    if let Some(letter) = upper.chars().next()
        && single
        && is_frame_letter(letter)
    {
        return Ok(letter);
    }
    Err(SpriteNameError::Frame)
}

/// A sprite rotation number: `0`-`8`, `0` being [`ALL_ANGLES`]. Whether a
/// missing rotation defaults to a particular number is the caller's
/// choice; [`parse_frame_letter`] is the frame-letter counterpart.
///
/// # Errors
///
/// [`SpriteNameError::Rotation`] for any other number.
#[inline]
pub const fn parse_rotation(rotation: u8) -> Result<u8, SpriteNameError> {
    if rotation <= MAX_ROTATION {
        Ok(rotation)
    } else {
        Err(SpriteNameError::Rotation)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        SpriteName, SpriteNameError, parse_frame_letter, parse_rotation, parse_sprite_prefix,
    };

    /// A parsed fixture with the common fields spelled out once.
    fn parsed(sprite: &str, frame: char, rotation: u8) -> SpriteName {
        SpriteName {
            sprite: sprite.to_owned(),
            frame,
            rotation,
            mirror: None,
        }
    }

    #[test]
    fn parses_a_six_character_name() {
        assert_eq!(
            SpriteName::parse("TROOA1"),
            Some(parsed("TROO", 'A', 1)),
            "plain frame and rotation"
        );
        assert_eq!(
            SpriteName::parse("PLAYA0"),
            Some(parsed("PLAY", 'A', 0)),
            "rotation 0 is the all-angles image"
        );
        assert_eq!(
            SpriteName::parse("B4R1A2"),
            Some(parsed("B4R1", 'A', 2)),
            "digits may sit in the sprite prefix"
        );
    }

    #[test]
    fn parses_frames_past_z() {
        assert_eq!(
            SpriteName::parse("TROO[3"),
            Some(parsed("TROO", '[', 3)),
            "the character right after Z is a frame"
        );
        assert_eq!(
            SpriteName::parse("TROO\\4"),
            Some(parsed("TROO", '\\', 4)),
            "so is the backslash"
        );
        assert_eq!(
            SpriteName::parse("TROO]8"),
            Some(parsed("TROO", ']', 8)),
            "and the last of the 29 frames"
        );
    }

    #[test]
    fn parses_the_mirrored_eight_character_name() {
        let got = SpriteName::parse("POSSA2A8").expect("a standard mirrored name parses");
        assert_eq!(got.sprite, "POSS", "the prefix");
        assert_eq!((got.frame, got.rotation), ('A', 2), "the direct pair");
        assert_eq!(got.mirror, Some(('A', 8)), "the mirrored pair");
        let views = SpriteName::parse("TROOA1A5").expect("the A1A5 pair form");
        assert_eq!(views.mirror, Some(('A', 5)), "rotation 1 with mirror 5");
        let other_frame = SpriteName::parse("TROOA1B5").expect("a differing mirror frame");
        assert_eq!(other_frame.mirror, Some(('B', 5)), "the grammar allows it");
        let past_z = SpriteName::parse("TROO[1[5").expect("a mirrored past-Z pair");
        assert_eq!(past_z.mirror, Some(('[', 5)), "the same frame letter twice");
    }

    #[test]
    fn rejects_names_outside_the_grammar() {
        for text in [
            "",
            "TROO",
            "TROOA",
            "TROOA?",
            "TROOAA",
            "TROOA9",
            "TROOA1A9",
            "trooa1",
            "TROOa1",
            "TROOA1A2A3",
            "TROO?A1",
            "TROOA1X",
            "TROOA0A8",
            "TROOA8A0",
            "TROO^1",
        ] {
            assert_eq!(
                SpriteName::parse(text),
                None,
                "{text:?} is not a sprite name"
            );
        }
    }

    #[test]
    fn format_writes_the_name_back_as_parsed() {
        for text in [
            "PLAYA0", "TROOA1", "POSSA2A8", "TROOA1A5", "TROO[3", "TROO\\8", "TROO]1]7",
        ] {
            let parsed_name = SpriteName::parse(text).expect("each fixture is a sprite name");
            assert_eq!(parsed_name.format(), text, "{text:?} round-trips");
        }
    }

    #[test]
    fn frame_letters_are_one_uppercase_grammar_character() {
        for (text, want) in [
            ("A", 'A'),
            ("b", 'B'),
            ("Z", 'Z'),
            ("[", '['),
            ("\\", '\\'),
            ("]", ']'),
        ] {
            assert_eq!(
                parse_frame_letter(text),
                Ok(want),
                "{text:?} names frame {want:?}"
            );
        }
        for text in ["", "AB", "aa", "1", "-", "^", "_", " "] {
            assert_eq!(
                parse_frame_letter(text),
                Err(SpriteNameError::Frame),
                "{text:?} is no frame letter"
            );
        }
    }

    #[test]
    fn rotations_are_the_nine_numbers() {
        for rotation in 0_u8..=8_u8 {
            assert_eq!(
                parse_rotation(rotation),
                Ok(rotation),
                "rotation {rotation} parses"
            );
        }
        for rotation in [9_u8, 10_u8, 255_u8] {
            assert_eq!(
                parse_rotation(rotation),
                Err(SpriteNameError::Rotation),
                "rotation {rotation} is refused"
            );
        }
    }

    #[test]
    fn prefixes_are_four_letters_or_digits() {
        assert_eq!(parse_sprite_prefix("troo"), Ok("TROO".to_owned()));
        assert_eq!(parse_sprite_prefix("B4R1"), Ok("B4R1".to_owned()));
        for text in ["", "TRO", "TROOO", "TR!O", "TROO1", "TR--"] {
            assert_eq!(
                parse_sprite_prefix(text),
                Err(SpriteNameError::Prefix),
                "{text:?} is no sprite prefix"
            );
        }
    }
}
