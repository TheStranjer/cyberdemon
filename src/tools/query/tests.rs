//! Shared builders for the query tools' tests: the fixture map, block and
//! session, the parsed-parameter readers and the argument builders every
//! topic runs through.
//!
//! [`find`] checks the row cap, the paging window, the summary rows and
//! `find_run`'s refusals. [`replace_tools`] runs the three `replace_*`
//! swaps: the narrowed faces and surfaces, the stock-checked targets, the
//! one-step labels, the previews and the resolved thing numbers. [`bulk`]
//! folds the wire parts into the engine request and runs `bulk_in_session`
//! with its guards, `dry_run`, stock checks and paged report.

mod bulk;
mod find;
mod replace_tools;

use std::path::PathBuf;

use serde_json::{Value, json};

use crate::session::{Layer, LayerRole, Session};
use crate::tools::query::bulk::BulkEditParams;
use crate::tools::query::find::FindElementsParams;
use crate::tools::query::replace::ThingRef;
use crate::tools::query::replace_tools::{
    ReplaceFlatParams, ReplaceTextureParams, ReplaceThingTypeParams,
};
use crate::wad::archive::{Lump, Wad, WadKind};
use crate::wad::map::query::{ElementKind, Filter};
use crate::wad::map::{Linedef, Map, MapFormat, Sector, Sidedef, Thing, Vertex, write_map};
use crate::wad::name::LumpName;

/// A parsed test map name.
pub(super) fn parsed(text: &str) -> LumpName {
    LumpName::parse(text).expect("test names are valid")
}

/// A `find_elements` call over the fixture map with this query.
pub(super) fn params(kind: ElementKind, filters: Vec<Filter>) -> FindElementsParams {
    FindElementsParams {
        kind,
        r#where: filters,
        within: None,
        map: Some("MAP01".to_owned()),
        offset: None,
        limit: None,
    }
}

/// A one-line, one-side, one-sector map with a start and two vertices.
pub(super) fn mini_map() -> Map {
    Map {
        name: parsed("MAP01"),
        format: MapFormat::Doom,
        other: Vec::new(),
        things: vec![Thing {
            kind: 1_i32,
            x: 64_i32,
            y: 128_i32,
            angle: 90_i32,
            flags: 7_u32,
            ..Default::default()
        }],
        vertices: vec![
            Vertex {
                x: 0_i32,
                y: 0_i32,
                ..Default::default()
            },
            Vertex {
                x: 128_i32,
                y: 0_i32,
                ..Default::default()
            },
        ],
        linedefs: vec![Linedef {
            v1: 0_usize,
            v2: 1_usize,
            flags: 1_u32,
            special: 10_i32,
            tag: 3_i32,
            front: Some(0_usize),
            ..Default::default()
        }],
        sidedefs: vec![Sidedef {
            middle: "STARTAN".to_owned(),
            sector: 0_usize,
            ..Default::default()
        }],
        sectors: vec![Sector {
            floor_height: 0_i32,
            ceiling_height: 128_i32,
            floor_flat: "FLOOR1".to_owned(),
            ceiling_flat: "F_SKY1".to_owned(),
            light: 170_i32,
            special: 9_i32,
            tag: 3_i32,
            ..Default::default()
        }],
        ..Map::default()
    }
}

/// An eight-byte dummy lump named `text`.
pub(super) fn dummy(text: &str) -> Lump {
    Lump::new(parsed(text), vec![0xA5_u8; 8])
}

/// A fully visible spawn of `kind` at `x`/`y` facing `angle`.
pub(super) fn loaded(kind: i32, x: i32, y: i32, angle: i32) -> Thing {
    Thing {
        kind,
        x,
        y,
        angle,
        flags: 7_u32,
        ..Default::default()
    }
}

