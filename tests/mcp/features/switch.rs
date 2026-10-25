//! `add_switch`: a panel split out of a plain wall wearing `SW1STRTN` and
//! answering with the target's tag, the fresh tag minted onto an untagged
//! sector, the whole-wall exit switch needing no split (`SW1EXIT`), the
//! `Hexen` `Door_Open`/`Portal` use lines, and the switch closing a `remote`
//! door's loop with exactly the tag `add_door` reported; the argument checks
//! that refuse a missing target, an unknown action, an out-of-range
//! linedef/sector and a non-positive width all leave nothing copied.

use serde_json::{Value, json};

use crate::features::{
    Client, features_loaded, flag_names, formats_loaded, validate_clean, working_linedef,
    working_linedefs, working_sector,
};
use crate::support::{call, call_err};

/// Requires `add_switch` to reject `args` with `invalid_argument`
/// naming `needle`.
async fn refuse_switch(client: &Client, args: Value, needle: &str) {
    let note = args.to_string();
    let (code, message, _) = call_err(client, "add_switch", args).await;
    assert_eq!(code, "invalid_argument", "refusal code for {note}");
    assert!(
        message.contains(needle),
        "the refusal for {note} must mention {needle:?}: {message}"
    );
}

/// Requires `add_switch` to reject `args` naming a missing element with
/// the shared `not_found` index refusal mentioning `needle`.
async fn refuse_missing_switch(client: &Client, args: Value, needle: &str) {
    let note = args.to_string();
    let (code, message, hint) = call_err(client, "add_switch", args).await;
    assert_eq!(code, "not_found", "index-refusal code for {note}");
    assert!(
        message.contains(needle) && message.contains("does not exist"),
        "the refusal for {note} must mention {needle:?}: {message}"
    );
    assert_eq!(
        hint, "describe_map or find_elements returns indices",
        "the shared index hint names the readers"
    );
}

/// The index of the one-sided working linedef running between the
/// map points `(x1, y1)` and `(x2, y2)` in either direction.
async fn solid_wall(client: &Client, map: &str, a: (i64, i64), b: (i64, i64)) -> usize {
    let rows = working_linedefs(client, map).await;
    let touches = |first: &Value, second: &Value| {
        first["x"] == json!(a.0)
            && first["y"] == json!(a.1)
            && second["x"] == json!(b.0)
            && second["y"] == json!(b.1)
    };
    let wanted = |row: &Value| -> bool {
        row["back"].is_null()
            && (touches(&row["v1"], &row["v2"]) || touches(&row["v2"], &row["v1"]))
    };
    rows.iter()
        .position(wanted)
        .unwrap_or_else(|| panic!("no one-sided {map} line from {a:?} to {b:?}"))
}

#[tokio::test]
async fn a_switch_splits_the_wall_and_answers_with_the_targets_tag() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    let result = call(
        &client,
        "add_switch",
        json!({
            "linedef": 0_i32,
            "action": "open_door",
            "target_sector": 1_i32,
            "width": 32_i32,
        }),
    )
    .await;
    assert_eq!(result["map"], json!("MAP01"), "the only map is default");
    assert_eq!(result["action"], json!("open_door"), "echoed canonically");
    assert_eq!(result["linedef"], json!(7_i32), "the chunk is its own line");
    assert_eq!(result["split"], json!(true), "the long wall was cut");
    assert_eq!(result["special"], json!(103_i32), "the S1 door action");
    assert_eq!(result["tag"], json!(1_i32), "sector 1's own tag");
    assert_eq!(result["texture"], json!("SW1STRTN"), "the theme's switch");
    assert_eq!(
        result["nodes_invalidated"],
        json!(true),
        "the split is new geometry"
    );
    assert_eq!(result["copied_to_working"], json!(true));
    let chunk = working_linedef(&client, "MAP01", 7_usize).await;
    let ends = [
        (
            chunk["v1"]["x"].as_i64().expect("x"),
            chunk["v1"]["y"].as_i64().expect("y"),
        ),
        (
            chunk["v2"]["x"].as_i64().expect("x"),
            chunk["v2"]["y"].as_i64().expect("y"),
        ),
    ];
    assert!(
        ends.contains(&(0_i64, 112_i64)) && ends.contains(&(0_i64, 144_i64)),
        "the 32-unit chunk is centred on the wall's midpoint: {ends:?}"
    );
    assert_eq!(
        chunk["special"],
        json!(103_i32),
        "the chunk carries the action"
    );
    assert_eq!(chunk["tag"], json!(1_i32), "and the tag");
    assert_eq!(
        chunk["front"]["middle"],
        json!("SW1STRTN"),
        "the panel wears the switch"
    );
    let standing = working_linedef(&client, "MAP01", 0_usize).await;
    assert_eq!(
        standing["special"],
        json!(0_i32),
        "the outer piece is inert"
    );
    assert_eq!(standing["front"]["middle"], json!("WALL1"), "and plain");
    validate_clean(&client, "MAP01").await;
}

