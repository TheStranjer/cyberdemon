//! The well-known `Doom` lump names, and the named sets of them that
//! carry engine meaning.
//!
//! Every engine-format lump this crate reads or writes is named here
//! exactly once, as a [`LumpName`] constant built by `LumpName`'s
//! crate-private `const` constructor; no other production module writes
//! a raw directory byte of these names. Where a group of names *is* the
//! meaning — the lumps a binary writer emits itself, the run a geometry
//! edit invalidates, the names whose layout is tied to the map format —
//! the group is a named constant array below, so each purpose is stated
//! once next to the names it keys on.

use crate::wad::name::LumpName;

/// The `PLAYPAL` global palette lump.
pub const PLAYPAL: LumpName = LumpName::padded(*b"PLAYPAL\0");
/// The `COLORMAP` global light-translation lump.
pub const COLORMAP: LumpName = LumpName::padded(*b"COLORMAP");
/// The `PNAMES` lump listing every texture patch name.
pub const PNAMES: LumpName = LumpName::padded(*b"PNAMES\0\0");
/// The `TEXTURE1` lump of composite texture definitions.
pub const TEXTURE1: LumpName = LumpName::padded(*b"TEXTURE1");
/// The `TEXTURE2` lump of further composite texture definitions.
pub const TEXTURE2: LumpName = LumpName::padded(*b"TEXTURE2");

/// The `THINGS` member lump of a binary map block.
pub const THINGS: LumpName = LumpName::padded(*b"THINGS\0\0");
/// The `LINEDEFS` member lump of a binary map block.
pub const LINEDEFS: LumpName = LumpName::padded(*b"LINEDEFS");
/// The `SIDEDEFS` member lump of a binary map block.
pub const SIDEDEFS: LumpName = LumpName::padded(*b"SIDEDEFS");
/// The `VERTEXES` member lump of a binary map block.
pub const VERTEXES: LumpName = LumpName::padded(*b"VERTEXES");
/// The `SEGS` member lump: the `BSP` split edges.
pub const SEGS: LumpName = LumpName::padded(*b"SEGS\0\0\0\0");
/// The `SSECTORS` member lump: the convex subsectors.
pub const SSECTORS: LumpName = LumpName::padded(*b"SSECTORS");
/// The `NODES` member lump: the `BSP` tree (or the `ZDoom` extended
/// lump of a binary block).
pub const NODES: LumpName = LumpName::padded(*b"NODES\0\0\0");
/// The `SECTORS` member lump of a binary map block.
pub const SECTORS: LumpName = LumpName::padded(*b"SECTORS\0");
/// The `REJECT` member lump: the sector visibility matrix.
pub const REJECT: LumpName = LumpName::padded(*b"REJECT\0\0");
/// The `BLOCKMAP` member lump: the collision grid.
pub const BLOCKMAP: LumpName = LumpName::padded(*b"BLOCKMAP");
/// The `ZNODES` lump holding a `UDMF` block's extended node data.
pub const ZNODES: LumpName = LumpName::padded(*b"ZNODES\0\0");
/// The `BEHAVIOR` lump: the compiled `ACS` of a `Hexen` block.
pub const BEHAVIOR: LumpName = LumpName::padded(*b"BEHAVIOR");
/// The `SCRIPTS` lump: the `ACS` source directory of a `Hexen` block.
pub const SCRIPTS: LumpName = LumpName::padded(*b"SCRIPTS\0");
/// The `LEAFS` lump of the `Hexen` demo format.
pub const LEAFS: LumpName = LumpName::padded(*b"LEAFS\0\0\0");
/// The `LIGHTS` lump of the `Hexen` demo format.
pub const LIGHTS: LumpName = LumpName::padded(*b"LIGHTS\0\0");
/// The `MACROS` lump of the `Hexen` demo format.
pub const MACROS: LumpName = LumpName::padded(*b"MACROS\0\0");
/// The `TEXTMAP` lump: the `UDMF` map body.
pub const TEXTMAP: LumpName = LumpName::padded(*b"TEXTMAP\0");
/// The `ENDMAP` lump: the `UDMF` map block's end marker.
pub const ENDMAP: LumpName = LumpName::padded(*b"ENDMAP\0\0");

