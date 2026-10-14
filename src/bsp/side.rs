//! Side tests: which side of a line a point or a seg falls on.
//!
//! [`point_side`] is `Doom`'s `R_PointOnSide` convention: `c` is positive
//! to the right (front) of the partition and zero or negative to the left
//! (back). [`classify_seg`] adds `Doom`'s node-builder tolerance: an
//! endpoint counts as *on* the line when `|c|` is within the partition
//! length scaled by `1/64` (in 16.16 units, `|c| <= len * 1024`), so
//! segs lying on the line, touching it or grazing it are never treated
//! as crossings.

use crate::bsp::{BuildSeg, Partition, Point};
use crate::fixed::hypot;

/// A line in 16.16 fixed coordinates: origin `(x, y)` and direction
/// `(dx, dy)`, all `i64`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FixedLine {
    /// `X` of the origin point, 16.16 fixed.
    pub x: i64,
    /// `Y` of the origin point, 16.16 fixed.
    pub y: i64,
    /// `X` of the direction vector, 16.16 fixed.
    pub dx: i64,
    /// `Y` of the direction vector, 16.16 fixed.
    pub dy: i64,
}

impl FixedLine {
    /// The integer partition scaled into 16.16; the shift cannot fail.
    #[must_use]
    #[inline]
    pub fn from_partition(partition: Partition) -> Self {
        let shift = |value: i32| i64::from(value).checked_shl(crate::fixed::FRACBITS);
        Self {
            x: shift(partition.x).unwrap_or(0_i64),
            y: shift(partition.y).unwrap_or(0_i64),
            dx: shift(partition.dx).unwrap_or(0_i64),
            dy: shift(partition.dy).unwrap_or(0_i64),
        }
    }

    /// The line through a seg, origin at its start, direction along it.
    /// Used by the convexity test, which sees each seg as its own line.
    #[must_use]
    #[inline]
    pub const fn through_seg(seg: &BuildSeg) -> Self {
        Self {
            x: seg.start.x,
            y: seg.start.y,
            dx: seg.end.x.saturating_sub(seg.start.x),
            dy: seg.end.y.saturating_sub(seg.start.y),
        }
    }
}

/// Which half-plane a point falls in, strictly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    /// Right (front): the side `Doom` numbers `0`.
    Right,
    /// Left (back): the side `Doom` numbers `1`, including points exactly
    /// on the line.
    Left,
}

/// The tolerance sign of a point: left, on (within slack) or right.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sign {
    /// Strictly left of the line, beyond the tolerance.
    Left,
    /// On the line, within the tolerance.
    On,
    /// Strictly right of the line, beyond the tolerance.
    Right,
}

/// Where a seg lies relative to a line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SegPlace {
    /// Entirely right: both ends right, or one on and one right, or the
    /// whole seg on the line running in the line's direction.
    Right,
    /// Entirely left: the mirror of [`SegPlace::Right`], including a seg
    /// lying on the line against its direction.
    Left,
    /// The ends are strictly on opposite sides: the seg crosses.
    Crossing,
}

/// `c = dy * (p.x - x) - dx * (p.y - y)` in `i128`; positive is right.
/// The products saturate, which keeps the sign for every real map.
#[must_use]
#[inline]
pub fn side_value(line: &FixedLine, point: Point) -> i128 {
    let px = i128::from(point.x).saturating_sub(i128::from(line.x));
    let py = i128::from(point.y).saturating_sub(i128::from(line.y));
    i128::from(line.dy)
        .saturating_mul(px)
        .saturating_sub(i128::from(line.dx).saturating_mul(py))
}

/// The on-line tolerance for `line`: `len * 1024`, the 16.16 length of
/// the direction vector times `1/64` of a unit, so a point within
/// `1/64` map unit of the line counts as on it.
#[must_use]
#[inline]
pub fn on_line_slack(line: &FixedLine) -> i128 {
    i128::from(hypot(line.dx, line.dy)).saturating_mul(1024_i128)
}

/// The side of `point` with the tolerance applied.
#[must_use]
#[inline]
pub fn sign_of(line: &FixedLine, point: Point) -> Sign {
    let value = side_value(line, point);
    let slack = on_line_slack(line);
    if value > slack {
        Sign::Right
    } else if value.checked_neg().is_some_and(|negated| negated > slack) {
        Sign::Left
    } else {
        Sign::On
    }
}

/// `Doom`'s `R_PointOnSide`: right (front, `0`) above the line, left
/// (back, `1`) on or below it, with no tolerance.
#[must_use]
#[inline]
pub fn point_side(partition: Partition, point: Point) -> Side {
    if side_value(&FixedLine::from_partition(partition), point) > 0_i128 {
        Side::Right
    } else {
        Side::Left
    }
}

/// Whether the seg runs in the line's direction (dot product positive).
fn same_direction(line: &FixedLine, seg: &BuildSeg) -> bool {
    let vx = i128::from(seg.end.x).saturating_sub(i128::from(seg.start.x));
    let vy = i128::from(seg.end.y).saturating_sub(i128::from(seg.start.y));
    i128::from(line.dx)
        .saturating_mul(vx)
        .saturating_add(i128::from(line.dy).saturating_mul(vy))
        > 0_i128
}

