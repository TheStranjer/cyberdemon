//! The field tables themselves. Every wire name a table lists parses
//! back onto its [`Field`] enum and finds that very table entry again;
//! every variant of every per-kind field enum is tabled in its kind's
//! plan; and one fresh [`apply`](super::super::apply) run per listed
//! field proves the field lands on the model column it names — the
//! vertex block's coordinates through their `add`, everything else
//! through `set` — so a table entry that read or wrote nothing cannot
//! hide.

use serde_json::{Value, json};

use crate::wad::map::bulk::edits::apply;
use crate::wad::map::flags::FlagFamily;
use crate::wad::map::query::ElementKind;

use super::super::commit::find_field;
use super::super::fields::{
    Field, LinedefField, SectorField, SidedefField, ThingField, VertexField,
};
use super::super::tables::plan_for;
use super::{Stock, add_request, hexen_map, sample_map, set_request, stored};

/// The six field tables, each with the kind its names parse under.
fn plans() -> [(ElementKind, FlagFamily); 6] {
    [
        (ElementKind::Sectors, FlagFamily::Doom),
        (ElementKind::Sidedefs, FlagFamily::Doom),
        (ElementKind::Vertices, FlagFamily::Doom),
        (ElementKind::Linedefs, FlagFamily::Doom),
        (ElementKind::Linedefs, FlagFamily::Hexen),
        (ElementKind::Things, FlagFamily::Doom),
    ]
}

/// Every table entry's wire name parses back onto its field enum, and
/// the enum finds that very entry in the table again.
#[test]
fn every_table_name_round_trips_through_its_field_enum() {
    for (kind, family) in plans() {
        let fields = plan_for(kind, family).fields;
        for spec in fields {
            let name = spec.field.name();
            let parsed = Field::of(kind, name);
            assert_eq!(parsed, Some(spec.field), "{name} parses back onto itself");
            let listed = find_field(fields, spec.field);
            assert!(
                matches!(listed, Some(found) if core::ptr::eq(found, spec)),
                "{name} finds its own table entry"
            );
        }
    }
}

/// No field enum variant hides outside the tables: each is listed in a
/// plan of its kind (the two linedef tables together carry all seven).
#[test]
fn every_field_variant_is_tabled_in_its_kind() {
    for field in SectorField::ALL {
        let listed = find_field(
            plan_for(ElementKind::Sectors, FlagFamily::Doom).fields,
            Field::Sector(field),
        );
        assert!(listed.is_some(), "sector {field:?} is tabled");
    }
    for field in SidedefField::ALL {
        let listed = find_field(
            plan_for(ElementKind::Sidedefs, FlagFamily::Doom).fields,
            Field::Sidedef(field),
        );
        assert!(listed.is_some(), "sidedef {field:?} is tabled");
    }
    for field in VertexField::ALL {
        let listed = find_field(
            plan_for(ElementKind::Vertices, FlagFamily::Doom).fields,
            Field::Vertex(field),
        );
        assert!(listed.is_some(), "vertex {field:?} is tabled");
    }
    for field in ThingField::ALL {
        let listed = find_field(
            plan_for(ElementKind::Things, FlagFamily::Doom).fields,
            Field::Thing(field),
        );
        assert!(listed.is_some(), "thing {field:?} is tabled");
    }
    let doom = plan_for(ElementKind::Linedefs, FlagFamily::Doom).fields;
    let hexen = plan_for(ElementKind::Linedefs, FlagFamily::Hexen).fields;
    for field in LinedefField::ALL {
        let key = Field::Linedef(field);
        assert!(
            find_field(doom, key).is_some() || find_field(hexen, key).is_some(),
            "linedef {field:?} is tabled in a family"
        );
    }
}

