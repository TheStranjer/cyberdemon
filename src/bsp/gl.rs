//! `glBSP` v2 serialization of a built [`BspTree`]: the `GL_*` lump run
//! whose every subsector is a closed convex polygon.
//!
//! `OpenGL` ports cannot draw a subsector whose seg loop leaves gaps
//! along the partition lines, so this builder recreates every region
//! polygon (the map box, the ancestor partition halves and the leaf's own
//! seg lines — see [`polygon`](super::polygon)) and stretches a
//! **miniseg** (a seg with linedef `0xFFFF`) over every edge stretch no
//! real seg covers, snapping endpoints within `1/16` unit so each
//! subsector's loop closes exactly, clockwise, its interior right.
//! Segs that face each other across a partition (one's `(v1, v2)` is the
//! other's `(v2, v1)`) become **partners**; unmatched segs get partner
//! `0xFFFF`. The result is the record lumps: `GL_VERT` (`gNd2` magic plus
//! the fraction points beyond `VERTEXES`), `GL_SEGS` (10 bytes each),
//! `GL_SSECT` and `GL_NODES` (the vanilla layout, children with `bit 15`
//! marking a subsector), plus an empty `GL_PVS`. Nothing here knows about
//! archives, sessions or `MCP`: the caller owns the lump names and the
//! classic run written alongside.
//!
//! The region polygons and the closed loops stretched over them are built
//! in the private `regions` module; the seg pool, the partner pass and the
//! record lumps live in the private `write` module, whose node record and
//! vertex table are the shared `bsp/records.rs` ones. The constants
//! below are shared by both, the `Internal` refusals at the bottom of this
//! file belong to the run itself, and [`GlNodes`] with [`to_gl`] are the
//! whole outward face.

mod regions;
mod write;

use crate::bsp::BspTree;
use crate::error::CyberError;
use crate::wad::map::Map;
use regions::{close_region, region_polygons};
use write::{SegPool, VertexWords, write_nodes, write_vertices};

/// Whole map units the bounding box is enlarged by.
const MARGIN_UNITS: i64 = 64;
/// Snap threshold and miniseg cutoff: `1/16` of a unit, raw `16.16`.
const SNAP: u64 = 4096_u64;
/// Collinearity slack as a multiple of the edge length: `1/16` unit.
const COLLINEAR_SLACK: i128 = 4096_i128;
/// `GL_VERT` magic of the `glBSP` v2 run.
const MAGIC_GND2: &[u8; 4] = b"gNd2";
/// Flag marking a seg endpoint as an index into `GL_VERT`.
const GL_VERTEX_FLAG: u16 = 0x8000_u16;
/// `linedef` word of a miniseg.
const MINISEG_LINEDEF: u16 = 0xFFFF_u16;
/// Partner word of a seg with no partner.
const NO_PARTNER: u16 = 0xFFFF_u16;
/// Highest vertex, node or subsector index below the `bit 15` flag.
const MAX_INDEX: usize = 0x7FFF_usize;
/// Highest `GL_SEGS` index below the `0xFFFF` sentinel.
const MAX_SEG_INDEX: usize = 0xFFFE_usize;
/// Highest linedef index below the miniseg sentinel.
const MAX_LINEDEF_INDEX: usize = 0xFFFE_usize;
/// Bytes in one `GL_SEGS` record.
const SEG_RECORD_LEN: usize = 10;
/// Bytes in one `GL_SSECT` record.
const SSECT_RECORD_LEN: usize = 4;
/// Bytes in one `GL_NODES` record.
const NODE_RECORD_LEN: usize = 28;
/// Bytes in one `GL_VERT` row.
const VERTEX_ROW_LEN: usize = 8;

/// The whole `glBSP` v2 run of one tree: the record lumps and the counts
/// a report needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlNodes {
    /// The `GL_VERT` lump: `gNd2` magic, then new vertices as raw
    /// `16.16` `i32` pairs.
    pub vertices: Vec<u8>,
    /// The `GL_SEGS` lump: 10 bytes per seg, in subsector order.
    pub segs: Vec<u8>,
    /// The `GL_SSECT` lump: 4 bytes per subsector (count, first).
    pub ssectors: Vec<u8>,
    /// The `GL_NODES` lump: 28 bytes per node, root last.
    pub nodes: Vec<u8>,
    /// The `GL_PVS` lump: always empty (no visibility set is computed).
    pub pvs: Vec<u8>,
    /// Total segs written, real and mini alike.
    pub seg_count: usize,
    /// Minisegs among them.
    pub minisegs: usize,
    /// Vertices written to `GL_VERT`, beyond the map's own.
    pub new_vertices: usize,
}

/// Serialize `tree` as the `glBSP` v2 `GL_*` lumps over `map`.
///
/// # Errors
///
/// [`CyberError::LimitExceeded`] when a vertex, seg, node, subsector or
/// linedef index leaves the words the records hold, or a `GL_VERT`
/// coordinate leaves `i32`; [`CyberError::Internal`] when a region
/// collapses or a seg cannot be placed on its subsector's polygon.
#[inline]
pub fn to_gl(map: &Map, tree: &BspTree) -> Result<GlNodes, CyberError> {
    let regions = region_polygons(map, tree)?;
    let mut vertices = VertexWords::new(map)?;
    let mut run = SegPool::default();
    for (index, region) in regions.iter().enumerate() {
        let segs = tree.subsector_segs(index).ok_or_else(|| broken(index))?;
        run.emit(close_region(region, segs)?, &mut vertices)?;
    }
    let new_vertices = vertices.added_count();
    let records = run.write()?;
    Ok(GlNodes {
        vertices: write_vertices(&vertices),
        segs: records.segs,
        ssectors: records.ssectors,
        nodes: write_nodes(tree)?,
        pvs: Vec::new(),
        seg_count: records.seg_count,
        minisegs: records.minisegs,
        new_vertices,
    })
}

/// The `Internal` refusal for a map whose vertices are gone.
fn no_vertices() -> CyberError {
    CyberError::Internal(
        "the GL build found a map with no vertices to bound: give the map \
         geometry before building nodes"
            .to_owned(),
    )
}

/// The `Internal` refusal for a tree reference that is not there.
fn broken(index: usize) -> CyberError {
    CyberError::Internal(format!(
        "the node tree references subsector or node {index}, which it does \
         not hold: rebuild the nodes"
    ))
}

/// The `Internal` refusal for a region the clips reduced below a polygon.
fn collapsed() -> CyberError {
    CyberError::Internal(
        "a GL subsector region collapsed below a triangle: its partition and \
         seg lines leave no area; build variant \"classic\" and adjust \
         split_cost"
            .to_owned(),
    )
}

/// The `Internal` refusal for a real seg no polygon edge could hold.
fn lost_seg(seg: usize, linedef: usize, side: u8) -> CyberError {
    CyberError::Internal(format!(
        "seg {seg} (linedef {linedef} side {side}) lies outside every edge of \
         its subsector's region: build variant \"classic\", or adjust \
         split_cost and build again"
    ))
}

#[cfg(test)]
mod tests;
