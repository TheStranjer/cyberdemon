//! Shared fixtures and readers for the format-conversion tests: the target
//! constructors, the write-then-read [`cycle`], the `UDMF`, binary `Hexen`
//! and door fixtures, the [`KindCounts`] builder and the refusal reader
//! every topic asserts against. Each helper is a function of its arguments
//! and the fixtures in [`crate::wad::map::common::fixtures`], so no topic
//! can disturb what another one measures.
//!
//! [`same_family`] rewrites a map inside one flag family,
//! [`cross_family`] runs it through the [`xlat`](crate::wad::map::xlat)
//! table both ways and [`requests`] checks which requests refuse, how a
//! target validates its words and what the documented families, names and
//! node lumps are.

mod cross_family;
mod requests;
mod same_family;

use super::{Conversion, KindCounts, Target, convert_map};
use crate::wad::classify::{MapFormat, analyze};
use crate::wad::map::common::fixtures::{lump, marker, name, two_room_map};
use crate::wad::map::udmf_map::read_udmf_map;
use crate::wad::map::{Map, read_map, write_map};

/// The target for the binary layout `to`.
pub(super) fn binary(to: &str) -> Target {
    Target::parse(to, None).expect("the test targets are valid")
}

/// The parsed `UDMF` target, naming no namespace.
pub(super) fn udmf_default() -> Target {
    Target::parse("udmf", None).expect("udmf is a target")
}

/// The parsed `UDMF` target declaring `namespace`.
pub(super) fn udmf(namespace: &str) -> Target {
    Target::parse("udmf", Some(namespace)).expect("the test namespaces are valid")
}

/// Writes `map` and reads it back through the format dispatcher.
pub(super) fn cycle(map: &Map) -> Map {
    let lumps = write_map(map).expect("the test map writes");
    let analysis = analyze(&lumps);
    let block = analysis
        .maps
        .first()
        .expect("the written lumps are a block");
    read_map(&lumps, block).expect("and decodes back")
}

/// The `UDMF` two-room map in `namespace`, carrying node lumps.
pub(super) fn udmf_map(namespace: &str) -> Map {
    let mut map = two_room_map();
    map.format = MapFormat::Udmf;
    map.udmf_namespace = Some(namespace.to_owned());
    map.other = vec![
        lump("ZNODES", b"znod"),
        lump("NODES", b"nodes"),
        lump("BEHAVIOR", b"behavior"),
    ];
    map
}

/// The two-room map as binary `Hexen`: `tid`s, `z`, one
/// `Line_SetIdentification` line and the two `Hexen` script lumps.
pub(super) fn hexen_map() -> Map {
    let mut map = two_room_map();
    map.format = MapFormat::Hexen;
    if let Some(first) = map.things.first_mut() {
        first.tid = 5_i32;
        first.z = 128_i32;
    }
    if let Some(portal) = map.linedefs.get_mut(2) {
        portal.special = 121_i32;
        portal.args = [7_i32, 0_i32, 0_i32, 0_i32, 0_i32];
    }
    map.other.push(lump("BEHAVIOR", b"ACS\0script"));
    map.other.push(lump("SCRIPTS", b"animated"));
    map
}

/// A `Doom` family `UDMF` map from one `TEXTMAP` body, whose block also
/// carries a `BEHAVIOR` lump.
pub(super) fn doom_udmf(body: &str) -> Map {
    let text = format!("namespace = \"doom\"; {body}");
    let members = [
        lump("TEXTMAP", text.as_bytes()),
        lump("BEHAVIOR", b"acs"),
        marker("ENDMAP"),
    ];
    read_udmf_map(name("MAP01"), &members).expect("the test TEXTMAP decodes")
}

/// A `zdoom` map from one `TEXTMAP` body.
pub(super) fn zdoom_udmf(body: &str) -> Map {
    let text = format!("namespace = \"zdoom\"; {body}");
    let members = [lump("TEXTMAP", text.as_bytes()), marker("ENDMAP")];
    read_udmf_map(name("MAP01"), &members).expect("the test TEXTMAP decodes")
}

/// The block lump names of `map`, in order.
pub(super) fn carried(map: &Map) -> Vec<String> {
    map.other.iter().map(|lump| lump.name.to_string()).collect()
}

/// The refusal `convert_map` gives `map` for `target`, as `(code, text)`.
pub(super) fn refusal(map: &Map, target: &Target) -> (String, String) {
    let mut copy = map.clone();
    match convert_map(&mut copy, target) {
        Err(error) => (error.code().to_owned(), error.to_string()),
        Ok(conversion) => panic!("expected a refusal, converted into {conversion:?}"),
    }
}

/// One `KindCounts` row from its five numbers, in [`KindCounts`] field
/// order: things, linedefs, sidedefs, sectors, vertices.
pub(super) fn counts(row: [usize; 5]) -> KindCounts {
    let [things, linedefs, sidedefs, sectors, vertices] = row;
    KindCounts {
        things,
        linedefs,
        sidedefs,
        sectors,
        vertices,
    }
}

/// The cross-family report keys of `out` as serialized: all four at
/// their `derive(Default)` zero, alongside the ever-present
/// `dropped_fields`, the shape a same-family step always publishes.
pub(super) fn assert_cross_family_keys_are_zero(out: &Conversion) {
    let value = serde_json::to_value(out).expect("a Conversion serializes");
    assert_eq!(value["translated_specials"], 0_i32, "the table did not run");
    assert_eq!(
        value["untranslated"],
        serde_json::json!([]),
        "nothing failed to translate"
    );
    assert_eq!(value["thing_flags_changed"], 0_i32, "no thing flag moved");
    assert_eq!(value["sector_types_changed"], 0_i32, "no sector type moved");
    assert_eq!(
        value["dropped_fields"],
        serde_json::json!({
            "things": 0_i32,
            "linedefs": 0_i32,
            "sidedefs": 0_i32,
            "sectors": 0_i32,
            "vertices": 0_i32,
        }),
        "the pre-existing key rides on the same object"
    );
}

/// The two-room map with a `Doom` door line: linedef 2 is special 1
/// (`Door_Raise`, tag 5) and carries the `passuse` bit.
fn door_map() -> Map {
    let mut map = two_room_map();
    if let Some(step) = map.linedefs.get_mut(2) {
        step.special = 1_i32;
        step.tag = 5_i32;
        step.flags = 0x201_u32;
    }
    map
}

/// The two-room map with the door line converted forward to `zdoom`:
/// the model and the conversion report, for the forward and reverse
/// tests to share.
pub(super) fn door_udmf_zdoom() -> (Map, Conversion) {
    let mut map = door_map();
    let out = convert_map(&mut map, &udmf("zdoom")).expect("the forward direction converts");
    (map, out)
}
