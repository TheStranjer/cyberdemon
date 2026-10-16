//! Typed parsing of the node-builder output: `SEGS`, `SSECTORS` and
//! `NODES`, in the vanilla record layout and in `ZDoom`'s extended one.
//!
//! A vanilla node run is three parallel record lumps: 12-byte [`Seg`]s,
//! 4-byte [`Subsector`]s (a seg count and the first seg) and 28-byte
//! [`Node`]s, the last node being the root of the tree. `ZDoom` keeps the
//! same logical structure in one lump starting with the magic `XNOD`
//! (stored raw) or `ZNOD` (zlib-compressed after the magic): extra
//! fraction-precise vertices, then subsector seg counts, then wider segs
//! and nodes whose child words reach `bit 31` for the subsector flag.
//! [`parse_vanilla_nodes`] and [`parse_extended_nodes`] both produce a
//! [`NodeSet`]; [`lump_format`] names whatever magic a node lump starts
//! with, including the `GL` variants this module reports but never parses.
//! The writers that rebuild these lumps join here later. Nothing in this
//! module knows about sessions or MCP.

use serde::Serialize;
use std::io::Read as _;

use crate::error::CyberError;
use crate::fixed::Fixed;
use crate::wad::bytes::Reader;

use super::common::records;

/// Bytes in one vanilla `SEGS` record.
const SEG_LEN: usize = 12;
/// Bytes in one vanilla `SSECTORS` record.
const SSECTOR_LEN: usize = 4;
/// Bytes in one vanilla `NODES` record.
const NODE_LEN: usize = 28;
/// Bytes in a node-lump magic.
const MAGIC_LEN: usize = 4;
/// `SSECTORS`-record child flag in a vanilla child word (`bit 15`).
const SUBSECTOR_FLAG: u16 = 0x8000_u16;
/// Low 15 bits of a vanilla child word.
const SUBSECTOR_INDEX: u16 = 0x7FFF_u16;
/// Subsector flag in an extended child word (`bit 31`).
const EXT_SUBSECTOR_FLAG: u32 = 0x8000_0000_u32;
/// Low 31 bits of an extended child word.
const EXT_SUBSECTOR_INDEX: u32 = 0x7FFF_FFFF_u32;
/// Bytes of a seg's side byte in an extended seg record.
const EXT_SEG_LEN: usize = 11;
/// Bytes of one extended node record: 4 `i16` + two 4-`i16` boxes + two
/// `u32` children = 32.
const EXT_NODE_LEN: usize = 32;
/// Cap on what a `ZNOD` payload may inflate to, guarding against bombs.
const MAX_ZNOD_BYTES: usize = 64 * 1024 * 1024;

/// Known node-lump magics with their lowercase report names.
const NODE_MAGICS: [(&[u8; MAGIC_LEN], &str); 8] = [
    (b"XNOD", "xnod"),
    (b"ZNOD", "znod"),
    (b"XGLN", "xgln"),
    (b"ZGLN", "zgln"),
    (b"XGL2", "xgl2"),
    (b"ZGL2", "zgl2"),
    (b"XGL3", "xgl3"),
    (b"ZGL3", "zgl3"),
];

/// One segment between two vertices: a piece of a linedef side that bounds
/// one subsector.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Seg {
    /// Start vertex index.
    pub v1: u32,
    /// End vertex index.
    pub v2: u32,
    /// Vanilla angle: the high 16 bits of the seg's `BAM` direction;
    /// extended segs store no angle and report `0`.
    pub angle: i16,
    /// Linedef this seg lies on.
    pub linedef: u32,
    /// `0` when the seg runs along its linedef, `1` against it; extended
    /// segs store their side (also `0` or `1`) here.
    pub direction: i16,
    /// Vanilla distance along the linedef side from the side's start to
    /// the seg's start; extended segs store no offset and report `0`.
    pub offset: i16,
}

/// One convex leaf of the BSP: a run of consecutive [`Seg`]s.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Subsector {
    /// How many segs start at [`Subsector::first_seg`].
    pub seg_count: u32,
    /// Index of this subsector's first seg in [`NodeSet::segs`].
    pub first_seg: u32,
}

/// A node's child: either an earlier node or a subsector leaf.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Child {
    /// Interior node at this index in [`NodeSet::nodes`].
    Node(u32),
    /// Leaf subsector at this index in [`NodeSet::subsectors`].
    Subsector(u32),
}

/// An axis-aligned bounding box in map units, stored the engine way:
/// `top`/`bottom` for `Y` and `left`/`right` for `X`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct BoundingBox {
    /// Largest `Y`.
    pub top: i16,
    /// Smallest `Y`.
    pub bottom: i16,
    /// Smallest `X`.
    pub left: i16,
    /// Largest `X`.
    pub right: i16,
}

