//! The `REJECT` builder: the sector visibility matrix, conservatively.
//!
//! [`build`] writes a map's `REJECT` lump in one of two modes.
//! [`RejectMode::Zero`] clears the matrix: every sector may see every
//! other, which is always correct and what `ZDBSP` writes by default.
//! [`RejectMode::Components`] walks the sector graph — an edge joins the
//! two sectors on the sides of every linedef whose sides resolve to two
//! different sectors — and sets the bit for every ordered pair in
//! *different* connected components: sectors in separate components share
//! no opening, so they can never see each other whatever the heights.
//! Pairs inside one component stay clear, visible until proven otherwise.
//! The bits are the lump's `ceil(sectors² / 8)` bytes, bit
//! `i * sectors + j`, row-major, least significant bit first; sizing and
//! serialization live in [`Reject`]. Nothing here knows about sessions
//! or MCP.

use crate::error::CyberError;
use crate::wad::map::{Linedef, Map, Reject};

/// Bits in one `REJECT` byte.
const BITS_PER_BYTE: usize = 8_usize;

/// How a [`build`] fills the matrix.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RejectMode {
    /// Clear the matrix: everything may see everything.
    Zero,
    /// Reject exactly the pairs from different connected components of
    /// the sector graph.
    Components,
}

impl RejectMode {
    /// The mode named by `text`, case-insensitive.
    ///
    /// # Errors
    ///
    /// [`CyberError::InvalidArgument`] naming both accepted values.
    #[inline]
    pub fn parse(text: &str) -> Result<Self, CyberError> {
        match text.to_ascii_lowercase().as_str() {
            "zero" => Ok(Self::Zero),
            "components" => Ok(Self::Components),
            other => Err(CyberError::InvalidArgument(format!(
                "reject mode {other:?} is neither \"zero\" nor \"components\"; pass one \
                 of those two or leave the parameter out"
            ))),
        }
    }

    /// The mode's wire name.
    #[must_use]
    #[inline]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Zero => "zero",
            Self::Components => "components",
        }
    }
}

/// What one [`build`] produced: the finished lump and what it says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RejectBuild {
    /// The `REJECT` lump bytes.
    pub bytes: Vec<u8>,
    /// The map's sector count.
    pub sectors: usize,
    /// Rejected pairs written into the matrix.
    pub rejected_pairs: usize,
}

/// Builds `map`'s `REJECT` lump in `mode`.
///
/// # Errors
///
/// [`CyberError::LimitExceeded`] when the matrix size leaves the
/// addressable range (see [`Reject::to_bytes`]).
#[inline]
pub fn build(map: &Map, mode: RejectMode) -> Result<RejectBuild, CyberError> {
    let sectors = map.sectors.len();
    sectors.checked_mul(sectors).ok_or_else(|| {
        CyberError::LimitExceeded(format!(
            "reject matrix too large: {sectors} sectors squared leaves the addressable range"
        ))
    })?;
    let (bits, rejected_pairs) = match mode {
        RejectMode::Zero => (Vec::new(), 0_usize),
        RejectMode::Components => component_bits(map, sectors),
    };
    Ok(RejectBuild {
        bytes: Reject::new(sectors, &bits).to_bytes()?,
        sectors,
        rejected_pairs,
    })
}

/// The components matrix: bits for every ordered pair of sectors in
/// different connected components, and the pair count. `sectors²` must
/// be addressable ([`build`] checks before calling).
fn component_bits(map: &Map, sectors: usize) -> (Vec<u8>, usize) {
    let parents = components(map, sectors);
    let roots: Vec<usize> = (0..sectors).map(|sector| find(&parents, sector)).collect();
    let want = Reject::expected_bytes(sectors);
    let mut bits = vec![0_u8; want];
    let mut rejected = 0_usize;
    for row in 0_usize..sectors {
        rejected = rejected.saturating_add(set_row(&mut bits, &roots, sectors, row));
    }
    (bits, rejected)
}

/// Rejects in `row` every sector outside its own component, counting the
/// pairs set.
fn set_row(bits: &mut [u8], roots: &[usize], sectors: usize, row: usize) -> usize {
    let Some(&root) = roots.get(row) else {
        return 0_usize;
    };
    let mut fresh = 0_usize;
    for (column, &other) in roots.iter().enumerate() {
        if row == column || root == other {
            continue;
        }
        let Some(index) = bit_index(row, sectors, column) else {
            continue;
        };
        fresh = fresh.saturating_add(usize::from(set_bit(bits, index)));
    }
    fresh
}

/// The flat matrix bit of the ordered pair `(row, column)`, `None` past
/// the addressable range.
fn bit_index(row: usize, sectors: usize, column: usize) -> Option<usize> {
    row.checked_mul(sectors)?.checked_add(column)
}

