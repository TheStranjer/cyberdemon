//! Picking the sprite rotation an engine shows for one viewing angle.
//!
//! Vanilla `Doom` selects one of the eight rotations of a sprite frame
//! from the angle between the viewer and the thing it draws:
//!
//! ```text
//! rot = ((angle_viewer_to_thing - thing_angle + ANG45/2 * 9) >> 29) & 7
//! ```
//!
//! with everything in `BAM` (`crate::fixed::Angle`, wrapping arithmetic,
//! `ANG45 = 0x2000_0000`); the rotation lump number is `rot + 1`, so
//! number `1` shows the thing's front and number `5` its back.
//! `angle_viewer_to_thing` is `atan2(thing.y - viewer.y, thing.x -
//! viewer.x)`, the direction from the viewer to the thing, and
//! `thing_angle` is the thing's facing angle, whole degrees converted to
//! `BAM`. [`rotation_for`] applies that to map positions;
//! [`rotation_between`] is the same bucketing for angles already in
//! `BAM`, which is what a renderer carrying interpolated angles calls.

use crate::fixed::{Angle, atan2};

/// The `ANG45/2 * 9` bucket bias (`202.5` degrees) as raw `BAM`.
const ROTATION_BIAS: u32 = 0x9000_0000;
/// Right shift turning a biased `BAM` into its `0..=7` bucket.
const BUCKET_SHIFT: u32 = 29;
/// Mask keeping the three bucket bits.
const BUCKET_MASK: u32 = 7;
/// The rotation lump number of bucket `0`.
const FIRST_ROTATION: u32 = 1;

/// The `BAM` bucketing of vanilla's rotation formula.
///
/// Answers the sprite rotation number `1..=8` between a viewer and a
/// thing when both angles are already in `BAM`: the bucketing of
/// `angle_viewer_to_thing - thing_angle + ANG45/2 * 9` shifted down to
/// its `0..=7` bucket, plus one, on the wrapping arithmetic of [`Angle`].
#[must_use]
#[inline]
pub fn rotation_between(angle_viewer_to_thing: Angle, thing_angle: Angle) -> u8 {
    let biased = angle_viewer_to_thing
        .wrapping_sub(thing_angle)
        .wrapping_add(Angle(ROTATION_BIAS));
    let bucket = biased.0.checked_shr(BUCKET_SHIFT).unwrap_or(0) & BUCKET_MASK;
    bucket
        .checked_add(FIRST_ROTATION)
        .and_then(|value| u8::try_from(value).ok())
        .unwrap_or(1_u8)
}

/// The sprite rotation number `1..=8` a viewer sees of a thing.
///
/// `viewer` and `thing_pos` are map positions, `thing_angle_deg` the
/// thing's facing in whole degrees: `1` when the thing faces the viewer,
/// `5` when it shows its back. The viewer-to-thing direction is the
/// [`atan2`] of the position difference, so a viewer standing on the
/// thing reads the `BAM` zero direction; the number is always `1..=8`.
#[must_use]
#[inline]
pub fn rotation_for(viewer: (i64, i64), thing_pos: (i64, i64), thing_angle_deg: i32) -> u8 {
    let (viewer_x, viewer_y) = viewer;
    let (thing_x, thing_y) = thing_pos;
    let angle = atan2(
        thing_y.wrapping_sub(viewer_y),
        thing_x.wrapping_sub(viewer_x),
    );
    rotation_between(angle, Angle::from_degrees(i64::from(thing_angle_deg)))
}

#[cfg(test)]
mod tests {
    use super::{ROTATION_BIAS, rotation_between, rotation_for};
    use crate::fixed::{Angle, atan2};

