//! Shared builders for the bulk-edit machinery's tests: the encoded
//! `Doom` blocks, the layer-only session, the merged-view reader, the
//! `maps` selectors and the [`Replacement`] the tool-shaped runs use.
//!
//! [`maps`] selects and applies: the `maps` wire shapes, `resolve`'s
//! default, `"all"` and listed-name paths, and the one-undo-step,
//! dry-run and empty-selection shapes over many blocks. [`stock`]
//! checks existence: the merged stock's namespaces, `require_target`
//! and `parse_terms`, `replace_thing_type`'s resolved numbers and the
//! folded `run_replacement` reports.

mod maps;
mod stock;

use std::path::PathBuf;

use super::{AllMaps, MapsSelector, Replacement};
use crate::session::{Layer, LayerRole, Scope, Session};
use crate::tools::maps::common;
use crate::wad::archive::{Lump, Wad, WadKind};
use crate::wad::map::bulk::replace_flats;
use crate::wad::map::{Map, Sector, Sidedef, Thing, Vertex, write_map};
use crate::wad::name::LumpName;

/// A parsed test name.
pub(super) fn name(text: &str) -> LumpName {
    LumpName::parse(text).expect("test names are valid")
}

/// An eight-byte dummy lump named `text`.
fn dummy(text: &str) -> Lump {
    Lump::new(name(text), vec![0xA5_u8; 8])
}

/// A tiny `Doom` block named `text`: one sector floored and ceiled
/// `FLOOR4`, one `WALL64` middle and one type-`3004` thing.
fn demo_map(text: &str) -> Map {
    Map {
        name: name(text),
        things: vec![Thing {
            kind: 3004_i32,
            x: 64_i32,
            y: -32_i32,
            angle: 90_i32,
            flags: 7_u32,
            ..Default::default()
        }],
        vertices: vec![
            Vertex {
                x: 0_i32,
                ..Default::default()
            },
            Vertex {
                x: 128_i32,
                ..Default::default()
            },
        ],
        sidedefs: vec![Sidedef {
            middle: "WALL64".to_owned(),
            ..Default::default()
        }],
        sectors: vec![Sector {
            floor_flat: "FLOOR4".to_owned(),
            ceiling_flat: "FLOOR4".to_owned(),
            ..Default::default()
        }],
        ..Map::default()
    }
}

/// The encoded blocks named `texts`, concatenated.
pub(super) fn blocks(texts: &[&str]) -> Vec<Lump> {
    let mut lumps = Vec::new();
    for text in texts {
        let written = write_map(&demo_map(text)).expect("the fixture encodes");
        lumps.extend(written);
    }
    lumps
}

/// A session whose only layer is an IWAD of `lumps`.
pub(super) fn base_session(lumps: Vec<Lump>) -> Session {
    let mut session = Session::default();
    session
        .set_iwad(Layer::new(
            PathBuf::from("base.wad"),
            LayerRole::Iwad,
            Wad {
                kind: WadKind::Iwad,
                lumps,
            },
        ))
        .expect("no batch is open");
    session
}

/// The named map of the merged view, decoded for assertions.
pub(super) fn merged_map(session: &Session, text: &str) -> Map {
    common::load_map(session, name(text), Scope::Merged).expect("the fixture map reads")
}

/// A `Names` selector for the listed texts.
pub(super) fn names_of(texts: &[&str]) -> MapsSelector {
    MapsSelector::Names(texts.iter().map(|text| (*text).to_owned()).collect())
}

/// The `"all"` selector: every distinct block of the merged view.
pub(super) fn all_maps() -> MapsSelector {
    MapsSelector::All(AllMaps)
}

/// A pass that repaints every flat, for the no-op shape checks.
pub(super) fn repaint(map: &mut Map) -> usize {
    replace_flats(map, "*", "FLOOR9", &[])
}

/// The session for the stock checks: `WALL1` (named by its `TX_`
/// lump) and `FLOOR4` exist, nothing else does.
pub(super) fn stocked_session() -> Session {
    let mut lumps = blocks(&["MAP01"]);
    lumps.extend(vec![
        Lump::marker(name("F_START")),
        dummy("FLOOR4"),
        Lump::marker(name("F_END")),
        Lump::marker(name("TX_START")),
        dummy("TX_WALL1"),
        Lump::marker(name("TX_END")),
    ]);
    base_session(lumps)
}

/// The one-map-list [`Replacement`] for the tool machinery tests.
pub(super) fn replacement(selector: &MapsSelector, dry_run: bool) -> Replacement<'_> {
    Replacement {
        label: "replace_texture WALL* -> STEP1",
        maps: Some(selector),
        dry_run,
    }
}
