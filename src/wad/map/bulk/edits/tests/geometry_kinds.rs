//! The blocks that carry no flag column. Every settable sector and
//! sidedef field is applied one at a time on a fresh copy of the sample
//! map, the vertices shift by `add` only, one `set` and one `add` share a
//! row, face and flat names answer to the stock sets (with `"-"` legal on
//! a sidedef face and `allow_missing` letting an unstocked name through),
//! and the refusals — a value outside its range, a ceiling below its
//! floor, one bad row in a wider selection, a `set` on vertices, an
//! unknown field name and the same field in both maps — all leave the map
//! exactly as it was.

use serde_json::json;

use crate::wad::map::bulk::edits::{FieldEdits, apply};
use crate::wad::map::query::ElementKind;

use super::{Stock, add_request, mixed, sample_map, set_request, stored};

/// Every settable sector field, on a fresh copy of the map.
#[test]
fn every_settable_sector_field_applies() {
    let stock = Stock::new();
    let scope = stock.doom();
    for (field, key, before, value) in [
        ("ceiling", "ceiling_height", json!(128_i64), json!(200_i64)),
        (
            "ceiling_flat",
            "ceiling_flat",
            json!("CEIL5"),
            json!("FLAT1"),
        ),
        ("floor", "floor_height", json!(0_i64), json!(8_i64)),
        (
            "floor_flat",
            "floor_flat",
            json!("FLOOR4_8"),
            json!("CEIL5"),
        ),
        ("light", "light", json!(160_i64), json!(64_i64)),
        ("special", "special", json!(0_i64), json!(11_i64)),
        ("tag", "tag", json!(0_i64), json!(5_i64)),
    ] {
        let mut map = sample_map();
        let applied = apply(
            &mut map,
            ElementKind::Sectors,
            &[0],
            &set_request(&[(field, value.clone())]),
            &scope,
        )
        .expect("a table field sets");
        assert_eq!(applied.matched, 1_usize, "{field} matched the row");
        assert_eq!(applied.changed, 1_usize, "{field} changed the row");
        assert_eq!(applied.indices, vec![0_usize], "{field} is listed");
        assert_eq!(stored(&map.sectors[0], key), value, "{field} landed");
        let row = applied.preview.first().expect("one preview row");
        assert_eq!(
            row.before.get(field).expect("the old value"),
            &before,
            "{field} previews its past"
        );
        assert_eq!(
            row.after.get(field).expect("the new value"),
            &value,
            "{field} previews its future"
        );
    }
}

/// Every settable sidedef field, on a fresh copy of the map.
#[test]
fn every_settable_sidedef_field_applies() {
    let stock = Stock::new();
    let scope = stock.doom();
    for (field, before, value) in [
        ("lower", json!("BRICK1"), json!("-")),
        ("middle", json!("STARTAN1"), json!("BRICK1")),
        ("upper", json!("-"), json!("STARTAN1")),
        ("x_offset", json!(0_i64), json!(8_i64)),
        ("y_offset", json!(0_i64), json!(-8_i64)),
    ] {
        let mut map = sample_map();
        let applied = apply(
            &mut map,
            ElementKind::Sidedefs,
            &[0],
            &set_request(&[(field, value.clone())]),
            &scope,
        )
        .expect("a table field sets");
        assert_eq!(applied.changed, 1_usize, "{field} changed the row");
        assert_eq!(stored(&map.sidedefs[0], field), value, "{field} landed");
        let row = applied.preview.first().expect("one preview row");
        assert_eq!(
            row.before.get(field).expect("the old value"),
            &before,
            "{field} previews its past"
        );
    }
}