/// The model column one sector field lands on, with a value the sample
/// room 0 does not already hold.
fn sector_case(field: SectorField) -> (&'static str, Value) {
    match field {
        SectorField::Ceiling => ("ceiling_height", json!(200_i64)),
        SectorField::CeilingFlat => ("ceiling_flat", json!("FLAT1")),
        SectorField::Floor => ("floor_height", json!(8_i64)),
        SectorField::FloorFlat => ("floor_flat", json!("CEIL5")),
        SectorField::Light => ("light", json!(64_i64)),
        SectorField::Special => ("special", json!(11_i64)),
        SectorField::Tag => ("tag", json!(5_i64)),
    }
}

/// The model column one sidedef field lands on, with a value the sample
/// side 0 does not already hold.
fn side_case(field: SidedefField) -> (&'static str, Value) {
    match field {
        SidedefField::Lower => ("lower", json!("-")),
        SidedefField::Middle => ("middle", json!("BRICK1")),
        SidedefField::Upper => ("upper", json!("STARTAN1")),
        SidedefField::XOffset => ("x_offset", json!(8_i64)),
        SidedefField::YOffset => ("y_offset", json!(-8_i64)),
    }
}

/// The model column one linedef field lands on, with the amount to set
/// and the whole stored column it expects afterwards (the `Hexen` fixture
/// row 0 carries args `[7, 0, 0, 0, 250]` and special 10).
fn linedef_case(field: LinedefField) -> (&'static str, i64, Value) {
    match field {
        LinedefField::Arg0 => ("args", 3_i64, json!([3_i64, 0_i64, 0_i64, 0_i64, 250_i64])),
        LinedefField::Arg1 => ("args", 5_i64, json!([7_i64, 5_i64, 0_i64, 0_i64, 250_i64])),
        LinedefField::Arg2 => ("args", 6_i64, json!([7_i64, 0_i64, 6_i64, 0_i64, 250_i64])),
        LinedefField::Arg3 => ("args", 9_i64, json!([7_i64, 0_i64, 0_i64, 9_i64, 250_i64])),
        LinedefField::Arg4 => (
            "args",
            200_i64,
            json!([7_i64, 0_i64, 0_i64, 0_i64, 200_i64]),
        ),
        LinedefField::Special => ("special", 88_i64, json!(88_i64)),
        LinedefField::Tag => ("tag", 5_i64, json!(5_i64)),
    }
}

/// The model column one thing field lands on, with a value the sample
/// thing 0 does not already hold.
fn thing_case(field: ThingField) -> (&'static str, Value) {
    match field {
        ThingField::Angle => ("angle", json!(45_i64)),
        ThingField::Type => ("type", json!(3004_i64)),
        ThingField::X => ("x", json!(100_i64)),
        ThingField::Y => ("y", json!(100_i64)),
    }
}

/// One `set` of every field the sector table lists lands on its column.
#[test]
fn every_listed_sector_field_moves_its_column() {
    let stock = Stock::new();
    let scope = stock.doom();
    for field in SectorField::ALL {
        let (stored_key, value) = sector_case(field);
        let mut map = sample_map();
        let applied = apply(
            &mut map,
            ElementKind::Sectors,
            &[0_usize],
            &set_request(&[(field.name(), value.clone())]),
            &scope,
        )
        .expect("a tabled field sets");
        assert_eq!(applied.changed, 1_usize, "sector {field:?} changed row 0");
        let row = applied.preview.first().expect("one preview row");
        assert_eq!(
            row.after.get(field.name()),
            Some(&value),
            "sector {field:?} previews its future"
        );
        assert_eq!(
            stored(&map.sectors[0], stored_key),
            value,
            "sector {field:?} landed in {stored_key}"
        );
    }
}

/// One `set` of every field the sidedef table lists lands on its column.
#[test]
fn every_listed_sidedef_field_moves_its_column() {
    let stock = Stock::new();
    let scope = stock.doom();
    for field in SidedefField::ALL {
        let (stored_key, value) = side_case(field);
        let mut map = sample_map();
        let applied = apply(
            &mut map,
            ElementKind::Sidedefs,
            &[0_usize],
            &set_request(&[(field.name(), value.clone())]),
            &scope,
        )
        .expect("a tabled field sets");
        assert_eq!(applied.changed, 1_usize, "sidedef {field:?} changed row 0");
        assert_eq!(
            stored(&map.sidedefs[0], stored_key),
            value,
            "sidedef {field:?} landed in {stored_key}"
        );
    }
}