/// The bulk-edit fixture map: three sidedef faces named `WALL1`
/// (two middles, one upper), three sector surfaces named `FLOOR1`
/// (both of sector 0, sector 1's floor), a type-1 player start and
/// one imp (3001) for the thing-type swap.
pub(super) fn bulk_map() -> Map {
    Map {
        name: parsed("MAP01"),
        format: MapFormat::Doom,
        things: vec![
            loaded(1_i32, 64_i32, 128_i32, 0_i32),
            loaded(3001_i32, -64_i32, 64_i32, 90_i32),
        ],
        vertices: vec![
            Vertex {
                x: 0_i32,
                y: 0_i32,
                ..Default::default()
            },
            Vertex {
                x: 128_i32,
                y: 0_i32,
                ..Default::default()
            },
        ],
        linedefs: vec![Linedef {
            v1: 0_usize,
            v2: 1_usize,
            flags: 1_u32,
            ..Default::default()
        }],
        sidedefs: vec![
            Sidedef {
                upper: "-".to_owned(),
                middle: "WALL1".to_owned(),
                lower: "-".to_owned(),
                sector: 0_usize,
                ..Default::default()
            },
            Sidedef {
                upper: "WALL1".to_owned(),
                middle: "WALL1".to_owned(),
                lower: "-".to_owned(),
                sector: 1_usize,
                ..Default::default()
            },
        ],
        sectors: vec![
            Sector {
                floor_flat: "FLOOR1".to_owned(),
                ceiling_flat: "FLOOR1".to_owned(),
                ..Default::default()
            },
            Sector {
                floor_flat: "FLOOR1".to_owned(),
                ceiling_flat: "F_SKY1".to_owned(),
                ..Default::default()
            },
        ],
        ..Map::default()
    }
}

/// A session whose only layer holds [`bulk_map`] plus the stock the
/// bulk tools check: flats `FLOOR1` and `NUKAGE1`, wall `STEP1`
/// (named by its `TX_` lump).
pub(super) fn bulk_session() -> Session {
    let mut lumps = write_map(&bulk_map()).expect("the fixture map encodes");
    lumps.extend(vec![
        Lump::marker(parsed("F_START")),
        dummy("FLOOR1"),
        dummy("NUKAGE1"),
        Lump::marker(parsed("F_END")),
        Lump::marker(parsed("TX_START")),
        dummy("TX_STEP1"),
        Lump::marker(parsed("TX_END")),
    ]);
    let mut session = Session::default();
    session
        .set_iwad(Layer::new(
            PathBuf::from("bulk.wad"),
            LayerRole::Iwad,
            Wad {
                kind: WadKind::Iwad,
                lumps,
            },
        ))
        .expect("no batch is open");
    session
}

/// `replace_texture` arguments over the fixture's default map.
pub(super) fn texture_params(from: &str, to: &str) -> ReplaceTextureParams {
    ReplaceTextureParams {
        from: from.to_owned(),
        to: to.to_owned(),
        parts: None,
        maps: None,
        dry_run: None,
    }
}

/// `replace_flat` arguments over the fixture's default map.
pub(super) fn flat_params(from: &str, to: &str) -> ReplaceFlatParams {
    ReplaceFlatParams {
        from: from.to_owned(),
        to: to.to_owned(),
        surfaces: None,
        maps: None,
        dry_run: None,
    }
}

/// `replace_thing_type` arguments over the fixture's default map.
pub(super) fn thing_params(from: ThingRef, to: ThingRef) -> ReplaceThingTypeParams {
    ReplaceThingTypeParams {
        from,
        to,
        maps: None,
        dry_run: None,
        allow_unknown: None,
    }
}

/// The doomednum variant of the test [`ThingRef`].
pub(super) fn number(kind: u32) -> ThingRef {
    ThingRef::Number(kind)
}

/// The name variant of the test [`ThingRef`].
pub(super) fn name(text: &str) -> ThingRef {
    ThingRef::Name(text.to_owned())
}

/// Parse `bulk_edit` arguments from wire `JSON`, defaulting `map`.
pub(super) fn bulk_args(wire: Value) -> BulkEditParams {
    let mut full = wire;
    full.as_object_mut()
        .expect("bulk arguments are an object")
        .entry("map")
        .or_insert_with(|| json!("MAP01"));
    serde_json::from_value(full).expect("test bulk arguments parse")
}

/// One `{field, op, value}` filter as wire `JSON`.
pub(super) fn one_filter(field: &str, op: &str, value: &Value) -> Value {
    json!({"field": field, "op": op, "value": value})
}