/// `add` shifts both coordinates of every selected vertex.
#[test]
fn vertices_shift_only_by_add() {
    let stock = Stock::new();
    let scope = stock.doom();
    let mut map = sample_map();
    let applied = apply(
        &mut map,
        ElementKind::Vertices,
        &[1, 0],
        &add_request(&[("x", 16_i64), ("y", -16_i64)]),
        &scope,
    )
    .expect("adds slide the sheet");
    assert_eq!(applied.matched, 2_usize, "both vertices");
    assert_eq!(applied.changed, 2_usize, "both moved");
    assert_eq!(applied.indices, vec![0_usize, 1], "reported ascending");
    assert_eq!(
        stored(&map.vertices[0], "x"),
        json!(16_i64),
        "the first slid"
    );
    assert_eq!(stored(&map.vertices[0], "y"), json!(-16_i64), "and down");
    assert_eq!(
        stored(&map.vertices[1], "x"),
        json!(144_i64),
        "the second slid too"
    );
    assert_eq!(
        applied.preview[1].before,
        json!({"x": 128_i64, "y": 0_i64}),
        "the preview shows the past"
    );
    assert_eq!(
        applied.preview[1].after,
        json!({"x": 144_i64, "y": -16_i64}),
        "and the future"
    );
}

/// `set` and `add` ride one pass over the same sector.
#[test]
fn add_and_set_mix_on_one_sector() {
    let stock = Stock::new();
    let scope = stock.doom();
    let mut map = sample_map();
    let applied = apply(
        &mut map,
        ElementKind::Sectors,
        &[0],
        &mixed(
            &[("light", json!(96_i64))],
            &[("floor", 16_i64), ("tag", 2_i64)],
        ),
        &scope,
    )
    .expect("set and add mix");
    assert_eq!(applied.changed, 1_usize, "one element changed");
    assert_eq!(applied.matched, 1_usize, "one element matched");
    let sector = &map.sectors[0];
    assert_eq!(
        (
            sector.floor_height,
            sector.ceiling_height,
            sector.light,
            sector.tag
        ),
        (16_i32, 128_i32, 96_i32, 2_i32),
        "the set light and the added floor and tag all landed"
    );
    let row = applied.preview.first().expect("one preview row");
    assert_eq!(
        row.before,
        json!({"floor": 0_i64, "light": 160_i64, "tag": 0_i64}),
        "every touched field previews"
    );
    assert_eq!(
        row.after,
        json!({"floor": 16_i64, "light": 96_i64, "tag": 2_i64}),
        "with its future"
    );
}

/// Faces take stock names and always `"-"`; unstocked names refuse.
#[test]
fn face_names_need_stock_or_the_dash() {
    let stock = Stock::new();
    let scope = stock.doom();
    let mut map = sample_map();
    let applied = apply(
        &mut map,
        ElementKind::Sidedefs,
        &[0],
        &set_request(&[("upper", json!("-")), ("middle", json!("BRICK1"))]),
        &scope,
    )
    .expect("\"-\" is always legal on a face");
    assert_eq!(applied.changed, 1_usize, "only the moved face counts");
    assert_eq!(map.sidedefs[0].upper, "-", "the face already \"-\" stayed");
    assert_eq!(map.sidedefs[0].middle, "BRICK1", "the stock name landed");
    let mut other = sample_map();
    let error = apply(
        &mut other,
        ElementKind::Sidedefs,
        &[0],
        &set_request(&[("lower", json!("NOPE1"))]),
        &scope,
    )
    .expect_err("an unstocked face name is missing");
    assert_eq!(error.code(), "not_found", "missing stock is not_found");
    assert!(
        error.to_string().contains("allow_missing"),
        "the refusal names the escape hatch: {error}"
    );
    assert_eq!(other, sample_map(), "nothing was written");
}

/// `"-"` names no flat: it is not exempt on the sector surfaces.
#[test]
fn flat_dash_is_not_exempt() {
    let stock = Stock::new();
    let scope = stock.doom();
    let mut map = sample_map();
    let error = apply(
        &mut map,
        ElementKind::Sectors,
        &[0],
        &set_request(&[("floor_flat", json!("-"))]),
        &scope,
    )
    .expect_err("\"-\" is no floor");
    assert_eq!(error.code(), "not_found", "the missing flat is not_found");
    assert!(
        error.to_string().contains("floor_flat"),
        "and named: {error}"
    );
    assert_eq!(map, sample_map(), "nothing was written");
}

