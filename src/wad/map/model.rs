//! The format-neutral map model every map tool edits.
//!
//! A [`Map`] holds the five editable lump classes as typed rows: [`Thing`]s,
//! [`Vertex`]es, [`Linedef`]s, [`Sidedef`]s and [`Sector`]s. Every binary or
//! textural map format (`wad::map::doom`, `wad::map::hexen` and
//! `wad::map::udmf_map`) maps into these rows and back, and the formats'
//! differences stay
//! expressible: `flags` keep the raw bit semantics of the map's own format,
//! `args`, `tid` and `special` on things carry only `Hexen`/`UDMF` data
//! (zero for `Doom`), each element's `extra` map holds `UDMF` fields
//! this model does not name, and [`Map::extra_globals`] and
//! [`Map::extra_blocks`] carry a `UDMF` document's global assignments and
//! unmodelled block kinds (all of these always empty for the binary
//! formats).
//!
//! Indices between elements (`Linedef::v1`, `Sidedef::sector`, ...) are
//! positions in the matching `Vec`, so editing never depends on how a format
//! encodes "no side": the binary formats' `0xFFFF` sentinel lives only in
//! `wad::map::doom`. Texture and flat names are stored uppercase with `NUL`
//! padding stripped; `"-"` means no texture.

use alloc::collections::BTreeMap;
use alloc::format;

use serde::Serialize;

use crate::error::CyberError;
use crate::wad::archive::Lump;
use crate::wad::classify::MapFormat;
use crate::wad::name::{LumpName, NAME_LEN};
use crate::wad::udmf::{UdmfBlock, UdmfValue};

use super::flags::FlagFamily;

/// A linedef's two endpoints as `(x, y)` pairs of map-unit coordinates.
pub type Endpoints = ((i32, i32), (i32, i32));

/// A map vertex: one point of the map's geometry.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Vertex {
    /// `X` coordinate in map units.
    pub x: i32,
    /// `Y` coordinate in map units.
    pub y: i32,
    /// Unmodelled format fields (see [`Linedef::extra`]), including the
    /// written text of fractional coordinates.
    pub extra: BTreeMap<String, String>,
}

/// A map line between two vertices, with at most one [`Sidedef`] per side.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Linedef {
    /// Index of the start vertex in [`Map::vertices`].
    pub v1: usize,
    /// Index of the end vertex in [`Map::vertices`].
    pub v2: usize,
    /// Flags exactly as the map's format stores them: `Doom` special and
    /// blocking bits for binary `Doom` maps, the integer flag value for
    /// `UDMF` maps.
    pub flags: u32,
    /// Line special number.
    pub special: i32,
    /// Trigger tag, matched against [`Sector::tag`]; unused by `Hexen`.
    pub tag: i32,
    /// The five `Hexen`/`UDMF` script arguments; zero for `Doom` format.
    pub args: [i32; 5],
    /// Index of the front-side (`Doom` right-side) sidedef in
    /// [`Map::sidedefs`], or `None` when the side is absent.
    pub front: Option<usize>,
    /// Index of the back-side (`Doom` left-side) sidedef in
    /// [`Map::sidedefs`], or `None` when the side is absent.
    pub back: Option<usize>,
    /// Fields the format carries that this model does not name, such as
    /// `UDMF` line properties; empty for the binary formats.
    pub extra: BTreeMap<String, String>,
}

/// One side of a [`Linedef`]: its textures and the [`Sector`] it bounds.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Sidedef {
    /// Texture shift along the line, in map units.
    pub x_offset: i32,
    /// Texture shift across the line, in map units.
    pub y_offset: i32,
    /// Upper texture name, `"-"` for none.
    pub upper: String,
    /// Lower texture name, `"-"` for none.
    pub lower: String,
    /// Middle texture name, `"-"` for none.
    pub middle: String,
    /// Index of the sector this side opens into in [`Map::sectors`].
    pub sector: usize,
    /// Unmodelled format fields (see [`Linedef::extra`]).
    pub extra: BTreeMap<String, String>,
}

