//! The `get_map_summary` overview and the `set_active_map` selection it
//! shares with every map tool: the whole fixture summary and the working
//! copy after a raw lump replace, the `UDMF` and `Hexen` node shapes, the
//! unknown-map, no-default, bad-scope and empty-session refusals and the
//! selection's parsing, clearing and `get_session_info` report.
//!
//! The expected summary body is `summary_json` here; the loaded fixtures
//! come from [`crate::maps`].

use serde_json::{Value, json};

use crate::fixtures::iwad;
use crate::maps::{formats_loaded, iwad_plus_pwad};
use crate::support::{call, call_err, connect};

/// The whole `get_map_summary` result for the two-room fixture map
/// under the given `source` value.
fn summary_json(source: &Value) -> Value {
    json!({
        "name": "MAP01", "format": "doom", "udmf_namespace": null, "source": source,
        "counts": {"things": 2_u64, "vertices": 6_u64, "linedefs": 7_u64, "sidedefs": 8_u64, "sectors": 2_u64},
        "bounds": {"min_x": 0_i64, "min_y": 0_i64, "max_x": 512_i64, "max_y": 256_i64, "width": 512_i64, "height": 256_i64},
        "lines": {"one_sided": 6_u64, "two_sided": 1_u64, "with_special": 0_u64},
        "heights": {"floor_min": 0_i64, "floor_max": 32_i64, "ceiling_min": 128_i64, "ceiling_max": 128_i64},
        "things_by_type": [{"type": 1_i64, "count": 1_u64}, {"type": 3001_i64, "count": 1_u64}],
        "player_starts": [1_i64],
        "textures_used": ["STEP1", "WALL1"], "flats_used": ["FLOOR1"],
        "nodes": {
            "present": false, "segs": 0_u64, "subsectors": 0_u64, "nodes": 0_u64,
            "reject_bytes": 0_u64, "blockmap_bytes": 0_u64, "znodes": false, "gl": false,
        },
    })
}

#[tokio::test]
async fn get_map_summary_reports_the_fixture_map_then_the_working_copy() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = connect().await;
    let base = iwad(dir.path());
    call(
        &client,
        "open_iwad",
        json!({ "path": base.display().to_string() }),
    )
    .await;
    let report = call(&client, "get_map_summary", json!({})).await;
    assert_eq!(
        report,
        summary_json(&json!({ "layer": 0_u64 })),
        "the whole summary of the only map, resolved without a name",
    );
    call(
        &client,
        "replace_lump",
        json!({ "name": "REJECT", "hex": "AA BB" }),
    )
    .await;
    let working = call(&client, "get_map_summary", json!({ "map": "MAP01" })).await;
    assert_eq!(
        working["source"],
        json!("working"),
        "the copied block reads from the working archive",
    );
    assert_eq!(
        working["nodes"]["reject_bytes"],
        json!(2_u64),
        "the new REJECT",
    );
    assert_eq!(
        working["nodes"]["present"],
        json!(false),
        "REJECT bytes alone are no node data",
    );
    assert_eq!(
        working["counts"]["things"],
        json!(2_u64),
        "the map itself is unchanged",
    );
}

#[tokio::test]
async fn get_map_summary_covers_the_udmf_and_hexen_node_shapes() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = formats_loaded(dir.path()).await;
    let udmf = call(&client, "get_map_summary", json!({ "map": "MAP03" })).await;
    assert_eq!(udmf["format"], json!("udmf"), "the TEXTMAP block");
    assert_eq!(
        udmf["udmf_namespace"],
        json!("zdoom"),
        "the namespace rides along",
    );
    assert_eq!(
        udmf["bounds"]["width"],
        json!(512_i64),
        "the decoded map bounds",
    );
    assert_eq!(
        udmf["nodes"],
        json!({
            "present": true, "segs": 0_u64, "subsectors": 0_u64, "nodes": 0_u64,
            "reject_bytes": 0_u64, "blockmap_bytes": 0_u64, "znodes": true, "gl": true,
        }),
        "bytes in ZNODES and an attached GL run",
    );
    let hexen = call(&client, "get_map_summary", json!({ "map": "MAP04" })).await;
    assert_eq!(hexen["format"], json!("hexen"), "the BEHAVIOR marks it");
    assert_eq!(
        hexen["udmf_namespace"],
        Value::Null,
        "binary has no namespace",
    );
    assert_eq!(
        hexen["nodes"],
        json!({
            "present": true, "segs": 0_u64, "subsectors": 0_u64, "nodes": 0_u64,
            "reject_bytes": 0_u64, "blockmap_bytes": 0_u64, "znodes": false, "gl": false,
        }),
        "8 NODES bytes are present but hold no whole vanilla record",
    );
}