/// `allow_missing` writes names the stock does not hold, uppercased.
#[test]
fn allow_missing_writes_unknown_names() {
    let stock = Stock::new();
    let scope = stock.permissive();
    let mut map = sample_map();
    apply(
        &mut map,
        ElementKind::Sectors,
        &[1],
        &set_request(&[("floor_flat", json!("slime13"))]),
        &scope,
    )
    .expect("the escape hatch lets the name through");
    assert_eq!(map.sectors[1].floor_flat, "SLIME13", "stored uppercase");
    let mut side = sample_map();
    apply(
        &mut side,
        ElementKind::Sidedefs,
        &[0],
        &set_request(&[("upper", json!("nope1"))]),
        &scope,
    )
    .expect("and so for a face");
    assert_eq!(side.sidedefs[0].upper, "NOPE1", "the face too");
    assert_eq!(
        stored(&map.sectors[1], "floor_flat"),
        json!("SLIME13"),
        "the preview-relevant row value is the stored one"
    );
}

/// Out-of-range results refuse with the rule, writing nothing.
#[test]
fn out_of_range_values_refuse_and_write_nothing() {
    let stock = Stock::new();
    let scope = stock.doom();
    for (request, index, needle) in [
        (
            set_request(&[("light", json!(-40_i64))]),
            1_usize,
            "sector 1 light would become -40; light must stay within 0..=255",
        ),
        (
            add_request(&[("light", 100_i64)]),
            0_usize,
            "sector 0 light would become 260; light must stay within 0..=255",
        ),
        (
            set_request(&[("ceiling", json!(0x8000_i64))]),
            0_usize,
            "sector 0 ceiling would become 32768; ceiling must stay within -32768..=32767",
        ),
        (
            set_request(&[("floor", json!(200_i64)), ("ceiling", json!(64_i64))]),
            0_usize,
            "sector 0 would end with floor 200 above ceiling 64",
        ),
    ] {
        let mut map = sample_map();
        let error = apply(&mut map, ElementKind::Sectors, &[index], &request, &scope)
            .expect_err("the range rule bites before any write");
        assert_eq!(error.code(), "invalid_argument", "code for {needle}");
        assert!(
            error.to_string().contains(needle),
            "the message says \"{needle}\": {error}"
        );
        assert_eq!(map, sample_map(), "the map is untouched: {needle}");
    }
}

/// A vertex coordinate leaving the binary `i16` range refuses.
#[test]
fn vertex_coordinates_keep_the_binary_range() {
    let stock = Stock::new();
    let scope = stock.doom();
    let mut map = sample_map();
    let error = apply(
        &mut map,
        ElementKind::Vertices,
        &[1],
        &add_request(&[("x", 32700_i64)]),
        &scope,
    )
    .expect_err("128 + 32700 leaves i16");
    assert_eq!(error.code(), "invalid_argument", "out of i16");
    assert!(
        error
            .to_string()
            .contains("vertex 1 x would become 32828; x must stay within -32768..=32767"),
        "the refusal names row, field and range: {error}"
    );
    assert_eq!(map, sample_map(), "nothing was written");
}

/// One bad row holds back the whole selection, even good fields.
#[test]
fn one_bad_row_holds_back_the_whole_selection() {
    let stock = Stock::new();
    let scope = stock.doom();
    let mut map = sample_map();
    let error = apply(
        &mut map,
        ElementKind::Sectors,
        &[0, 1],
        &mixed(&[("light", json!(32_i64))], &[("floor", 100_i64)]),
        &scope,
    )
    .expect_err("sector 1 would end 132 over 128");
    assert!(
        error
            .to_string()
            .contains("sector 1 would end with floor 132 above ceiling 128"),
        "the refusal names the offending row: {error}"
    );
    assert_eq!(
        map,
        sample_map(),
        "the legal row 0 and the light set stand untouched"
    );
}

