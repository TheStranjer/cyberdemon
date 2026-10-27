//! Shared builders for the bulk-edit engine's tests: the test stock of
//! wall and flat names with the `Doom`, `Hexen`, permissive and `UDMF`
//! scopes it answers with, the `set`/`add`/flag request builders, the
//! two-vertex, two-line, two-sidedef, two-sector, two-thing sample map in
//! its `Doom` and `Hexen` forms, and the reader that pulls one stored
//! field off a model row through its serialization.
//!
//! [`geometry_kinds`] measures the sector, sidedef and vertex blocks with
//! their stock and refusal rules, [`lines_things`] the linedef and thing
//! blocks with their flags, [`report`] the answer shape beside everything
//! the entry gate refuses before a map is read, and [`table`] the field
//! tables themselves: wire names round-tripping their field enums, every
//! enum variant tabled, and every listed field moving its column.

mod geometry_kinds;
mod lines_things;
mod report;
mod table;

use alloc::collections::BTreeSet;
use alloc::string::String;

use serde::Serialize;
use serde_json::Value;

use super::{EditScope, FieldEdits};
use crate::wad::classify::MapFormat;
use crate::wad::map::flags::FlagFamily;
use crate::wad::map::{Linedef, Map, Sector, Sidedef, Thing, Vertex};

/// The test stock: the names the sample map's textures and flats
/// come from, and the scopes the engine runs under.
pub(super) struct Stock {
    /// The walls the sidedef faces may wear.
    walls: BTreeSet<String>,
    /// The flats the sector surfaces may wear.
    flats: BTreeSet<String>,
}

impl Stock {
    /// The stock the sample map wears.
    pub(super) fn new() -> Self {
        Self {
            walls: set_of(&["BRICK1", "STARTAN1"]),
            flats: set_of(&["CEIL5", "FLAT1", "FLOOR4_8"]),
        }
    }

    /// The binary `Doom` scope over this stock.
    pub(super) fn doom(&self) -> EditScope<'_> {
        EditScope {
            format: MapFormat::Doom,
            family: FlagFamily::Doom,
            walls: &self.walls,
            flats: &self.flats,
            allow_missing: false,
        }
    }

    /// The binary `Hexen` scope over this stock.
    pub(super) fn hexen(&self) -> EditScope<'_> {
        EditScope {
            format: MapFormat::Hexen,
            family: FlagFamily::Hexen,
            walls: &self.walls,
            flats: &self.flats,
            allow_missing: false,
        }
    }

    /// The binary `Doom` scope that lets unstocked names through.
    pub(super) fn permissive(&self) -> EditScope<'_> {
        EditScope {
            allow_missing: true,
            ..self.doom()
        }
    }

    /// The `UDMF` scope over this stock.
    pub(super) fn udmf(&self) -> EditScope<'_> {
        EditScope {
            format: MapFormat::Udmf,
            ..self.doom()
        }
    }
}

/// A stock set from `items`.
pub(super) fn set_of(items: &[&str]) -> BTreeSet<String> {
    items.iter().copied().map(str::to_owned).collect()
}

/// A `set`-only request.
pub(super) fn set_request(pairs: &[(&str, Value)]) -> FieldEdits {
    FieldEdits {
        set: pairs
            .iter()
            .map(|entry| (entry.0.to_owned(), entry.1.clone()))
            .collect(),
        ..Default::default()
    }
}

/// An `add`-only request.
pub(super) fn add_request(pairs: &[(&str, i64)]) -> FieldEdits {
    FieldEdits {
        add: pairs
            .iter()
            .map(|entry| (entry.0.to_owned(), entry.1))
            .collect(),
        ..Default::default()
    }
}

/// A request carrying both `set` and `add` fields.
pub(super) fn mixed(set_pairs: &[(&str, Value)], add_pairs: &[(&str, i64)]) -> FieldEdits {
    FieldEdits {
        set: set_request(set_pairs).set,
        add: add_request(add_pairs).add,
        ..Default::default()
    }
}

