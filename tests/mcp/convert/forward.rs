//! The forward trips out of a binary `Doom` block.
//!
//! The cross-family table's forward direction: a `Doom` door riding the
//! `zdoom` `UDMF` table and back, a tagged door becoming a binary `Hexen`
//! generalized action, an action too wide for `Hexen` bytes clearing with an
//! `untranslated` entry and its own warning while a `UDMF` target keeps it,
//! a row's activation arriving as `flag_names`, `dry_run` reporting the
//! numbers the real step writes and touching neither archive nor history,
//! and one `undo` taking a whole conversion, translated or plain, back.

use serde_json::{Value, json};

use crate::convert::{
    DOOM_MEMBERS, NODES_HINT, TWO_ROOM_COUNTS, convert, counts, cross_family, flag_names_contain,
    linedef_row, loaded, lump_names, map_row, row_of, undo_labels, warns, working_maps,
};
use crate::fixtures::iwad;
use crate::support::call;

#[tokio::test]
async fn a_doom_door_travels_through_the_zdoom_table_and_back() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = loaded(&iwad(dir.path())).await;
    call(
        &client,
        "edit_linedef",
        json!({ "map": "MAP01", "index": 2_i32, "special": 1_i32, "tag": 0_i32 }),
    )
    .await;
    let out = convert(
        &client,
        json!({ "map": "MAP01", "to": "udmf", "namespace": "zdoom" }),
    )
    .await;
    assert_eq!(out["from"], "doom", "a Doom family source");
    assert_eq!(out["namespace"], "zdoom", "into a Hexen family namespace");
    assert_eq!(
        out["translated_specials"], 1_i32,
        "the door line rode the table"
    );
    assert_eq!(out["untranslated"], json!([]), "row 1 translates");
    assert_eq!(
        out["thing_flags_changed"], 2_i32,
        "both things moved families"
    );
    assert_eq!(out["sector_types_changed"], 0_i32, "plain sectors stand");
    let door = linedef_row(&client, "MAP01", "working", 2_i32).await;
    assert_eq!(door["special"], 12_i32, "Door_Raise");
    assert!(
        flag_names_contain(&door, &["playeruse", "repeatspecial", "monsteractivate"]),
        "row 1's USE|MONST|REP activation: {door}"
    );
    assert_eq!(
        door["args"],
        json!([0_i32, 16_i32, 150_i32, 0_i32, 0_i32]),
        "the row's template"
    );
    let back = convert(&client, json!({ "map": "MAP01", "to": "doom" })).await;
    assert_eq!(back["from"], "udmf", "read as zdoom UDMF");
    assert_eq!(back["to"], "doom", "written as binary Doom");
    assert_eq!(
        back["translated_specials"], 1_i32,
        "the same line came back through the table"
    );
    assert_eq!(back["untranslated"], json!([]), "and matched a row");
    let names = lump_names(&client, "MAP01", "merged").await;
    assert_eq!(names, DOOM_MEMBERS.to_vec(), "the block is binary Doom");
    let home = linedef_row(&client, "MAP01", "merged", 2_i32).await;
    assert_eq!(home["special"], 1_i32, "special 1 again");
    assert_eq!(home["tag"], 0_i32, "with its tag restored in the tag field");
    assert_eq!(
        counts(&client, "MAP01", "merged").await,
        row_of(TWO_ROOM_COUNTS),
        "and the same elements throughout"
    );
}

