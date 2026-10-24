//! Shared builders and readers for the optimize tests: the lump and
//! marker builders, the plan runners and row readers, the `PNAMES`,
//! `TEXTURE1` and flat-`MAP` payload builders and the `patches`-region
//! fixture. `redundant` runs the three redundancy sweeps and the plan
//! bookkeeping; `unused` the content-reading sweeps, their errors, the
//! wire shape of an [`Action`] row and [`super::apply`].

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
