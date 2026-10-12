//! 256-colour `Doom` palettes: `PLAYPAL` storage and `#RRGGBB` lists.

use crate::error::CyberError;

/// Entries in one palette.
const PALETTE_SIZE: usize = 256;
/// Bytes of one palette inside `PLAYPAL` (256 entries of red, green, blue).
const PLAYPAL_ENTRY_BYTES: usize = 768;
/// Bytes of one palette colour.
const COLOR_BYTES: usize = 3;
/// Hex digits of an `RRGGBB` colour.
const HEX_DIGITS: usize = 6;

/// A 256-entry `RGB` palette, as stored in `PLAYPAL`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Palette(pub [[u8; COLOR_BYTES]; PALETTE_SIZE]);

impl From<[[u8; COLOR_BYTES]; PALETTE_SIZE]> for Palette {
    /// Wraps a full 256-entry colour table.
    #[inline]
    fn from(colors: [[u8; COLOR_BYTES]; PALETTE_SIZE]) -> Self {
        Self(colors)
    }
}

impl Palette {
    /// The `index`th palette of raw `PLAYPAL` bytes (768 bytes each).
    ///
    /// # Errors
    ///
    /// [`CyberError::NotFound`] when `index` names a palette the data does
    /// not hold; trailing bytes that do not complete a palette are ignored.
    #[inline]
    pub fn from_playpal(bytes: &[u8], index: usize) -> Result<Self, CyberError> {
        let available = bytes.chunks_exact(PLAYPAL_ENTRY_BYTES).count();
        let Some(entry) = bytes.chunks_exact(PLAYPAL_ENTRY_BYTES).nth(index) else {
            return Err(CyberError::NotFound(format!(
                "PLAYPAL data holds {available} palettes, index {index} is out of range"
            )));
        };
        let mut colors = [[0_u8; COLOR_BYTES]; PALETTE_SIZE];
        for (color, triplet) in colors.iter_mut().zip(entry.chunks_exact(COLOR_BYTES)) {
            color.copy_from_slice(triplet);
        }
        Ok(Self(colors))
    }

    /// This palette as the 768 bytes one `PLAYPAL` entry occupies.
    #[must_use]
    #[inline]
    pub fn to_bytes(&self) -> Vec<u8> {
        self.0.into_iter().flatten().collect()
    }

    /// Builds a palette from exactly 256 `#RRGGBB` (or `RRGGBB`) strings.
    ///
    /// # Errors
    ///
    /// [`CyberError::InvalidArgument`] when the list does not hold exactly
    /// 256 entries or an entry is not a hex colour.
    #[inline]
    pub fn from_hex_list(values: &[String]) -> Result<Self, CyberError> {
        if values.len() != PALETTE_SIZE {
            return Err(CyberError::InvalidArgument(format!(
                "expected {PALETTE_SIZE} palette colours, got {}",
                values.len()
            )));
        }
        let mut colors = [[0_u8; COLOR_BYTES]; PALETTE_SIZE];
        for (color, value) in colors.iter_mut().zip(values) {
            *color = parse_hex_color(value)?;
        }
        Ok(Self(colors))
    }

    /// This palette as 256 `#RRGGBB` strings with uppercase hex digits.
    #[must_use]
    #[inline]
    pub fn to_hex_list(&self) -> Vec<String> {
        self.0
            .iter()
            .copied()
            .map(|[r, g, b]| format!("#{r:02X}{g:02X}{b:02X}"))
            .collect()
    }

    /// The colour at `index`, or `None` beyond the 256 entries.
    #[must_use]
    #[inline]
    pub fn color(&self, index: usize) -> Option<[u8; 3]> {
        self.0.get(index).copied()
    }

    /// All 256 colours in index order.
    #[inline]
    pub fn colors(&self) -> core::slice::Iter<'_, [u8; 3]> {
        self.0.iter()
    }
}

/// Parses one `#RRGGBB` or `RRGGBB` colour string.
fn parse_hex_color(value: &str) -> Result<[u8; 3], CyberError> {
    let digits = value.strip_prefix('#').unwrap_or(value);
    let malformed = || {
        CyberError::InvalidArgument(format!(
            "palette colour {value} is not a #RRGGBB or RRGGBB hex triple"
        ))
    };
    if digits.len() != HEX_DIGITS || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(malformed());
    }
    let Some(packed) = u32::from_str_radix(digits, 16).ok() else {
        return Err(malformed());
    };
    // Big-endian bytes (the denied `to_be_bytes`) by reversing the
    // little-endian form: [zero, red, green, blue] for a 24-bit value.
    let mut words = packed.to_le_bytes();
    words.reverse();
    let [_zero, r, g, b] = words;
    Ok([r, g, b])
}