/// The `ENDOOM` lump: the exit screen text.
pub const ENDOOM: LumpName = LumpName::padded(*b"ENDOOM\0\0");
/// The `DEMO1` lump: the first saved game demo.
pub const DEMO1: LumpName = LumpName::padded(*b"DEMO1\0\0\0");
/// The `DEMO2` lump: the second saved game demo.
pub const DEMO2: LumpName = LumpName::padded(*b"DEMO2\0\0\0");
/// The `DEMO3` lump: the third saved game demo.
pub const DEMO3: LumpName = LumpName::padded(*b"DEMO3\0\0\0");
/// The `DEMO4` lump: the fourth saved game demo.
pub const DEMO4: LumpName = LumpName::padded(*b"DEMO4\0\0\0");
/// The `DEHACKED` lump: the `DeHackEd`/`BEX` patch script.
pub const DEHACKED: LumpName = LumpName::padded(*b"DEHACKED");
/// The `DECORATE` lump: the `ZDoom` class script.
pub const DECORATE: LumpName = LumpName::padded(*b"DECORATE");
/// The `ZSCRIPT` lump: the `ZDoom` `ZScript` class script.
pub const ZSCRIPT: LumpName = LumpName::padded(*b"ZSCRIPT\0");
/// The `MAPINFO` lump: the `ZDoom` map and thing definitions.
pub const MAPINFO: LumpName = LumpName::padded(*b"MAPINFO\0");
/// The `ZMAPINFO` lump: a later `MAPINFO` part.
pub const ZMAPINFO: LumpName = LumpName::padded(*b"ZMAPINFO");
/// The `UMAPINFO` lump: the universal map definitions.
pub const UMAPINFO: LumpName = LumpName::padded(*b"UMAPINFO");
/// The `EMAPINFO` lump: `Eternity`'s map definitions.
pub const EMAPINFO: LumpName = LumpName::padded(*b"EMAPINFO");

/// The five member lumps the map editors decode into the model and can
/// rewrite: the record lumps behind `add_thing`, `edit_linedef` and
/// friends.
pub const EDITABLE_MAP_LUMPS: [LumpName; 5] = [THINGS, LINEDEFS, SIDEDEFS, VERTEXES, SECTORS];

/// The five generated lumps of a binary node run, in directory order:
/// what a geometry edit must invalidate (kept as empty placeholders,
/// since the vanilla readers require them).
pub const BINARY_NODE_LUMPS: [LumpName; 5] = [SEGS, SSECTORS, NODES, REJECT, BLOCKMAP];

/// The three record lumps of the classic `BSP` run, in the order the
/// binary writers emit them.
///
/// [`crate::wad::map::validate`] calls the run built when any one of
/// the three carries bytes: an extended run lives entirely inside
/// `NODES`, so one lump with data stands for all three.
pub const CLASSIC_NODE_RUN_LUMPS: [LumpName; 3] = [SEGS, SSECTORS, NODES];

/// The ten lumps [`crate::wad::map::doom::write_doom_map`] emits
/// itself, in the order it emits them: everything else in [`Map::other`]
/// is carried after them.
///
/// [`Map::other`]: crate::wad::map::Map::other
pub const STANDARD_DOOM_MAP_LUMPS: [LumpName; 10] = [
    THINGS, LINEDEFS, SIDEDEFS, VERTEXES, SEGS, SSECTORS, NODES, SECTORS, REJECT, BLOCKMAP,
];

/// The twelve lumps [`crate::wad::map::hexen::write_hexen_map`] emits
/// itself: the `Doom` ten plus the `ACS` pair a `Hexen` block owns.
pub const STANDARD_HEXEN_MAP_LUMPS: [LumpName; 12] = [
    THINGS, LINEDEFS, SIDEDEFS, VERTEXES, SEGS, SSECTORS, NODES, SECTORS, REJECT, BLOCKMAP,
    BEHAVIOR, SCRIPTS,
];

