//! `ZDoom`'s extended node lump: one payload with fraction-precise
//! vertices and wider seg and node records, raw or behind a `zlib` stream.
//!
//! [`parse_extended_nodes`] reads the magic, inflates a `ZNOD` payload and
//! hands the result to [`parse_extended_payload`], which walks the four
//! sections in their fixed order — added vertices, subsector seg counts,
//! segs, nodes — and refuses anything left over. Node bounding boxes come
//! from the vanilla reader's [`read_box`].

use std::io::Read as _;

use crate::error::CyberError;
use crate::fixed::Fixed;
use crate::wad::bytes::Reader;

use super::vanilla::read_box;
use super::{
    Child, EXT_NODE_LEN, EXT_SEG_LEN, EXT_SUBSECTOR_FLAG, EXT_SUBSECTOR_INDEX, ExtraVertices,
    MAGIC_LEN, MAX_ZNOD_BYTES, Node, NodeSet, NodesFormat, Seg, Subsector,
};

/// An extended child word: `bit 31` selects a subsector leaf.
#[must_use]
#[inline]
pub(super) const fn extended_child(raw: u32) -> Child {
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
