//! Shared builders and readers for the optimize tests: the lump and
//! marker builders, the plan runners and row readers, the `PNAMES`,
//! `TEXTURE1` and flat-`MAP` payload builders, the `patches`-region
//! fixture and the `ANIMATED`/`SWITCHES` lump writers. `redundant` runs
//! the three redundancy sweeps and the plan bookkeeping; `unused` the
//! content-reading sweeps, their errors, the wire shape of an [`Action`]
//! row and [`super::apply`]; `animations` the animation and switch tables
//! the sweeps expand with.

mod animations;
mod redundant;
mod unused;

use super::{Action, Inputs, Operation, Plan, plan};
use crate::wad::archive::Lump;
use crate::wad::classify::MapFormat;
use crate::wad::map::{Map, Sector, write_map};
use crate::wad::name::LumpName;
use crate::wad::textures::{
    PatchRef, TextureDef, TextureLayout, TextureLump, parse_pnames, write_pnames,
    write_texture_lump,
};

/// A valid parsed name for test data.
pub(super) fn name(text: &str) -> LumpName {
    LumpName::parse(text).expect("test names are valid")
}

/// A lump named `text` carrying `payload`.
pub(super) fn lump(text: &str, payload: &[u8]) -> Lump {
    Lump::new(name(text), payload.to_vec())
}

/// An empty marker lump named `text`.
pub(super) fn marker(text: &str) -> Lump {
    Lump::marker(name(text))
}

/// The `(operation, target)` pairs of a plan, for one-glance asserts.
pub(super) fn rows(plan: &Plan) -> Vec<(Operation, String)> {
    plan.actions
        .iter()
        .map(|action| (action.operation, action.name()))
        .collect()
}

/// The names an action list targets, lump or map.
pub(super) fn targets(plan: &Plan) -> Vec<String> {
    plan.actions.iter().map(Action::name).collect()
}

/// The indices `operation` planned to drop, in action order.
pub(super) fn dropped(plan: &Plan, operation: Operation) -> Vec<usize> {
    plan.actions
        .iter()
        .filter(|action| action.operation == operation)
        .map(|action| action.index)
        .collect()
}

/// A plan of `operations` where merged is base then working.
pub(super) fn planned(working: &[Lump], base: &[Lump], operations: &[Operation]) -> Plan {
    let mut merged: Vec<Lump> = base.to_vec();
    merged.extend_from_slice(working);
    let inputs = Inputs {
        working,
        base,
        merged: &merged,
    };
    plan(&inputs, operations).expect("the test sweeps run")
}

/// A one-sector binary `Doom` block named `text` whose sector floors
/// `floor` and ceilings `ceiling`.
pub(super) fn flat_map(text: &str, floor: &str, ceiling: &str) -> Vec<Lump> {
    let mut map = Map::new_empty(name(text), MapFormat::Doom);
    map.sectors.push(Sector {
        floor_flat: floor.to_owned(),
        ceiling_flat: ceiling.to_owned(),
        ..Sector::default()
    });
    write_map(&map).expect("the test map serializes")
}

/// A `PNAMES` lump listing `names`.
pub(super) fn pnames(names: &[&str]) -> Lump {
    let list: Vec<String> = names.iter().map(|text| (*text).to_owned()).collect();
    Lump::new(
        name("PNAMES"),
        write_pnames(&list).expect("test names are valid"),
    )
}

/// A one-texture `TEXTURE1` referencing `patches`.
pub(super) fn texture1(patches: &[u16]) -> Lump {
    let lump = TextureLump {
        layout: TextureLayout::Doom,
        textures: vec![TextureDef {
            name: "WALL1".to_owned(),
            flags_raw: 0,
            width: 64,
            height: 128,
            patches: patches
                .iter()
                .copied()
                .map(|patch| PatchRef { patch, x: 0, y: 0 })
                .collect(),
        }],
    };
    Lump::new(
        name("TEXTURE1"),
        write_texture_lump(&lump).expect("the test lump serializes"),
    )
}

/// The `PNAMES` names a payload carries.
pub(super) fn pnames_of(data: &[u8]) -> Vec<String> {
    parse_pnames(data).expect("the test PNAMES parses")
}

/// A working `patches` region holding each of `names`.
pub(super) fn patch_region(names: &[&str]) -> Vec<Lump> {
    let mut out = vec![marker("P_START")];
    for text in names {
        out.push(lump(text, b"patch-bytes"));
    }
    out.push(marker("P_END"));
    out
}

/// `text` padded with `NUL` to `WIDTH` bytes: the shape of one name field
/// in either `Boom` lump.
pub(super) fn padded_field<const WIDTH: usize>(text: &str) -> [u8; WIDTH] {
    let mut field = [0_u8; WIDTH];
    for (slot, byte) in field.iter_mut().zip(text.as_bytes()) {
        *slot = *byte;
    }
    field
}

/// One `ANIMATED` record's bytes: two nine-byte name fields, a speed and
/// the `kind` byte, `255` for the record that ends the list.
pub(super) fn animated_record(first: &str, second: &str, kind: u8) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&padded_field::<9>(first));
    out.extend_from_slice(&padded_field::<9>(second));
    out.extend_from_slice(&8_i32.to_le_bytes());
    out.push(kind);
    out
}

/// The `ANIMATED` lump `records` make, closed by the `255` terminator.
pub(super) fn animated_lump(records: &[Vec<u8>]) -> Lump {
    let mut out = Vec::new();
    for record in records {
        out.extend_from_slice(record);
    }
    out.push(0xFF);
    lump("ANIMATED", &out)
}

/// One `SWITCHES` record's bytes: the episode and two eight-byte names,
/// the `0` episode ending the list.
pub(super) fn switch_record(episode: i16, first: &str, second: &str) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&episode.to_le_bytes());
    out.extend_from_slice(&padded_field::<8>(first));
    out.extend_from_slice(&padded_field::<8>(second));
    out
}

/// The `SWITCHES` lump `records` make, closed by the zero-episode record.
pub(super) fn switches_lump(records: &[Vec<u8>]) -> Lump {
    let mut out = Vec::new();
    for record in records {
        out.extend_from_slice(record);
    }
    out.extend_from_slice(&switch_record(0, "", ""));
    lump("SWITCHES", &out)
}

/// The `ANIMATED` lump declaring one flat range per row of `rows`.
pub(super) fn animated_ranges(rows: &[(&str, &str)]) -> Lump {
    let records: Vec<Vec<u8>> = rows
        .iter()
        .map(|&(first, second)| animated_record(first, second, 0))
        .collect();
    animated_lump(&records)
}

/// The `SWITCHES` lump holding one pair per row of `pairs`.
pub(super) fn switch_pairs(pairs: &[(&str, &str)]) -> Lump {
    let records: Vec<Vec<u8>> = pairs
        .iter()
        .map(|&(first, second)| switch_record(1, first, second))
        .collect();
    switches_lump(&records)
}