/// One interior `BSP` node: a partition line and the two child sides,
/// each with the bounding box of everything below it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Node {
    /// Partition line start `X`.
    pub x: i16,
    /// Partition line start `Y`.
    pub y: i16,
    /// Partition line direction `X`.
    pub dx: i16,
    /// Partition line direction `Y`.
    pub dy: i16,
    /// Bounding box of the right (front) child side.
    pub right: BoundingBox,
    /// Bounding box of the left (back) child side.
    pub left: BoundingBox,
    /// Right (front) child.
    pub right_child: Child,
    /// Left (back) child.
    pub left_child: Child,
}

/// The layout one [`NodeSet`] was parsed from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum NodesFormat {
    /// The three vanilla record lumps.
    Vanilla,
    /// A raw `XNOD` extended lump.
    Xnod,
    /// A zlib-compressed `ZNOD` extended lump.
    Znod,
}

impl NodesFormat {
    /// The lowercase report name, matching this type's `JSON` form.
    #[must_use]
    #[inline]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Vanilla => "vanilla",
            Self::Xnod => "xnod",
            Self::Znod => "znod",
        }
    }
}

/// A whole parsed node run, from either layout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeSet {
    /// Which layout this was parsed from.
    pub format: NodesFormat,
    /// Fraction-precise vertices the extended format adds to the map's
    /// `VERTEXES` (segs index past the originals to reach them); always
    /// empty for the vanilla format.
    pub vertices_extra: Vec<(Fixed, Fixed)>,
    /// The segs, in lump order.
    pub segs: Vec<Seg>,
    /// The subsectors, in lump order.
    pub subsectors: Vec<Subsector>,
    /// The interior nodes; the last is the root.
    pub nodes: Vec<Node>,
}

impl NodeSet {
    /// The root node's index, or `None` for a node-less set.
    #[must_use]
    #[inline]
    pub fn root(&self) -> Option<usize> {
        self.nodes.len().checked_sub(1)
    }

    /// The tree's depth in node levels: `1` when the root's children are
    /// both subsectors; `None` when there is no node to walk.
    ///
    /// [`parse_vanilla_nodes`] and [`parse_extended_nodes`] reject
    /// child references that do not point strictly backwards, so the
    /// subtree levels below can be folded front to back.
    #[must_use]
    #[inline]
    pub fn depth(&self) -> Option<usize> {
        let root = self.root()?;
        let mut levels: Vec<usize> = Vec::with_capacity(self.nodes.len());
        for node in &self.nodes {
            let right = Self::child_level(&levels, node.right_child)?;
            let left = Self::child_level(&levels, node.left_child)?;
            levels.push(right.max(left));
        }
        levels.get(root).and_then(|level| level.checked_add(1))
    }

    /// The depth below one child: `0` under a leaf, one more than the
    /// child's own level under an earlier node.
    fn child_level(levels: &[usize], child: Child) -> Option<usize> {
        match child {
            Child::Subsector(_) => Some(0),
            Child::Node(target) => {
                let index = usize::try_from(target).ok()?;
                levels.get(index)?.checked_add(1)
            }
        }
    }

    /// Cross-references between the three record kinds: subsector seg
    /// ranges inside the segs, child indices inside their table and node
    /// children strictly backwards.
    ///
    /// # Errors
    ///
    /// [`CyberError::Format`] naming the offending record for any of the
    /// three rules.
    fn validate(&self) -> Result<(), CyberError> {
        for (index, &subsector) in self.subsectors.iter().enumerate() {
            self.check_seg_range(index, subsector)?;
        }
        for (index, node) in self.nodes.iter().enumerate() {
            self.check_child(node.right_child, index)?;
            self.check_child(node.left_child, index)?;
        }
        Ok(())
    }

    /// A subsector's seg range must lie inside the stored segs.
    fn check_seg_range(&self, index: usize, subsector: Subsector) -> Result<(), CyberError> {
        let first = to_index(subsector.first_seg, "subsector first_seg")?;
        let count = to_index(subsector.seg_count, "subsector seg_count")?;
        let Some(end) = first.checked_add(count) else {
            return Err(CyberError::Format(format!(
                "subsector {index} spans {count} segs from {first}: the range \
                 overflows; rebuild the node lumps"
            )));
        };
        if end > self.segs.len() {
            return Err(CyberError::Format(format!(
                "subsector {index} spans segs {first}..{end}, past the {} segs \
                 stored; rebuild the node lumps",
                self.segs.len()
            )));
        }
        Ok(())
    }

    /// One child reference of node `parent`: in range and, for nodes,
    /// strictly pointing backwards.
    fn check_child(&self, child: Child, parent: usize) -> Result<(), CyberError> {
        match child {
            Child::Node(target) => Self::check_node_child(target, parent),
            Child::Subsector(target) => self.check_leaf_child(target, parent),
        }
    }

