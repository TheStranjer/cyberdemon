//! The `Hexen` family coming back and every refusal.
//!
//! The reverse direction of the cross-family trips: a `Hexen` map folding
//! into a `Doom` family layout, a `zdoom` `UDMF` map becoming binary `Doom`,
//! a `Hexen` map choosing a `Doom` family namespace, a special with no row
//! reported `untranslated`, `dry_run` and `undo` on the reverse step, and the
//! refusals: already-there, unknown map and malformed parameters. The last
//! test checks the fixture against the library model it was built from.

use serde_json::{Value, json};

use crate::convert::{
    DOOM_MEMBERS, NODES_HINT, TWO_ROOM_COUNTS, assert_no_op, convert, counts, first_linedef,
    loaded, lump_names, map_row, refusal, row_of, undo_labels, warns, working_maps,
};
use crate::fixtures::{iwad, pwad_conversions, pwad_map_formats, two_room_map};
use crate::support::{call, call_err};

#[tokio::test]
async fn a_hexen_map_converts_back_to_a_doom_family_layout() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = loaded(&pwad_conversions(dir.path())).await;
    let out = convert(&client, json!({ "map": "MAP06", "to": "doom" })).await;
    assert_eq!(out["from"], "hexen", "read as binary Hexen");
    assert_eq!(out["to"], "doom", "written as binary Doom");
    assert_eq!(
        out["translated_specials"], 0_i32,
        "the id fold needs no table"
    );
    assert_eq!(
        out["untranslated"],
        json!([]),
        "nothing failed to translate"
    );
    assert_eq!(
        out["thing_flags_changed"], 1_i32,
        "the lone thing folds back"
    );
    assert_eq!(
        out["sector_types_changed"], 0_i32,
        "its one plain sector stands"
    );
    assert!(
        warns(&out, "BEHAVIOR"),
        "the generated BEHAVIOR cannot ride into a Doom block: {out}"
    );
    let line = first_linedef(&client, "MAP06", "working").await;
    assert_eq!(line["special"], 0_i32, "Line_SetIdentification folded away");
    assert_eq!(line["tag"], 7_i32, "its id came back as the Doom tag");
    assert_eq!(
        lump_names(&client, "MAP06", "working").await,
        DOOM_MEMBERS.to_vec(),
        "the block is a vanilla Doom run again"
    );
    let row = map_row(&client, "MAP06", "working").await;
    assert_eq!(row["format"], "doom", "and it reads back as Doom");
}

#[tokio::test]
async fn a_zdoom_map_converts_back_to_binary_doom() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = loaded(&pwad_map_formats(dir.path())).await;
    let out = convert(&client, json!({ "map": "MAP03", "to": "doom" })).await;
    assert_eq!(out["from"], "udmf", "read as zdoom UDMF");
    assert_eq!(out["to"], "doom", "written as binary Doom");
    assert_eq!(
        out["namespace"],
        Value::Null,
        "a binary target declares none"
    );
    assert_eq!(
        out["translated_specials"], 0_i32,
        "plain lines need no table"
    );
    assert_eq!(out["untranslated"], json!([]), "and report nothing");
    assert_eq!(
        out["thing_flags_changed"], 2_i32,
        "the things fold to Doom bits"
    );
    assert_eq!(out["nodes_removed"], true, "the ZNODES and GL run went");
    let row = map_row(&client, "MAP03", "working").await;
    assert_eq!(row["format"], "doom", "the block reads as Doom");
    assert_eq!(row["has_gl_nodes"], false, "and lost the GL run");
    assert_eq!(
        counts(&client, "MAP03", "working").await,
        row_of(TWO_ROOM_COUNTS),
        "with the same elements"
    );
}

#[tokio::test]
async fn a_hexen_map_converts_to_a_doom_family_namespace() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = loaded(&pwad_map_formats(dir.path())).await;
    let wanted = json!({ "map": "MAP04", "to": "udmf", "namespace": "heretic" });
    let out = convert(&client, wanted).await;
    assert_eq!(out["from"], "hexen", "read as binary Hexen");
    assert_eq!(out["to"], "udmf", "written as UDMF");
    assert_eq!(
        out["namespace"], "heretic",
        "a best-effort Doom family spelling"
    );
    assert_eq!(
        out["translated_specials"], 0_i32,
        "plain lines need no table"
    );
    assert_eq!(
        out["thing_flags_changed"], 2_i32,
        "the things fold to Doom bits"
    );
    let names = lump_names(&client, "MAP04", "working").await;
    assert!(
        names.contains(&"BEHAVIOR".to_owned()),
        "a UDMF target keeps the script lump: {names:?}"
    );
    let row = map_row(&client, "MAP04", "working").await;
    assert_eq!(row["format"], "udmf", "the block reads as UDMF");
    assert_eq!(
        row["udmf_namespace"], "heretic",
        "and declares the named namespace"
    );
    assert_eq!(
        counts(&client, "MAP04", "working").await,
        row_of(TWO_ROOM_COUNTS),
        "with the same elements"
    );
}

#[tokio::test]
async fn a_zdoom_special_with_no_row_comes_back_untranslated() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = loaded(&pwad_map_formats(dir.path())).await;
    call(
        &client,
        "edit_linedef",
        json!({ "map": "MAP03", "index": 0_i32, "special": 999_i32 }),
    )
    .await;
    let out = convert(&client, json!({ "map": "MAP03", "to": "doom" })).await;
    assert_eq!(
        out["untranslated"],
        json!([{"linedef": 0_i32, "special": 999_i32}]),
        "the line and its lost special are named"
    );
    assert_eq!(out["translated_specials"], 0_i32, "nothing made the trip");
    assert!(
        warns(&out, "no Doom translation"),
        "the grouped warning follows: {out}"
    );
    let line = first_linedef(&client, "MAP03", "working").await;
    assert_eq!(line["special"], 0_i32, "and the line ends special 0");
}

