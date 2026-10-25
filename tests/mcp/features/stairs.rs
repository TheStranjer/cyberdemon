//! `add_stairs`: four evenly rising step sectors climbing from the hall
//! floor to a raised room at 64 (`validate_map` stays clean, one `undo`
//! rolls the whole staircase back, risers in the theme's `step` texture),
//! and the headroom, `step_depth`, `step_height`, endpoint and width
//! refusals.

use serde_json::{Value, json};

use crate::features::{Client, features_loaded, validate_clean, working_linedefs, working_sectors};
use crate::support::{call, call_err};

/// Requires `add_stairs` to reject `args` with `invalid_argument`
/// naming `needle`, and the working archive to hold nothing.
async fn refuse_stairs(client: &Client, args: Value, needle: &str) {
    let note = args.to_string();
    let (code, message, _) = call_err(client, "add_stairs", args).await;
    assert_eq!(code, "invalid_argument", "refusal code for {note}");
    assert!(
        message.contains(needle),
        "the refusal for {note} must mention {needle:?}: {message}"
    );
    let (missing, _, _) = call_err(
        client,
        "assemble_map",
        json!({"map": "MAP01", "scope": "working", "include": ["sectors"]}),
    )
    .await;
    assert_eq!(
        missing, "not_found",
        "the refused staircase did not even copy the block into the working archive"
    );
}

/// Raises the fixture's rooms so a 64-unit climb has headroom:
/// both ceilings to 160 and room 1's floor to 64.
async fn raise_the_fixture(client: &Client) {
    call(
        client,
        "edit_sector",
        json!({"index": 0_i32, "ceiling_height": 160_i32}),
    )
    .await;
    call(
        client,
        "edit_sector",
        json!({"index": 1_i32, "floor_height": 64_i32, "ceiling_height": 160_i32}),
    )
    .await;
}

/// Checks the four created step rows (sectors 2 to 5): their even
/// floors, the probed 160 ceiling, and that each stays inert.
fn expect_step_sectors(rows: &[Value]) {
    let wanted = [
        (2_usize, 16_i32),
        (3_usize, 32_i32),
        (4_usize, 48_i32),
        (5_usize, 64_i32),
    ];
    for (index, floor) in wanted {
        let step = rows.get(index).expect("the step sector row exists");
        assert_eq!(step["floor_height"], json!(floor), "step sector {index}");
        assert_eq!(step["ceiling_height"], json!(160_i32), "the probed ceiling");
        assert_eq!(step["special"], json!(0_i32), "inert");
        assert_eq!(step["tag"], json!(0_i32), "untagged");
    }
}

/// Checks the four riser lines across the staircase (at `x` 64, 160,
/// 256 and 352): each two-sided, and its `lower` wears the theme's
/// `STEP1` on at least one face.
fn expect_stairs_risers(rows: &[Value]) {
    let risers = [64_i64, 160_i64, 256_i64, 352_i64]
        .into_iter()
        .map(|x| {
            (
                x,
                find_riser(rows, x).expect("a two-sided riser line at that x"),
            )
        })
        .collect::<Vec<(i64, &Value)>>();
    assert_eq!(risers.len(), 4_usize, "four risers across the path");
    for (x, riser) in risers {
        assert!(
            riser["front"]["lower"] == json!("STEP1") || riser["back"]["lower"] == json!("STEP1"),
            "the riser at x {x} wears the theme's step: {riser}"
        );
    }
}

/// The two-sided row in `rows` running north-south across the
/// staircase at `x`, from `y 96` to `y 160`: one step's riser.
fn find_riser(rows: &[Value], x: i64) -> Option<&Value> {
    let vertical = |first: &Value, second: &Value| {
        first["x"] == json!(x)
            && second["x"] == json!(x)
            && ((first["y"] == json!(96_i64) && second["y"] == json!(160_i64))
                || (first["y"] == json!(160_i64) && second["y"] == json!(96_i64)))
    };
    let wanted = |row: &Value| -> bool {
        !row["back"].is_null()
            && (vertical(&row["v1"], &row["v2"]) || vertical(&row["v2"], &row["v1"]))
    };
    rows.iter().find(|row| wanted(row))
}