#[tokio::test]
async fn an_untagged_target_sector_gets_a_fresh_tag() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    let result = call(
        &client,
        "add_switch",
        json!({"linedef": 0_i32, "action": "lower_floor", "target_sector": 0_i32}),
    )
    .await;
    assert_eq!(result["action"], json!("lower_floor"));
    assert_eq!(result["special"], json!(23_i32), "the S1 floor action");
    assert_eq!(result["tag"], json!(2_i32), "a fresh tag above room 1's 1");
    let line = working_linedef(&client, "MAP01", 7_usize).await;
    assert_eq!(line["tag"], json!(2_i32), "the line answers with it");
    let target = working_sector(&client, "MAP01", 0_usize).await;
    assert_eq!(target["tag"], json!(2_i32), "and so does the sector now");
}

#[tokio::test]
async fn an_exit_switch_on_the_whole_wall_needs_no_split_and_no_target() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    let result = call(
        &client,
        "add_switch",
        json!({"linedef": 0_i32, "action": "exit", "width": 256_i32}),
    )
    .await;
    assert_eq!(result["linedef"], json!(0_i32), "the line is the chunk");
    assert_eq!(result["split"], json!(false), "nothing was cut");
    assert_eq!(result["special"], json!(11_i32), "the S1 exit");
    assert_eq!(result["tag"], json!(0_i32), "an exit answers with no tag");
    assert_eq!(
        result["texture"],
        json!("SW1EXIT"),
        "the theme's exit switch"
    );
    assert_eq!(
        result["nodes_invalidated"],
        json!(false),
        "a dressed wall is not new geometry"
    );
    let line = working_linedef(&client, "MAP01", 0_usize).await;
    assert_eq!(line["front"]["middle"], json!("SW1EXIT"));
}

#[tokio::test]
async fn a_hexen_map_gets_the_script_switch() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = formats_loaded(dir.path()).await;
    let door = call(
        &client,
        "add_switch",
        json!({
            "map": "MAP04",
            "linedef": 0_i32,
            "action": "open_door",
            "target_sector": 1_i32,
            "repeatable": true,
        }),
    )
    .await;
    assert_eq!(door["special"], json!(11_i32), "Door_Open");
    assert_eq!(
        door["tag"],
        json!(1_i32),
        "the sector's own tag is reported"
    );
    assert_eq!(door["linedef"], json!(7_i32), "the chunk is its own line");
    let door_line = working_linedef(&client, "MAP04", 7_usize).await;
    assert_eq!(
        door_line["args"],
        json!([1_i32, 16_i32, 0_i32, 0_i32, 0_i32]),
        "tag in args[0], speed 16"
    );
    assert_eq!(
        door_line["tag"],
        json!(0_i32),
        "a Hexen line carries no tag"
    );
    let door_flags = flag_names(&door_line);
    assert!(
        door_flags.contains(&"playeruse"),
        "it is a use line: {door_flags:?}"
    );
    assert!(door_flags.contains(&"repeatspecial"), "and it repeats");
}

