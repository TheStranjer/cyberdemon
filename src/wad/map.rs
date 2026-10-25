//! The map layer: a typed, format-neutral map model and its codecs.
//!
//! [`model`] defines the rows every map tool edits ([`Map`] and friends,
//! re-exported here); [`doom`] and [`hexen`] read and write the binary
//! `Doom` and `Hexen` lump formats, sharing the identical record codecs
//! through the private `common` module, and [`udmf_map`] maps the model
//! to and from the `UDMF` text format parsed by [`crate::wad::udmf`],
//! with the named bit tables of both flag families in [`flags`].
//! [`read_map`] and [`write_map`] dispatch on a block's [`MapFormat`].
//! [`remove`] is the pure deletion engine behind the element-removal
//! tools: it reports dependents, grows a removal by its private `cascade`
//! rules and renumbers the references of what survives in its private
//! `renumber` pass. [`stats`] folds a decoded map plus its
//! block's lump sizes into the compact summary the overview tool reports.
//! [`nodes`], [`reject`] and [`blockmap`] type the node-builder output:
//! the `SEGS`/`SSECTORS`/`NODES` tree in its vanilla and `ZDoom` extended
//! layouts, the `REJECT` visibility matrix and the `BLOCKMAP` spatial
//! index; [`build_blockmap`] and [`build_reject`] compute the last two
//! from a map's geometry. [`geometry`] answers the pure integer shape
//! questions over a decoded map: sector boundaries, point containment,
//! bounding boxes, line lengths and sector adjacency. [`describe`] folds
//! a decoded map into the geometry-and-identity fact rows
//! ([`describe::SectorFacts`]) and the whole-map [`describe::MapOverview`]
//! the `describe_map` tool reports: area, bounding box, label centre,
//! flats, sky, named special, tag, wall-texture tally, counted things,
//! roles and neighbours. [`convert`] rewrites
//! a decoded map in another layout, inside one flag family mechanically and
//! across the two families through the [`xlat`] table, the pure step
//! behind `convert_map_format`. [`overlay`] paints one polygon onto a
//! decoded map through [`arrange`](fn@crate::geom::arrange), the pure room-drawing and
//! void-carving step behind `create_sector_from_loop` and `carve_void`.
//! [`ops`] cuts one linedef at a point and welds two collinear ones
//! together, the pure geometry behind `split_linedef`
//! and `join_linedefs`. [`placement`] is the pure thing-placement core:
//! the clearance primitives the plan builder's room fills share with the
//! [`distribute`](placement::distribute) engine behind `place_things`.
//! [`features`] holds the pure cores of assembled map structures: [`features::doors`] is the door action table and the
//! doorway dressing behind the door tools, [`features::place`] the
//! wall selection, width fit and depth raycast behind their placement,
//! [`features::window`] the sill/lintel heights and the blocking view
//! lines behind `add_window`, [`features::stairs`] the even step division
//! and bottom-up paints behind `add_stairs`, [`features::lift`] the
//! lower-facing edge selection and down-wait-up action behind `add_lift`,
//! [`features::teleport`] the pad and destination squares, destination
//! things and border actions behind `add_teleporter`,
//! [`features::switches`] the switch action table behind
//! `add_switch`, and [`features::specials`] the secret bit and the light
//! effect numbers behind `mark_secret` and `set_light_effect`.
//! [`transform`] scales, rotates
//! and translates a selection of vertices and things in integer math,
//! the pure engine behind `transform_selection`. [`validate`] runs the
//! pure map-health checks behind `validate_map`: reference integrity,
//! line structure, thing
//! presence and special/tag sanity, each returning its [`validate::Issue`]
//! rows. [`playability`] answers the player's question instead: it walks the
//! sector graph from the player start, opens the doors, lifts and teleporters
//! the walk can reach and the keys it finds, and reports what a player can
//! and cannot get to. [`query`] is the pure filter engine behind
//! `find_elements`: it turns a [`query::Query`] (an element kind, combined
//! [`query::Filter`]s and an optional [`query::Within`] bound) over a
//! decoded map into the ascending indices of the matching elements, reading
//! the derived fields through [`geometry`] and [`describe`] and the flag and
//! definition names through the [`query::QueryContext`] the caller passes.
//! [`consts`] holds the names and limits the whole layer shares: the
//! `"-"` none-texture marker, the sky flat with its readers
//! ([`consts::normalized_picture`] and [`consts::is_sky_flat`]), the
//! player's step height, standing height and minimum line width, the
//! window lintel default, the unknown-thing radius and the player-start
//! thing types. [`bulk`] is the bulk-editing core: [`bulk::replace`]
//! globs texture
//! and flat names over a map's sidedef faces and sector surfaces and
//! rewrites thing types, counting changed fields for the multi-map
//! `replace_*` tools, and [`bulk::edits`] applies whole
//! [`bulk::edits::FieldEdits`] set/`add` requests to a selection of
//! sector, sidedef or vertex rows, validated before any write, for the
//! `bulk_edit` tool.
//! [`xlat`] holds the embedded Doom ↔ ZDoom linedef-special
//! translation table (rows, flags and lookups) and the cross-family thing,
//! sector and linedef stages [`convert`] runs.