/// A contiguous 3-dimensional region of the map.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Sector {
    /// Floor height in map units.
    pub floor_height: i32,
    /// Ceiling height in map units.
    pub ceiling_height: i32,
    /// Floor flat name, `"-"` for none.
    pub floor_flat: String,
    /// Ceiling flat name, `"-"` for none.
    pub ceiling_flat: String,
    /// Light level, 0 to 255 in the vanilla ranges.
    pub light: i32,
    /// Sector effect number.
    pub special: i32,
    /// Tag matched by [`Linedef::tag`] and `Doom` thing triggers.
    pub tag: i32,
    /// Unmodelled format fields (see [`Linedef::extra`]).
    pub extra: BTreeMap<String, String>,
}

/// A spawned object: a player start, monster, item or decoration.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Thing {
    /// Thing id used by `Hexen`/`UDMF` scripts; zero for `Doom` format.
    pub tid: i32,
    /// `X` coordinate in map units.
    pub x: i32,
    /// `Y` coordinate in map units.
    pub y: i32,
    /// Spawn height in map units; zero in the binary formats, which place
    /// things on the floor of the sector containing them.
    pub z: i32,
    /// Facing direction in degrees, 0 to 359.
    pub angle: i32,
    /// Thing number, matched against the game's definitions.
    #[serde(rename = "type")]
    pub kind: i32,
    /// Flags exactly as the map's format stores them ([`Linedef::flags`]).
    pub flags: u32,
    /// Thing special; zero for `Doom` format.
    pub special: i32,
    /// The five `Hexen`/`UDMF` script arguments; zero for `Doom` format.
    pub args: [i32; 5],
    /// Unmodelled format fields (see [`Linedef::extra`]).
    pub extra: BTreeMap<String, String>,
}

/// A whole map: the five editable element blocks plus everything else the
/// map's block carries.
///
/// Unlike the element rows, `Map` derives no `Serialize`: [`Lump`] carries
/// raw bytes this crate has no wire form for, and map tools will report
/// `other` as name and size rows when they arrive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Map {
    /// Map name, taken from the block header lump.
    pub name: LumpName,
    /// Layout this map's lumps use.
    pub format: MapFormat,
    /// The namespace a `UDMF` header declares (`zdoom`, `glbdoom`, ...);
    /// `None` for binary maps and `UDMF` maps without one.
    pub udmf_namespace: Option<String>,
    /// The `UDMF` global assignments other than the `namespace`
    /// declaration, in written order and carried verbatim so a rewrite
    /// emits them again; empty for the binary formats and for `UDMF`
    /// documents that name none.
    pub extra_globals: Vec<(String, UdmfValue)>,
    /// The `UDMF` blocks whose kind is none of the five element kinds, in
    /// document order and carried verbatim; a rewrite appends them after
    /// the element blocks. Empty for the binary formats and for documents
    /// that hold none.
    pub extra_blocks: Vec<UdmfBlock>,
    /// The things, in lump order.
    pub things: Vec<Thing>,
    /// The vertices, in lump order.
    pub vertices: Vec<Vertex>,
    /// The linedefs, in lump order.
    pub linedefs: Vec<Linedef>,
    /// The sidedefs, in lump order.
    pub sidedefs: Vec<Sidedef>,
    /// The sectors, in lump order.
    pub sectors: Vec<Sector>,
    /// Every block lump that is not one of the five editable lumps, in
    /// original order and carried verbatim: the generated nodes
    /// (`SEGS`, `SSECTORS`, `NODES`), `REJECT`, `BLOCKMAP` and any format
    /// extras such as `BEHAVIOR` or `ZNODES`. `GL` node blocks are
    /// separate and never appear here.
    pub other: Vec<Lump>,
}

impl Map {
    /// An empty map named `name` in `format`: the starting point for
    /// building one from scratch.
    #[must_use]
    #[inline]
    pub const fn new_empty(name: LumpName, format: MapFormat) -> Self {
        Self {
            name,
            format,
            udmf_namespace: None,
            extra_globals: Vec::new(),
            extra_blocks: Vec::new(),
            things: Vec::new(),
            vertices: Vec::new(),
            linedefs: Vec::new(),
            sidedefs: Vec::new(),
            sectors: Vec::new(),
            other: Vec::new(),
        }
    }

