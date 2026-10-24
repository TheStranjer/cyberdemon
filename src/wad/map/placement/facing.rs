//! Facing choice: how each thing a [`distribute`](fn@super::distribute) run
//! places is angled.
//!
//! [`Facing`] names the mode, [`bearing`] measures a direction with the
//! integer `atan2`, [`snap_angle`] rounds it to the nearest multiple of
//! 45 degrees, and [`FacingRun`] threads one mode with its run state
//! through a whole fill: [`Facing::Seeded`] draws its compass points from
//! a xorshift32 advanced deterministically from the caller's `seed`, the
//! engine's only allowed randomness.

use crate::fixed::atan2;
use crate::geom::Pt;
use crate::wad::map::model::Map;

use super::player_start;

/// Degrees in a full circle; facings normalise into `0..=359`.
const DEGREES: i32 = 360_i32;
/// Every facing snaps to a multiple of this many degrees.
const ANGLE_STEP: i32 = 45_i32;
/// Half the snap step: the rounding bias of [`snap_angle`].
const HALF_ANGLE_STEP: i32 = 22_i32;
/// The seeded facing draws one of this many compass points.
const OCTANTS: u32 = 8_u32;
/// Degrees between two compass points of the seeded facing.
const OCTANT_DEGREES: u32 = 45_u32;
/// The xorshift32 shifts of [`next_u32`].
const XOR_SHIFT_A: u32 = 13_u32;
/// Second xorshift32 shift.
const XOR_SHIFT_B: u32 = 17_u32;
/// Third xorshift32 shift.
const XOR_SHIFT_C: u32 = 5_u32;

/// How the facing of the things one [`distribute`](fn@super::distribute) run
/// places is chosen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Facing {
    /// Every thing faces this angle, already snapped by [`snap_angle`].
    Fixed(i32),
    /// Every thing faces the map's player 1 start.
    PlayerStart,
    /// Every thing faces its area's centre.
    Centre,
    /// A deterministic pseudo-random compass point per thing: a xorshift32
    /// advanced from `seed` in placement order, the only randomness here.
    Seeded(u32),
}

impl Facing {
    /// The seed of [`Self::Seeded`], `None` for every other mode.
    #[must_use]
    #[inline]
    pub const fn seed(self) -> Option<u32> {
        match self {
            Self::Seeded(seed) => Some(seed),
            Self::Fixed(_) | Self::PlayerStart | Self::Centre => None,
        }
    }
}

/// `degrees` normalised into `0..=359` and rounded to the nearest
/// multiple of 45.
#[must_use]
#[inline]
pub const fn snap_angle(degrees: i32) -> i32 {
    let turned = degrees.rem_euclid(DEGREES);
    let steps = turned
        .saturating_add(HALF_ANGLE_STEP)
        .saturating_div(ANGLE_STEP);
    steps.saturating_mul(ANGLE_STEP).rem_euclid(DEGREES)
}

/// The direction from `from` to `to` in whole degrees, `0..=359`.
pub(super) fn bearing(from: Pt, to: Pt) -> i32 {
    let dx = to.0.saturating_sub(from.0);
    let dy = to.1.saturating_sub(from.1);
    i32::try_from(atan2(dy, dx).to_degrees()).unwrap_or_default()
}

/// The next value of the xorshift32 stream; a zero state is replaced by
/// one so the generator never sticks.
pub(super) const fn next_u32(state: u32) -> u32 {
    let mut x = if state == 0_u32 { 1_u32 } else { state };
    x ^= x.wrapping_shl(XOR_SHIFT_A);
    x ^= x.wrapping_shr(XOR_SHIFT_B);
    x ^= x.wrapping_shl(XOR_SHIFT_C);
    x
}

/// The facing mode with the state a [`distribute`](fn@super::distribute) run
/// carries along.
pub(super) struct FacingRun {
    /// The mode the angles answer with.
    mode: Facing,
    /// The player start [`Facing::PlayerStart`] faces, when the map has one.
    start: Option<Pt>,
    /// The xorshift32 state [`Facing::Seeded`] advances.
    random: u32,
}

impl FacingRun {
    /// The run state for `mode` on `map`.
    pub(super) fn new(mode: Facing, map: &Map) -> Self {
        Self {
            mode,
            start: match mode {
                Facing::PlayerStart => player_start(map),
                Facing::Fixed(_) | Facing::Centre | Facing::Seeded(_) => None,
            },
            random: mode.seed().unwrap_or_default(),
        }
    }

    /// The facing of the thing standing at `at` in an area centred `centre`.
    pub(super) fn angle(&mut self, at: Pt, centre: Pt) -> i32 {
        match self.mode {
            Facing::Fixed(angle) => angle,
            Facing::PlayerStart => self
                .start
                .map_or(0_i32, |start| snap_angle(bearing(at, start))),
            Facing::Centre => snap_angle(bearing(at, centre)),
            Facing::Seeded(_) => self.next_seeded(),
        }
    }

    /// The next seeded compass point, `0` to `315` in steps of 45.
    fn next_seeded(&mut self) -> i32 {
        self.random = next_u32(self.random);
        let octant = self.random.checked_rem(OCTANTS).unwrap_or_default();
        i32::try_from(octant.saturating_mul(OCTANT_DEGREES)).unwrap_or_default()
    }
}