pub mod blockmap;
pub mod build_blockmap;
pub mod build_reject;
pub mod bulk;
pub mod consts;
pub mod convert;
pub mod describe;
pub mod doom;
pub mod features;
pub mod flags;
pub mod geometry;
pub mod hexen;
pub mod model;
pub mod nodes;
pub mod ops;
pub mod overlay;
pub mod placement;
pub mod playability;
pub mod query;
pub mod reject;
pub mod remove;
pub mod stats;
pub mod transform;
pub mod udmf_map;
pub mod validate;
pub mod xlat;

mod common;

pub use crate::wad::classify::MapFormat;
pub use blockmap::Blockmap;
pub use flags::{FlagFamily, FlagKind, Skill};
pub use model::{Endpoints, Linedef, Map, Sector, Sidedef, Thing, Vertex};
pub use nodes::{
    BoundingBox, Child, Node, NodeSet, NodesFormat, Seg, Subsector, lump_format,
    parse_extended_nodes, parse_vanilla_nodes,
};
pub use reject::Reject;
pub use remove::{
    CascadeOptions, Dependents, Removal, RemovalReport, apply_removal, dependents, expand_cascade,
};

use crate::error::CyberError;
use crate::wad::archive::Lump;
use crate::wad::classify::MapBlock;

/// Reads the map whose block `block` describes out of the directory
/// `block_lumps`.
///
/// # Errors
///
/// [`CyberError::Format`] when the block's range leaves `block_lumps` or
/// the block's decoder rejects the members (see
/// [`doom::read_doom_map`], [`hexen::read_hexen_map`] and
/// [`udmf_map::read_udmf_map`]).
#[inline]
pub fn read_map(block_lumps: &[Lump], block: &MapBlock) -> Result<Map, CyberError> {
    let members = block_members(block_lumps, block)?;
    match block.format {
        MapFormat::Doom => doom::read_doom_map(block.name, members),
        MapFormat::Hexen => hexen::read_hexen_map(block.name, members),
        MapFormat::Udmf => udmf_map::read_udmf_map(block.name, members),
    }
}

/// The members of `block` inside `block_lumps`, or a `Format` error when
/// the block's range leaves the directory.
fn block_members<'lumps>(
    block_lumps: &'lumps [Lump],
    block: &MapBlock,
) -> Result<&'lumps [Lump], CyberError> {
    let start = block.header.saturating_add(1);
    block_lumps.get(start..block.end).ok_or_else(|| {
        CyberError::Format(format!(
            "map block {} spans lumps {start}..{} of a {}-lump directory: \
             pass the archive that holds the whole block",
            block.name,
            block.end,
            block_lumps.len()
        ))
    })
}

/// Writes `map` back out as its block's lumps, header first.
///
/// # Errors
///
/// [`CyberError::LimitExceeded`] or [`CyberError::InvalidArgument`] from
/// the block's encoder (see [`doom::write_doom_map`],
/// [`hexen::write_hexen_map`] and [`udmf_map::write_udmf_map`]).
#[inline]
pub fn write_map(map: &Map) -> Result<Vec<Lump>, CyberError> {
    match map.format {
        MapFormat::Doom => doom::write_doom_map(map),
        MapFormat::Hexen => hexen::write_hexen_map(map),
        MapFormat::Udmf => udmf_map::write_udmf_map(map),
    }
}

#[cfg(test)]
mod tests {
    use crate::error::CyberError;
    use crate::wad::archive::Lump;
    use crate::wad::classify::{MapBlock, MapFormat, analyze};
    use crate::wad::map::common::fixtures::lump;
    use crate::wad::map::{Linedef, Map, Vertex, read_map, write_map};
    use crate::wad::name::LumpName;

    /// A valid parsed name for test data.
    fn name(text: &str) -> LumpName {
        LumpName::parse(text).expect("test names are valid")
    }

    /// A `MAP01` block whose header sits at zero and whose members end at
    /// `end` in `format`.
    fn block(end: usize, format: MapFormat) -> MapBlock {
        MapBlock {
            name: name("MAP01"),
            header: 0,
            end,
            format,
            gl: None,
        }
    }

    /// A minimal valid `Doom` block: header plus the five empty members.
    fn doom_lumps() -> Vec<Lump> {
        let mut lumps = vec![Lump::marker(name("MAP01"))];
        for member in ["THINGS", "LINEDEFS", "SIDEDEFS", "VERTEXES", "SECTORS"] {
            lumps.push(Lump::marker(name(member)));
        }
        lumps
    }