    /// A node child must name a stored node before `parent`.
    fn check_node_child(target: u32, parent: usize) -> Result<(), CyberError> {
        let next = to_index(target, "node child")?;
        if next >= parent {
            return Err(CyberError::Format(format!(
                "node {parent} references node {next}: children must point at \
                 an earlier node; rebuild the node lumps"
            )));
        }
        Ok(())
    }

    /// A leaf child must name a stored subsector.
    fn check_leaf_child(&self, target: u32, parent: usize) -> Result<(), CyberError> {
        let leaf = to_index(target, "subsector child")?;
        if leaf >= self.subsectors.len() {
            return Err(CyberError::Format(format!(
                "node {parent} references subsector {leaf}, which does not \
                 exist among the {} subsectors",
                self.subsectors.len()
            )));
        }
        Ok(())
    }
}

/// A `u32` index as a `usize` position.
///
/// # Errors
///
/// [`CyberError::Format`] when the value does not fit this target's
/// `usize` (only possible on 16-bit platforms).
#[inline]
fn to_index(value: u32, what: &str) -> Result<usize, CyberError> {
    usize::try_from(value).map_err(|error| {
        CyberError::Format(format!(
            "the {what} {value} does not index this platform: {error}"
        ))
    })
}

/// The lowercase report name of the magic `data` starts with (`xnod`,
/// `znod`, `xgln`, `zgl2`, ...), or `None` for vanilla record data.
#[must_use]
#[inline]
pub fn lump_format(data: &[u8]) -> Option<&'static str> {
    let head = data.get(..MAGIC_LEN)?;
    NODE_MAGICS
        .iter()
        .find(|entry| head == entry.0.as_slice())
        .map(|entry| entry.1)
}

/// Added fraction-precise vertices, raw `16.16` pairs.
type ExtraVertices = Vec<(Fixed, Fixed)>;

/// Parses a vanilla node run from its three record lumps.
///
/// Empty inputs parse to an empty [`NodeSet`]; cross-references are
/// validated the way `NodeSet::validate` spells them.
///
/// # Errors
///
/// [`CyberError::Format`] when a lump's size is not a whole number of its
/// records, a read runs short, or a reference leaves its table.
#[inline]
pub fn parse_vanilla_nodes(
    segs: &[u8],
    ssectors: &[u8],
    nodes: &[u8],
) -> Result<NodeSet, CyberError> {
    let set = NodeSet {
        format: NodesFormat::Vanilla,
        vertices_extra: Vec::new(),
        segs: read_segs(segs)?,
        subsectors: read_subsectors(ssectors)?,
        nodes: read_nodes(nodes)?,
    };
    set.validate()?;
    Ok(set)
}

/// The vanilla `SEGS` records.
fn read_segs(data: &[u8]) -> Result<Vec<Seg>, CyberError> {
    let mut segs = Vec::new();
    for record in records("SEGS", data, SEG_LEN)? {
        let mut reader = Reader::new(record);
        segs.push(Seg {
            v1: u32::from(reader.read_u16()?),
            v2: u32::from(reader.read_u16()?),
            angle: reader.read_i16()?,
            linedef: u32::from(reader.read_u16()?),
            direction: reader.read_i16()?,
            offset: reader.read_i16()?,
        });
    }
    Ok(segs)
}

/// The vanilla `SSECTORS` records.
fn read_subsectors(data: &[u8]) -> Result<Vec<Subsector>, CyberError> {
    let mut subsectors = Vec::new();
    for record in records("SSECTORS", data, SSECTOR_LEN)? {
        let mut reader = Reader::new(record);
        subsectors.push(Subsector {
            seg_count: u32::from(reader.read_u16()?),
            first_seg: u32::from(reader.read_u16()?),
        });
    }
    Ok(subsectors)
}

/// The vanilla `NODES` records.
fn read_nodes(data: &[u8]) -> Result<Vec<Node>, CyberError> {
    let mut nodes = Vec::new();
    for record in records("NODES", data, NODE_LEN)? {
        let mut reader = Reader::new(record);
        let x = reader.read_i16()?;
        let y = reader.read_i16()?;
        let dx = reader.read_i16()?;
        let dy = reader.read_i16()?;
        let right = read_box(&mut reader)?;
        let left = read_box(&mut reader)?;
        nodes.push(Node {
            x,
            y,
            dx,
            dy,
            right,
            left,
            right_child: vanilla_child(reader.read_u16()?),
            left_child: vanilla_child(reader.read_u16()?),
        });
    }
    Ok(nodes)
}

/// One node bounding box: `top`, `bottom`, `left`, `right`.
fn read_box(reader: &mut Reader<'_>) -> Result<BoundingBox, CyberError> {
    Ok(BoundingBox {
        top: reader.read_i16()?,
        bottom: reader.read_i16()?,
        left: reader.read_i16()?,
        right: reader.read_i16()?,
    })
}