#[cfg(test)]
mod tests {
    use super::{PALETTE_SIZE, Palette};
    use crate::error::CyberError;

    /// One `PLAYPAL` entry: the grey ramp, `[v, v, v]` per index.
    fn grey_ramp() -> Vec<u8> {
        (0..=255_u8)
            .flat_map(|value| [value, value, value])
            .collect()
    }

    /// A second distinct entry: the inverted grey ramp.
    fn inverted_ramp() -> Vec<u8> {
        (0..=255_u8)
            .rev()
            .flat_map(|value| [value, value, value])
            .collect()
    }

    #[test]
    fn from_playpal_reads_first_and_last_palette() {
        let mut bytes = grey_ramp();
        bytes.extend_from_slice(&inverted_ramp());
        let first = Palette::from_playpal(&bytes, 0).expect("the first palette exists");
        assert_eq!(first.color(0), Some([0, 0, 0]), "first entry of palette 0");
        assert_eq!(
            first.color(255),
            Some([255, 255, 255]),
            "last entry of palette 0"
        );
        let second = Palette::from_playpal(&bytes, 1).expect("the second palette exists");
        assert_eq!(
            second.color(0),
            Some([255, 255, 255]),
            "first entry of palette 1"
        );
        assert_eq!(
            second.color(255),
            Some([0, 0, 0]),
            "last entry of palette 1"
        );
    }

    #[test]
    fn from_playpal_rejects_indexes_beyond_the_data() {
        let bytes = grey_ramp();
        let error = Palette::from_playpal(&bytes, 1).expect_err("one palette has no index 1");
        assert_eq!(error.code(), "not_found", "out-of-range palette code");
        let empty = Palette::from_playpal(b"", 0).expect_err("no data holds no palette");
        assert_eq!(empty.code(), "not_found", "empty data code");
    }

    #[test]
    fn to_bytes_round_trips_through_from_playpal() {
        let bytes = grey_ramp();
        let palette = Palette::from_playpal(&bytes, 0).expect("palette 0 exists");
        assert_eq!(palette.to_bytes(), bytes, "byte layout is preserved");
    }

    /// 256 hex colours: index `i` is `#ii0000`.
    fn red_ramp_hex() -> Vec<String> {
        (0..256_u32)
            .map(|index| format!("#{index:02X}0000"))
            .collect()
    }

    #[test]
    fn hex_list_round_trips() {
        let list = red_ramp_hex();
        let palette = Palette::from_hex_list(&list).expect("256 valid colours");
        assert_eq!(
            palette.to_hex_list(),
            list,
            "hex list survives the round trip"
        );
        assert_eq!(
            palette.color(16),
            Some([16, 0, 0]),
            "colour parsed from hex"
        );
        assert_eq!(palette.color(255), Some([255, 0, 0]), "last colour parsed");
        assert_eq!(palette.color(256), None, "index beyond the palette");
    }

    #[test]
    fn hex_list_accepts_the_bare_form() {
        let list: Vec<String> = red_ramp_hex()
            .iter()
            .map(|value| value.strip_prefix('#').unwrap_or(value).to_owned())
            .collect();
        let palette = Palette::from_hex_list(&list).expect("RRGGBB without # parses");
        assert_eq!(palette.color(1), Some([1, 0, 0]), "bare form parses");
    }

    #[test]
    fn hex_list_rejects_bad_input() {
        let CyberError::InvalidArgument(message) =
            Palette::from_hex_list(&[]).expect_err("an empty list must fail")
        else {
            panic!("expected InvalidArgument for an empty list");
        };
        assert!(
            message.contains("expected 256"),
            "message names the count: {message}"
        );
        let mut list = red_ramp_hex();
        list[7] = "zz0000".to_owned();
        let bad_digit = Palette::from_hex_list(&list).expect_err("non-hex digits must fail");
        assert_eq!(bad_digit.code(), "invalid_argument", "bad digit code");
        let mut second_list = red_ramp_hex();
        second_list[7] = "#ff00".to_owned();
        let short = Palette::from_hex_list(&second_list).expect_err("short colours must fail");
        assert_eq!(short.code(), "invalid_argument", "short colour code");
    }

    #[test]
    fn colors_iterates_in_index_order() {
        let palette = Palette::from_playpal(&grey_ramp(), 0).expect("palette 0 exists");
        let collected: Vec<[u8; 3]> = palette.colors().copied().collect();
        assert_eq!(collected.len(), PALETTE_SIZE, "all entries iterate");
        assert_eq!(collected[3], [3, 3, 3], "iteration follows index order");
    }
}
