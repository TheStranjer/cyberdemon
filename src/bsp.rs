//! In-memory `BSP` node builder: segs, partitions and the tree.
//!
//! This is the pure builder behind future `NODES` generation. It turns a
//! [`Map`](crate::wad::map::Map) into a [`BspTree`]: every linedef side
//! becomes a [`BuildSeg`], the builder recursively picks a [`Partition`]
//! line taken from one of the segs' source linedefs, puts the segs on its
//! right into one child and the segs on its left into the other (splitting
//! the segs that cross), and stops when the segs of a child form a convex
//! region, a subsector.
//!
//! Nothing here touches archives, sessions or `MCP` types, and no float is
//! involved: coordinates are `Doom` 16.16 fixed values held in `i64`
//! ([`Point`]), partitions are whole-unit [`Partition`] lines, and every
//! crossing test and product is computed in `i128`.
//! [`build_tree`] is the entry point; [`vanilla::to_vanilla`] renders a
//! finished tree as the vanilla `SEGS`/`SSECTORS`/`NODES` record bytes,
//! [`extended::to_extended`] as one `ZDoom` extended lump, and
//! [`gl::to_gl`] as the `glBSP` v2 `GL_*` run, whose miniseg-closed
//! subsector polygons `OpenGL` ports need (grown over the clipped
//! polygons of [`polygon`]). The node record, the child-word encoding and
//! the vertex table those three writers share live in `bsp/records.rs`;
//! the maps, segs and trees they are tested against in the test-only
//! `bsp/fixtures.rs`.

pub mod choose;
pub mod extended;
#[cfg(test)]
mod fixtures;
pub mod gl;
pub mod polygon;
mod records;
pub mod seg;
pub mod side;
pub mod split;
pub mod tree;
pub mod vanilla;

pub use extended::{ExtendedNodes, to_extended};
pub use gl::{GlNodes, to_gl};
pub use seg::BuildSeg;
pub use tree::{BBox, BspTree, BuiltNode, Child, build_tree};
pub use vanilla::{VanillaNodes, to_vanilla};

use crate::wad::map::Vertex;

/// A map point in `Doom` 16.16 fixed coordinates held in `i64`, so sums
/// and crossings of realistic maps never leave the range.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Point {
    /// `X` in 16.16 fixed.
    pub x: i64,
    /// `Y` in 16.16 fixed.
    pub y: i64,
}

impl Point {
    /// The vertex as a 16.16 [`Point`]; the shift of an `i32` into `i64`
    /// cannot fail.
    #[must_use]
    #[inline]
    pub fn from_vertex(vertex: &Vertex) -> Self {
        Self {
            x: records::scaled(vertex.x),
            y: records::scaled(vertex.y),
        }
    }
}

/// A partition line in whole map units, always taken from a linedef so
/// its coordinates are integral. Side 0 faces `v1` to `v2`, side 1 back
/// along the same line from `v2` (Doom's seg convention).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Partition {
    /// `X` of the line's origin point, map units.
    pub x: i32,
    /// `Y` of the line's origin point, map units.
    pub y: i32,
    /// `X` of the line's direction vector, map units.
    pub dx: i32,
    /// `Y` of the line's direction vector, map units.
    pub dy: i32,
}

impl Partition {
    /// The partition of one side of the linedef `v1` to `v2`: `(v1, 0)`
    /// runs `v1` to `v2`, `(v2, 1)` runs `v2` back to `v1`. The
    /// differences saturate at the `i32` extremes.
    #[must_use]
    #[inline]
    pub const fn from_vertices(v1: &Vertex, v2: &Vertex, side: u8) -> Self {
        if side == 0 {
            Self {
                x: v1.x,
                y: v1.y,
                dx: v2.x.saturating_sub(v1.x),
                dy: v2.y.saturating_sub(v1.y),
            }
        } else {
            Self {
                x: v2.x,
                y: v2.y,
                dx: v1.x.saturating_sub(v2.x),
                dy: v1.y.saturating_sub(v2.y),
            }
        }
    }
}

/// Taste parameters of [`build_tree`], with vanilla-flavoured defaults.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BuildOptions {
    /// Cost added by each seg a candidate partition splits.
    pub split_cost: i64,
    /// Upper bound on partition candidates evaluated per region; more
    /// distinct candidates are sampled evenly. `0` means no candidate is
    /// ever evaluated, so non-convex regions fail with an error.
    pub max_candidates: usize,
}

impl Default for BuildOptions {
    /// [`split_cost`](Self::split_cost) `8`,
    /// [`max_candidates`](Self::max_candidates) `128`.
    #[inline]
    fn default() -> Self {
        Self {
            split_cost: 8_i64,
            max_candidates: 128_usize,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::bsp::{BuildOptions, Partition, Point};
    use crate::wad::map::Vertex;

    #[test]
    fn options_default_to_the_documented_values() {
        let opts = BuildOptions::default();
        assert_eq!(opts.split_cost, 8_i64, "default split cost");
        assert_eq!(opts.max_candidates, 128_usize, "default candidate cap");
    }

    #[test]
    fn point_from_vertex_scales_to_16_16() {
        let point = Point::from_vertex(&Vertex {
            x: 256,
            y: -3,
            ..Default::default()
        });
        assert_eq!(point.x, 256_i64.checked_mul(0x1_0000).unwrap(), "x scaled");
        assert_eq!(point.y, (-3_i64).checked_mul(0x1_0000).unwrap(), "y scaled");
        assert_eq!(Point::default(), Point { x: 0, y: 0 }, "zero point");
    }

    #[test]
    fn partition_side_0_runs_v1_to_v2() {
        let v1 = Vertex {
            x: 4,
            y: 6,
            ..Default::default()
        };
        let v2 = Vertex {
            x: 10,
            y: 6,
            ..Default::default()
        };
        assert_eq!(
            Partition::from_vertices(&v1, &v2, 0),
            Partition {
                x: 4,
                y: 6,
                dx: 6,
                dy: 0
            },
            "front side"
        );
        assert_eq!(
            Partition::from_vertices(&v1, &v2, 1),
            Partition {
                x: 10,
                y: 6,
                dx: -6,
                dy: 0
            },
            "back side runs v2 to v1"
        );
    }
}
