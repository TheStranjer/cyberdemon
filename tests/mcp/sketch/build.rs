//! The traced plan going all the way through: the `plan_rooms` rows with a
//! connection at the gap centre building an error-free map, and the same
//! sketch traced twice answering identically.

use serde_json::json;

use crate::fixtures::iwad_features;
use crate::sketch::{sketch, strip_debug, trace_args, write_png};
use crate::support::{call, connect};

#[tokio::test]
async fn plan_rooms_and_the_gap_opening_build_an_error_free_map() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = connect().await;
    let base = iwad_features(dir.path());
    call(
        &client,
        "open_iwad",
        json!({"path": base.display().to_string()}),
    )
    .await;
    let file = write_png(dir.path(), "sketch.png", &sketch(1));
    let mut args = trace_args(&file);
    args["units"] = json!({"scale_num": 4_i64, "flip_y": true});
    let report = call(&client, "trace_sketch", args).await;
    let door = report["gaps"][0].as_object().expect("the gap row");
    let centre = door["center"].as_array().expect("a [x, y] centre");
    let request = json!({
        "map": "MAP05",
        "rooms": report["plan_rooms"],
        "connections": [{
            "from": "R2",
            "to": "R3",
            "kind": "opening",
            "at": centre,
        }],
        "units": {"scale_num": 4_i64, "flip_y": true},
        "player_start": {"room": "R1"},
    });
    let built = call(&client, "build_map_from_plan", request).await;
    let rooms = built["rooms"].as_object().expect("the room bookkeeping");
    assert_eq!(rooms.len(), 3, "all three traced shapes became rooms");
    assert!(rooms.contains_key("R1") && rooms.contains_key("R2") && rooms.contains_key("R3"));
    let validation = call(
        &client,
        "validate_map",
        json!({"map": "MAP05", "min_severity": "error"}),
    )
    .await;
    assert_eq!(
        validation["issues"],
        json!([]),
        "the traced map carries no errors: {validation}"
    );
}

#[tokio::test]
async fn the_same_sketch_traced_twice_answers_identically() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = connect().await;
    let file = write_png(dir.path(), "sketch.png", &sketch(1));
    let first = call(&client, "trace_sketch", trace_args(&file)).await;
    let second = call(&client, "trace_sketch", trace_args(&file)).await;
    assert_eq!(
        strip_debug(first),
        strip_debug(second),
        "measurement is deterministic"
    );
}
