//! Names and limits the whole map layer shares.
//!
//! The `"-"` none-texture marker, the sky flat and the two picture-name
//! readers built from them ([`normalized_picture`] and [`is_sky_flat`]),
//! beside the player's body and step limits the movement, stair,
//! connector and dressing passes quote, and the player-start thing
//! types. Every former per-module copy of one of these values reads from
//! here; error and hint texts that quote a number format it from the
//! constant.

use alloc::string::String;

/// The model's name for "no texture here" / "no flat": what the readers
/// store for a blank field and the writers emit for an unset one.
pub const NO_TEXTURE: &str = "-";

/// The flat every port draws the sky with. (The per-game sky flat is a
/// separate concern; this is the vanilla marker.)
pub const SKY_FLAT: &str = "F_SKY1";

/// Whether a texture or flat name is [`SKY_FLAT`]: trimmed and
/// case-insensitive, the way every stored name is read.
#[must_use]
#[inline]
pub fn is_sky_flat(name: &str) -> bool {
    name.trim().eq_ignore_ascii_case(SKY_FLAT)
}

/// The scene key of a texture or flat name: uppercased and trimmed,
/// `None` for the empty name and the [`NO_TEXTURE`] none-marker.
#[must_use]
#[inline]
pub fn normalized_picture(name: &str) -> Option<String> {
    let normalized = name.trim().to_ascii_uppercase();
    if normalized.is_empty() || normalized == NO_TEXTURE {
        return None;
    }
    Some(normalized)
}

/// The highest ledge a player can walk up, in map units.
///
/// Falling is free at any depth, so movement is directional and a drop
/// is a one-way connection. The stair, connector and auto-dressing
/// passes and the movement walk all cap a single climb at this.
pub const MAX_STEP_HEIGHT: i32 = 24_i32;

/// The height a player needs to stand in, and the smallest gap across
/// any line they can cross.
pub const PLAYER_HEIGHT: i32 = 56_i32;

/// The shortest two-sided line a player can walk through: twice the
/// 16-unit player radius. A line shorter than this is a blocker, not a
/// way through.
pub const MIN_LINE_WIDTH: i32 = 32_i32;

/// The default gap a window leaves between its top and the lower
/// ceiling: the `lintel` parameter's resting value.
pub const DEFAULT_LINTEL: i32 = 24_i32;

/// The body half-side assumed for a thing whose definition the session
/// does not carry: the monster default the stuck and clearance tests
/// fall back to.
pub const UNKNOWN_THING_RADIUS: i64 = 20_i64;

/// The thing types that stand a player in the map: one per start slot.
pub const PLAYER_START_TYPES: [i32; 4] = [1_i32, 2_i32, 3_i32, 4_i32];

/// The thing type of the player 1 start: the first
/// [`PLAYER_START_TYPES`] entry.
pub const PLAYER1_START: i32 = {
    let [first, ..] = PLAYER_START_TYPES;
    first
};

/// [`PLAYER1_START`] in the unsigned `u32` shape the plan wire rows
/// and the thing definitions carry.
pub const PLAYER1_START_U32: u32 = PLAYER1_START.unsigned_abs();

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_LINTEL, MAX_STEP_HEIGHT, MIN_LINE_WIDTH, NO_TEXTURE, PLAYER_HEIGHT,
        PLAYER_START_TYPES, PLAYER1_START, PLAYER1_START_U32, SKY_FLAT, UNKNOWN_THING_RADIUS,
        is_sky_flat, normalized_picture,
    };

    #[test]
    fn the_shared_names_hold_their_values() {
        assert_eq!(NO_TEXTURE, "-");
        assert_eq!(SKY_FLAT, "F_SKY1");
        assert_eq!(MAX_STEP_HEIGHT, 24_i32);
        assert_eq!(DEFAULT_LINTEL, 24_i32);
        assert_eq!(PLAYER_HEIGHT, 56_i32);
        assert_eq!(MIN_LINE_WIDTH, 32_i32);
        assert_eq!(UNKNOWN_THING_RADIUS, 20_i64);
    }

    #[test]
    fn the_player_starts_derive_from_one_table() {
        assert_eq!(PLAYER_START_TYPES, [1_i32, 2_i32, 3_i32, 4_i32]);
        assert_eq!(PLAYER1_START, PLAYER_START_TYPES[0]);
        assert_eq!(PLAYER1_START_U32, u32::try_from(PLAYER1_START).unwrap_or(0));
    }

    #[test]
    fn sky_flat_reads_case_insensitively_and_trimmed() {
        assert!(is_sky_flat("F_SKY1"));
        assert!(is_sky_flat("f_sky1"), "lowercase is the same flat");
        assert!(is_sky_flat("F_SKY1 "), "a trailing space is stored noise");
        assert!(is_sky_flat(" f_sKy1 "), "both ends trim");
        assert!(!is_sky_flat("F_SKY2"), "another flat is nobody's sky");
        assert!(!is_sky_flat(""), "the empty name is not the sky");
        assert!(!is_sky_flat("-"), "the none-marker is not the sky");
    }

    #[test]
    fn normalized_picture_uppercases_trims_and_drops_the_blanks() {
        assert_eq!(normalized_picture("brick").as_deref(), Some("BRICK"));
        assert_eq!(normalized_picture(" BRICK ").as_deref(), Some("BRICK"));
        assert_eq!(normalized_picture("BRICK").as_deref(), Some("BRICK"));
        assert_eq!(
            normalized_picture(""),
            None,
            "the empty name answers nothing"
        );
        assert_eq!(
            normalized_picture("   "),
            None,
            "spaces alone answer nothing"
        );
        assert_eq!(
            normalized_picture("-"),
            None,
            "the none-marker answers nothing"
        );
        assert_eq!(
            normalized_picture(" - "),
            None,
            "trimmed, it still answers nothing"
        );
    }
}
