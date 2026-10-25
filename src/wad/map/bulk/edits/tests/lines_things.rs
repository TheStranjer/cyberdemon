//! The two flag-bearing blocks. `Doom`-family linedefs set their action
//! and tag, refuse any `add` amount and keep the `u16` columns; the
//! `Hexen` ones carry `special` with their five `u8` script arguments,
//! applied and validated as one row, and each family's field names are
//! gated out of the other's table. Both kinds' flag lists round-trip
//! through the family table — linedefs welded across the whole set,
//! things setting and clearing — and an unknown flag name answers with
//! the table it searched. Things set and add their type, facing and
//! position in one pass, keep the vanilla circle and the binary ranges,
//! and take an integer doomednum only.

use serde_json::json;

use crate::wad::map::bulk::edits::apply;
use crate::wad::map::flags::{FlagFamily, FlagKind, to_names};
use crate::wad::map::query::ElementKind;

use super::{Stock, add_request, flags_request, hexen_map, mixed, sample_map, set_request};

/// `Doom`-family linedefs set their action and tag, refuse any `add`
/// and keep the `u16` columns.
#[test]
fn linedef_numbers_set_and_never_add() {
    let stock = Stock::new();
    let scope = stock.doom();
    let mut map = sample_map();
    let applied = apply(
        &mut map,
        ElementKind::Linedefs,
        &[0, 1],
        &set_request(&[("special", json!(88_i64)), ("tag", json!(5_i64))]),
        &scope,
    )
    .expect("special and tag set");
    assert_eq!(applied.matched, 2_usize, "both lines matched");
    assert_eq!(applied.changed, 2_usize, "both lines moved");
    assert_eq!(map.linedefs[0].special, 88_i32, "the action landed");
    assert_eq!(map.linedefs[1].tag, 5_i32, "and the tag");
    let row = applied.preview.first().expect("one preview row");
    assert_eq!(
        row.before.get("special"),
        Some(&json!(10_i64)),
        "the old action previews"
    );
    assert_eq!(row.after.get("tag"), Some(&json!(5_i64)), "and the new tag");
    let error = apply(
        &mut map,
        ElementKind::Linedefs,
        &[0],
        &add_request(&[("tag", 1_i64)]),
        &scope,
    )
    .expect_err("linedef numbers are never added to");
    assert_eq!(error.code(), "invalid_argument", "linedef add code");
    assert!(
        error
            .to_string()
            .contains("linedef numbers are set, never added to"),
        "the refusal says so: {error}"
    );
    let over = apply(
        &mut map,
        ElementKind::Linedefs,
        &[0],
        &set_request(&[("tag", json!(0x1_0000_i64))]),
        &scope,
    )
    .expect_err("65536 leaves the u16 tag column");
    assert!(
        over.to_string()
            .contains("linedef 0 tag would become 65536; tag must stay within 0..=65535"),
        "the refusal names row, field and range: {over}"
    );
}

/// The `Hexen` family sets `special` and the script arguments; a bad
/// `arg4` holds the whole selection back, the legal `arg0` included.
#[test]
fn hexen_linedef_fields_apply_and_validate_as_one() {
    let stock = Stock::new();
    let scope = stock.hexen();
    let mut map = hexen_map();
    let applied = apply(
        &mut map,
        ElementKind::Linedefs,
        &[0, 1],
        &set_request(&[("arg0", json!(3_i64)), ("special", json!(80_i64))]),
        &scope,
    )
    .expect("args and special set on a Hexen map");
    assert_eq!(applied.changed, 2_usize, "both lines moved");
    assert_eq!(map.linedefs[0].args[0], 3_i32, "arg0 landed");
    assert_eq!(map.linedefs[0].special, 80_i32, "and the action");
    assert_eq!(
        map.linedefs[0].args[4], 250_i32,
        "the untouched argument stands"
    );
    let row = applied.preview.first().expect("one preview row");
    assert_eq!(
        row.before.get("arg0"),
        Some(&json!(7_i64)),
        "the argument previews its past"
    );
    assert_eq!(row.after.get("arg0"), Some(&json!(3_i64)), "and its future");
    let mut standing = hexen_map();
    let error = apply(
        &mut standing,
        ElementKind::Linedefs,
        &[0, 1],
        &set_request(&[("arg0", json!(9_i64)), ("arg4", json!(300_i64))]),
        &scope,
    )
    .expect_err("300 leaves the u8 argument column");
    assert_eq!(error.code(), "invalid_argument", "arg4 range code");
    assert!(
        error
            .to_string()
            .contains("linedef 0 arg4 would become 300; arg4 must stay within 0..=255"),
        "the refusal names the offending row: {error}"
    );
    assert_eq!(standing, hexen_map(), "the legal arg0 never landed either");
}