/// Sets the matrix bit `index` and reports whether it was freshly set.
fn set_bit(bits: &mut [u8], index: usize) -> bool {
    let Some(byte_index) = index.checked_div(BITS_PER_BYTE) else {
        return false;
    };
    let Some(shift) = index
        .checked_rem(BITS_PER_BYTE)
        .and_then(|bit| u32::try_from(bit).ok())
    else {
        return false;
    };
    let mask = 1_u8.checked_shl(shift).unwrap_or(0_u8);
    let Some(byte) = bits.get_mut(byte_index) else {
        return false;
    };
    if (*byte & mask) != 0_u8 {
        return false;
    }
    *byte |= mask;
    true
}

/// The union-find parents of the sector graph: every linedef with two
/// resolving sides over different sectors joins those two sectors.
fn components(map: &Map, sectors: usize) -> Vec<usize> {
    let mut parents: Vec<usize> = (0..sectors).collect();
    let mut sizes = vec![1_usize; sectors];
    for line in &map.linedefs {
        if let Some((front, back)) = joined_pair(map, line) {
            unite(&mut parents, &mut sizes, front, back);
        }
    }
    parents
}

/// The two sectors `line` joins: present on both sides, resolving, and
/// different.
fn joined_pair(map: &Map, line: &Linedef) -> Option<(usize, usize)> {
    let front = side_sector(map, line.front)?;
    let back = side_sector(map, line.back)?;
    (front != back).then_some((front, back))
}

/// The sector the sidedef `side` opens into, when the side exists, its
/// sidedef resolves and that sidedef's sector index is in range.
fn side_sector(map: &Map, side: Option<usize>) -> Option<usize> {
    let sidedef = side.and_then(|index| map.sidedefs.get(index))?;
    map.sectors.get(sidedef.sector).map(|_| sidedef.sector)
}

/// The root of `node`'s component. The trees stay shallow because
/// [`unite`] balances them by size, so no path compression is needed.
fn find(parents: &[usize], mut node: usize) -> usize {
    while let Some(&parent) = parents.get(node) {
        if parent == node {
            return node;
        }
        node = parent;
    }
    node
}

/// Joins the components of `a` and `b`, attaching the smaller under the
/// bigger so every walk stays logarithmic.
fn unite(parents: &mut [usize], sizes: &mut [usize], a: usize, b: usize) {
    let root_a = find(parents, a);
    let root_b = find(parents, b);
    if root_a == root_b {
        return;
    }
    let size_a = sizes.get(root_a).copied().unwrap_or(1_usize);
    let size_b = sizes.get(root_b).copied().unwrap_or(1_usize);
    let (small, big) = if size_a <= size_b {
        (root_a, root_b)
    } else {
        (root_b, root_a)
    };
    if let Some(slot) = parents.get_mut(small) {
        *slot = big;
    }
    if let Some(slot) = sizes.get_mut(big) {
        *slot = size_a.saturating_add(size_b);
    }
}

#[cfg(test)]
mod tests {
    use super::{RejectMode, build, find};
    use crate::wad::map::common::fixtures::{room, side, solid_line, two_room_map, vertex};
    use crate::wad::map::{Map, Reject};

    /// Two one-sided rooms that share no line: two sector components.
    fn disconnected_rooms() -> Map {
        let mut map = two_room_map();
        map.things.clear();
        map.vertices = vec![
            vertex(0, 0),
            vertex(64, 0),
            vertex(64, 64),
            vertex(0, 64),
            vertex(256, 0),
            vertex(320, 0),
            vertex(320, 64),
            vertex(256, 64),
        ];
        map.linedefs = vec![
            solid_line(0, 1, 0),
            solid_line(1, 2, 0),
            solid_line(2, 3, 0),
            solid_line(3, 0, 0),
            solid_line(4, 5, 1),
            solid_line(5, 6, 1),
            solid_line(6, 7, 1),
            solid_line(7, 4, 1),
        ];
        map.sidedefs = vec![side("-", "-", "WALL1", 0), side("-", "-", "WALL1", 1)];
        map.sectors = vec![room(0, 128, 160, 0), room(0, 128, 192, 0)];
        map
    }

    /// The two-room map plus a third sector no sidedef uses.
    fn with_spare_sector() -> Map {
        let mut map = two_room_map();
        map.sectors.push(room(0, 128, 128, 7));
        map
    }

    /// Reads `built` back as a matrix.
    fn matrix(built: &super::RejectBuild) -> Reject {
        Reject::new(built.sectors, &built.bytes)
    }