/// The fifteen member names [`crate::wad::classify`] accepts inside a
/// binary map block: what the writers emit plus the `Hexen` demo
/// lumps, so a block carrying one of those does not end at it.
pub const BINARY_BLOCK_MEMBERS: [LumpName; 15] = [
    THINGS, LINEDEFS, SIDEDEFS, VERTEXES, SEGS, SSECTORS, NODES, SECTORS, REJECT, BLOCKMAP,
    BEHAVIOR, SCRIPTS, LEAFS, LIGHTS, MACROS,
];

/// The node lumps whose record layout is tied to the map format, so a
/// format conversion must drop them: the binary five and the `UDMF`
/// extended lump.
pub const FORMAT_BOUND_NODE_LUMPS: [LumpName; 6] =
    [SEGS, SSECTORS, NODES, REJECT, BLOCKMAP, ZNODES];

/// The two lump names a block's node data can hide behind, however the
/// readers weigh them: a non-empty `NODES` (classic or extended) or a
/// present `ZNODES`.
pub const NODE_DATA_LUMPS: [LumpName; 2] = [NODES, ZNODES];

/// The run an extended build drops before writing its `ZNODES` into a
/// `UDMF` block: the classic trio and any stale extended lump.
pub const EXTENDED_NODES_SUPERSEDE_LUMPS: [LumpName; 4] = [SEGS, SSECTORS, NODES, ZNODES];

/// The two composite-texture definition lumps, in load order.
pub const TEXTURE_DEF_LUMPS: [LumpName; 2] = [TEXTURE1, TEXTURE2];