/// The other family's linedef names refuse, naming the map's family
/// and `convert_map_format`.
#[test]
fn linedef_fields_are_gated_to_their_family() {
    let stock = Stock::new();
    let mut map = sample_map();
    let doom_error = apply(
        &mut map,
        ElementKind::Linedefs,
        &[0],
        &set_request(&[("arg0", json!(7_i64))]),
        &stock.doom(),
    )
    .expect_err("args are Hexen-only");
    assert_eq!(doom_error.code(), "invalid_argument", "family gate code");
    let doom_message = doom_error.to_string();
    assert!(
        doom_message.contains("doom family")
            && doom_message.contains("special and tag")
            && doom_message.contains("convert_map_format"),
        "the refusal names the stored family, its fields and the fix: {doom_message}"
    );
    let mut standing = hexen_map();
    let hexen_error = apply(
        &mut standing,
        ElementKind::Linedefs,
        &[0],
        &set_request(&[("tag", json!(4_i64))]),
        &stock.hexen(),
    )
    .expect_err("Hexen-family lines take no tag");
    let hexen_message = hexen_error.to_string();
    assert!(
        hexen_message.contains("hexen family")
            && hexen_message.contains("arg0")
            && hexen_message.contains("convert_map_format"),
        "and the same the other way: {hexen_message}"
    );
    assert_eq!(map, sample_map(), "nothing was written");
    assert_eq!(standing, hexen_map(), "nor on the Hexen side");
}

/// Named linedef flags fold into the bits, set applied after clear,
/// and read back through the family's table.
#[test]
fn linedef_flags_round_trip_through_the_table() {
    let stock = Stock::new();
    let scope = stock.doom();
    let mut map = sample_map();
    let applied = apply(
        &mut map,
        ElementKind::Linedefs,
        &[0, 1],
        &flags_request(&["blockmonsters", "mapped"], &[]),
        &scope,
    )
    .expect("two linedef names set");
    assert_eq!(applied.changed, 2_usize, "both lines gained the bits");
    assert_eq!(map.linedefs[0].flags, 259_u32, "blocking and more");
    let names = to_names(map.linedefs[0].flags, FlagKind::Linedef, FlagFamily::Doom);
    assert!(
        names.contains(&"blocking")
            && names.contains(&"blockmonsters")
            && names.contains(&"mapped"),
        "the set names read back: {}",
        names.join(", ")
    );
    let row = applied.preview.first().expect("one preview row");
    assert_eq!(
        row.before.get("flags"),
        Some(&json!(1_i64)),
        "the past bits preview"
    );
    assert_eq!(
        row.after.get("flags"),
        Some(&json!(259_i64)),
        "and the future ones"
    );
    let mut both = sample_map();
    apply(
        &mut both,
        ElementKind::Linedefs,
        &[0],
        &flags_request(&["secret"], &["blocking", "secret"]),
        &scope,
    )
    .expect("one name on both ends");
    assert_eq!(
        both.linedefs[0].flags, 32_u32,
        "the clear mask went first and the set mask last"
    );
    let standing = apply(
        &mut both,
        ElementKind::Linedefs,
        &[0],
        &flags_request(&["secret"], &[]),
        &scope,
    )
    .expect("setting a held bit is legal");
    assert_eq!(standing.matched, 1_usize, "the line matched");
    assert_eq!(standing.changed, 0_usize, "and nothing moved");
}