    /// The bounding box of the vertices as `(min_x, min_y, max_x, max_y)`,
    /// or `None` when the map holds no vertices.
    #[must_use]
    #[inline]
    pub fn bounds(&self) -> Option<(i32, i32, i32, i32)> {
        let mut rest = self.vertices.iter();
        let first = rest.next()?;
        let mut min_x = first.x;
        let mut min_y = first.y;
        let mut max_x = first.x;
        let mut max_y = first.y;
        for vertex in rest {
            min_x = min_x.min(vertex.x);
            min_y = min_y.min(vertex.y);
            max_x = max_x.max(vertex.x);
            max_y = max_y.max(vertex.y);
        }
        Some((min_x, min_y, max_x, max_y))
    }

    /// The flag family this map's `flags` columns are stored in: binary
    /// `Doom` maps use the [`Doom`] family, binary `Hexen` maps the
    /// [`Hexen`] one, and a `UDMF` map follows its declared namespace —
    /// `doom`, `heretic` and `strife` pick `Doom`, any other namespace
    /// (including no namespace at all, which writes as `zdoom`) picks
    /// `Hexen`.
    ///
    /// [`Doom`]: FlagFamily::Doom
    /// [`Hexen`]: FlagFamily::Hexen
    #[must_use]
    #[inline]
    pub fn flag_family(&self) -> FlagFamily {
        match self.format {
            MapFormat::Doom => FlagFamily::Doom,
            MapFormat::Hexen => FlagFamily::Hexen,
            MapFormat::Udmf => self
                .udmf_namespace
                .as_deref()
                .map_or(FlagFamily::Hexen, FlagFamily::from_udmf_namespace),
        }
    }

    /// The sector the sidedef slot `side` opens into: `None` when the
    /// side is absent, its sidedef index dangles, or that sidedef's
    /// sector index dangles. Every side read that must not hand out an
    /// out-of-range index answers through this one check.
    #[must_use]
    #[inline]
    pub fn side_sector(&self, side: Option<usize>) -> Option<usize> {
        self.stored_side_sector(side)
            .filter(|index| self.sectors.get(*index).is_some())
    }

    /// The sector index *stored* behind the sidedef slot `side`: `None`
    /// only when the side is absent or its sidedef index dangles.
    /// Unlike [`Map::side_sector`] this keeps an out-of-range sector
    /// reference distinguishable as the index it is, which is what
    /// [`crate::wad::map::ops::join_linedefs`] compares: two lines
    /// fronted by two different broken references must still read as
    /// disagreeing sides and be refused with a clear argument error,
    /// before the removal engine below fails on them as broken state.
    #[must_use]
    #[inline]
    pub fn stored_side_sector(&self, side: Option<usize>) -> Option<usize> {
        side.and_then(|index| self.sidedefs.get(index))
            .map(|sidedef| sidedef.sector)
    }

    /// The sectors behind both sides of `line`, each `None` exactly where
    /// [`Map::side_sector`] answers `None` for that side.
    #[must_use]
    #[inline]
    pub fn line_sectors(&self, line: &Linedef) -> (Option<usize>, Option<usize>) {
        (self.side_sector(line.front), self.side_sector(line.back))
    }

    /// The two endpoint coordinates of `line` as an [`Endpoints`] pair,
    /// `None` when either vertex index dangles.
    #[must_use]
    #[inline]
    pub fn line_endpoints(&self, line: &Linedef) -> Option<Endpoints> {
        let start = self.vertices.get(line.v1)?;
        let end = self.vertices.get(line.v2)?;
        Some(((start.x, start.y), (end.x, end.y)))
    }

    /// The sector index stored behind sidedef `side` of linedef `line`,
    /// for callers that report a missing sidedef as their own error.
    /// Unlike [`Map::side_sector`] this checks only that the *sidedef*
    /// exists and answers with the stored index even when the sector it
    /// names dangles: each caller re-reads the sector row itself after
    /// this. The `line` index only names the owner in the refusal.
    ///
    /// # Errors
    ///
    /// [`CyberError::InvalidArgument`] when the map holds no sidedef
    /// `side`.
    #[inline]
    pub fn required_side_sector(&self, line: usize, side: usize) -> Result<usize, CyberError> {
        self.stored_side_sector(Some(side)).ok_or_else(|| {
            CyberError::InvalidArgument(format!(
                "linedef {line} references sidedef {side}, which the map does not hold \
                     (it has {} sidedefs)",
                self.sidedefs.len()
            ))
        })
    }
}