#[tokio::test]
async fn a_tagged_door_line_becomes_a_binary_hexen_generalized_action() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = loaded(&iwad(dir.path())).await;
    call(
        &client,
        "edit_linedef",
        json!({ "map": "MAP01", "index": 2_i32, "special": 26_i32, "tag": 3_i32 }),
    )
    .await;
    let out = convert(&client, json!({ "map": "MAP01", "to": "hexen" })).await;
    assert_eq!(out["from"], "doom", "a Doom family source");
    assert_eq!(out["to"], "hexen", "written as binary Hexen");
    assert_eq!(
        out["namespace"],
        Value::Null,
        "a binary layout declares none"
    );
    assert_eq!(out["translated_specials"], 1_i32, "row 26 rode the table");
    assert_eq!(
        out["untranslated"],
        json!([]),
        "and every value fits a byte"
    );
    assert_eq!(
        out["thing_flags_changed"], 2_i32,
        "both things moved families"
    );
    assert_eq!(out["sector_types_changed"], 0_i32, "plain sectors stand");
    let door = linedef_row(&client, "MAP01", "working", 2_i32).await;
    assert_eq!(door["special"], 13_i32, "Door_LockedRaise");
    assert_eq!(
        door["args"],
        json!([0_i32, 16_i32, 150_i32, 130_i32, 3_i32]),
        "the row's template with the tag in its slot"
    );
    assert_eq!(door["tag"], 0_i32, "binary Hexen models no line tag");
    assert!(
        flag_names_contain(&door, &["playeruse", "repeatspecial"]),
        "row 26's USE|REP activation: {door}"
    );
    let row = map_row(&client, "MAP01", "merged").await;
    assert_eq!(row["format"], "hexen", "the block reads as binary Hexen");
    let names = lump_names(&client, "MAP01", "working").await;
    assert!(
        names.contains(&"BEHAVIOR".to_owned()),
        "the Hexen writer synthesized BEHAVIOR: {names:?}"
    );
}

#[tokio::test]
async fn an_action_too_wide_for_hexen_bytes_clears_but_survives_udmf() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let doomed = loaded(&iwad(dir.path())).await;
    call(
        &doomed,
        "edit_linedef",
        json!({ "map": "MAP01", "index": 2_i32, "special": 100_i32 }),
    )
    .await;
    let out = convert(&doomed, json!({ "map": "MAP01", "to": "hexen" })).await;
    assert_eq!(
        out["translated_specials"], 0_i32,
        "row 100's 273 is no byte"
    );
    assert_eq!(
        out["untranslated"],
        json!([{"linedef": 2_i32, "special": 100_i32}]),
        "the line and its lost special are named"
    );
    assert!(
        warns(&out, "linedef 2 special 100 was set to 0")
            && warns(&out, "Stairs_BuildUpDoomCrush")
            && warns(&out, "convert to a UDMF Hexen-family map"),
        "the line is told its own action is the one no byte can hold: {out}"
    );
    let cleared = linedef_row(&doomed, "MAP01", "working", 2_i32).await;
    assert_eq!(cleared["special"], 0_i32, "the line ends special 0");
    let second = tempfile::tempdir().expect("a second scratch directory");
    let zdoom = loaded(&iwad(second.path())).await;
    call(
        &zdoom,
        "edit_linedef",
        json!({ "map": "MAP01", "index": 2_i32, "special": 100_i32 }),
    )
    .await;
    let there = convert(
        &zdoom,
        json!({ "map": "MAP01", "to": "udmf", "namespace": "zdoom" }),
    )
    .await;
    assert_eq!(there["translated_specials"], 1_i32, "a UDMF target fits");
    assert_eq!(there["untranslated"], json!([]), "and nothing is lost");
    let kept = linedef_row(&zdoom, "MAP01", "working", 2_i32).await;
    assert_eq!(kept["special"], 273_i32, "the action stands intact");
    assert_eq!(
        kept["args"],
        json!([0_i32, 32_i32, 16_i32, 0_i32, 0_i32]),
        "the row's template at tag 0"
    );
}

#[tokio::test]
async fn a_dry_run_touches_neither_the_archive_nor_the_history() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = loaded(&iwad(dir.path())).await;
    let out = convert(
        &client,
        json!({ "map": "MAP01", "to": "udmf", "namespace": "strife", "dry_run": true }),
    )
    .await;
    assert_eq!(out["dry_run"], true, "the call reports only");
    assert_eq!(
        out["namespace"], "strife",
        "and still resolves the namespace"
    );
    assert_eq!(
        out["nodes_removed"], true,
        "it would have removed the nodes"
    );
    assert_eq!(
        undo_labels(&client).await,
        Vec::<String>::new(),
        "a dry run adds no undo entry"
    );
    assert_eq!(working_maps(&client).await, 0, "nothing was copied in");
    let row = map_row(&client, "MAP01", "merged").await;
    assert_eq!(row["format"], "doom", "the map is still binary Doom");
    let info = call(&client, "get_session_info", json!({})).await;
    assert_eq!(
        info["working"]["dirty"], false,
        "and the session is still clean"
    );
}