/// Named thing flags fold the same way, through the thing table.
#[test]
fn thing_flags_round_trip_through_the_table() {
    let stock = Stock::new();
    let scope = stock.doom();
    let mut map = sample_map();
    let applied = apply(
        &mut map,
        ElementKind::Things,
        &[0, 1],
        &flags_request(&["ambush"], &["easy"]),
        &scope,
    )
    .expect("one name on each end");
    assert_eq!(applied.changed, 2_usize, "both things moved");
    for row in &map.things {
        let names = to_names(row.flags, FlagKind::Thing, FlagFamily::Doom);
        let listed = names.join(", ");
        assert!(
            names.contains(&"ambush")
                && !names.contains(&"easy")
                && names.contains(&"medium")
                && names.contains(&"hard"),
            "ambush set, easy cleared, the rest stands: {listed}"
        );
    }
}

/// A flag name the family's table does not carry is refused with the
/// table listed, before anything is planned.
#[test]
fn unknown_flag_names_list_the_family_table() {
    let stock = Stock::new();
    let mut map = sample_map();
    let error = apply(
        &mut map,
        ElementKind::Things,
        &[0],
        &flags_request(&["twosided"], &[]),
        &stock.doom(),
    )
    .expect_err("twosided is no thing flag");
    assert_eq!(error.code(), "invalid_argument", "unknown flag code");
    assert!(
        error.to_string().contains("unknown thing flag 'twosided'")
            && error.to_string().contains("easy, medium, hard, ambush"),
        "the refusal names the bad flag and lists the table: {error}"
    );
    let mut standing = hexen_map();
    let line_error = apply(
        &mut standing,
        ElementKind::Linedefs,
        &[0],
        &flags_request(&["passuse"], &[]),
        &stock.hexen(),
    )
    .expect_err("passuse is a Doom-only linedef name");
    assert!(
        line_error.to_string().contains("hexen family"),
        "the refusal reads the scope's family table: {line_error}"
    );
    assert_eq!(map, sample_map(), "nothing was written");
    assert_eq!(standing, hexen_map(), "and nothing on the Hexen side");
}

/// Things take an integer type, a `add`ed turn and a shift, all
/// previewed in table order.
#[test]
fn thing_numbers_set_and_add_in_one_pass() {
    let stock = Stock::new();
    let scope = stock.doom();
    let mut map = sample_map();
    let applied = apply(
        &mut map,
        ElementKind::Things,
        &[0],
        &mixed(
            &[("type", json!(3002_i64))],
            &[("angle", 90_i64), ("x", -16_i64)],
        ),
        &scope,
    )
    .expect("a number, a turn and a shift ride one pass");
    assert_eq!(applied.matched, 1_usize, "one thing matched");
    assert_eq!(applied.changed, 1_usize, "and moved");
    assert_eq!(map.things[0].kind, 3002_i32, "the doomednum landed");
    assert_eq!(map.things[0].angle, 180_i32, "the turn landed");
    assert_eq!(map.things[0].x, 48_i32, "and the shift");
    assert_eq!(map.things[1].kind, 1_i32, "the unselected thing stands");
    let row = applied.preview.first().expect("one preview row");
    assert_eq!(
        row.before,
        json!({"angle": 90_i64, "type": 3001_i64, "x": 64_i64}),
        "the preview lists every touched field"
    );
    assert_eq!(
        row.after,
        json!({"angle": 180_i64, "type": 3002_i64, "x": 48_i64}),
        "before and after"
    );
    let error = apply(
        &mut map,
        ElementKind::Things,
        &[0],
        &add_request(&[("type", 1_i64)]),
        &scope,
    )
    .expect_err("the type is set, never added to");
    assert!(
        error
            .to_string()
            .contains("not addable on things: addable fields are angle, x, y"),
        "the refusal names the addable table: {error}"
    );
}