impl Default for Map {
    /// An empty `Doom` map with the all-`NUL` name: [`Map::new_empty`] with
    /// the zero-value defaults.
    #[inline]
    fn default() -> Self {
        Self::new_empty(LumpName::from_raw([0_u8; NAME_LEN]), MapFormat::Doom)
    }
}

#[cfg(test)]
mod tests {
    use super::{Map, Vertex};
    use crate::wad::classify::MapFormat;
    use crate::wad::map::common::fixtures::{solid_line, two_room_map};
    use crate::wad::map::flags::FlagFamily;
    use crate::wad::name::LumpName;

    /// A valid parsed name for test data.
    fn name(text: &str) -> LumpName {
        LumpName::parse(text).expect("test names are valid")
    }

    /// A vertex at `x`/`y`.
    const fn vertex(x: i32, y: i32) -> Vertex {
        Vertex {
            x,
            y,
            extra: alloc::collections::BTreeMap::new(),
        }
    }

    #[test]
    fn new_empty_builds_an_element_less_map() {
        let map = Map::new_empty(name("MAP01"), MapFormat::Hexen);
        assert_eq!(map.name, name("MAP01"), "the name is kept");
        assert_eq!(map.format, MapFormat::Hexen, "the format is kept");
        assert_eq!(map.udmf_namespace, None, "no namespace yet");
        assert!(map.extra_globals.is_empty(), "no carried globals");
        assert!(map.extra_blocks.is_empty(), "no carried blocks");
        assert!(map.things.is_empty(), "no things");
        assert!(map.vertices.is_empty(), "no vertices");
        assert!(map.linedefs.is_empty(), "no linedefs");
        assert!(map.sidedefs.is_empty(), "no sidedefs");
        assert!(map.sectors.is_empty(), "no sectors");
        assert!(map.other.is_empty(), "no other lumps");
        assert_eq!(map.bounds(), None, "an empty map bounds nothing");
    }

    #[test]
    fn default_is_a_doom_map_with_the_empty_name() {
        let map = Map::default();
        assert_eq!(map.format, MapFormat::Doom, "the default format");
        assert!(map.name.is_empty(), "the default name is the NUL name");
        assert_eq!(map, Map::new_empty(map.name, MapFormat::Doom), "same build");
    }

    #[test]
    fn flag_family_follows_the_format_and_the_udmf_namespace() {
        assert_eq!(
            Map::new_empty(name("MAP01"), MapFormat::Doom).flag_family(),
            FlagFamily::Doom,
            "binary Doom maps are the Doom family"
        );
        assert_eq!(
            Map::new_empty(name("MAP01"), MapFormat::Hexen).flag_family(),
            FlagFamily::Hexen,
            "binary Hexen maps are the Hexen family"
        );
        let mut udmf = Map::new_empty(name("MAP01"), MapFormat::Udmf);
        assert_eq!(
            udmf.flag_family(),
            FlagFamily::Hexen,
            "a UDMF map without a namespace writes as zdoom"
        );
        for (namespace, family) in [
            ("doom", FlagFamily::Doom),
            ("heretic", FlagFamily::Doom),
            ("Strife", FlagFamily::Doom),
            ("hexen", FlagFamily::Hexen),
            ("zdoom", FlagFamily::Hexen),
            ("gzdoom", FlagFamily::Hexen),
            ("vavoom", FlagFamily::Hexen),
        ] {
            udmf.udmf_namespace = Some(namespace.to_owned());
            assert_eq!(
                udmf.flag_family(),
                family,
                "the '{namespace}' namespace is the {namespace} map's family"
            );
        }
    }

    #[test]
    fn bounds_spans_the_vertices_including_negatives() {
        let map = Map {
            vertices: vec![
                vertex(256, -128),
                vertex(0, 0),
                vertex(-64, 512),
                vertex(256, 0),
            ],
            ..Map::default()
        };
        assert_eq!(
            map.bounds(),
            Some((-64_i32, -128_i32, 256_i32, 512_i32)),
            "min and max on each axis"
        );
        let single = Map {
            vertices: vec![vertex(7, -7)],
            ..Map::default()
        };
        assert_eq!(
            single.bounds(),
            Some((7_i32, -7_i32, 7_i32, -7_i32)),
            "one vertex bounds itself"
        );
    }