/// A request carrying only flag names.
pub(super) fn flags_request(set: &[&str], clear: &[&str]) -> FieldEdits {
    FieldEdits {
        flags_set: set.iter().copied().map(str::to_owned).collect(),
        flags_clear: clear.iter().copied().map(str::to_owned).collect(),
        ..Default::default()
    }
}

/// A vertex at `x`/`y`.
pub(super) fn vertex(x: i32, y: i32) -> Vertex {
    Vertex {
        x,
        y,
        ..Default::default()
    }
}

/// A fully visible spawn of `kind` at `x`/`y` facing `angle`.
pub(super) fn spawn(kind: i32, x: i32, y: i32, angle: i32) -> Thing {
    Thing {
        kind,
        x,
        y,
        angle,
        flags: 7_u32,
        ..Default::default()
    }
}

/// The sample lines: a `Doom`-family action line with sidedef 0 in
/// front, and a plain two-sided one.
pub(super) fn sample_lines() -> Vec<Linedef> {
    vec![
        Linedef {
            v1: 0_usize,
            v2: 1_usize,
            flags: 1_u32,
            special: 10_i32,
            tag: 3_i32,
            front: Some(0_usize),
            ..Default::default()
        },
        Linedef {
            v1: 1_usize,
            v2: 0_usize,
            flags: 4_u32,
            front: Some(0_usize),
            back: Some(1_usize),
            ..Default::default()
        },
    ]
}

/// The sample surfaces: both `"-"` uppers, one wearing a `BRICK1`
/// lower, the other shifted off its seam.
pub(super) fn sample_sides() -> Vec<Sidedef> {
    vec![
        Sidedef {
            upper: "-".to_owned(),
            middle: "STARTAN1".to_owned(),
            lower: "BRICK1".to_owned(),
            ..Default::default()
        },
        Sidedef {
            upper: "-".to_owned(),
            middle: "STARTAN1".to_owned(),
            lower: "-".to_owned(),
            x_offset: 8_i32,
            y_offset: -8_i32,
            ..Default::default()
        },
    ]
}

/// The sample rooms: the plain hall and the tagged, brighter back
/// room on `FLAT1`.
pub(super) fn sample_sectors() -> Vec<Sector> {
    vec![
        Sector {
            floor_height: 0_i32,
            ceiling_height: 128_i32,
            floor_flat: "FLOOR4_8".to_owned(),
            ceiling_flat: "CEIL5".to_owned(),
            light: 160_i32,
            ..Default::default()
        },
        Sector {
            floor_height: 32_i32,
            ceiling_height: 128_i32,
            floor_flat: "FLAT1".to_owned(),
            ceiling_flat: "FLAT1".to_owned(),
            light: 192_i32,
            special: 9_i32,
            tag: 1_i32,
            ..Default::default()
        },
    ]
}

/// The sample map: two vertices, two lines, two things, two sidedefs
/// and two sectors.
pub(super) fn sample_map() -> Map {
    Map {
        vertices: vec![vertex(0_i32, 0_i32), vertex(128_i32, 0_i32)],
        linedefs: sample_lines(),
        things: vec![
            spawn(3001_i32, 64_i32, 128_i32, 90_i32),
            spawn(1_i32, -64_i32, -128_i32, 0_i32),
        ],
        sidedefs: sample_sides(),
        sectors: sample_sectors(),
        ..Map::default()
    }
}

/// The sample block as a binary `Hexen` map: the first line carries
/// `Hexen` script arguments.
pub(super) fn hexen_map() -> Map {
    let mut map = sample_map();
    map.format = MapFormat::Hexen;
    let line = map
        .linedefs
        .get_mut(0_usize)
        .expect("the fixture has linedefs");
    line.args = [7_i32, 0_i32, 0_i32, 0_i32, 250_i32];
    map
}

/// The stored value of `key` on `row`, read through its serialization.
pub(super) fn stored<R: Serialize>(row: &R, key: &str) -> Value {
    serde_json::to_value(row)
        .expect("model rows serialize")
        .get(key)
        .expect("test rows carry the field")
        .clone()
}