/// Classify a seg against a line: right, left or crossing, with both
/// ends tested under the on-line tolerance and a seg lying along the
/// line placed by its direction.
#[must_use]
#[inline]
pub fn classify_seg(line: &FixedLine, seg: &BuildSeg) -> SegPlace {
    match (sign_of(line, seg.start), sign_of(line, seg.end)) {
        (Sign::Right, Sign::Right | Sign::On) | (Sign::On, Sign::Right) => SegPlace::Right,
        (Sign::Left, Sign::Left | Sign::On) | (Sign::On, Sign::Left) => SegPlace::Left,
        (Sign::On, Sign::On) => {
            if same_direction(line, seg) {
                SegPlace::Right
            } else {
                SegPlace::Left
            }
        }
        (Sign::Right, Sign::Left) | (Sign::Left, Sign::Right) => SegPlace::Crossing,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        FixedLine, SegPlace, Side, Sign, classify_seg, on_line_slack, point_side, sign_of,
    };
    use crate::bsp::{BuildSeg, Partition, Point};
    use crate::wad::map::Vertex;

    /// A fixed point at whole map units.
    fn point(x: i64, y: i64) -> Point {
        Point::from_vertex(Vertex {
            x: i32::try_from(x).unwrap(),
            y: i32::try_from(y).unwrap(),
        })
    }

    /// A seg between whole-unit endpoints.
    fn seg(x1: i64, y1: i64, x2: i64, y2: i64) -> BuildSeg {
        BuildSeg {
            start: point(x1, y1),
            end: point(x2, y2),
            linedef: 0,
            side: 0,
            offset: 0,
            sector: 0,
        }
    }

    /// A north-facing line at `x = 0` one thousand units long.
    const fn north_line() -> FixedLine {
        FixedLine {
            x: 0,
            y: 0,
            dx: 0,
            dy: 65_536_000,
        }
    }

    #[test]
    fn point_side_matches_hand_computed_signs() {
        // East-facing line through the origin: south is right, north is
        // left, on the line counts as left/back.
        let east = Partition {
            x: 0,
            y: 0,
            dx: 1,
            dy: 0,
        };
        assert_eq!(
            point_side(east, point(0, -5)),
            Side::Right,
            "south of an east line is right"
        );
        assert_eq!(
            point_side(east, point(0, 5)),
            Side::Left,
            "north of an east line is left"
        );
        assert_eq!(
            point_side(east, point(123, 0)),
            Side::Left,
            "exactly on the line is back"
        );
        // Diagonal partition (2,3) direction (1,1): c = (px-2) - (py-3).
        let diagonal = Partition {
            x: 2,
            y: 3,
            dx: 1,
            dy: 1,
        };
        assert_eq!(
            point_side(diagonal, point(5, 5)),
            Side::Right,
            "c = 3 - 2 = 1 > 0"
        );
        assert_eq!(
            point_side(diagonal, point(5, 7)),
            Side::Left,
            "c = 3 - 4 = -1 < 0"
        );
        assert_eq!(
            point_side(diagonal, point(3, 4)),
            Side::Left,
            "c = 0, on the line, is left"
        );
    }

    #[test]
    fn point_side_uses_scaled_partition_coordinates() {
        // North-facing line at x = 256: east (right) of it is positive.
        let vertical = Partition {
            x: 256,
            y: 0,
            dx: 0,
            dy: 128,
        };
        assert_eq!(
            point_side(vertical, point(384, 64)),
            Side::Right,
            "east is right"
        );
        assert_eq!(
            point_side(vertical, point(128, 64)),
            Side::Left,
            "west is left"
        );
        assert_eq!(
            point_side(vertical, point(256, 64)),
            Side::Left,
            "on the line is left"
        );
    }

    #[test]
    fn tolerance_counts_a_grazing_point_as_on_the_line() {
        let line = north_line();
        assert_eq!(
            on_line_slack(&line),
            65_536_000_i128.checked_mul(1024_i128).unwrap(),
            "slack is the 16.16 length times 1024"
        );
        // c = dy * x, so 1024 fixed units of x put c exactly at the
        // slack (on) and 1025 put it past (right / left).
        assert_eq!(
            sign_of(&line, Point { x: 1024, y: 0 }),
            Sign::On,
            "|c| == slack is on"
        );
        assert_eq!(
            sign_of(&line, Point { x: 1025, y: 0 }),
            Sign::Right,
            "beyond slack east is right"
        );
        assert_eq!(
            sign_of(&line, Point { x: -1025, y: 0 }),
            Sign::Left,
            "beyond slack west is left"
        );
    }

    #[test]
    fn classify_places_collinear_segs_by_direction() {
        let line = FixedLine {
            x: 0,
            y: 0,
            dx: 0x1_0000,
            dy: 0,
        };
        assert_eq!(
            classify_seg(&line, &seg(-5, 0, 5, 0)),
            SegPlace::Right,
            "along the line, same direction"
        );
        assert_eq!(
            classify_seg(&line, &seg(5, 0, -5, 0)),
            SegPlace::Left,
            "along the line, opposite direction"
        );
        assert_eq!(
            classify_seg(&line, &seg(-5, 5, 5, 5)),
            SegPlace::Left,
            "a seg north of an east line is left"
        );
    }

    #[test]
    fn classify_places_mixed_and_crossing_segs() {
        let line = north_line();
        assert_eq!(
            classify_seg(&line, &seg(0, 0, 64, 64)),
            SegPlace::Right,
            "on the line then right"
        );
        assert_eq!(
            classify_seg(&line, &seg(-64, 64, 0, 0)),
            SegPlace::Left,
            "left then on the line"
        );
        assert_eq!(
            classify_seg(&line, &seg(-64, 0, 64, 0)),
            SegPlace::Crossing,
            "west to east crosses"
        );
        assert_eq!(
            classify_seg(&line, &seg(64, 32, 64, -32)),
            SegPlace::Right,
            "a vertical seg wholly east is right"
        );
    }
}