    /// The ticket formula recomputed with raw `BAM` arithmetic, for
    /// asserting that the helper matches it.
    fn formula(angle_viewer_to_thing: Angle, thing_angle: Angle) -> u8 {
        let biased = angle_viewer_to_thing
            .wrapping_sub(thing_angle)
            .wrapping_add(Angle(ROTATION_BIAS))
            .0;
        u8::try_from(biased.checked_shr(29).unwrap_or(0) & 7)
            .expect("a bucket is 0..=7")
            .checked_add(1)
            .expect("numbers are 1..=8")
    }

    #[test]
    fn a_facing_thing_shows_rotation_1_and_its_back_shows_5() {
        assert_eq!(
            rotation_for((100, 0), (0, 0), 0),
            1,
            "viewer due east of a thing facing east sees its front"
        );
        assert_eq!(
            rotation_for((-100, 0), (0, 0), 0),
            5,
            "viewer due west of it sees its back"
        );
        assert_eq!(
            rotation_for((0, 100), (0, 0), 90),
            1,
            "the facing case rotated with the thing"
        );
    }

    #[test]
    fn the_side_of_a_thing_follows_the_formula_and_a_hand_check() {
        let viewer = (0_i64, 100_i64);
        let thing = (0_i64, 0_i64);
        let angle = atan2(
            thing.1.wrapping_sub(viewer.1),
            thing.0.wrapping_sub(viewer.0),
        );
        let thing_angle = Angle::from_degrees(0);
        assert_eq!(
            rotation_for(viewer, thing, 0),
            formula(angle, thing_angle),
            "the helper is the formula"
        );
        assert_eq!(
            rotation_for(viewer, thing, 0),
            3,
            "viewer north of a thing facing east: 270 + 202.5 wraps to \
             112.5, the third bucket"
        );
        assert_eq!(
            rotation_for((0, -100), thing, 0),
            7,
            "viewer south of it sees the other side"
        );
    }

    #[test]
    fn the_buckets_open_and_close_exactly_22_5_degrees_off_center() {
        // With the thing facing east, the front bucket covers the viewer
        // directions 157.5..202.5 degrees, half-open on both edges.
        assert_eq!(
            rotation_between(Angle(0x7000_0000), Angle::EAST),
            1,
            "157.5 degrees is inside the front bucket"
        );
        assert_eq!(
            rotation_between(Angle(0x6F00_0000), Angle::EAST),
            8,
            "one quarter-bucket earlier is the last rotation"
        );
        assert_eq!(
            rotation_between(Angle(0x8F00_0000), Angle::EAST),
            1,
            "just before 202.5 degrees is still the front"
        );
        assert_eq!(
            rotation_between(Angle(0x9000_0000), Angle::EAST),
            2,
            "202.5 degrees opens the next bucket"
        );
    }

    #[test]
    fn every_bucket_center_numbered_by_its_offset_degrees() {
        for index in 0_u8..8 {
            let degrees = i64::from(index).checked_mul(45).expect("test degrees fit");
            let viewer_direction =
                Angle::from_degrees(180_i64.checked_add(degrees).expect("test degrees fit"));
            assert_eq!(
                rotation_between(viewer_direction, Angle::EAST),
                index.checked_add(1).expect("numbers fit"),
                "bucket center {degrees} past due west"
            );
        }
    }

    #[test]
    fn angles_and_positions_wrap_without_failing() {
        assert_eq!(
            rotation_for((0, 0), (0, 0), 0),
            5,
            "the zero vector reads east: a thing facing east shows its back"
        );
        assert_eq!(
            rotation_for((0, 0), (100, 0), -90),
            7,
            "a thing facing -90 (due south) shows its right side to a viewer west of it"
        );
        assert_eq!(
            rotation_between(Angle::from_degrees(-1), Angle::from_degrees(359)),
            rotation_between(Angle::EAST, Angle::EAST),
            "the same relative angle buckets the same"
        );
        let far = 0x2000_0000_0000_0000_i64;
        assert_eq!(
            rotation_for((0, 0), (far, far), 45),
            5,
            "huge separations still resolve"
        );
    }
}
