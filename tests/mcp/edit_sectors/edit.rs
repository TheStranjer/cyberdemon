//! `add_sector` and `edit_sector` as an MCP client calls them: the appended
//! dead sector with its echoed defaults, the custom values with their
//! uppercasing and the bad-flat, over-bright-light, out-of-range-height and
//! unknown-map refusals, the height edit that keeps the node lumps, the delta
//! shifts over a list of indices, the absolute/delta conflict, the empty
//! request, the index past the end and the closed door whose heights are
//! equal.

use serde_json::json;

use crate::edit_sectors::{doom_loaded, merged_sectors, refuse, working_sectors};
use crate::support::{call, call_err};

#[tokio::test]
async fn add_sector_appends_a_dead_sector_with_defaults() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = doom_loaded(dir.path()).await;
    let result = call(&client, "add_sector", json!({})).await;
    assert_eq!(result["map"], json!("MAP01"), "the only map is default");
    assert_eq!(result["index"], json!(2_i32), "appended after the two");
    assert_eq!(
        result["copied_to_working"],
        json!(true),
        "the block copied in"
    );
    assert_eq!(
        result["nodes_invalidated"],
        json!(true),
        "the sector count is geometry"
    );
    assert_eq!(
        result["sector"],
        json!({
            "floor_height": 0_i32, "ceiling_height": 128_i32,
            "floor_flat": "FLOOR4_8", "ceiling_flat": "CEIL3_5",
            "light": 160_i32, "special": 0_i32, "tag": 0_i32,
        }),
        "the defaults echo"
    );
    let rows = working_sectors(&client, "MAP01").await;
    assert_eq!(rows.len(), 3, "the map holds three sectors now");
    let added = rows.last().expect("the appended sector");
    assert_eq!(added["floor_flat"], json!("FLOOR4_8"), "read back");
    assert_eq!(added["ceiling_flat"], json!("CEIL3_5"));
    assert_eq!(added["light"], json!(160_i32));
    assert_eq!(
        added["sidedefs"].as_array().expect("a list").len(),
        0,
        "no sidedef references it yet"
    );
}

#[tokio::test]
async fn add_sector_stores_custom_values_and_refuses_bad_ones() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = doom_loaded(dir.path()).await;
    let result = call(
        &client,
        "add_sector",
        json!({"map": "MAP01", "floor_height": -8_i32, "ceiling_height": 256_i32,
               "floor_flat": "floor1", "light": 255_i32, "special": 1_i32,
               "tag": 66_i32}),
    )
    .await;
    assert_eq!(result["sector"]["floor_height"], json!(-8_i32), "stored");
    assert_eq!(result["sector"]["ceiling_height"], json!(256_i32));
    assert_eq!(
        result["sector"]["floor_flat"],
        json!("FLOOR1"),
        "uppercased"
    );
    assert_eq!(
        result["sector"]["ceiling_flat"],
        json!("CEIL3_5"),
        "absent stays the default"
    );
    assert_eq!(result["sector"]["light"], json!(255_i32));
    assert_eq!(result["sector"]["tag"], json!(66_i32));
    refuse(&client, "add_sector", json!({"floor_flat": "TOOLONG9C"})).await;
    refuse(&client, "add_sector", json!({"ceiling_flat": "-"})).await;
    refuse(&client, "add_sector", json!({"light": 300_i32})).await;
    refuse(&client, "add_sector", json!({"light": -1_i32})).await;
    refuse(&client, "add_sector", json!({"floor_height": 200_i32})).await;
    refuse(&client, "add_sector", json!({"tag": 70_000_i32})).await;
    let (map_code, _, _) = call_err(
        &client,
        "add_sector",
        json!({"map": "MAP09", "floor_flat": "FLOOR4_8"}),
    )
    .await;
    assert_eq!(map_code, "not_found", "unknown map");
    let rows = merged_sectors(&client, "MAP01").await;
    assert_eq!(
        rows.len(),
        3,
        "only the one successful add stands in the merged view"
    );
    assert_eq!(rows[2]["tag"], json!(66_i32), "and it is the custom one");
}

#[tokio::test]
async fn edit_sector_changes_heights_and_keeps_the_nodes() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = doom_loaded(dir.path()).await;
    let result = call(
        &client,
        "edit_sector",
        json!({"index": 0_i32, "floor_height": 24_i32, "ceiling_height": 200_i32}),
    )
    .await;
    assert_eq!(result["map"], json!("MAP01"), "the default map");
    assert_eq!(
        result["nodes_invalidated"],
        json!(false),
        "sector properties are not geometry"
    );
    assert_eq!(
        result["copied_to_working"],
        json!(true),
        "first edit copied"
    );
    let rows = result["sectors"].as_array().expect("a sectors array");
    assert_eq!(rows.len(), 1, "one row reported");
    let row = &rows[0];
    assert_eq!(row["index"], json!(0_i32), "for the selected sector");
    assert_eq!(row["floor_height"], json!(24_i32), "the asked heights");
    assert_eq!(row["ceiling_height"], json!(200_i32));
    assert_eq!(row["floor_flat"], json!("FLOOR1"), "the rest stands");
    assert_eq!(row["light"], json!(160_i32));
    assert_eq!(row["tag"], json!(0_i32));
    let stored = working_sectors(&client, "MAP01").await;
    assert_eq!(
        stored[0]["floor_height"],
        json!(24_i32),
        "and the map holds it"
    );
    assert_eq!(
        stored[1]["floor_height"],
        json!(32_i32),
        "neighbours untouched"
    );
}