/// The two lumps of a `UDMF` map block between its header and end
/// marker: the body and the marker [`crate::wad::map::udmf_map`]
/// never carries into [`Map::other`].
///
/// [`Map::other`]: crate::wad::map::Map::other
pub const UDMF_BLOCK_MEMBERS: [LumpName; 2] = [TEXTMAP, ENDMAP];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wad::name::NAME_LEN;

    /// One constant with the word it must spell and the eight directory
    /// bytes behind it.
    type Expected = (&'static str, LumpName, [u8; NAME_LEN]);

    /// Every constant paired with its word and bytes.
    const EXPECTED: [Expected; 35] = [
        ("PLAYPAL", PLAYPAL, *b"PLAYPAL\0"),
        ("COLORMAP", COLORMAP, *b"COLORMAP"),
        ("PNAMES", PNAMES, *b"PNAMES\0\0"),
        ("TEXTURE1", TEXTURE1, *b"TEXTURE1"),
        ("TEXTURE2", TEXTURE2, *b"TEXTURE2"),
        ("THINGS", THINGS, *b"THINGS\0\0"),
        ("LINEDEFS", LINEDEFS, *b"LINEDEFS"),
        ("SIDEDEFS", SIDEDEFS, *b"SIDEDEFS"),
        ("VERTEXES", VERTEXES, *b"VERTEXES"),
        ("SEGS", SEGS, *b"SEGS\0\0\0\0"),
        ("SSECTORS", SSECTORS, *b"SSECTORS"),
        ("NODES", NODES, *b"NODES\0\0\0"),
        ("SECTORS", SECTORS, *b"SECTORS\0"),
        ("REJECT", REJECT, *b"REJECT\0\0"),
        ("BLOCKMAP", BLOCKMAP, *b"BLOCKMAP"),
        ("ZNODES", ZNODES, *b"ZNODES\0\0"),
        ("BEHAVIOR", BEHAVIOR, *b"BEHAVIOR"),
        ("SCRIPTS", SCRIPTS, *b"SCRIPTS\0"),
        ("LEAFS", LEAFS, *b"LEAFS\0\0\0"),
        ("LIGHTS", LIGHTS, *b"LIGHTS\0\0"),
        ("MACROS", MACROS, *b"MACROS\0\0"),
        ("TEXTMAP", TEXTMAP, *b"TEXTMAP\0"),
        ("ENDMAP", ENDMAP, *b"ENDMAP\0\0"),
        ("ENDOOM", ENDOOM, *b"ENDOOM\0\0"),
        ("DEMO1", DEMO1, *b"DEMO1\0\0\0"),
        ("DEMO2", DEMO2, *b"DEMO2\0\0\0"),
        ("DEMO3", DEMO3, *b"DEMO3\0\0\0"),
        ("DEMO4", DEMO4, *b"DEMO4\0\0\0"),
        ("DEHACKED", DEHACKED, *b"DEHACKED"),
        ("DECORATE", DECORATE, *b"DECORATE"),
        ("ZSCRIPT", ZSCRIPT, *b"ZSCRIPT\0"),
        ("MAPINFO", MAPINFO, *b"MAPINFO\0"),
        ("ZMAPINFO", ZMAPINFO, *b"ZMAPINFO"),
        ("UMAPINFO", UMAPINFO, *b"UMAPINFO"),
        ("EMAPINFO", EMAPINFO, *b"EMAPINFO"),
    ];

    #[test]
    fn every_constant_holds_the_vanilla_directory_bytes() {
        for (text, name, raw) in EXPECTED {
            assert_eq!(name.raw(), raw, "{text} stores the exact eight bytes");
            assert_eq!(
                name.as_bytes(),
                text.as_bytes(),
                "{text} reads back its word"
            );
        }
    }

    #[test]
    fn the_sets_hold_exactly_the_names_their_purpose_names() {
        assert_eq!(
            EDITABLE_MAP_LUMPS,
            [THINGS, LINEDEFS, SIDEDEFS, VERTEXES, SECTORS]
        );
        assert_eq!(BINARY_NODE_LUMPS, [SEGS, SSECTORS, NODES, REJECT, BLOCKMAP]);
        assert_eq!(CLASSIC_NODE_RUN_LUMPS, [SEGS, SSECTORS, NODES]);
        assert_eq!(
            FORMAT_BOUND_NODE_LUMPS,
            [SEGS, SSECTORS, NODES, REJECT, BLOCKMAP, ZNODES]
        );
        assert_eq!(NODE_DATA_LUMPS, [NODES, ZNODES]);
        assert_eq!(
            EXTENDED_NODES_SUPERSEDE_LUMPS,
            [SEGS, SSECTORS, NODES, ZNODES]
        );
        assert_eq!(TEXTURE_DEF_LUMPS, [TEXTURE1, TEXTURE2]);
        assert_eq!(UDMF_BLOCK_MEMBERS, [TEXTMAP, ENDMAP]);
    }

    #[test]
    fn the_writer_and_block_sets_cover_the_editable_lumps_and_node_runs() {
        for name in EDITABLE_MAP_LUMPS {
            assert!(
                STANDARD_DOOM_MAP_LUMPS.contains(&name),
                "{name} is a Doom map lump"
            );
            assert!(
                BINARY_BLOCK_MEMBERS.contains(&name),
                "{name} is a block member"
            );
        }
        for name in BINARY_NODE_LUMPS {
            assert!(
                STANDARD_DOOM_MAP_LUMPS.contains(&name),
                "{name} is a Doom map lump"
            );
        }
        for name in STANDARD_DOOM_MAP_LUMPS {
            assert!(
                STANDARD_HEXEN_MAP_LUMPS.contains(&name),
                "{name} is also a Hexen map lump"
            );
            assert!(
                BINARY_BLOCK_MEMBERS.contains(&name),
                "{name} is a block member"
            );
        }
        assert!(
            STANDARD_HEXEN_MAP_LUMPS.contains(&BEHAVIOR),
            "Hexen carries BEHAVIOR"
        );
        assert!(
            STANDARD_HEXEN_MAP_LUMPS.contains(&SCRIPTS),
            "Hexen carries SCRIPTS"
        );
    }
}