/// One `add` of every vertex field shifts its coordinate: vertices are
/// moved, never set.
#[test]
fn every_listed_vertex_field_moves_its_coordinate() {
    let stock = Stock::new();
    let scope = stock.doom();
    for field in VertexField::ALL {
        let mut map = sample_map();
        let applied = apply(
            &mut map,
            ElementKind::Vertices,
            &[0_usize],
            &add_request(&[(field.name(), 8_i64)]),
            &scope,
        )
        .expect("a tabled vertex field adds");
        assert_eq!(applied.changed, 1_usize, "vertex {field:?} moved row 0");
        assert_eq!(
            stored(&map.vertices[0], field.name()),
            json!(8_i64),
            "vertex {field:?} landed"
        );
    }
}

/// One `set` of every field the `Doom` linedef table lists lands on its
/// column.
#[test]
fn every_listed_doom_linedef_field_moves_its_column() {
    let stock = Stock::new();
    let scope = stock.doom();
    let fields = plan_for(ElementKind::Linedefs, FlagFamily::Doom).fields;
    for field in LinedefField::ALL {
        if find_field(fields, Field::Linedef(field)).is_none() {
            continue;
        }
        let (stored_key, amount, expected) = linedef_case(field);
        let mut map = sample_map();
        let applied = apply(
            &mut map,
            ElementKind::Linedefs,
            &[0_usize],
            &set_request(&[(field.name(), json!(amount))]),
            &scope,
        )
        .expect("a tabled linedef field sets");
        assert_eq!(
            applied.changed, 1_usize,
            "doom line {field:?} changed row 0"
        );
        assert_eq!(
            stored(&map.linedefs[0], stored_key),
            expected,
            "doom line {field:?} landed in {stored_key}"
        );
    }
}

/// One `set` of every field the `Hexen` linedef table lists — the action
/// and all five script arguments — lands on its column.
#[test]
fn every_listed_hexen_linedef_field_moves_its_column() {
    let stock = Stock::new();
    let scope = stock.hexen();
    let fields = plan_for(ElementKind::Linedefs, FlagFamily::Hexen).fields;
    for field in LinedefField::ALL {
        if find_field(fields, Field::Linedef(field)).is_none() {
            continue;
        }
        let (stored_key, amount, expected) = linedef_case(field);
        let mut map = hexen_map();
        let applied = apply(
            &mut map,
            ElementKind::Linedefs,
            &[0_usize],
            &set_request(&[(field.name(), json!(amount))]),
            &scope,
        )
        .expect("a tabled Hexen linedef field sets");
        assert_eq!(
            applied.changed, 1_usize,
            "Hexen line {field:?} changed row 0"
        );
        assert_eq!(
            stored(&map.linedefs[0], stored_key),
            expected,
            "Hexen line {field:?} landed in {stored_key}"
        );
    }
}

/// One `set` of every field the thing table lists lands on its column.
#[test]
fn every_listed_thing_field_moves_its_column() {
    let stock = Stock::new();
    let scope = stock.doom();
    for field in ThingField::ALL {
        let (stored_key, value) = thing_case(field);
        let mut map = sample_map();
        let applied = apply(
            &mut map,
            ElementKind::Things,
            &[0_usize],
            &set_request(&[(field.name(), value.clone())]),
            &scope,
        )
        .expect("a tabled thing field sets");
        assert_eq!(applied.changed, 1_usize, "thing {field:?} changed row 0");
        assert_eq!(
            stored(&map.things[0], stored_key),
            value,
            "thing {field:?} landed in {stored_key}"
        );
    }
}