    #[test]
    fn modes_parse_case_insensitively_and_refuse_anything_else() {
        assert_eq!(RejectMode::parse("zero").expect("zero"), RejectMode::Zero);
        assert_eq!(
            RejectMode::parse("COMPONENTS").expect("components"),
            RejectMode::Components
        );
        assert_eq!(RejectMode::Zero.as_str(), "zero", "wire name");
        assert_eq!(RejectMode::Components.as_str(), "components", "wire name");
        let error = RejectMode::parse("visibility").expect_err("not a mode");
        assert_eq!(error.code(), "invalid_argument", "bad mode code");
        let message = error.to_string();
        assert!(
            message.contains("zero") && message.contains("components"),
            "the message names both values: {message}"
        );
    }

    #[test]
    fn zero_mode_writes_the_clear_matrix_at_the_expected_size() {
        let built = build(&two_room_map(), RejectMode::Zero).expect("the fixture builds");
        assert_eq!(built.bytes, vec![0_u8; 1], "two sectors need one byte");
        assert_eq!(built.sectors, 2, "the fixture's sector count");
        assert_eq!(built.rejected_pairs, 0, "nothing rejected");
        let wide = build(&with_spare_sector(), RejectMode::Zero).expect("three sectors");
        assert_eq!(wide.bytes, vec![0_u8; 2], "nine bits round up to two bytes");
        let empty = build(&Map::default(), RejectMode::Zero).expect("no sectors");
        assert!(empty.bytes.is_empty(), "a sector-less map writes no bytes");
        assert_eq!(empty.rejected_pairs, 0, "and rejects nothing");
    }

    #[test]
    fn components_mode_rejects_only_disconnected_pairs() {
        let built = build(&disconnected_rooms(), RejectMode::Components).expect("two rooms build");
        assert_eq!(
            built.bytes,
            vec![0b0000_0110_u8],
            "bits (0,1) and (1,0) only"
        );
        assert_eq!(built.rejected_pairs, 2, "one ordered pair each way");
        let reject = matrix(&built);
        assert!(reject.size_ok(), "the lump is exactly sized");
        assert!(reject.is_rejected(0, 1), "sector 1 is never visible from 0");
        assert!(reject.is_rejected(1, 0), "and the way back");
        assert!(!reject.is_rejected(0, 0), "a sector sees itself");
        assert!(!reject.is_rejected(1, 1), "in both rows");
    }

    #[test]
    fn components_mode_connects_two_sided_lines_and_ignores_the_rest() {
        let built = build(&two_room_map(), RejectMode::Components).expect("the fixture builds");
        assert_eq!(
            built.bytes,
            vec![0_u8; 1],
            "the step line joins both sectors"
        );
        assert_eq!(built.rejected_pairs, 0, "one component rejects nothing");
    }

    #[test]
    fn components_mode_isolates_an_unreferenced_sector() {
        let built = build(&with_spare_sector(), RejectMode::Components).expect("three sectors");
        // Sectors 0 and 1 stay one component; 2 shares no line with them:
        // bits (0,2)=2, (1,2)=5, (2,0)=6, (2,1)=7 in a 3-wide matrix.
        assert_eq!(
            built.bytes,
            vec![0b1110_0100_u8, 0_u8],
            "the spare is isolated"
        );
        assert_eq!(built.rejected_pairs, 4, "two pairs each way");
        let reject = matrix(&built);
        assert!(
            reject.is_rejected(2, 1) && reject.is_rejected(1, 2),
            "spare rows and columns"
        );
        assert!(
            !reject.is_rejected(0, 1) && !reject.is_rejected(1, 0),
            "the joined pair"
        );
    }

    #[test]
    fn dangling_side_references_join_nothing() {
        let mut map = two_room_map();
        // Sidedef 3 (the step line's back side) points past SECTORS.
        map.sidedefs
            .get_mut(3)
            .expect("the fixture has a sidedef 3")
            .sector = 99;
        let built = build(&map, RejectMode::Components).expect("the map builds");
        assert_eq!(
            built.rejected_pairs, 2,
            "the divider stops joining the sectors"
        );
        assert_eq!(
            built.bytes,
            vec![0b0000_0110_u8],
            "the same bits as two bare rooms"
        );
    }

    #[test]
    fn union_find_roots_walk_to_the_representative() {
        let parents = vec![0_usize, 0, 1, 3];
        assert_eq!(find(&parents, 0), 0, "a root is its own parent");
        assert_eq!(find(&parents, 2), 0, "through the chain");
        assert_eq!(find(&parents, 3), 3, "another root");
        assert_eq!(
            find(&parents, 9),
            9,
            "a node outside the table stands alone"
        );
    }
}
