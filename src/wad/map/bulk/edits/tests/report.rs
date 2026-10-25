//! The [`Applied`] answer and
//! everything the entry gate refuses before a map is read: the
//! matched/changed counts with the before/after preview of the changed
//! rows only, the empty selection and the empty request, an index past
//! the block, the values whose types are wrong and the `add` that leaves
//! the integers, the `UDMF` maps that ignore the binary ranges, and the
//! request's own wire shape.

use serde_json::{from_str, json};

use crate::wad::classify::MapFormat;
use crate::wad::map::bulk::edits::{Applied, Change, FieldEdits, apply};
use crate::wad::map::query::ElementKind;

use super::{Stock, add_request, sample_map, set_request};

/// The report: matched counts the selection, changed counts moves.
#[test]
fn the_report_counts_matches_and_previews_only_changes() {
    let stock = Stock::new();
    let scope = stock.doom();
    let mut map = sample_map();
    let applied = apply(
        &mut map,
        ElementKind::Sectors,
        &[1, 0, 1],
        &set_request(&[("light", json!(160_i64))]),
        &scope,
    )
    .expect("an in-range set applies");
    assert_eq!(
        applied.matched, 2_usize,
        "duplicates collapse into the two rows"
    );
    assert_eq!(
        applied.changed, 1_usize,
        "a no-op counts as matched, not changed"
    );
    assert_eq!(
        applied.indices,
        vec![1_usize],
        "only the moved row is listed"
    );
    assert_eq!(
        applied.preview,
        vec![Change {
            index: 1_usize,
            before: json!({"light": 192_i64}),
            after: json!({"light": 160_i64}),
        }],
        "the preview names the field it moved"
    );
    assert_eq!(
        map.sectors[0].light, 160_i32,
        "the no-op row kept its light"
    );
    assert_eq!(
        map.sectors[1].light, 160_i32,
        "the changed row took the new one"
    );
}

/// An empty selection succeeds with zeroes; an empty request refuses.
#[test]
fn empty_selections_and_requests_are_answered_at_the_gate() {
    let stock = Stock::new();
    let scope = stock.doom();
    let mut map = sample_map();
    let applied = apply(
        &mut map,
        ElementKind::Sectors,
        &[],
        &set_request(&[("light", json!(96_i64))]),
        &scope,
    )
    .expect("an empty selection is a successful nothing");
    assert_eq!(applied, Applied::default(), "zeroes all round");
    assert_eq!(map, sample_map(), "nothing was written");
    let error = apply(
        &mut map,
        ElementKind::Sectors,
        &[0],
        &FieldEdits::default(),
        &scope,
    )
    .expect_err("an empty request is no edit");
    assert!(
        error.to_string().contains("nothing to change"),
        "and says so: {error}"
    );
}

/// `is_empty` reads all four parts of the request.
#[test]
fn is_empty_reads_every_part_of_the_request() {
    assert!(
        FieldEdits::default().is_empty(),
        "the default asks for nothing"
    );
    assert!(
        !set_request(&[("light", json!(0_i64))]).is_empty(),
        "even a zero set asks"
    );
    assert!(
        !add_request(&[("light", 0_i64)]).is_empty(),
        "and a zero add asks"
    );
    let flags = FieldEdits {
        flags_clear: vec!["secret".to_owned()],
        ..Default::default()
    };
    assert!(!flags.is_empty(), "one flag name is a request");
}

/// An index past the block is `not_found`, naming the block's size.
#[test]
fn indices_the_blocks_do_not_hold_are_not_found() {
    let stock = Stock::new();
    let scope = stock.doom();
    let mut map = sample_map();
    let error = apply(
        &mut map,
        ElementKind::Sectors,
        &[2],
        &set_request(&[("light", json!(160_i64))]),
        &scope,
    )
    .expect_err("there is no third sector");
    assert_eq!(error.code(), "not_found", "past the end is not_found");
    assert!(
        error
            .to_string()
            .contains("sector 2 does not exist (the map has 2 sectors, valid indices are 0..=1)"),
        "the message counts the block: {error}"
    );
    let vertex_error = apply(
        &mut map,
        ElementKind::Vertices,
        &[5],
        &add_request(&[("x", 1_i64)]),
        &scope,
    )
    .expect_err("there is no sixth vertex");
    assert!(
        vertex_error.to_string().contains("vertex 5"),
        "and names the row: {vertex_error}"
    );
    assert_eq!(map, sample_map(), "nothing was written");
}