#[tokio::test]
async fn a_hexen_portal_switch_uses_once_and_carries_no_arguments() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = formats_loaded(dir.path()).await;
    let exit = call(
        &client,
        "add_switch",
        json!({"map": "MAP04", "linedef": 3_i32, "action": "secret_exit"}),
    )
    .await;
    assert_eq!(exit["special"], json!(244_i32), "the secret Portal");
    assert_eq!(exit["tag"], json!(0_i32), "an exit acts on no tag");
    assert_eq!(exit["split"], json!(true), "the south wall was cut");
    let index = usize::try_from(exit["linedef"].as_u64().expect("an index")).expect("fits");
    let exit_line = working_linedef(&client, "MAP04", index).await;
    assert_eq!(
        exit_line["args"],
        json!([0_i32, 0_i32, 0_i32, 0_i32, 0_i32]),
        "a portal takes no arguments"
    );
    assert_eq!(exit_line["special"], json!(244_i32), "the chunk carries it");
    let exit_flags = flag_names(&exit_line);
    assert!(
        exit_flags.contains(&"playeruse"),
        "it is a use line: {exit_flags:?}"
    );
    assert!(
        !exit_flags.contains(&"repeatspecial"),
        "a level change fires once: {exit_flags:?}"
    );
}

#[tokio::test]
async fn switch_arguments_are_validated() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    refuse_switch(
        &client,
        json!({"linedef": 0_i32, "action": "open_door"}),
        "target_sector",
    )
    .await;
    refuse_switch(
        &client,
        json!({"linedef": 0_i32, "action": "press"}),
        "open_door, exit, secret_exit, raise_floor, lower_floor",
    )
    .await;
    refuse_missing_switch(
        &client,
        json!({"linedef": 999_i32, "action": "exit"}),
        "linedef 999 does not exist",
    )
    .await;
    refuse_missing_switch(
        &client,
        json!({"linedef": 0_i32, "action": "raise_floor", "target_sector": 99_i32}),
        "sector 99 does not exist",
    )
    .await;
    refuse_switch(
        &client,
        json!({"linedef": 0_i32, "action": "open_door", "target_sector": 0_i32, "width": 0_i32}),
        "positive",
    )
    .await;
    let (code, _, _) = call_err(
        &client,
        "assemble_map",
        json!({"map": "MAP01", "scope": "working", "include": ["sectors"]}),
    )
    .await;
    assert_eq!(
        code, "not_found",
        "no refused switch even copied the block into the working archive"
    );
}

#[tokio::test]
async fn a_switch_opens_the_remote_door_that_carries_its_tag() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    let door = call(
        &client,
        "add_door",
        json!({"between": [0_i32, 1_i32], "style": "remote"}),
    )
    .await;
    let wall = solid_wall(&client, "MAP01", (0_i64, 0_i64), (0_i64, 256_i64)).await;
    let result = call(
        &client,
        "add_switch",
        json!({
            "linedef": wall,
            "action": "open_door",
            "target_sector": door["door_sector"],
        }),
    )
    .await;
    assert_eq!(
        result["tag"], door["tag"],
        "the switch answers with exactly the tag the remote door got"
    );
    assert_eq!(result["tag"], json!(2_i32), "the minted tag");
    assert_eq!(result["special"], json!(103_i32), "the S1 door action");
    let lines = working_linedefs(&client, "MAP01").await;
    let index = usize::try_from(result["linedef"].as_u64().expect("an index")).expect("fits");
    let line = lines.get(index).expect("the switch line");
    assert_eq!(line["tag"], json!(2_i32), "the line carries the tag");
    assert_eq!(line["special"], json!(103_i32));
    let door_sector = working_sector(&client, "MAP01", 2_usize).await;
    assert_eq!(door_sector["tag"], json!(2_i32), "the sector still answers");
    assert_eq!(
        door_sector["ceiling_height"], door_sector["floor_height"],
        "closed until a player uses the switch"
    );
    validate_clean(&client, "MAP01").await;
}
