//! Generated-lump checks over a binary-format [`Map`]: the node tree,
//! the `BLOCKMAP` grid and the `REJECT` matrix the game loads with the
//! map. `UDMF` blocks are skipped entirely: `ZDoom`-family ports build
//! all three in memory, so their absence is normal there.
//!
//! The checks look at the block's [`Map::other`] lumps, never parsing
//! records: [`crate::wad::map::nodes`] owns that, and the tools that
//! read the parsed tree report malformed data themselves. An empty lump
//! reads as absent - that is what an invalidated build leaves behind.

use alloc::format;
use alloc::vec;
use alloc::vec::Vec;

use super::{Issue, Severity};
use crate::wad::classify::MapFormat;
use crate::wad::lumps;
use crate::wad::map::model::Map;
use crate::wad::map::reject::Reject;
use crate::wad::name::LumpName;

/// The payload of the block's last lump named `wanted`, `None` when the
/// block carries no such lump.
fn lump_data(map: &Map, wanted: LumpName) -> Option<&[u8]> {
    map.other
        .iter()
        .rev()
        .find(|lump| lump.name == wanted)
        .map(|lump| lump.data.as_ref())
}

/// Whether the block is one a port cannot build the nodes of itself.
fn is_binary(map: &Map) -> bool {
    map.format != MapFormat::Udmf
}

/// Whether any of the three record lumps carries bytes: a vanilla run
/// needs all three, an extended one lives entirely inside `NODES`, so
/// any data at all counts as built.
fn has_tree(map: &Map) -> bool {
    lumps::CLASSIC_NODE_RUN_LUMPS
        .into_iter()
        .any(|wanted| lump_data(map, wanted).is_some_and(|data| !data.is_empty()))
}

/// Binary maps whose `SEGS`, `SSECTORS` and `NODES` lumps are all empty
/// or absent: vanilla engines refuse (or crash on) such a block.
pub(super) fn nodes_missing(map: &Map) -> Vec<Issue> {
    if !is_binary(map) || has_tree(map) {
        return Vec::new();
    }
    vec![Issue::new(
        Severity::Warning,
        "the map carries no node data: SEGS, SSECTORS and NODES are all empty, so vanilla \
         engines cannot render it; run build_all_nodes (ZDoom-family ports build the nodes \
         themselves, but every other port needs them)"
            .to_owned(),
    )]
}

/// Binary maps without a usable `BLOCKMAP` lump: the collision and
/// sound-propagation grid the game looks up every frame.
pub(super) fn blockmap_missing(map: &Map) -> Vec<Issue> {
    if !is_binary(map) {
        return Vec::new();
    }
    if lump_data(map, lumps::BLOCKMAP).is_some_and(|data| !data.is_empty()) {
        return Vec::new();
    }
    vec![Issue::new(
        Severity::Info,
        "the map carries no BLOCKMAP (empty or absent): the collision grid is rebuilt at \
         load but nothing saved; run build_blockmap or build_all_nodes to store it"
            .to_owned(),
    )]
}