/// Wrong value types refuse before the selection is even read.
#[test]
fn value_types_are_checked_before_the_selection_runs() {
    let stock = Stock::new();
    let scope = stock.doom();
    let mut map = sample_map();
    let light_error = apply(
        &mut map,
        ElementKind::Sectors,
        &[0],
        &set_request(&[("light", json!("160"))]),
        &scope,
    )
    .expect_err("a string is no light level");
    assert_eq!(
        light_error.code(),
        "invalid_argument",
        "a wrong type refuses"
    );
    assert!(
        light_error.to_string().contains("needs an integer value"),
        "the refusal says so: {light_error}"
    );
    let fraction_error = apply(
        &mut map,
        ElementKind::Sectors,
        &[0],
        &set_request(&[("floor", json!(1.5_f64))]),
        &scope,
    )
    .expect_err("a fraction is no height");
    assert!(
        fraction_error.to_string().contains("'floor'"),
        "the field is named: {fraction_error}"
    );
    let name_error = apply(
        &mut map,
        ElementKind::Sidedefs,
        &[0],
        &set_request(&[("middle", json!(7_i64))]),
        &scope,
    )
    .expect_err("a number is no texture name");
    assert!(
        name_error.to_string().contains("needs a name string"),
        "faces want names: {name_error}"
    );
    let gate_error = apply(
        &mut map,
        ElementKind::Sectors,
        &[],
        &set_request(&[("light", json!("160"))]),
        &scope,
    )
    .expect_err("bad types refuse even with nothing selected");
    assert!(
        gate_error.to_string().contains("'light'"),
        "and the field again: {gate_error}"
    );
    assert_eq!(map, sample_map(), "nothing was ever written");
}

/// An `add` amount that leaves the integers refuses.
#[test]
fn an_add_that_leaves_the_integers_is_refused() {
    let stock = Stock::new();
    let scope = stock.doom();
    let mut map = sample_map();
    let error = apply(
        &mut map,
        ElementKind::Sectors,
        &[0],
        &add_request(&[("light", i64::MAX)]),
        &scope,
    )
    .expect_err("160 + i64::MAX has no integer answer");
    assert_eq!(error.code(), "invalid_argument", "overflow refuses");
    assert!(
        error.to_string().contains("leaves the range of integers"),
        "the refusal says so: {error}"
    );
    assert_eq!(map, sample_map(), "nothing was written");
}

/// `UDMF` carries no binary caps; the light rule stays universal.
#[test]
fn udmf_maps_ignore_the_binary_ranges() {
    let stock = Stock::new();
    let scope = stock.udmf();
    let mut tall = sample_map();
    tall.format = MapFormat::Udmf;
    let applied = apply(
        &mut tall,
        ElementKind::Sectors,
        &[0],
        &set_request(&[("ceiling", json!(40000_i64)), ("tag", json!(-5_i64))]),
        &scope,
    )
    .expect("UDMF holds what an i16 column cannot");
    assert_eq!(applied.changed, 1_usize, "both fields rode one pass");
    assert_eq!(
        tall.sectors[0].ceiling_height, 40000_i32,
        "the height landed"
    );
    assert_eq!(tall.sectors[0].tag, -5_i32, "and the negative tag");
    let mut bright = sample_map();
    bright.format = MapFormat::Udmf;
    let error = apply(
        &mut bright,
        ElementKind::Sectors,
        &[0],
        &set_request(&[("light", json!(300_i64))]),
        &scope,
    )
    .expect_err("the light level stays vanilla everywhere");
    assert!(
        error.to_string().contains("light must stay within 0..=255"),
        "the light rule is universal: {error}"
    );
    let mut standing = sample_map();
    standing.format = MapFormat::Udmf;
    assert_eq!(bright, standing, "the map stands");
}

/// The wire shape of [`FieldEdits`], as the next slice's tool sends it.
#[test]
fn field_edits_deserialize_from_the_tool_shape() {
    let parsed: FieldEdits = from_str(
        r#"{"set": {"light": 64, "floor_flat": "FLAT1"}, "add": {"tag": 2},
                "flags_set": ["mapped"], "flags_clear": []}"#,
    )
    .expect("the wire shape parses");
    assert_eq!(parsed.set.len(), 2_usize, "two set fields");
    assert_eq!(
        parsed.set.get("light"),
        Some(&json!(64_i64)),
        "numbers ride as values"
    );
    assert_eq!(parsed.add.get("tag"), Some(&2_i64), "amounts land in add");
    assert_eq!(
        parsed.flags_set,
        vec!["mapped".to_owned()],
        "names in flags_set"
    );
    assert!(!parsed.is_empty(), "a full request is not empty");
    let bare: FieldEdits = from_str("{}").expect("every part defaults");
    assert!(bare.is_empty(), "and so the empty object is empty");
}