/// A vanilla child word: `bit 15` selects a subsector leaf.
#[must_use]
#[inline]
fn vanilla_child(raw: u16) -> Child {
    if raw & SUBSECTOR_FLAG == 0 {
        Child::Node(u32::from(raw))
    } else {
        Child::Subsector(u32::from(raw & SUBSECTOR_INDEX))
    }
}

/// An extended child word: `bit 31` selects a subsector leaf.
#[must_use]
#[inline]
const fn extended_child(raw: u32) -> Child {
    if raw & EXT_SUBSECTOR_FLAG == 0 {
        Child::Node(raw)
    } else {
        Child::Subsector(raw & EXT_SUBSECTOR_INDEX)
    }
}

/// Parses a `ZDoom` extended node lump: bytes starting with the magic
/// `XNOD` (raw payload) or `ZNOD` (zlib-compressed payload).
///
/// # Errors
///
/// [`CyberError::Format`] for a short or unknown magic, a broken `zlib`
/// stream, a section whose records do not fit the remaining bytes,
/// trailing bytes past the last node, a failed `NodeSet::validate` or a
/// subsector seg-count total that does not match the stored segs;
/// [`CyberError::LimitExceeded`] when a `ZNOD` inflates past the 64 MiB
/// decompression cap (see the crate-internal `MAX_ZNOD_BYTES`).
#[inline]
pub fn parse_extended_nodes(data: &[u8]) -> Result<NodeSet, CyberError> {
    let magic: [u8; MAGIC_LEN] = data.get(..MAGIC_LEN).map_or_else(
        || {
            Err(CyberError::Format(format!(
                "an extended node lump needs a {MAGIC_LEN}-byte magic, the data holds {} bytes",
                data.len()
            )))
        },
        |head| Ok(head.try_into().unwrap_or([0_u8; MAGIC_LEN])),
    )?;
    let tail = data.get(MAGIC_LEN..).unwrap_or_default();
    let (payload, format) = match &magic {
        b"XNOD" => (tail.to_vec(), NodesFormat::Xnod),
        b"ZNOD" => (inflate(tail)?, NodesFormat::Znod),
        other => {
            return Err(CyberError::Format(format!(
                "extended node data starts with {:?}, not XNOD or ZNOD: \
                 the GL node formats are reported, not parsed",
                String::from_utf8_lossy(other)
            )));
        }
    };
    parse_extended_payload(&payload).map(|mut set| {
        set.format = format;
        set
    })
}

/// Reads the sections of one (already inflated) extended payload.
fn parse_extended_payload(payload: &[u8]) -> Result<NodeSet, CyberError> {
    let mut reader = Reader::new(payload);
    let vertices_extra = read_extra_vertices(&mut reader)?;
    let subsectors = read_extended_subsectors(&mut reader)?;
    let segs = read_extended_segs(&mut reader)?;
    let nodes = read_extended_nodes(&mut reader, &subsectors, segs.len())?;
    if reader.remaining() > 0 {
        return Err(CyberError::Format(format!(
            "{} bytes trail the last extended node; rebuild the node lump",
            reader.remaining()
        )));
    }
    let set = NodeSet {
        format: NodesFormat::Xnod,
        vertices_extra,
        segs,
        subsectors,
        nodes,
    };
    set.validate()?;
    Ok(set)
}

/// Skips the original vertex count and reads the added fraction-precise
/// vertices.
fn read_extra_vertices(reader: &mut Reader<'_>) -> Result<ExtraVertices, CyberError> {
    let (_originals, added) = (reader.read_u32()?, reader.read_u32()?);
    check_room(reader, added, 8, "extra vertex")?;
    let mut vertices = Vec::new();
    for _ in 0..added {
        vertices.push((Fixed(reader.read_i32()?), Fixed(reader.read_i32()?)));
    }
    Ok(vertices)
}

/// Reads the subsector count and per-subsector seg counts, deriving each
/// subsector's first seg as the running total.
fn read_extended_subsectors(reader: &mut Reader<'_>) -> Result<Vec<Subsector>, CyberError> {
    let count = reader.read_u32()?;
    check_room(reader, count, 4, "extended subsector")?;
    let mut subsectors = Vec::new();
    let mut first = 0_u32;
    for _ in 0..count {
        let seg_count = reader.read_u32()?;
        subsectors.push(Subsector {
            seg_count,
            first_seg: first,
        });
        first = first.checked_add(seg_count).ok_or_else(|| {
            CyberError::Format(
                "the extended subsector seg counts overflow a u32; rebuild \
                 the node lump"
                    .to_owned(),
            )
        })?;
    }
    Ok(subsectors)
}