#[tokio::test]
async fn edit_sector_applies_to_lists_and_shifts_with_deltas() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = doom_loaded(dir.path()).await;
    let first = call(
        &client,
        "edit_sector",
        json!({"index": [1_i32, 0_i32], "floor_delta": 16_i32}),
    )
    .await;
    let rows = first["sectors"].as_array().expect("a sectors array");
    assert_eq!(rows.len(), 2, "both sectors moved");
    assert_eq!(rows[0]["index"], json!(0_i32), "the rows come sorted");
    assert_eq!(rows[0]["floor_height"], json!(16_i32), "0 plus 16");
    assert_eq!(
        rows[1]["floor_height"],
        json!(48_i32),
        "32 plus 16 from its own value"
    );
    assert_eq!(
        first["nodes_invalidated"],
        json!(false),
        "heights are not geometry either"
    );
    let second = call(
        &client,
        "edit_sector",
        json!({"index": [0_i32, 1_i32], "ceiling_delta": -16_i32, "floor_flat": "ceil5"}),
    )
    .await;
    let lowered = second["sectors"].as_array().expect("a sectors array");
    assert_eq!(lowered[0]["ceiling_height"], json!(112_i32), "128 minus 16");
    assert_eq!(
        lowered[0]["floor_flat"],
        json!("CEIL5"),
        "names go on all selected"
    );
    assert_eq!(lowered[1]["ceiling_height"], json!(112_i32));
    assert_eq!(
        lowered[1]["floor_height"],
        json!(48_i32),
        "the earlier delta stands"
    );
    let stored = working_sectors(&client, "MAP01").await;
    assert_eq!(
        stored[0]["ceiling_height"],
        json!(112_i32),
        "written and read back"
    );
    assert_eq!(stored[0]["floor_flat"], json!("CEIL5"));
}

#[tokio::test]
async fn edit_sector_refuses_conflicting_empty_and_impossible_requests() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = doom_loaded(dir.path()).await;
    refuse(
        &client,
        "edit_sector",
        json!({"index": 0_i32, "floor_height": 8_i32, "floor_delta": 8_i32}),
    )
    .await;
    refuse(
        &client,
        "edit_sector",
        json!({"index": 0_i32, "ceiling_height": 8_i32, "ceiling_delta": 8_i32}),
    )
    .await;
    refuse(&client, "edit_sector", json!({"index": 0_i32})).await;
    refuse(&client, "edit_sector", json!({"index": []})).await;
    refuse(
        &client,
        "edit_sector",
        json!({"index": 0_i32, "floor_height": 200_i32}),
    )
    .await;
    refuse(
        &client,
        "edit_sector",
        json!({"index": 0_i32, "light": 300_i32}),
    )
    .await;
    refuse(
        &client,
        "edit_sector",
        json!({"index": 0_i32, "floor_flat": "flat-dash"}),
    )
    .await;
    refuse(
        &client,
        "edit_sector",
        json!({"index": 0_i32, "tag": 70_000_i32}),
    )
    .await;
    let (index_code, message, index_hint) = call_err(
        &client,
        "edit_sector",
        json!({"index": 9_i32, "light": 200_i32}),
    )
    .await;
    assert_eq!(index_code, "not_found", "an index past the end is missing");
    let phrasing = format!("{message} and hint {index_hint}");
    assert!(
        phrasing.contains("sector 9 does not exist")
            && phrasing.contains("valid indices are 0..=1")
            && phrasing.contains("describe_map or find_elements"),
        "the refusal states the range and names the readers: {phrasing}"
    );
    let stored = merged_sectors(&client, "MAP01").await;
    assert_eq!(stored.len(), 2, "the refusals appended nothing");
    assert_eq!(stored[0]["floor_height"], json!(0_i32), "nothing changed");
    assert_eq!(stored[1]["light"], json!(192_i32));
}

#[tokio::test]
async fn a_closed_door_equal_heights_is_allowed() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = doom_loaded(dir.path()).await;
    let result = call(
        &client,
        "edit_sector",
        json!({"index": 0_i32, "ceiling_height": 0_i32}),
    )
    .await;
    assert_eq!(
        result["sectors"][0]["ceiling_height"],
        json!(0_i32),
        "floor and ceiling equal: a closed door is fine"
    );
    assert_eq!(
        result["nodes_invalidated"],
        json!(false),
        "still a property edit"
    );
}
