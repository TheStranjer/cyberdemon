//! Splitting segs at partition crossings.
//!
//! A seg classified [`SegPlace::Crossing`] is cut at the intersection of
//! its line with the partition: `t = num / den` where `num` is the side
//! value of the seg start and `den` the cross product of the partition
//! direction with the seg direction; the new point is
//! `start + (end - start) * num / den` rounded to the nearest 16.16 in
//! `i128`. The piece that continues past the cut inherits its start's
//! partition side, and its `offset` grows by the distance from the old
//! start to the cut. If the rounded cut would land on either endpoint,
//! the lines are parallel, or the arithmetic leaves the range, the seg
//! is not split and goes whole to the side of its endpoint further from
//! the line.

use crate::bsp::side::{FixedLine, SegPlace, classify_seg, side_value};
use crate::bsp::{BuildSeg, Point};
use crate::fixed::hypot;

/// Where a seg ends up relative to a partition line.
pub(crate) enum Placement {
    /// Whole seg on the right (front).
    Right(BuildSeg),
    /// Whole seg on the left (back).
    Left(BuildSeg),
    /// The seg was cut; the two halves, one per side.
    Split {
        /// The half that lies right of the line.
        right: BuildSeg,
        /// The half that lies left of the line.
        left: BuildSeg,
    },
}

/// Place `seg` on `line`: classify, and on a crossing cut the seg, or
/// fall back to the endpoint further from the line (ties to the start)
/// when the cut degenerates.
pub(crate) fn place_seg(line: &FixedLine, seg: &BuildSeg) -> Placement {
    match classify_seg(line, seg) {
        SegPlace::Right => Placement::Right(*seg),
        SegPlace::Left => Placement::Left(*seg),
        SegPlace::Crossing => cut_point(line, seg)
            .map_or_else(|| fallback(line, seg), |cut| split_pieces(line, seg, cut)),
    }
}

/// The two halves of a cut seg, each on the side of the endpoint it
/// contains.
fn split_pieces(line: &FixedLine, seg: &BuildSeg, cut: Point) -> Placement {
    let start_piece = BuildSeg { end: cut, ..*seg };
    let distance = hypot(
        cut.x.saturating_sub(seg.start.x),
        cut.y.saturating_sub(seg.start.y),
    );
    let end_piece = BuildSeg {
        start: cut,
        offset: seg
            .offset
            .saturating_add(i64::try_from(distance).unwrap_or(i64::MAX)),
        ..*seg
    };
    if side_value(line, seg.start) > 0_i128 {
        Placement::Split {
            right: start_piece,
            left: end_piece,
        }
    } else {
        Placement::Split {
            right: end_piece,
            left: start_piece,
        }
    }
}

/// The side the whole seg takes when it cannot be split: the endpoint
/// with the larger absolute side value decides, the start on a tie.
fn fallback(line: &FixedLine, seg: &BuildSeg) -> Placement {
    let start = side_value(line, seg.start);
    let end = side_value(line, seg.end);
    let chosen = if start.saturating_abs() >= end.saturating_abs() {
        start
    } else {
        end
    };
    if chosen > 0_i128 {
        Placement::Right(*seg)
    } else {
        Placement::Left(*seg)
    }
}

/// The interior intersection of the seg's line with `line`, rounded to
/// 16.16, or `None` when the lines are parallel, the intersection is
/// outside the seg, the arithmetic overflows, or the rounded point
/// equals either endpoint.
fn cut_point(line: &FixedLine, seg: &BuildSeg) -> Option<Point> {
    let vx = i128::from(seg.end.x).saturating_sub(i128::from(seg.start.x));
    let vy = i128::from(seg.end.y).saturating_sub(i128::from(seg.start.y));
    let den = i128::from(line.dx)
        .saturating_mul(vy)
        .saturating_sub(i128::from(line.dy).saturating_mul(vx));
    let num = side_value(line, seg.start);
    if den == 0_i128 || num == 0_i128 || !between(num, den) {
        return None;
    }
    let x = seg.start.x.checked_add(scale_round(vx, num, den)?)?;
    let y = seg.start.y.checked_add(scale_round(vy, num, den)?)?;
    let cut = Point { x, y };
    if cut == seg.start || cut == seg.end {
        return None;
    }
    Some(cut)
}

/// Whether `num / den` lies strictly inside `(0, 1)`.
const fn between(num: i128, den: i128) -> bool {
    let (Some(magnitude), Some(divisor)) = (num.checked_abs(), den.checked_abs()) else {
        return false;
    };
    magnitude < divisor && (num > 0_i128) == (den > 0_i128)
}

/// `value * num / den` rounded to the nearest (halves away from zero)
/// through an `i128` intermediate, `None` on overflow of `i128` or of
/// the `i64` result. Shared with the polygon clipper, which cuts at the
/// same rounded points the seg splitter does.
pub(crate) fn scale_round(value: i128, num: i128, den: i128) -> Option<i64> {
    let product = value.checked_mul(num)?;
    let quotient = product.checked_div(den)?;
    let remainder = product.checked_rem(den)?;
    let twice = remainder.checked_abs()?.checked_mul(2_i128)?;
    if twice < den.checked_abs()? {
        return i64::try_from(quotient).ok();
    }
    let rounded = if (remainder >= 0_i128) == (den >= 0_i128) {
        quotient.checked_add(1)?
    } else {
        quotient.checked_sub(1)?
    };
    i64::try_from(rounded).ok()
}