/// Reads the seg count and the 11-byte extended seg records.
fn read_extended_segs(reader: &mut Reader<'_>) -> Result<Vec<Seg>, CyberError> {
    let count = reader.read_u32()?;
    check_room(reader, count, EXT_SEG_LEN, "extended seg")?;
    let mut segs = Vec::new();
    for _ in 0..count {
        let v1 = reader.read_u32()?;
        let v2 = reader.read_u32()?;
        let linedef = reader.read_u16()?;
        let side = reader.read_u8()?;
        segs.push(Seg {
            v1,
            v2,
            angle: 0_i16,
            linedef: u32::from(linedef),
            direction: i16::from(side),
            offset: 0_i16,
        });
    }
    Ok(segs)
}

/// Reads the node count and the extended node records, then requires the
/// subsector totals to account for exactly the stored segs.
fn read_extended_nodes(
    reader: &mut Reader<'_>,
    subsectors: &[Subsector],
    seg_count: usize,
) -> Result<Vec<Node>, CyberError> {
    let count = reader.read_u32()?;
    check_room(reader, count, EXT_NODE_LEN, "extended node")?;
    let mut nodes = Vec::new();
    for _ in 0..count {
        nodes.push(read_extended_node(reader)?);
    }
    let total: usize = subsectors
        .iter()
        .map(|sub| usize::try_from(sub.seg_count).unwrap_or(usize::MAX))
        .sum();
    if total != seg_count {
        return Err(CyberError::Format(format!(
            "the extended subsectors claim {total} segs but the lump stores \
             {seg_count}; rebuild the node lump"
        )));
    }
    Ok(nodes)
}

/// One 36-byte extended node record.
fn read_extended_node(reader: &mut Reader<'_>) -> Result<Node, CyberError> {
    let x = reader.read_i16()?;
    let y = reader.read_i16()?;
    let dx = reader.read_i16()?;
    let dy = reader.read_i16()?;
    let right = read_box(reader)?;
    let left = read_box(reader)?;
    Ok(Node {
        x,
        y,
        dx,
        dy,
        right,
        left,
        right_child: extended_child(reader.read_u32()?),
        left_child: extended_child(reader.read_u32()?),
    })
}

/// Requires `count` records of `record_len` bytes to fit what is left.
fn check_room(
    reader: &Reader<'_>,
    count: u32,
    record_len: usize,
    what: &str,
) -> Result<(), CyberError> {
    let needed = usize::try_from(count)
        .ok()
        .and_then(|n| n.checked_mul(record_len));
    if needed.is_some_and(|n| n <= reader.remaining()) {
        return Ok(());
    }
    Err(CyberError::Format(format!(
        "{what}: {count} records of {record_len} bytes do not fit the {} bytes left in the lump",
        reader.remaining()
    )))
}

