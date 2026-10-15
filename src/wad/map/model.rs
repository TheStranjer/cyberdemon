//! The format-neutral map model every map tool edits.
//!
//! A [`Map`] holds the five editable lump classes as typed rows: [`Thing`]s,
//! [`Vertex`]es, [`Linedef`]s, [`Sidedef`]s and [`Sector`]s. Every binary or
//! textural map format (`wad::map::doom`, `wad::map::hexen` and
//! `wad::map::udmf_map`) maps into these rows and back, and the formats'
//! differences stay
//! expressible: `flags` keep the raw bit semantics of the map's own format,
//! `args`, `tid` and `special` on things carry only `Hexen`/`UDMF` data
//! (zero for `Doom`), and each element's `extra` map holds `UDMF` fields
//! this model does not name (always empty for the binary formats).
//!
//! Indices between elements (`Linedef::v1`, `Sidedef::sector`, ...) are
//! positions in the matching `Vec`, so editing never depends on how a format
//! encodes "no side": the binary formats' `0xFFFF` sentinel lives only in
//! `wad::map::doom`. Texture and flat names are stored uppercase with `NUL`
//! padding stripped; `"-"` means no texture.

use alloc::collections::BTreeMap;

use serde::Serialize;

use crate::wad::archive::Lump;
use crate::wad::classify::MapFormat;
use crate::wad::name::{LumpName, NAME_LEN};

use super::flags::FlagFamily;

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
}
