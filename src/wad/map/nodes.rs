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
//! [`NodeSet`]; [`lump_format`] is the one sniffer of what magic a node lump
//! starts with, answering a [`NodeMagic`] that covers the `GL` variants this
//! module reports but never parses.
//! The writers that rebuild these lumps join here later. Nothing in this
//! module knows about sessions or MCP.
//!
//! The two readers are implementation details of those entry points: the
//! vanilla record walker lives in the private `vanilla` module, the
//! extended lump walker in the private `extended` one, and this root holds
//! what both share — the record sizes and child-word masks, the element
//! types, [`NodeSet`] and its cross-reference checks, and [`NodeMagic`]: the
//! one answer every reader and reporter of a node lump's leading magic uses.

mod extended;
mod vanilla;

use serde::Serialize;

use crate::error::CyberError;
use crate::fixed::Fixed;

pub use extended::parse_extended_nodes;
pub use vanilla::parse_vanilla_nodes;

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

/// The magic a node lump's bytes start with, as [`lump_format`] answers it.
///
/// That magic is the only thing telling the three vanilla record lumps
/// apart from one extended lump, and the parseable `ZDoom` runs apart from
/// the `GL` family the tools report but never read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeMagic {
    /// No magic: the lump holds plain records.
    Vanilla,
    /// `XNOD`: a raw `ZDoom` extended run.
    Xnod,
    /// `ZNOD`: a `zlib`-compressed `ZDoom` extended run.
    Znod,
    /// `XGLN`: raw extended `GL` nodes, reported but never parsed.
    Xgln,
    /// `ZGLN`: compressed extended `GL` nodes, reported but never parsed.
    Zgln,
    /// `XGL2`: raw `GL` nodes of the second extended format, never parsed.
    Xgl2,
    /// `ZGL2`: its compressed form, never parsed.
    Zgl2,
    /// `XGL3`: raw `GL` nodes of the third extended format, never parsed.
    Xgl3,
    /// `ZGL3`: its compressed form, never parsed.
    Zgl3,
}

impl NodeMagic {
    /// Every magic a node lump can start with, in the order [`lump_format`]
    /// tries them; [`Self::Vanilla`] names the absence of a magic and so is
    /// not one of them.
    pub const ALL: [Self; 8] = [
        Self::Xnod,
        Self::Znod,
        Self::Xgln,
        Self::Zgln,
        Self::Xgl2,
        Self::Zgl2,
        Self::Xgl3,
        Self::Zgl3,
    ];

    /// The four letters a lump with this magic starts with, `None` for
    /// [`Self::Vanilla`].
    #[must_use]
    #[inline]
    pub const fn bytes(self) -> Option<&'static [u8; MAGIC_LEN]> {
        match self {
            Self::Vanilla => None,
            Self::Xnod => Some(b"XNOD"),
            Self::Znod => Some(b"ZNOD"),
            Self::Xgln => Some(b"XGLN"),
            Self::Zgln => Some(b"ZGLN"),
            Self::Xgl2 => Some(b"XGL2"),
            Self::Zgl2 => Some(b"ZGL2"),
            Self::Xgl3 => Some(b"XGL3"),
            Self::Zgl3 => Some(b"ZGL3"),
        }
    }

    /// The lowercase report name of this magic: its stored letters turned to
    /// lowercase, `"vanilla"` for a lump that starts with no magic at all.
    /// The `get_nodes` report prints exactly these names.
    #[must_use]
    #[inline]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Vanilla => "vanilla",
            Self::Xnod => "xnod",
            Self::Znod => "znod",
            Self::Xgln => "xgln",
            Self::Zgln => "zgln",
            Self::Xgl2 => "xgl2",
            Self::Zgl2 => "zgl2",
            Self::Xgl3 => "xgl3",
            Self::Zgl3 => "zgl3",
        }
    }

    /// The magic's letters as they are stored, uppercase, `None` for
    /// [`Self::Vanilla`]. `get_map_summary` prints these as the `NODES`
    /// lump's format.
    #[must_use]
    #[inline]
    pub const fn stored_name(self) -> Option<&'static str> {
        match self {
            Self::Vanilla => None,
            Self::Xnod => Some("XNOD"),
            Self::Znod => Some("ZNOD"),
            Self::Xgln => Some("XGLN"),
            Self::Zgln => Some("ZGLN"),
            Self::Xgl2 => Some("XGL2"),
            Self::Zgl2 => Some("ZGL2"),
            Self::Xgl3 => Some("XGL3"),
            Self::Zgl3 => Some("ZGL3"),
        }
    }

    /// Whether `head`, a lump's first four bytes, carries this magic's
    /// letters. [`Self::Vanilla`] never matches: it names no letters.
    #[must_use]
    fn matches_head(self, head: &[u8]) -> bool {
        self.bytes()
            .is_some_and(|letters| head == letters.as_slice())
    }

    /// Whether [`parse_extended_nodes`] reads a lump of this magic.
    #[must_use]
    #[inline]
    pub const fn is_extended(self) -> bool {
        matches!(self, Self::Xnod | Self::Znod)
    }

    /// Whether this magic names a `GL`-family run: reportable, never
    /// parsed.
    #[must_use]
    #[inline]
    pub const fn is_gl(self) -> bool {
        matches!(
            self,
            Self::Xgln | Self::Zgln | Self::Xgl2 | Self::Zgl2 | Self::Xgl3 | Self::Zgl3
        )
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

/// The magic a node lump starts with: [`NodeMagic::Vanilla`] when the data
/// is plain records or too short to carry a magic, the named variant when it
/// starts with one of [`NodeMagic::ALL`]'s letters.
#[must_use]
#[inline]
pub fn lump_format(data: &[u8]) -> NodeMagic {
    let Some(head) = data.get(..MAGIC_LEN) else {
        return NodeMagic::Vanilla;
    };
    NodeMagic::ALL
        .into_iter()
        .find(|magic| magic.matches_head(head))
        .unwrap_or(NodeMagic::Vanilla)
}

/// The four alphanumeric letters a lump starts with, exactly as stored.
///
/// `None` when the lump is shorter than four bytes or its head is no
/// letters at all. The `GL_VERT` report of a `glBSP` run reads its magic
/// this way: `gNd2`, `gNd5` and friends name no [`NodeMagic`], so the head
/// travels to the report unparsed.
#[must_use]
#[inline]
pub fn lump_head(data: &[u8]) -> Option<String> {
    let head = data.get(..MAGIC_LEN)?;
    head.iter()
        .all(u8::is_ascii_alphanumeric)
        .then(|| String::from_utf8_lossy(head).into_owned())
}

/// Added fraction-precise vertices, raw `16.16` pairs.
type ExtraVertices = Vec<(Fixed, Fixed)>;

#[cfg(test)]
mod tests;