#[tokio::test]
async fn a_reverse_dry_run_touches_neither_the_archive_nor_the_history() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = loaded(&pwad_map_formats(dir.path())).await;
    let out = convert(
        &client,
        json!({ "map": "MAP04", "to": "doom", "dry_run": true }),
    )
    .await;
    assert_eq!(out["dry_run"], true, "the call reports only");
    assert_eq!(out["from"], "hexen", "read as binary Hexen");
    assert_eq!(out["to"], "doom", "would write binary Doom");
    assert_eq!(
        out["thing_flags_changed"], 2_i32,
        "and still counts the translation"
    );
    assert!(
        warns(&out, "BEHAVIOR") && warns(&out, NODES_HINT),
        "the losses it reports: {out}"
    );
    assert_eq!(
        undo_labels(&client).await,
        Vec::<String>::new(),
        "a dry run adds no undo entry"
    );
    assert_eq!(working_maps(&client).await, 0, "nothing was copied in");
    let row = map_row(&client, "MAP04", "merged").await;
    assert_eq!(row["format"], "hexen", "the map is still binary Hexen");
}

#[tokio::test]
async fn one_undo_takes_a_whole_reverse_conversion_back() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = loaded(&pwad_map_formats(dir.path())).await;
    convert(&client, json!({ "map": "MAP04", "to": "doom" })).await;
    assert_eq!(
        undo_labels(&client).await,
        vec!["convert MAP04 to doom"],
        "one step for the copy, the translation and the rewrite"
    );
    let undone = call(&client, "undo", json!({})).await;
    assert_eq!(
        undone["undone"][0], "convert MAP04 to doom",
        "the label comes back"
    );
    assert_eq!(undone["remaining_undo"], 0_i32, "the stack is empty again");
    assert_eq!(
        working_maps(&client).await,
        0,
        "the copied block rolled back too"
    );
    let row = map_row(&client, "MAP04", "merged").await;
    assert_eq!(
        row["format"], "hexen",
        "the merged view shows the layer's Hexen map again"
    );
}

#[tokio::test]
async fn a_no_op_request_names_the_layout_in_hand() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = loaded(&pwad_map_formats(dir.path())).await;
    let wanted = json!({ "to": "udmf", "namespace": "ZDoom" });
    let same_namespace = refusal(&client, "MAP03", wanted).await;
    assert_eq!(
        same_namespace.0, "invalid_argument",
        "the same namespace, any case"
    );
    assert_no_op(&same_namespace.1);
    let defaulted = refusal(&client, "MAP03", json!({ "to": "udmf" })).await;
    assert_eq!(
        defaulted.0, "invalid_argument",
        "the family default names zdoom"
    );
    assert_no_op(&defaulted.1);
    assert!(
        defaulted.1.contains("zdoom"),
        "a no-op UDMF label carries the namespace: {}",
        defaulted.1
    );
    let same_binary = refusal(&client, "MAP04", json!({ "to": "hexen" })).await;
    assert_eq!(same_binary.0, "invalid_argument", "the layout in hand");
    assert_no_op(&same_binary.1);
    assert_eq!(
        undo_labels(&client).await,
        Vec::<String>::new(),
        "a refusal enters no history entry"
    );
}

#[tokio::test]
async fn bad_parameters_and_unknown_maps_are_refused() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = loaded(&iwad(dir.path())).await;
    let unknown_map = refusal(&client, "MAP09", json!({ "to": "udmf" })).await;
    assert_eq!(unknown_map.0, "not_found", "no MAP09 here");
    let bad_format = refusal(&client, "MAP01", json!({ "to": "zmap" })).await;
    assert_eq!(bad_format.0, "invalid_argument", "zmap is no format");
    assert!(
        bad_format.1.contains("\"udmf\""),
        "the choices are listed: {}",
        bad_format.1
    );
    let namespace_args = json!({ "to": "hexen", "namespace": "zdoom" });
    let namespace_on_binary = refusal(&client, "MAP01", namespace_args).await;
    assert_eq!(
        namespace_on_binary.0, "invalid_argument",
        "a binary target declares no namespace"
    );
    let not_a_namespace = json!({ "to": "udmf", "namespace": "not a name" });
    let bad_namespace = refusal(&client, "MAP01", not_a_namespace).await;
    assert_eq!(
        bad_namespace.0, "invalid_argument",
        "spaces are no namespace"
    );
    let oversized_name = json!({ "map": "too long a name", "to": "udmf" });
    let bad_name = call_err(&client, "convert_map_format", oversized_name).await;
    assert_eq!(bad_name.0, "invalid_argument", "twelve characters is over");
    assert_eq!(
        undo_labels(&client).await,
        Vec::<String>::new(),
        "no refusal entered the history"
    );
}

#[tokio::test]
async fn the_fixture_counts_match_the_library_model() {
    let map = two_room_map();
    assert_eq!(map.things.len(), 2_usize, "the fixture has two things");
    assert_eq!(map.vertices.len(), 6_usize, "and six vertices");
    assert_eq!(map.linedefs.len(), 7_usize, "seven linedefs");
    assert_eq!(map.sidedefs.len(), 8_usize, "eight sidedefs");
    assert_eq!(map.sectors.len(), 2_usize, "two sectors");
}
