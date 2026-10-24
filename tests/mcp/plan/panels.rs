//! The wall a panel mounts on: a `C`-shaped cell with two equal west walls
//! answering its exit `at` point with the wall nearest it - through the `units`
//! transform as well - every feature row reporting its lines' map-unit
//! endpoints beside their indices, and a point whose wall cannot hold the
//! panel refusing the build by naming the room, the line and the knob.

use serde_json::{Value, json};

use crate::plan::{Client, features_loaded, first_line, line_row, working_lines};
use crate::support::{call, call_err};

/// The C-shaped single-sector room: a 128 x 128 cell with a bite out
/// of its west edge between `y = 48` and `y = 80`, leaving two equal
/// 48-unit west walls, the lower running `y = 0..48` and the upper
/// `y = 80..128`. Its exit stands on the wall nearest `at`, or on
/// `at`-less default the longest-first rule.
fn c_plan(map: &str, at: Option<[i64; 2]>) -> Value {
    let mut row = json!({"kind": "exit", "room": "cell", "wall": "west"});
    if let Some(point) = at {
        row["at"] = json!(point);
    }
    json!({
        "map": map,
        "rooms": [{
            "id": "cell",
            "points": [[0, 0], [128, 0], [128, 128], [0, 128],
                       [0, 80], [48, 80], [48, 48], [0, 48]]
        }],
        "connections": [],
        "features": [row],
        "player_start": {"room": "cell"}
    })
}

/// The one `exit` row of a build result.
fn exit_row(result: &Value) -> &Value {
    result["features"]
        .as_array()
        .expect("a feature list")
        .iter()
        .find(|row| row["kind"] == json!("exit"))
        .expect("an exit row")
}

/// The documented rule at work over the wire: a cell with two equal
/// west walls puts its exit panel on the one `at` stands nearest, and
/// the row reports the chosen line's endpoints beside its index —
/// endpoints the written block agrees with.
#[tokio::test]
async fn an_exit_at_a_point_lands_on_the_wall_the_drawing_meant() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    let lower = call(
        &client,
        "build_map_from_plan",
        c_plan("MAP05", Some([0, 24])),
    )
    .await;
    expect_exit_panel(&client, "MAP05", exit_row(&lower), (8_i64, 40_i64)).await;
    let upper = call(
        &client,
        "build_map_from_plan",
        c_plan("MAP06", Some([0, 104])),
    )
    .await;
    expect_exit_panel(&client, "MAP06", exit_row(&upper), (88_i64, 120_i64)).await;
    assert_ne!(
        exit_row(&lower)["line_ends"],
        exit_row(&upper)["line_ends"],
        "the two points answer with two different walls"
    );
}

/// Assert `row` reports one exit panel on `x = 0` running between
/// `run`'s two `y`s, and that the written block's line at the
/// reported index carries exactly those vertices and the level end.
async fn expect_exit_panel(client: &Client, map: &str, row: &Value, run: (i64, i64)) {
    let (from_y, to_y) = run;
    assert_eq!(row["room"], json!("cell"), "the room it stands in");
    assert_eq!(row["tag"], json!(null), "an exit carries no tag");
    let index = first_line(row);
    assert_eq!(
        row["line_ends"],
        json!([{
            "index": index,
            "x1": 0_i64,
            "y1": from_y,
            "x2": 0_i64,
            "y2": to_y,
        }]),
        "the reported line with its map-unit endpoints: {row}"
    );
    let lines = working_lines(client, map).await;
    let panel = line_row(&lines, index);
    assert_eq!(panel["special"], json!(11_i64), "the S1 level end");
    for (slot, (x, y)) in [("v1", (0_i64, from_y)), ("v2", (0_i64, to_y))] {
        assert_eq!(panel[slot]["x"], json!(x), "the written {slot} x");
        assert_eq!(panel[slot]["y"], json!(y), "the written {slot} y");
    }
}

/// A row point too near the end of its wall for the 32-unit panel
/// refuses the build: the message names the room and the line, the
/// hint names the knob, and nothing is written.
#[tokio::test]
async fn an_exit_point_the_wall_cannot_hold_refuses_and_names_the_knob() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    let (code, message, hint) = call_err(
        &client,
        "build_map_from_plan",
        c_plan("MAP05", Some([0, 4])),
    )
    .await;
    assert_eq!(code, "invalid_argument", "the panel has no room there");
    assert!(
        message.contains("room 'cell'") && message.contains("does not fit linedef"),
        "the room and the core's words both read: {message}"
    );
    assert!(
        message.starts_with("rooms[0]"),
        "the refusal carries its plan path: {message}"
    );
    assert!(
        hint.contains("move the row's at nearer the middle of the wall"),
        "the hint names the knob: {hint}"
    );
    let maps = call(&client, "list_maps", json!({"scope": "working"})).await;
    assert_eq!(maps["maps"], json!([]), "the refusal wrote nothing");
}

/// The C cell drawn in image-space pixels: `units` scales it by four
/// and mirrors `y`, so the exit row's `at` travels the plan transform
/// before it picks the wall — and the endpoints it reports are the
/// mirrored map units the block holds.
#[tokio::test]
async fn an_exit_point_travels_the_units_transform_before_it_picks_the_wall() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    expect_pixel_panel(&client, "MAP05", 24_i64, (-112_i64, -80_i64)).await;
    expect_pixel_panel(&client, "MAP06", 104_i64, (-432_i64, -400_i64)).await;
}

/// Assert the cell drawn at pixel scale with a mirrored `y` reports its
/// exit panel running between `want`'s `y`s on `x = 0`, asked for at
/// plan pixel `[0, pixel]`, and that the written block carries the
/// level end there.
async fn expect_pixel_panel(client: &Client, map: &str, pixel: i64, want: (i64, i64)) {
    let mut plan = c_plan(map, Some([0, pixel]));
    plan["units"] = json!({"scale_num": 4_i64, "flip_y": true});
    let result = call(client, "build_map_from_plan", plan).await;
    let ends = exit_row(&result)["line_ends"]
        .as_array()
        .expect("one reported line")
        .first()
        .expect("one pair")
        .clone();
    assert_eq!(ends["x1"], json!(0_i64), "the west wall: {ends}");
    assert_eq!(ends["x2"], json!(0_i64), "and a vertical run");
    let first = ends["y1"].as_i64().expect("a y");
    let second = ends["y2"].as_i64().expect("a y");
    assert_eq!(
        (first.min(second), first.max(second)),
        want,
        "the panel stands at the transformed pixel: {ends}"
    );
    let lines = working_lines(client, map).await;
    let index = ends["index"].as_u64().expect("an index");
    assert_eq!(
        line_row(&lines, index)["special"],
        json!(11_i64),
        "the S1 level end on the written line"
    );
}
