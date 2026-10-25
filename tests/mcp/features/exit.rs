//! `add_exit`: the level-ending switch on a named wall, the walk-over exit
//! dressing every two-sided boundary of a sector with 52/124 (`Doom`) or
//! `playercross` 243/244 (`Hexen`), the destination tag kept on the `W1`
//! line, and the missing-`linedef`/missing-`sector`/bad-`kind` refusals.

use serde_json::{Value, json};

use crate::features::{
    Client, features_loaded, flag_names, formats_loaded, l_room_loaded, validate_clean,
    working_linedef,
};
use crate::support::{call, call_err};

/// Requires `add_exit` to reject `args` with `invalid_argument`
/// naming `needle`.
async fn refuse_exit(client: &Client, args: Value, needle: &str) {
    let note = args.to_string();
    let (code, message, _) = call_err(client, "add_exit", args).await;
    assert_eq!(code, "invalid_argument", "refusal code for {note}");
    assert!(
        message.contains(needle),
        "the refusal for {note} must mention {needle:?}: {message}"
    );
}

#[tokio::test]
async fn an_exit_switch_mounts_on_the_wall_it_names() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    let result = call(
        &client,
        "add_exit",
        json!({"kind": "switch", "linedef": 0_i32}),
    )
    .await;
    assert_eq!(result["kind"], json!("switch"), "the trigger echoes");
    assert_eq!(result["secret"], json!(false), "the normal exit");
    assert_eq!(
        result["lines"],
        json!([7_i32]),
        "the split chunk carries it"
    );
    assert_eq!(result["special"], json!(11_i32), "the S1 exit");
    assert_eq!(result["map"], json!("MAP01"), "the only map is default");
    assert_eq!(
        result["nodes_invalidated"],
        json!(true),
        "the wall split is new geometry"
    );
    let line = working_linedef(&client, "MAP01", 7_usize).await;
    assert_eq!(line["special"], json!(11_i32), "the chunk carries the exit");
    assert_eq!(line["tag"], json!(0_i32), "a switch exit acts on no tag");
    assert_eq!(
        line["front"]["middle"],
        json!("SW1EXIT"),
        "the theme's exit panel"
    );
    validate_clean(&client, "MAP01").await;
    let second = features_loaded(dir.path()).await;
    let secret = call(
        &second,
        "add_exit",
        json!({"kind": "switch", "linedef": 0_i32, "secret": true}),
    )
    .await;
    assert_eq!(secret["special"], json!(51_i32), "the S1 secret exit");
    assert_eq!(secret["secret"], json!(true), "echoed back");
    assert_eq!(secret["lines"], json!([7_i32]), "the same wall chunk shape");
    validate_clean(&second, "MAP01").await;
}

#[tokio::test]
async fn a_walk_over_exit_dresses_every_boundary_of_the_sector() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    let result = call(
        &client,
        "add_exit",
        json!({"kind": "walk_over", "sector": 0_i32}),
    )
    .await;
    assert_eq!(result["kind"], json!("walk_over"), "the trigger echoes");
    assert_eq!(
        result["lines"],
        json!([2_i32]),
        "the sector's only two-sided line, not its one-sided walls"
    );
    assert_eq!(result["special"], json!(52_i32), "the W1 exit");
    assert_eq!(result["secret"], json!(false));
    assert_eq!(
        result["nodes_invalidated"],
        json!(false),
        "dressing lines is not new geometry"
    );
    assert_eq!(result["copied_to_working"], json!(true));
    let step = working_linedef(&client, "MAP01", 2_usize).await;
    assert_eq!(step["special"], json!(52_i32), "the boundary carries it");
    assert_eq!(step["tag"], json!(0_i32), "a plain line's tag stays plain");
    let from_behind = call(
        &client,
        "add_exit",
        json!({"kind": "walk_over", "sector": 1_i32}),
    )
    .await;
    assert_eq!(
        from_behind["lines"],
        json!([2_i32]),
        "sector 1 shares the line, so it counts from either side"
    );
    validate_clean(&client, "MAP01").await;
    let secret = call(
        &client,
        "add_exit",
        json!({"kind": "walk_over", "sector": 0_i32, "secret": true}),
    )
    .await;
    assert_eq!(secret["special"], json!(124_i32), "the W1 secret exit");
    assert_eq!(secret["secret"], json!(true), "echoed back");
    let line = working_linedef(&client, "MAP01", 2_usize).await;
    assert_eq!(line["special"], json!(124_i32), "the line now ends the map");
    validate_clean(&client, "MAP01").await;
}

#[tokio::test]
async fn a_walk_over_exit_keeps_a_vanilla_destination_tag() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    call(
        &client,
        "edit_linedef",
        json!({"index": 2_i32, "tag": 3_i32}),
    )
    .await;
    let result = call(
        &client,
        "add_exit",
        json!({"kind": "walk_over", "sector": 0_i32}),
    )
    .await;
    assert_eq!(result["special"], json!(52_i32));
    let line = working_linedef(&client, "MAP01", 2_usize).await;
    assert_eq!(line["special"], json!(52_i32), "the W1 exit");
    assert_eq!(
        line["tag"],
        json!(3_i32),
        "vanilla 52 reads the line's tag as the destination map, so it survives"
    );
}

#[tokio::test]
async fn a_hexen_walk_over_exit_crosses_the_boundary() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = formats_loaded(dir.path()).await;
    let result = call(
        &client,
        "add_exit",
        json!({"map": "MAP04", "kind": "walk_over", "sector": 0_i32}),
    )
    .await;
    assert_eq!(result["special"], json!(243_i32), "the Portal exit line");
    assert_eq!(result["lines"], json!([2_i32]), "the shared boundary");
    assert_eq!(result["secret"], json!(false));
    let line = working_linedef(&client, "MAP04", 2_usize).await;
    assert_eq!(line["special"], json!(243_i32));
    let names = flag_names(&line);
    assert!(
        names.contains(&"playercross"),
        "the activation field reads as a cross: {names:?}"
    );
    assert!(!names.contains(&"playeruse"), "not a use line: {names:?}");
    let secret = call(
        &client,
        "add_exit",
        json!({"map": "MAP04", "kind": "walk_over", "sector": 1_i32, "secret": true}),
    )
    .await;
    assert_eq!(secret["special"], json!(244_i32), "the secret Portal");
    assert_eq!(secret["secret"], json!(true), "echoed back");
    let again = working_linedef(&client, "MAP04", 2_usize).await;
    assert_eq!(
        again["special"],
        json!(244_i32),
        "the same line, from room 1"
    );
    assert!(flag_names(&again).contains(&"playercross"), "still cross");
}

#[tokio::test]
async fn exit_arguments_are_validated() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    refuse_exit(&client, json!({"kind": "switch"}), "linedef").await;
    refuse_exit(&client, json!({"kind": "walk_over"}), "sector").await;
    refuse_exit(
        &client,
        json!({"kind": "stairs", "linedef": 0_i32}),
        "is not an exit kind",
    )
    .await;
    refuse_exit(
        &client,
        json!({"kind": "walk_over", "sector": 99_i32}),
        "sector 99 is out of range",
    )
    .await;
    refuse_exit(
        &client,
        json!({"kind": "switch", "linedef": 999_i32}),
        "linedef 999 is out of range",
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
        "no refused exit even copied the block into the working archive"
    );
    let lonely = l_room_loaded(dir.path()).await;
    refuse_exit(
        &lonely,
        json!({"kind": "walk_over", "sector": 0_i32}),
        "no two-sided boundary",
    )
    .await;
}