#[tokio::test]
async fn a_forward_dry_run_reports_the_numbers_and_changes_nothing() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = loaded(&iwad(dir.path())).await;
    let wanted = json!({ "map": "MAP01", "to": "udmf", "namespace": "zdoom" });
    let mut dry_request = wanted.clone();
    dry_request["dry_run"] = json!(true);
    let dry = convert(&client, dry_request).await;
    assert_eq!(dry["dry_run"], true, "the call reports only");
    assert_eq!(
        dry["namespace"], "zdoom",
        "and still resolves the namespace"
    );
    assert_eq!(
        cross_family(&dry),
        json!({
            "translated_specials": 0_i32,
            "untranslated": [],
            "thing_flags_changed": 2_i32,
            "sector_types_changed": 0_i32,
        }),
        "the forward stages ran on the copy"
    );
    assert!(warns(&dry, NODES_HINT), "the hint rides along: {dry}");
    assert_eq!(
        undo_labels(&client).await,
        Vec::<String>::new(),
        "a dry run adds no undo entry"
    );
    assert_eq!(working_maps(&client).await, 0, "nothing was copied in");
    let row = map_row(&client, "MAP01", "merged").await;
    assert_eq!(row["format"], "doom", "the map is still binary Doom");
    let real = convert(&client, wanted).await;
    assert_eq!(cross_family(&real), cross_family(&dry), "the same numbers");
    assert_eq!(real["warnings"], dry["warnings"], "and the same notices");
}

#[tokio::test]
async fn one_undo_takes_a_whole_conversion_step_back() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = loaded(&iwad(dir.path())).await;
    convert(&client, json!({ "map": "MAP01", "to": "udmf" })).await;
    assert_eq!(
        undo_labels(&client).await,
        vec!["convert MAP01 to udmf"],
        "one step for the copy and the rewrite"
    );
    let undone = call(&client, "undo", json!({})).await;
    assert_eq!(
        undone["undone"][0], "convert MAP01 to udmf",
        "the label comes back"
    );
    assert_eq!(undone["remaining_undo"], 0_i32, "the stack is empty again");
    assert_eq!(undone["dirty"], false, "and the session is clean again");
    assert_eq!(
        working_maps(&client).await,
        0,
        "the copied block rolled back too"
    );
    let row = map_row(&client, "MAP01", "merged").await;
    assert_eq!(
        row["format"], "doom",
        "the merged view shows the layer's map again"
    );
}

#[tokio::test]
async fn one_undo_takes_a_whole_forward_conversion_back() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = loaded(&iwad(dir.path())).await;
    call(
        &client,
        "edit_linedef",
        json!({ "map": "MAP01", "index": 2_i32, "special": 1_i32 }),
    )
    .await;
    convert(
        &client,
        json!({ "map": "MAP01", "to": "udmf", "namespace": "zdoom" }),
    )
    .await;
    assert_eq!(
        undo_labels(&client).await,
        vec!["edit 1 linedefs", "convert MAP01 to udmf"],
        "one step for the copy, the translation and the rewrite"
    );
    let undone = call(&client, "undo", json!({})).await;
    assert_eq!(
        undone["undone"][0], "convert MAP01 to udmf",
        "the label comes back"
    );
    assert_eq!(undone["remaining_undo"], 1_i32, "the edit step stands");
    assert_eq!(undone["dirty"], true, "the edited block is still working");
    assert_eq!(working_maps(&client).await, 1, "and its block is copied in");
    let row = map_row(&client, "MAP01", "merged").await;
    assert_eq!(
        row["format"], "doom",
        "the source block came back under the translation"
    );
    let door = linedef_row(&client, "MAP01", "working", 2_i32).await;
    assert_eq!(door["special"], 1_i32, "the pre-conversion edit survived");
    assert!(
        flag_names_contain(&door, &["twosided"])
            && !flag_names_contain(&door, &["playeruse", "monsteractivate"]),
        "the row's Hexen trigger bits went with the step: {door}"
    );
}