/// Binary maps carrying a non-empty `REJECT` whose size is not the
/// `ceil(sectors * sectors / 8)` bytes the matrix needs.
pub(super) fn reject_size_mismatch(map: &Map) -> Vec<Issue> {
    if !is_binary(map) {
        return Vec::new();
    }
    let Some(bits) = lump_data(map, lumps::REJECT).filter(|data| !data.is_empty()) else {
        return Vec::new();
    };
    let sectors = map.sectors.len();
    let expected = Reject::expected_bytes(sectors);
    if bits.len() == expected {
        return Vec::new();
    }
    vec![Issue::new(
        Severity::Warning,
        format!(
            "the REJECT lump holds {} bytes but the matrix for {sectors} sectors needs \
             {expected}: run build_reject to rebuild it at the right size",
            bits.len(),
        ),
    )]
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;

    use super::{blockmap_missing, nodes_missing, reject_size_mismatch};
    use crate::wad::classify::MapFormat;
    use crate::wad::map::common::fixtures::{lump, marker, name};
    use crate::wad::map::validate::Severity;
    use crate::wad::map::{Map, Sector};

    /// A binary `Doom` map of `sector_count` sectors carrying `other`.
    fn map_with(
        format: MapFormat,
        sector_count: usize,
        other: Vec<crate::wad::archive::Lump>,
    ) -> Map {
        Map {
            name: name("MAP01"),
            format,
            sectors: (0..sector_count).map(|_| Sector::default()).collect(),
            other,
            ..Default::default()
        }
    }

    #[test]
    fn nodes_missing_fires_only_when_all_three_record_lumps_are_bare() {
        let empty = map_with(MapFormat::Doom, 2, Vec::new());
        let issue = nodes_missing(&empty)
            .first()
            .expect("no lumps at all is no tree")
            .clone();
        assert_eq!(
            issue.severity,
            Severity::Warning,
            "a vanilla map cannot play"
        );
        assert!(
            issue.message.contains("build_all_nodes"),
            "the message names the fix: {}",
            issue.message
        );
        let markers = map_with(
            MapFormat::Doom,
            2,
            vec![marker("SEGS"), marker("SSECTORS"), marker("NODES")],
        );
        assert!(
            !nodes_missing(&markers).is_empty(),
            "empty markers stay missing"
        );
        let extended = map_with(MapFormat::Hexen, 2, vec![lump("NODES", b"XNOD....")]);
        assert!(
            nodes_missing(&extended).is_empty(),
            "an extended lump lives in NODES alone"
        );
        let classic = map_with(
            MapFormat::Doom,
            2,
            vec![lump("SEGS", b"segs"), marker("SSECTORS"), marker("NODES")],
        );
        assert!(
            nodes_missing(&classic).is_empty(),
            "any record data counts as built (reading it is the nodes tools' problem)"
        );
        let udmf = map_with(MapFormat::Udmf, 2, Vec::new());
        assert!(
            nodes_missing(&udmf).is_empty(),
            "UDMF ports build the tree themselves"
        );
    }

    #[test]
    fn blockmap_missing_is_info_and_wants_data_in_the_lump() {
        let empty = map_with(MapFormat::Doom, 2, Vec::new());
        let issue = blockmap_missing(&empty)
            .first()
            .expect("absent is missing")
            .clone();
        assert_eq!(issue.severity, Severity::Info, "mild");
        assert!(
            issue.message.contains("build_blockmap"),
            "the message names a fix: {}",
            issue.message
        );
        let filled = map_with(MapFormat::Hexen, 2, vec![lump("BLOCKMAP", b"grid")]);
        assert!(blockmap_missing(&filled).is_empty(), "data is data");
        let blank = map_with(MapFormat::Doom, 2, vec![marker("BLOCKMAP")]);
        assert!(
            !blockmap_missing(&blank).is_empty(),
            "an emptied lump reads as absent"
        );
        let udmf = map_with(MapFormat::Udmf, 2, Vec::new());
        assert!(
            blockmap_missing(&udmf).is_empty(),
            "UDMF builds its own grid"
        );
    }

    #[test]
    fn reject_size_mismatch_compares_against_the_matrix_size() {
        let absent = map_with(MapFormat::Doom, 2, Vec::new());
        assert!(
            reject_size_mismatch(&absent).is_empty(),
            "an absent or empty REJECT is not a size mismatch"
        );
        let blank = map_with(MapFormat::Doom, 2, vec![marker("REJECT")]);
        assert!(
            reject_size_mismatch(&blank).is_empty(),
            "the emptied-by-an-edit lump stays quiet; nodes_missing covers bare blocks"
        );
        let right = map_with(MapFormat::Doom, 2, vec![lump("REJECT", &[0_u8; 1])]);
        assert!(
            reject_size_mismatch(&right).is_empty(),
            "2 sectors need ceil(4 / 8) = 1 byte"
        );
        let fat = map_with(MapFormat::Hexen, 2, vec![lump("REJECT", &[0_u8; 4])]);
        let issue = reject_size_mismatch(&fat)
            .first()
            .expect("4 bytes is not 1")
            .clone();
        assert_eq!(
            issue.severity,
            Severity::Warning,
            "engines tolerate it, barely"
        );
        assert!(
            issue.message.contains("holds 4 bytes") && issue.message.contains("needs 1"),
            "the message quotes both sizes: {}",
            issue.message
        );
        let udmf = map_with(MapFormat::Udmf, 2, vec![lump("REJECT", &[0_u8; 4])]);
        assert!(
            reject_size_mismatch(&udmf).is_empty(),
            "a stray REJECT in a UDMF block is nobody's matrix"
        );
        let big = map_with(MapFormat::Doom, 9, vec![lump("REJECT", &[0_u8; 10])]);
        assert!(
            !reject_size_mismatch(&big).is_empty(),
            "9 sectors need ceil(81 / 8) = 11 bytes, 10 is short"
        );
    }
}
