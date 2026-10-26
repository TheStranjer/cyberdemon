//! The `units` transform and the knobs around one call: the `units`
//! scale-and-mirror and `grid` snap that carry an image-space plan into map
//! units before compiling, with unusable `units.scale_den`/`grid` values
//! refused naming the parameter, a `UDMF` block with its declared namespace,
//! and a one-call `theme` override that dresses that call and never moves the
//! session's theme.

use serde_json::json;

use crate::plan::{features_loaded, plan_for, refuse_with};
use crate::support::{call, call_err};

#[tokio::test]
async fn image_space_units_scale_and_mirror_the_plan_into_map_units() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    let request = json!({
        "map": "MAP05",
        "rooms": [
            {"id": "shot", "rect": {"x": 0_i64, "y": 0_i64, "w": 100_i64, "h": 50_i64}}
        ],
        "connections": [],
        "units": {"scale_num": 4_i64, "flip_y": true},
        "player_start": {"room": "shot"}
    });
    let result = call(&client, "build_map_from_plan", request).await;
    assert_eq!(
        result["bounds"],
        json!({"x1": 0_i64, "y1": -200_i64, "x2": 400_i64, "y2": 0_i64}),
        "a 100x50 px room becomes a 400x200 room with y mirrored below 0"
    );
    let summary = call(&client, "get_map_summary", json!({"map": "MAP05"})).await;
    assert_eq!(
        summary["bounds"]["min_x"],
        json!(0_i64),
        "the scale is written"
    );
    assert_eq!(summary["bounds"]["max_x"], json!(400_i64));
    assert_eq!(
        summary["bounds"]["min_y"],
        json!(-200_i64),
        "the mirror is written"
    );
    assert_eq!(summary["bounds"]["max_y"], json!(0_i64));
}

#[tokio::test]
async fn the_grid_snaps_every_coordinate_to_the_nearest_line() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    let request = json!({
        "map": "MAP05",
        "rooms": [
            {"id": "a", "rect": {"x": 33_i64, "y": 33_i64, "w": 64_i64, "h": 64_i64}}
        ],
        "connections": [],
        "grid": 16_i64,
        "player_start": {"room": "a"}
    });
    let result = call(&client, "build_map_from_plan", request).await;
    assert_eq!(
        result["bounds"],
        json!({"x1": 32_i64, "y1": 32_i64, "x2": 96_i64, "y2": 96_i64}),
        "the 33,33-origin room snaps to 32,32 (and its far corner 97,97 to 96,96)"
    );
    let summary = call(&client, "get_map_summary", json!({"map": "MAP05"})).await;
    assert_eq!(
        summary["bounds"]["min_x"],
        json!(32_i64),
        "the snap is written"
    );
    assert_eq!(summary["bounds"]["min_y"], json!(32_i64));
    assert_eq!(summary["bounds"]["max_y"], json!(96_i64));
}

#[tokio::test]
async fn unusable_units_and_grid_are_refused_naming_the_parameter() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    let mut zero_den = plan_for("MAP05");
    zero_den["units"] = json!({"scale_den": 0_i64});
    refuse_with(&client, zero_den, "scale_den").await;
    let mut zero_num = plan_for("MAP05");
    zero_num["units"] = json!({"scale_num": 0_i64});
    refuse_with(&client, zero_num, "scale_num").await;
    let mut zero_grid = plan_for("MAP05");
    zero_grid["grid"] = json!(0_i64);
    refuse_with(&client, zero_grid, "grid").await;
    let maps = call(&client, "list_maps", json!({"scope": "working"})).await;
    assert_eq!(maps["maps"], json!([]), "no refusal wrote anything");
}

#[tokio::test]
async fn a_udmf_plan_lands_as_a_named_text_block() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    let mut request = plan_for("MAP05");
    request["format"] = json!("udmf");
    request["namespace"] = json!("zdoom");
    let result = call(&client, "build_map_from_plan", request).await;
    assert_eq!(result["format"], json!("udmf"), "the layout echoes");
    let summary = call(
        &client,
        "get_map_summary",
        json!({"map": "MAP05", "scope": "working"}),
    )
    .await;
    assert_eq!(summary["format"], json!("udmf"), "a TEXTMAP block");
    assert_eq!(
        summary["udmf_namespace"],
        json!("zdoom"),
        "the declared namespace"
    );
    assert_eq!(summary["counts"]["sectors"], json!(4_u64), "the same plan");
    let maps = call(&client, "list_maps", json!({"scope": "working"})).await;
    assert_eq!(maps["maps"][0]["format"], json!("udmf"), "listed as UDMF");
}

#[tokio::test]
async fn a_theme_override_dresses_one_call_and_leaves_the_session_alone() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    let before = call(&client, "get_texture_theme", json!({})).await;
    assert_eq!(before["theme"], json!("tech"), "the default theme");
    let mut request = plan_for("MAP05");
    request["theme"] = json!("HELL");
    call(&client, "build_map_from_plan", request).await;
    let after = call(&client, "get_texture_theme", json!({})).await;
    assert_eq!(after["theme"], json!("tech"), "the session never moved");
    let maps = call(&client, "list_maps", json!({"scope": "working"})).await;
    assert_eq!(maps["maps"].as_array().expect("a list").len(), 1);
    let mut bad_theme = plan_for("MAP06");
    bad_theme["theme"] = json!("nope");
    let (code, message, hint) = call_err(&client, "build_map_from_plan", bad_theme).await;
    assert_eq!(code, "invalid_argument", "an unknown theme is refused");
    assert!(
        message.starts_with("theme: "),
        "the refusal carries the plan path: {message}"
    );
    assert!(
        message.contains("tech") && message.contains("hell"),
        "the refusal lists the themes: {message}"
    );
    assert!(
        hint.contains("get_texture_theme"),
        "the hint points at the listing tool: {hint}"
    );
    let left = call(&client, "list_maps", json!({"scope": "working"})).await;
    assert_eq!(
        left["maps"].as_array().expect("a list").len(),
        1,
        "the refused MAP06 was never written"
    );
}
