//! The vanilla node run: the three parallel record lumps `Doom` writes,
//! read into one [`NodeSet`].
//!
//! [`parse_vanilla_nodes`] walks the `SEGS`, `SSECTORS` and `NODES` byte
//! strings record by record — each lump's size must be a whole number of
//! its records — and hands the decoded blocks to [`NodeSet`]'s
//! cross-reference checks. The extended lump's node records reuse
//! [`read_box`]; every other reader here is vanilla-only.

use crate::error::CyberError;
use crate::wad::bytes::Reader;
use crate::wad::map::common::records;

use super::{
    BoundingBox, Child, NODE_LEN, Node, NodeSet, NodesFormat, SEG_LEN, SSECTOR_LEN, SUBSECTOR_FLAG,
    SUBSECTOR_INDEX, Seg, Subsector,
};

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
pub(super) fn read_box(reader: &mut Reader<'_>) -> Result<BoundingBox, CyberError> {
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
pub(super) fn vanilla_child(raw: u16) -> Child {
    if raw & SUBSECTOR_FLAG == 0 {
        Child::Node(u32::from(raw))
    } else {
        Child::Subsector(u32::from(raw & SUBSECTOR_INDEX))
    }
}