/// Inflates a `zlib` stream, capped at [`MAX_ZNOD_BYTES`].
fn inflate(data: &[u8]) -> Result<Vec<u8>, CyberError> {
    let limit = u64::try_from(MAX_ZNOD_BYTES)
        .ok()
        .and_then(|value| value.checked_add(1))
        .ok_or_else(|| CyberError::Internal("the ZNOD cap does not fit a u64".to_owned()))?;
    let mut out = Vec::new();
    flate2::bufread::ZlibDecoder::new(data)
        .take(limit)
        .read_to_end(&mut out)
        .map_err(|error| {
            CyberError::Format(format!("the ZNOD payload does not inflate: {error}"))
        })?;
    if out.len() > MAX_ZNOD_BYTES {
        return Err(CyberError::LimitExceeded(format!(
            "the ZNOD payload inflates past the {MAX_ZNOD_BYTES}-byte cap; \
             the node lump is a compression bomb"
        )));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::{
        BoundingBox, Child, NODE_MAGICS, Node, Seg, Subsector, extended_child, lump_format,
        parse_extended_nodes, parse_vanilla_nodes, vanilla_child,
    };
    use crate::fixed::Fixed;
    use crate::wad::bytes::Writer;

    /// Appends one 12-byte vanilla `SEGS` record.
    fn put_seg(w: &mut Writer, fields: (u16, u16, i16, u16, i16, i16)) {
        let (v1, v2, angle, linedef, direction, offset) = fields;
        w.put_u16(v1);
        w.put_u16(v2);
        w.put_i16(angle);
        w.put_u16(linedef);
        w.put_i16(direction);
        w.put_i16(offset);
    }

    /// A vanilla bounding-box quad: `top`, `bottom`, `left`, `right`.
    fn put_box(w: &mut Writer, edges: (i16, i16, i16, i16)) {
        let (top, bottom, left, right) = edges;
        w.put_i16(top);
        w.put_i16(bottom);
        w.put_i16(left);
        w.put_i16(right);
    }

    /// A vanilla node record with a unit-`dx` partition line.
    fn put_node(w: &mut Writer, right_child: u16, left_child: u16) {
        w.put_i16(0);
        w.put_i16(0);
        w.put_i16(1);
        w.put_i16(0);
        put_box(w, (1, 0, 0, 1));
        put_box(w, (1, 0, 0, 1));
        w.put_u16(right_child);
        w.put_u16(left_child);
    }

    /// The three vanilla record lumps of a minimal run.
    type LumpTrio = (Vec<u8>, Vec<u8>, Vec<u8>);

    /// Two segs, two subsectors of one seg each and one node whose both
    /// children are subsectors: the smallest interesting vanilla run.
    fn vanilla_lumps() -> LumpTrio {
        let mut segs = Writer::new();
        put_seg(&mut segs, (0, 1, 0, 0, 0, 0));
        put_seg(&mut segs, (1, 2, -1, 1, 1, 64));
        let mut ssectors = Writer::new();
        ssectors.put_u16(1);
        ssectors.put_u16(0);
        ssectors.put_u16(1);
        ssectors.put_u16(1);
        let mut nodes = Writer::new();
        put_node(&mut nodes, 0x8001, 0x8000);
        (segs.into_inner(), ssectors.into_inner(), nodes.into_inner())
    }

    /// The [`Node`] the [`vanilla_lumps`] root decodes to.
    fn expected_root_node() -> Node {
        Node {
            x: 0,
            y: 0,
            dx: 1,
            dy: 0,
            right: BoundingBox {
                top: 1,
                bottom: 0,
                left: 0,
                right: 1,
            },
            left: BoundingBox {
                top: 1,
                bottom: 0,
                left: 0,
                right: 1,
            },
            right_child: Child::Subsector(1),
            left_child: Child::Subsector(0),
        }
    }

    /// The extended payload (magic excluded): one added vertex, two
    /// subsectors of one seg, two segs reaching the added vertex and one
    /// node with subsector children 0 and 1.
    fn extended_payload() -> Vec<u8> {
        let mut w = Writer::new();
        w.put_u32(6);
        w.put_u32(1);
        w.put_i32(0x0001_0000);
        w.put_i32(-0x0004_0000);
        w.put_u32(2);
        w.put_u32(1);
        w.put_u32(1);
        w.put_u32(2);
        for (v1, v2, linedef, side) in [(6, 0, 0_u16, 0_u8), (6, 1, 1, 1)] {
            w.put_u32(v1);
            w.put_u32(v2);
            w.put_u16(linedef);
            w.put_u8(side);
        }
        w.put_u32(1);
        w.put_i16(0);
        w.put_i16(0);
        w.put_i16(64);
        w.put_i16(0);
        put_box(&mut w, (256, 0, 0, 256));
        put_box(&mut w, (256, 0, 0, 256));
        w.put_u32(0x8000_0000);
        w.put_u32(0x8000_0001);
        w.into_inner()
    }

    #[test]
    fn vanilla_lumps_parse_to_the_expected_structure() {
        let (segs, ssectors, nodes) = vanilla_lumps();
        let set = parse_vanilla_nodes(&segs, &ssectors, &nodes).expect("the fixture parses");
        assert_eq!(set.format, super::NodesFormat::Vanilla, "vanilla layout");
        assert!(set.vertices_extra.is_empty(), "no extra vertices");
        assert_eq!(
            set.segs,
            vec![
                Seg {
                    v1: 0,
                    v2: 1,
                    angle: 0,
                    linedef: 0,
                    direction: 0,
                    offset: 0,
                },
                Seg {
                    v1: 1,
                    v2: 2,
                    angle: -1,
                    linedef: 1,
                    direction: 1,
                    offset: 64,
                },
            ],
            "the two segs"
        );
        assert_eq!(
            set.subsectors,
            vec![
                Subsector {
                    seg_count: 1,
                    first_seg: 0
                },
                Subsector {
                    seg_count: 1,
                    first_seg: 1
                },
            ],
            "the two subsectors"
        );
        assert_eq!(set.nodes, vec![expected_root_node()], "one node");
        assert_eq!(set.root(), Some(0), "the last node is the root");
        assert_eq!(set.depth(), Some(1), "one node level");
    }

    #[test]
    fn empty_vanilla_lumps_parse_empty() {
        let set = parse_vanilla_nodes(b"", b"", b"").expect("empty inputs parse");
        assert!(set.segs.is_empty(), "no segs");
        assert!(set.subsectors.is_empty(), "no subsectors");
        assert!(set.nodes.is_empty(), "no nodes");
        assert_eq!(set.root(), None, "no root without nodes");
        assert_eq!(set.depth(), None, "no depth without nodes");
    }

    #[test]
    fn nested_vanilla_children_walk_the_depth() {
        let (segs, _, _) = vanilla_lumps();
        let mut ssectors = Writer::new();
        ssectors.put_u16(2);
        ssectors.put_u16(0);
        let mut nodes = Writer::new();
        put_node(&mut nodes, 0x8000, 0x8000);
        put_node(&mut nodes, 0, 0x8000);
        let set = parse_vanilla_nodes(&segs, &ssectors.into_inner(), &nodes.into_inner())
            .expect("a two-level tree parses");
        assert_eq!(set.root(), Some(1), "the last node is the root");
        assert_eq!(set.depth(), Some(2), "root plus one interior level");
    }

    #[test]
    fn vanilla_truncations_are_format_errors() {
        let (_, ssectors, nodes) = vanilla_lumps();
        let segs_error = parse_vanilla_nodes(b"12345", &ssectors, &nodes)
            .expect_err("5 bytes are no 12-byte record");
        assert_eq!(segs_error.code(), "format_error", "SEGS remainder code");
        assert!(
            segs_error.to_string().contains("SEGS"),
            "SEGS named: {segs_error}"
        );
        let ssectors_error =
            parse_vanilla_nodes(b"", b"123", &nodes).expect_err("3 bytes are no 4-byte record");
        assert!(
            ssectors_error.to_string().contains("SSECTORS"),
            "SSECTORS named: {ssectors_error}"
        );
        let nodes_error =
            parse_vanilla_nodes(b"", b"", b"123").expect_err("3 bytes are no 28-byte record");
        assert!(
            nodes_error.to_string().contains("NODES"),
            "NODES named: {nodes_error}"
        );
    }

    #[test]
    fn vanilla_dangling_references_are_format_errors() {
        let (segs, ssectors, _) = vanilla_lumps();
        let mut leaf_beyond = Writer::new();
        put_node(&mut leaf_beyond, 0x8005, 0x8000);
        let error = parse_vanilla_nodes(&segs, &ssectors, &leaf_beyond.into_inner())
            .expect_err("subsector 5 does not exist");
        assert_eq!(error.code(), "format_error", "dangling leaf code");
        assert!(
            error.to_string().contains("subsector"),
            "leaf named: {error}"
        );
        let mut forward = Writer::new();
        put_node(&mut forward, 1, 0x8000);
        let forward_error = parse_vanilla_nodes(&segs, &ssectors, &forward.into_inner())
            .expect_err("node 0 cannot reference node 1");
        assert!(
            forward_error.to_string().contains("earlier node"),
            "the backwards rule is named: {forward_error}"
        );
        let mut over = Writer::new();
        over.put_u16(9);
        over.put_u16(0);
        let mut node_zero = Writer::new();
        put_node(&mut node_zero, 0x8000, 0x8000);
        let range_error = parse_vanilla_nodes(&segs, &over.into_inner(), &node_zero.into_inner())
            .expect_err("the subsector reaches past the segs");
        assert!(
            range_error.to_string().contains("past the"),
            "the seg range is named: {range_error}"
        );
    }

    #[test]
    fn xnod_bytes_parse_to_the_expected_structure() {
        let mut bytes = b"XNOD".to_vec();
        bytes.extend_from_slice(&extended_payload());
        let set = parse_extended_nodes(&bytes).expect("the fixture parses");
        assert_eq!(
            set.format,
            super::NodesFormat::Xnod,
            "raw payload keeps its magic"
        );
        assert_eq!(
            set.vertices_extra,
            vec![(Fixed(0x0001_0000), Fixed(-0x0004_0000))],
            "one added 16.16 vertex"
        );
        assert_eq!(
            set.subsectors,
            vec![
                Subsector {
                    seg_count: 1,
                    first_seg: 0
                },
                Subsector {
                    seg_count: 1,
                    first_seg: 1
                },
            ],
            "first_seg is the running total"
        );
        assert_eq!(
            set.segs,
            vec![
                Seg {
                    v1: 6,
                    v2: 0,
                    angle: 0,
                    linedef: 0,
                    direction: 0,
                    offset: 0,
                },
                Seg {
                    v1: 6,
                    v2: 1,
                    angle: 0,
                    linedef: 1,
                    direction: 1,
                    offset: 0,
                },
            ],
            "segs reference the extra vertex 6"
        );
        assert_eq!(set.nodes.len(), 1, "one node");
        assert_eq!(
            set.nodes.first().expect("the root").right_child,
            Child::Subsector(0),
            "bit 31 flags the leaf"
        );
        assert_eq!(set.root(), Some(0), "the last node is the root");
        assert_eq!(set.depth(), Some(1), "one level");
    }

    #[test]
    fn znod_inflates_to_the_same_nodeset_as_xnod() {
        use flate2::write::ZlibEncoder;
        use std::io::Write as _;

        let payload = extended_payload();
        let mut encoder = ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&payload).expect("encoding fits");
        let mut bytes = b"ZNOD".to_vec();
        bytes.extend_from_slice(&encoder.finish().expect("the encoder closes"));
        let zipped = parse_extended_nodes(&bytes).expect("the ZNOD fixture parses");
        let mut plain = b"XNOD".to_vec();
        plain.extend_from_slice(&payload);
        let raw = parse_extended_nodes(&plain).expect("the XNOD fixture parses");
        assert_eq!(
            zipped.format,
            super::NodesFormat::Znod,
            "compressed report name"
        );
        assert_eq!(raw.format, super::NodesFormat::Xnod, "raw report name");
        assert_eq!(zipped.vertices_extra, raw.vertices_extra, "same vertices");
        assert_eq!(zipped.subsectors, raw.subsectors, "same subsectors");
        assert_eq!(zipped.segs, raw.segs, "same segs");
        assert_eq!(zipped.nodes, raw.nodes, "same nodes");
    }

    #[test]
    fn extended_malformations_are_format_errors() {
        let payload = extended_payload();
        let short_magic = parse_extended_nodes(b"XNO").expect_err("3 bytes hold no magic");
        assert_eq!(short_magic.code(), "format_error", "short magic code");
        let unknown =
            parse_extended_nodes(b"WTF? payload").expect_err("an unknown magic is refused");
        assert!(
            unknown.to_string().contains("WTF?"),
            "the magic is named: {unknown}"
        );
        let broken =
            parse_extended_nodes(b"ZNODnot-zlib").expect_err("a broken zlib stream is refused");
        assert!(
            broken.to_string().contains("inflate"),
            "the failure is named: {broken}"
        );
        let cut = payload.len().checked_sub(8).expect("the payload is longer");
        let mut truncated = b"XNOD".to_vec();
        truncated.extend_from_slice(payload.get(..cut).expect("computed above"));
        let cut_error = parse_extended_nodes(&truncated).expect_err("the last node is cut");
        assert_eq!(cut_error.code(), "format_error", "truncated payload code");
        let mut trailing = b"XNOD".to_vec();
        trailing.extend_from_slice(&payload);
        trailing.extend_from_slice(&[0_u8; 4]);
        let error = parse_extended_nodes(&trailing).expect_err("trailing junk");
        assert!(
            error.to_string().contains("trail"),
            "trailing bytes named: {error}"
        );
    }

    #[test]
    fn extended_subsector_totals_must_match_the_segs() {
        let mut payload = extended_payload();
        payload[20] = 2;
        let mut bytes = b"XNOD".to_vec();
        bytes.extend_from_slice(&payload);
        let error = parse_extended_nodes(&bytes)
            .expect_err("the subsector counts now total three, not two");
        assert!(
            error.to_string().contains("claim"),
            "the mismatch is named: {error}"
        );
    }

    #[test]
    fn lump_format_names_every_known_magic() {
        for (magic, name) in NODE_MAGICS {
            assert_eq!(lump_format(magic), Some(name), "the bare {name:?} magic");
            let mut with_payload = magic.to_vec();
            with_payload.extend_from_slice(b"more bytes follow");
            assert_eq!(
                lump_format(&with_payload),
                Some(name),
                "{name:?} plus payload"
            );
        }
        assert_eq!(lump_format(b""), None, "no bytes, no magic");
        assert_eq!(lump_format(b"abc"), None, "three bytes are no magic");
        assert_eq!(
            lump_format(b"NODE"),
            None,
            "vanilla record data stays unnamed"
        );
    }

    #[test]
    fn child_words_split_the_flag_from_the_index() {
        assert_eq!(vanilla_child(0x8000), Child::Subsector(0), "the flag alone");
        assert_eq!(vanilla_child(0x8123), Child::Subsector(0x123), "leaf index");
        assert_eq!(vanilla_child(12), Child::Node(12), "plain node index");
        assert_eq!(
            extended_child(0x8000_0000),
            Child::Subsector(0),
            "the high flag"
        );
        assert_eq!(
            extended_child(0x8001_0002),
            Child::Subsector(0x1_0002),
            "leaf index below bit 31"
        );
        assert_eq!(
            extended_child(7),
            Child::Node(7),
            "plain extended node index"
        );
    }

    #[test]
    fn parts_serialize_for_json_assets() {
        let (segs, ssectors, nodes) = vanilla_lumps();
        let set = parse_vanilla_nodes(&segs, &ssectors, &nodes).expect("parses");
        let seg =
            serde_json::to_value(set.segs.get(1).expect("two segs")).expect("a seg serializes");
        assert_eq!(seg["v2"], serde_json::json!(2_u64), "unsigned fields land");
        assert_eq!(
            seg["angle"],
            serde_json::json!(-1_i64),
            "signed fields land"
        );
        let child = serde_json::to_value(Child::Subsector(3)).expect("a child serializes");
        assert_eq!(
            child,
            serde_json::json!({ "subsector": 3_u64 }),
            "tagged leaf"
        );
        let format = serde_json::to_value(set.format).expect("a format serializes");
        assert_eq!(format, serde_json::json!("vanilla"), "lowercase name");
    }
}