#[tokio::test]
async fn get_map_summary_reports_unknown_maps_and_missing_defaults() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_plus_pwad(dir.path()).await;
    let (code, message, hint) =
        call_err(&client, "get_map_summary", json!({ "map": "MAP99" })).await;
    assert_eq!(code, "not_found", "no such map");
    assert!(
        message.contains("MAP99"),
        "the message names the map: {message}"
    );
    assert!(
        hint.contains("list_maps") && hint.contains("create_map"),
        "the hint names the way out: {hint}",
    );
    let (default_code, default_message, _) = call_err(&client, "get_map_summary", json!({})).await;
    assert_eq!(default_code, "invalid_argument", "two maps, no default");
    assert!(
        default_message.contains("pass map or call set_active_map"),
        "the error names the fixes: {default_message}",
    );
    let (layer_code, _, _) = call_err(
        &client,
        "get_map_summary",
        json!({ "map": "MAP02", "scope": "layer:0" }),
    )
    .await;
    assert_eq!(layer_code, "not_found", "MAP02 is not in layer 0");
    let (scope_code, _, _) = call_err(
        &client,
        "get_map_summary",
        json!({ "map": "MAP02", "scope": "nope" }),
    )
    .await;
    assert_eq!(scope_code, "invalid_argument", "unknown scope");
    let (name_code, _, _) =
        call_err(&client, "get_map_summary", json!({ "map": "BAD NAME" })).await;
    assert_eq!(
        name_code, "invalid_argument",
        "spaces are not allowed in names"
    );
    let empty = connect().await;
    let (empty_code, _, _) = call_err(&empty, "get_map_summary", json!({ "map": "MAP01" })).await;
    assert_eq!(
        empty_code, "not_found",
        "nothing is loaded, MAP01 is nowhere"
    );
}

#[tokio::test]
async fn set_active_map_selects_validates_clears_and_shows_in_session_info() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = iwad_plus_pwad(dir.path()).await;
    let (missing_code, message, _) =
        call_err(&client, "set_active_map", json!({ "map": "MAP99" })).await;
    assert_eq!(missing_code, "not_found", "MAP99 exists nowhere");
    assert!(message.contains("MAP99"), "the message names it: {message}");
    let (blank_code, _, _) = call_err(&client, "set_active_map", json!({ "map": "" })).await;
    assert_eq!(
        blank_code, "invalid_argument",
        "an empty name does not parse"
    );
    let set = call(&client, "set_active_map", json!({ "map": "map01" })).await;
    assert_eq!(set["active_map"], json!("MAP01"), "parsed and uppercased");
    let after_set = call(&client, "get_session_info", Value::Null).await;
    assert_eq!(
        after_set["active_map"],
        json!("MAP01"),
        "the session reflects it"
    );
    assert_eq!(
        after_set["undo"],
        json!([]),
        "the selection is not an undoable edit"
    );
    let cleared = call(&client, "set_active_map", json!({ "map": null })).await;
    assert_eq!(cleared["active_map"], Value::Null, "null clears");
    let after_clear = call(&client, "get_session_info", Value::Null).await;
    assert_eq!(
        after_clear["active_map"],
        Value::Null,
        "and the session shows that"
    );
    call(&client, "set_active_map", json!({ "map": "MAP02" })).await;
    let omitted = call(&client, "set_active_map", json!({})).await;
    assert_eq!(
        omitted["active_map"],
        Value::Null,
        "an omitted map clears too"
    );
}