    #[test]
    fn side_sector_resolves_both_ends_of_the_reference_chain() {
        let map = two_room_map();
        assert_eq!(map.side_sector(Some(0)), Some(0), "the hall side");
        assert_eq!(map.side_sector(Some(3)), Some(1), "the back room side");
        assert_eq!(map.side_sector(None), None, "no side, no sector");
        assert_eq!(map.side_sector(Some(99)), None, "a dangling sidedef");
        let mut broken = two_room_map();
        broken.sidedefs.get_mut(0).expect("sidedef 0").sector = 99;
        assert_eq!(broken.side_sector(Some(0)), None, "a dangling sector");
    }

    #[test]
    fn line_sectors_answers_both_sides_with_the_checked_reads() {
        let map = two_room_map();
        let step = map.linedefs.get(2).expect("the step line");
        assert_eq!(
            map.line_sectors(step),
            (Some(0_usize), Some(1_usize)),
            "the hall in front, the back room behind"
        );
        let wall = map.linedefs.first().expect("the west wall");
        assert_eq!(
            map.line_sectors(wall),
            (Some(0_usize), None),
            "a one-sided line has no back sector"
        );
        let mut broken = two_room_map();
        broken.sidedefs.get_mut(3).expect("the step's back").sector = 42;
        let broken_step = broken.linedefs.get(2).expect("the step line");
        assert_eq!(
            broken.line_sectors(broken_step),
            (Some(0_usize), None),
            "the dangling far side reads as no sector"
        );
    }

    #[test]
    fn line_endpoints_pairs_the_vertices_or_finds_nothing() {
        let map = two_room_map();
        let step = map.linedefs.get(2).expect("the step line");
        assert_eq!(
            map.line_endpoints(step),
            Some(((256_i32, 256_i32), (256_i32, 0_i32))),
            "vertex 4 to vertex 1"
        );
        let mut broken = two_room_map();
        broken.linedefs.push(solid_line(999, 0, 0));
        let loose = broken.linedefs.last().expect("the pushed line");
        assert_eq!(broken.line_endpoints(loose), None, "a dangling v1");
        broken.linedefs.get_mut(0).expect("the west wall").v2 = 999;
        let wall = broken.linedefs.first().expect("the west wall");
        assert_eq!(broken.line_endpoints(wall), None, "a dangling v2");
    }

    #[test]
    fn stored_side_sector_keeps_out_of_range_references_apart() {
        let map = two_room_map();
        assert_eq!(
            map.stored_side_sector(Some(0)),
            Some(0),
            "a sound side reads the same as through the checked method"
        );
        assert_eq!(map.stored_side_sector(None), None, "an absent side");
        assert_eq!(
            map.stored_side_sector(Some(99)),
            None,
            "a dangling sidedef index"
        );
        let mut broken = two_room_map();
        broken.sidedefs.get_mut(0).expect("sidedef 0").sector = 99;
        assert_eq!(
            broken.stored_side_sector(Some(0)),
            Some(99),
            "the stored index survives the missing sector"
        );
        assert_eq!(
            broken.side_sector(Some(0)),
            None,
            "where the checked read refuses it"
        );
    }

    #[test]
    fn required_side_sector_reports_missing_sidedefs_only() {
        let map = two_room_map();
        assert!(
            map.required_side_sector(2, 2).is_ok_and(|s| s == 0_usize),
            "the step line's front row"
        );
        assert!(
            map.required_side_sector(2, 3).is_ok_and(|s| s == 1_usize),
            "and its back row"
        );
        let mut dangling = two_room_map();
        dangling.sidedefs.get_mut(3).expect("sidedef 3").sector = 99;
        assert!(
            dangling
                .required_side_sector(2, 3)
                .is_ok_and(|s| s == 99_usize),
            "the stored index answers unchecked, unlike side_sector"
        );
        assert_eq!(
            dangling.side_sector(Some(3)),
            None,
            "and the checked read says no"
        );
        let error = map
            .required_side_sector(2, 9)
            .expect_err("sidedef 9 is past the block");
        assert_eq!(error.code(), "invalid_argument", "the wire code");
        assert!(
            error.to_string().contains(
                "linedef 2 references sidedef 9, which the map does not hold \
                 (it has 8 sidedefs)"
            ),
            "the message names the line, the side and the count: {error}"
        );
    }
}