    #[test]
    fn the_dispatcher_reads_and_writes_doom_blocks() {
        let lumps = doom_lumps();
        let analysis = analyze(&lumps);
        let block = analysis.maps.first().expect("analyze found the block");
        let map = read_map(&lumps, block).expect("a Doom block reads");
        assert_eq!(map.name, name("MAP01"), "the header names the map");
        assert_eq!(map.format, MapFormat::Doom, "and its format");
        assert!(map.things.is_empty(), "the members are empty");
        let written = write_map(&map).expect("the read map writes back");
        assert_eq!(written.len(), 11, "header plus ten vanilla members");
        assert_eq!(written.first().map(|lump| lump.name), Some(name("MAP01")));
    }

    #[test]
    fn the_dispatcher_refuses_blocks_outside_the_directory() {
        let lumps = doom_lumps();
        let wild = block(lumps.len().checked_add(1).expect("small"), MapFormat::Doom);
        let CyberError::Format(message) =
            read_map(&lumps, &wild).expect_err("the range leaves the directory")
        else {
            panic!("expected Format for an out-of-range block");
        };
        assert!(
            message.contains("MAP01") && message.contains("spans lumps"),
            "the block is named: {message}"
        );
        let wild_hexen = block(lumps.len().checked_add(1).expect("small"), MapFormat::Hexen);
        let CyberError::Format(hexen_message) =
            read_map(&lumps, &wild_hexen).expect_err("Hexen blocks are bounds-checked too")
        else {
            panic!("expected Format for an out-of-range Hexen block");
        };
        assert!(
            hexen_message.contains("spans lumps"),
            "the same message shape: {hexen_message}"
        );
    }

    #[test]
    fn the_dispatcher_reads_and_writes_udmf_blocks() {
        let mut map = Map {
            name: name("MAP01"),
            format: MapFormat::Udmf,
            udmf_namespace: Some("zdoom".to_owned()),
            vertices: vec![Vertex {
                x: 0,
                y: 0,
                ..Default::default()
            }],
            ..Map::default()
        };
        let written = write_map(&map).expect("write_map routes UDMF");
        let analysis = analyze(&written);
        let block = analysis.maps.first().expect("analyze finds the block");
        assert_eq!(
            block.format,
            MapFormat::Udmf,
            "TEXTMAP marks the block UDMF"
        );
        let read = read_map(&written, block).expect("read_map routes UDMF");
        assert_eq!(
            read, map,
            "the empty UDMF map round trips through the dispatcher"
        );
        map.udmf_namespace = Some("doom".to_owned());
        let redone = write_map(&map).expect("the Doom namespace writes too");
        let read_again = read_map(&redone, block).expect("and reads back");
        assert_eq!(read_again, map, "in both families");
    }

    #[test]
    fn the_dispatcher_reports_a_broken_udmf_block_as_format_error() {
        let broken = read_map(&doom_lumps(), &block(doom_lumps().len(), MapFormat::Udmf));
        let CyberError::Format(message) = broken.expect_err("a UDMF block needs a TEXTMAP") else {
            panic!("expected Format for members without TEXTMAP");
        };
        assert!(message.contains("TEXTMAP"), "the lump is named: {message}");
        let lumps = vec![
            Lump::marker(name("MAP01")),
            lump("TEXTMAP", b"vertex { x = 0; y = 0; }"),
            Lump::marker(name("ENDMAP")),
        ];
        let analysis = analyze(&lumps);
        let block = analysis.maps.first().expect("analyze finds the block");
        let CyberError::Format(namespace_message) =
            read_map(&lumps, block).expect_err("the TEXTMAP has no namespace")
        else {
            panic!("expected Format for a namespace-less TEXTMAP");
        };
        assert!(
            namespace_message.contains("namespace"),
            "the global is named: {namespace_message}"
        );
    }

    #[test]
    fn the_dispatcher_routes_hexen_blocks_to_the_hexen_codec() {
        let map = Map {
            name: name("MAP01"),
            format: MapFormat::Hexen,
            other: vec![
                Lump::marker(name("SEGS")),
                Lump::marker(name("SSECTORS")),
                Lump::marker(name("NODES")),
                Lump::marker(name("REJECT")),
                Lump::marker(name("BLOCKMAP")),
                Lump::marker(name("BEHAVIOR")),
            ],
            ..Map::default()
        };
        let written = write_map(&map).expect("write_map routes Hexen");
        let analysis = analyze(&written);
        let block = analysis.maps.first().expect("analyze finds the block");
        assert_eq!(block.format, MapFormat::Hexen, "BEHAVIOR keeps it Hexen");
        let read = read_map(&written, block).expect("read_map routes Hexen");
        assert_eq!(read, map, "the empty Hexen map round trips");
    }

    #[test]
    fn doom_maps_route_through_the_model_defaults() {
        let lumps = doom_lumps();
        let analysis = analyze(&lumps);
        let block = analysis.maps.first().expect("one block");
        let map = read_map(&lumps, block).expect("the empty block reads");
        assert!(map.linedefs.is_empty(), "no linedefs in an empty block");
        let mut one = map;
        one.linedefs.push(Linedef::default());
        let written = write_map(&one).expect("one empty linedef writes");
        let things_and_lines = written.get(2).expect("the LINEDEFS lump");
        assert_eq!(things_and_lines.size(), 14, "one 14-byte record");
    }
}