#[tokio::test]
async fn stairs_climb_from_the_hall_to_a_raised_room() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    raise_the_fixture(&client).await;
    let result = call(
        &client,
        "add_stairs",
        json!({"from": {"x": 64_i64, "y": 128_i64}, "to": {"x": 448_i64, "y": 128_i64}}),
    )
    .await;
    assert_eq!(result["map"], json!("MAP01"), "the only map is default");
    assert_eq!(result["floor_from"], json!(0_i32), "the hall's floor");
    assert_eq!(result["floor_to"], json!(64_i32), "the raised room's floor");
    assert_eq!(result["step_height"], json!(16_i32), "the default climb");
    assert_eq!(result["width"], json!(64_i32), "the default width");
    assert_eq!(result["count"], json!(4_i32), "four 16-unit steps");
    assert_eq!(
        result["steps"],
        json!([
            {"sector": 2_i32, "floor_height": 16_i32, "ceiling_height": 160_i32},
            {"sector": 3_i32, "floor_height": 32_i32, "ceiling_height": 160_i32},
            {"sector": 4_i32, "floor_height": 48_i32, "ceiling_height": 160_i32},
            {"sector": 5_i32, "floor_height": 64_i32, "ceiling_height": 160_i32},
        ]),
        "the created sectors and their floors, in path order"
    );
    assert_eq!(
        result["nodes_invalidated"],
        json!(true),
        "the staircase is new geometry"
    );
    assert_eq!(
        result["copied_to_working"],
        json!(false),
        "the sector edits above had already copied the block in"
    );
    let sectors = working_sectors(&client, "MAP01").await;
    expect_step_sectors(&sectors);
    expect_stairs_risers(&working_linedefs(&client, "MAP01").await);
    validate_clean(&client, "MAP01").await;
}

#[tokio::test]
async fn a_staircase_undoes_as_one_step() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    call(
        &client,
        "add_stairs",
        json!({"from": {"x": 64_i64, "y": 128_i64}, "to": {"x": 448_i64, "y": 128_i64}}),
    )
    .await;
    let undone = call(&client, "undo", json!({})).await;
    assert_eq!(
        undone["undone"],
        json!(["add stairs"]),
        "the whole staircase rolls back together"
    );
    let (code, _, _) = call_err(
        &client,
        "assemble_map",
        json!({"map": "MAP01", "scope": "working", "include": ["sectors"]}),
    )
    .await;
    assert_eq!(code, "not_found", "and the copy went with it");
}

#[tokio::test]
async fn stairs_refuse_a_step_without_headroom() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    refuse_stairs(
        &client,
        json!({
            "from": {"x": 64_i64, "y": 128_i64},
            "to": {"x": 448_i64, "y": 128_i64},
            "floor_to": 64_i32,
        }),
        "not enough headroom",
    )
    .await;
}

#[tokio::test]
async fn stairs_step_depth_must_match_the_path() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    refuse_stairs(
        &client,
        json!({
            "from": {"x": 64_i64, "y": 128_i64},
            "to": {"x": 448_i64, "y": 128_i64},
            "step_depth": 24_i32,
        }),
        "wants a path 48 map units",
    )
    .await;
    refuse_stairs(
        &client,
        json!({
            "from": {"x": 64_i64, "y": 128_i64},
            "to": {"x": 448_i64, "y": 128_i64},
            "step_depth": 0_i32,
        }),
        "step_depth must be a positive",
    )
    .await;
}

#[tokio::test]
async fn stairs_step_height_stays_between_one_and_twenty_four() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    refuse_stairs(
        &client,
        json!({
            "from": {"x": 64_i64, "y": 128_i64},
            "to": {"x": 448_i64, "y": 128_i64},
            "step_height": 25_i32,
        }),
        "is out of range",
    )
    .await;
    refuse_stairs(
        &client,
        json!({
            "from": {"x": 64_i64, "y": 128_i64},
            "to": {"x": 448_i64, "y": 128_i64},
            "step_height": 0_i32,
        }),
        "step_height 0 is out of range",
    )
    .await;
}

#[tokio::test]
async fn stairs_endpoints_and_width_are_checked() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    refuse_stairs(
        &client,
        json!({"from": {"x": 5000_i64, "y": 5000_i64}, "to": {"x": 448_i64, "y": 128_i64}}),
        "from point 5000, 5000 lies in no sector",
    )
    .await;
    refuse_stairs(
        &client,
        json!({
            "from": {"x": 64_i64, "y": 128_i64},
            "to": {"x": 448_i64, "y": 128_i64},
            "width": 0_i32,
        }),
        "width must be a positive",
    )
    .await;
    refuse_stairs(
        &client,
        json!({"from": {"x": 64_i64, "y": 128_i64}, "to": {"x": 64_i64, "y": 128_i64}}),
        "zero-length",
    )
    .await;
}