#[cfg(test)]
mod tests {
    use super::{Placement, place_seg, scale_round};
    use crate::bsp::side::FixedLine;
    use crate::bsp::{BuildSeg, Point};
    use crate::wad::map::Vertex;

    /// A fixed point at whole map units.
    fn point(x: i64, y: i64) -> Point {
        Point::from_vertex(&Vertex {
            x: i32::try_from(x).unwrap(),
            y: i32::try_from(y).unwrap(),
            ..Default::default()
        })
    }

    /// A seg between whole-unit endpoints, carrying offset `0`.
    fn seg(x1: i64, y1: i64, x2: i64, y2: i64) -> BuildSeg {
        BuildSeg {
            start: point(x1, y1),
            end: point(x2, y2),
            linedef: 3,
            side: 0,
            offset: 0,
            sector: 5,
        }
    }

    /// A north-facing partition line 1000 units long through the origin.
    const fn north_line() -> FixedLine {
        FixedLine {
            x: 0,
            y: 0,
            dx: 0,
            dy: 65_536_000,
        }
    }

    #[test]
    fn a_horizontal_seg_splits_at_the_exact_midpoint() {
        let line = north_line();
        let mut source = seg(-128, 0, 128, 0);
        source.offset = 5_i64.checked_mul(0x1_0000).unwrap();
        let placement = place_seg(&line, &source);
        let Placement::Split { right, left } = placement else {
            panic!("expected a split");
        };
        assert_eq!(right.start, point(0, 0), "right half starts at the cut");
        assert_eq!(right.end, point(128, 0), "right half ends at the seg end");
        assert_eq!(left.start, point(-128, 0), "left half keeps the start");
        assert_eq!(left.end, point(0, 0), "left half ends at the cut");
        assert_eq!(
            right.offset,
            source.offset.checked_add(0x80_0000).unwrap(),
            "the piece past the cut advances its offset by the start-to-cut distance"
        );
        assert_eq!(
            left.offset, source.offset,
            "the first piece keeps the offset"
        );
        for piece in [right, left] {
            assert_eq!(piece.linedef, 3, "provenance is kept");
            assert_eq!(piece.side, 0, "side is kept");
            assert_eq!(piece.sector, 5, "sector is kept");
        }
    }

    #[test]
    fn a_cut_at_two_thirds_rounds_to_nearest_and_flips_the_pieces() {
        let line = north_line();
        // East-facing start, westward seg: the right piece keeps the
        // start and ends at the cut two thirds along.
        let placement = place_seg(&line, &seg(2, 1, -1, -1));
        let Placement::Split { right, left } = placement else {
            panic!("expected a split");
        };
        let cut = Point { x: 0, y: -21_845 };
        assert_eq!(right.start, point(2, 1), "right piece starts at the start");
        assert_eq!(right.end, cut, "the cut rounds 1 - 4/3 units to -21845");
        assert_eq!(right.offset, 0, "the first piece keeps the offset");
        assert_eq!(left.start, cut, "the second piece starts at the cut");
        assert_eq!(left.end, point(-1, -1), "the second piece keeps the end");
        assert_eq!(
            left.offset, 157_528,
            "hypot(-131072, -87381) floors to 157528"
        );
    }

    #[test]
    fn whole_segs_are_not_split() {
        let line = north_line();
        assert!(
            matches!(place_seg(&line, &seg(2, 0, 2, 4)), Placement::Right(_)),
            "east seg stays right"
        );
        assert!(
            matches!(place_seg(&line, &seg(-2, 0, -2, 4)), Placement::Left(_)),
            "west seg stays left"
        );
    }

    #[test]
    fn overflow_in_the_cut_falls_back_to_the_dominant_endpoint() {
        let line = north_line();
        let s = BuildSeg {
            start: Point {
                x: i64::MIN.checked_add(0x400).unwrap(),
                y: 0_i64,
            },
            end: Point {
                x: i64::MAX.checked_sub(0x4_0000_0000_0000).unwrap(),
                y: 0_i64,
            },
            linedef: 0,
            side: 0,
            offset: 0,
            sector: 0,
        };
        assert!(
            matches!(place_seg(&line, &s), Placement::Left(_)),
            "the wider |c| at the start side wins when the cut cannot be computed"
        );
    }

    #[test]
    fn scale_round_matches_nearest_away_from_zero_and_bails() {
        assert_eq!(scale_round(3, 1, 3), Some(1), "3 * 1/3 = 1");
        assert_eq!(scale_round(2, 1, 3), Some(1), "2/3 rounds up");
        assert_eq!(
            scale_round(-2, 1, 3),
            Some(-1),
            "-2/3 rounds away from zero"
        );
        assert_eq!(scale_round(1, 1, 2), Some(1), "0.5 tie goes away from zero");
        assert_eq!(scale_round(10, 1, 1_000_000_000), Some(0), "tiny ratio");
        assert_eq!(scale_round(5, 2, 0), None, "zero denominator");
        assert_eq!(
            scale_round(i128::from(i64::MAX), i128::MAX, 1_i128),
            None,
            "product leaves i128"
        );
        assert_eq!(
            scale_round(4, i128::from(i64::MAX), 1_i128),
            None,
            "quotient leaves the i64 result range"
        );
    }
}
