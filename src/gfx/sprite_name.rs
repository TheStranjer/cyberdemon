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
//! [`parse_sprite_name`] recognises exactly that grammar over uppercase
//! `ASCII` text (lump names are stored uppercase); everything else,
//! including lowercase input, is not a sprite name.

/// Characters of the sprite prefix at the start of a name.
const SPRITE_LEN: usize = 4;
/// First frame character; frames run `A`-`Z` and then `[`, `\`, `]`,
/// the three bytes after `Z`, so one contiguous range covers all 29.
const FIRST_FRAME: u8 = b'A';
/// Last frame character (`]`).
const LAST_FRAME: u8 = b']';
/// First rotation digit (`0`, the all-angles rotation).
const FIRST_ROTATION: u8 = b'0';
/// Last rotation digit (`8`).
const LAST_ROTATION: u8 = b'8';

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

/// Parses a sprite lump name, uppercase `ASCII` only: a four-character
/// sprite prefix, a frame+rotation pair, and optionally a second frame
/// and rotation served by the same image mirrored.
///
/// A mirrored name whose either rotation is `0` is rejected: the
/// all-angles rotation has no mirror partner.
#[must_use]
#[inline]
pub fn parse_sprite_name(text: &str) -> Option<SpriteName> {
    let (prefix, rest) = sprite_prefix(text.as_bytes())?;
    let (frame, rotation, tail) = parse_pair(rest)?;
    let mirror = if tail.is_empty() {
        None
    } else {
        Some(parse_mirror(rotation, tail)?)
    };
    Some(SpriteName {
        sprite: prefix.to_owned(),
        frame,
        rotation,
        mirror,
    })
}

/// The second frame+rotation pair of an eight-character name: both
/// rotations must be real (`0` has no mirror partner) and the pair must
/// run to the end of the name.
fn parse_mirror(rotation: u8, bytes: &[u8]) -> Option<(char, u8)> {
    if rotation == 0 {
        return None;
    }
    let (frame, mirror_rotation, rest) = parse_pair(bytes)?;
    (rest.is_empty() && mirror_rotation != 0).then_some((frame, mirror_rotation))
}

/// The four sprite characters and the frame/rotation text that follows,
/// or `None` when the name starts with fewer than four uppercase `ASCII`
/// letters or digits.
fn sprite_prefix(bytes: &[u8]) -> Option<(&str, &[u8])> {
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
    if !(FIRST_FRAME..=LAST_FRAME).contains(frame) {
        return None;
    }
    let (digit, tail) = rest.split_first()?;
    if !(FIRST_ROTATION..=LAST_ROTATION).contains(digit) {
        return None;
    }
    let rotation = digit.checked_sub(FIRST_ROTATION)?;
    Some((char::from(*frame), rotation, tail))
}

#[cfg(test)]
mod tests {
    use super::{SpriteName, parse_sprite_name};

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
            parse_sprite_name("TROOA1"),
            Some(parsed("TROO", 'A', 1)),
            "plain frame and rotation"
        );
        assert_eq!(
            parse_sprite_name("PLAYA0"),
            Some(parsed("PLAY", 'A', 0)),
            "rotation 0 is the all-angles image"
        );
        assert_eq!(
            parse_sprite_name("TROO]8"),
            Some(parsed("TROO", ']', 8)),
            "the three letters after Z are frames too"
        );
        assert_eq!(
            parse_sprite_name("B4R1A2"),
            Some(parsed("B4R1", 'A', 2)),
            "digits may sit in the sprite prefix"
        );
    }

    #[test]
    fn parses_the_mirrored_eight_character_name() {
        let got = parse_sprite_name("POSSA2A8").expect("a standard mirrored name parses");
        assert_eq!(got.sprite, "POSS", "the prefix");
        assert_eq!((got.frame, got.rotation), ('A', 2), "the direct pair");
        assert_eq!(got.mirror, Some(('A', 8)), "the mirrored pair");
        let other_frame = parse_sprite_name("TROOA1B5").expect("a differing mirror frame");
        assert_eq!(other_frame.mirror, Some(('B', 5)), "the grammar allows it");
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
            "TROOA1A2A3",
            "TROO?A1",
            "TROOA1X",
            "TROOA0A8",
            "TROOA8A0",
        ] {
            assert_eq!(
                parse_sprite_name(text),
                None,
                "{text:?} is not a sprite name"
            );
        }
    }
}