/// `set` on vertices is refused, naming the `add`-able fields.
#[test]
fn vertices_support_add_only_never_set() {
    let stock = Stock::new();
    let scope = stock.doom();
    let mut map = sample_map();
    let error = apply(
        &mut map,
        ElementKind::Vertices,
        &[0],
        &set_request(&[("x", json!(5_i64))]),
        &scope,
    )
    .expect_err("a vertex is moved, not set");
    assert_eq!(error.code(), "invalid_argument", "a vertex set is refused");
    let message = error.to_string();
    assert!(
        message.contains("add") && message.contains("x, y"),
        "the refusal lists what vertices take: {message}"
    );
    assert_eq!(map, sample_map(), "nothing was written");
}

/// Unknown and un-addable field names come back as the kind's table.
#[test]
fn unknown_fields_list_the_valid_ones() {
    let stock = Stock::new();
    let scope = stock.doom();
    let mut map = sample_map();
    let sector_error = apply(
        &mut map,
        ElementKind::Sectors,
        &[0],
        &set_request(&[("roof", json!(1_i64))]),
        &scope,
    )
    .expect_err("sectors have no roof");
    assert!(
        sector_error.to_string().contains(
            "valid fields are ceiling, ceiling_flat, floor, floor_flat, light, \
                 special, tag"
        ),
        "the sector table is listed: {sector_error}"
    );
    let side_error = apply(
        &mut map,
        ElementKind::Sidedefs,
        &[0],
        &add_request(&[("sector", 1_i64)]),
        &scope,
    )
    .expect_err("sidedef fields are not add targets here");
    assert!(
        side_error
            .to_string()
            .contains("valid fields are lower, middle, upper, x_offset, y_offset"),
        "the sidedef table is listed: {side_error}"
    );
    let special_error = apply(
        &mut map,
        ElementKind::Sectors,
        &[0],
        &add_request(&[("special", 1_i64)]),
        &scope,
    )
    .expect_err("special takes no amount");
    assert!(
        special_error
            .to_string()
            .contains("not addable on sectors: addable fields are ceiling, floor, light, tag"),
        "the addable table is listed: {special_error}"
    );
    assert_eq!(map, sample_map(), "nothing was written");
}

/// One field cannot carry a `set` value and an `add` amount at once.
#[test]
fn a_field_cannot_be_set_and_added_at_once() {
    let stock = Stock::new();
    let scope = stock.doom();
    let mut map = sample_map();
    let error = apply(
        &mut map,
        ElementKind::Sectors,
        &[0],
        &mixed(&[("light", json!(64_i64))], &[("light", 8_i64)]),
        &scope,
    )
    .expect_err("one field, two fates");
    assert_eq!(error.code(), "invalid_argument", "the clash refuses");
    assert!(
        error
            .to_string()
            .contains("carries both a set value and an add amount"),
        "and says so: {error}"
    );
    assert_eq!(map, sample_map(), "nothing was written");
}

/// These three kinds store no flags: flag names refuse with a pointer.
#[test]
fn flags_belong_to_linedefs_and_things() {
    let stock = Stock::new();
    let scope = stock.doom();
    let request = FieldEdits {
        flags_set: vec!["blocking".to_owned()],
        ..Default::default()
    };
    for kind in [
        ElementKind::Sectors,
        ElementKind::Sidedefs,
        ElementKind::Vertices,
    ] {
        let mut map = sample_map();
        let error =
            apply(&mut map, kind, &[0], &request, &scope).expect_err("these kinds store no flags");
        assert_eq!(
            error.code(),
            "invalid_argument",
            "flag refusal is invalid_argument"
        );
        assert!(
            error.to_string().contains("no flags field"),
            "the refusal says so for {}: {error}",
            kind.as_str()
        );
        assert_eq!(map, sample_map(), "nothing was written");
    }
}