/// A thing's facing must end inside the circle in every format,
/// and nothing normalises it.
#[test]
fn thing_angles_stay_within_the_circle() {
    let stock = Stock::new();
    let scope = stock.doom();
    let mut map = sample_map();
    let error = apply(
        &mut map,
        ElementKind::Things,
        &[0],
        &set_request(&[("angle", json!(360_i64))]),
        &scope,
    )
    .expect_err("360 is one past the circle");
    assert_eq!(error.code(), "invalid_argument", "angle code");
    assert!(
        error
            .to_string()
            .contains("thing 0 angle would become 360; angle must stay within 0..=359"),
        "the refusal says so instead of wrapping: {error}"
    );
    let negative = apply(
        &mut map,
        ElementKind::Things,
        &[0],
        &set_request(&[("angle", json!(-1_i64))]),
        &scope,
    )
    .expect_err("a backward degree is no facing");
    assert!(
        negative
            .to_string()
            .contains("angle would become -1; angle must stay within 0..=359"),
        "and the same below zero: {negative}"
    );
    let mut spun = sample_map();
    let past = apply(
        &mut spun,
        ElementKind::Things,
        &[0],
        &add_request(&[("angle", 270_i64)]),
        &stock.udmf(),
    )
    .expect_err("90 + 270 passes 359 even on UDMF");
    assert_eq!(past.code(), "invalid_argument", "added angle code");
    assert_eq!(spun, sample_map(), "nothing was written");
}

/// Thing coordinates and types keep their binary ranges; `UDMF`
/// stores wider values.
#[test]
fn thing_numbers_keep_the_binary_ranges() {
    let stock = Stock::new();
    let scope = stock.doom();
    let mut map = sample_map();
    let far = apply(
        &mut map,
        ElementKind::Things,
        &[1],
        &add_request(&[("x", 32832_i64)]),
        &scope,
    )
    .expect_err("-64 + 32832 leaves i16");
    assert!(
        far.to_string()
            .contains("thing 1 x would become 32768; x must stay within -32768..=32767"),
        "the refusal names the row and range: {far}"
    );
    let huge = apply(
        &mut map,
        ElementKind::Things,
        &[0],
        &set_request(&[("type", json!(0x1_0000_i64))]),
        &scope,
    )
    .expect_err("65536 leaves the u16 type column");
    assert!(
        huge.to_string()
            .contains("type would become 65536; type must stay within 0..=65535"),
        "and the type range: {huge}"
    );
    assert_eq!(map, sample_map(), "nothing was written");
    apply(
        &mut map,
        ElementKind::Things,
        &[0],
        &set_request(&[("x", json!(40000_i64))]),
        &stock.udmf(),
    )
    .expect("UDMF stores what an i16 column cannot");
    assert_eq!(map.things[0].x, 40000_i32, "the wide coordinate landed");
}

/// A string thing type is the tool layer's job: the engine takes the
/// integer doomednum only.
#[test]
fn thing_types_take_integers_only() {
    let stock = Stock::new();
    let scope = stock.doom();
    let mut map = sample_map();
    let error = apply(
        &mut map,
        ElementKind::Things,
        &[0],
        &set_request(&[("type", json!("demon"))]),
        &scope,
    )
    .expect_err("a name is no doomednum here");
    assert_eq!(error.code(), "invalid_argument", "name type code");
    assert!(
        error
            .to_string()
            .contains("thing field 'type' needs an integer value"),
        "the refusal points at the field: {error}"
    );
    assert_eq!(map, sample_map(), "nothing was written");
}
