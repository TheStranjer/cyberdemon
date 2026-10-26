//! The session custom definitions as an MCP client sees them: the DEHACKED
//! patch — comment lines inside its `Thing` block and all — moving the Imp
//! to its header name and fixed `Width`, the DECORATE actors joining the
//! table with their inheritance and the MAPINFO `DoomEdNums`
//! mapping with its unknown-class warning, `ZScript` classes arriving through
//! includes and `ZMapInfo`, `include_custom: false` hiding every custom row and
//! the extra rows shifting the filtered counts.

use serde_json::json;

use crate::defs::{custom_defs_client, single_row, types, warns};
use crate::support::call;

#[tokio::test]
async fn dehacked_moves_the_imp_and_decorate_actors_join_the_table() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = custom_defs_client(dir.path()).await;
    let moved = call(&client, "get_thing_types", json!({ "type": 20_000_i64 })).await;
    assert_eq!(
        single_row(&moved),
        json!({
            "type": 20_000_i64,
            "name": "Evil Imp",
            "sprite": "TROO",
            "frame": "A",
            "radius": 10_i64,
            "category": "Monsters",
            "hanging": false,
            "source": "dehacked",
        }),
        "the Imp moves with its header name and fixed Width"
    );
    let freed = call(&client, "get_thing_types", json!({ "type": 3_001_i64 })).await;
    assert_eq!(freed["count"].as_u64(), Some(0), "3001 no longer spawns it");
    let lamp = call(&client, "get_thing_types", json!({ "type": 20_010_i64 })).await;
    let row = single_row(&lamp);
    assert_eq!(row["name"], json!("PlasmaLamp"), "the decorate actor");
    assert_eq!(row["sprite"], json!("PLSM"), "its spawn sprite");
    assert_eq!(row["radius"], json!(24_i64), "its radius");
    assert_eq!(row["category"], json!("Custom (DECORATE)"), "the category");
    assert_eq!(row["source"], json!("decorate"), "the source");
}

#[tokio::test]
async fn decorate_inheritance_and_mapinfo_mapping_are_reported() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = custom_defs_client(dir.path()).await;
    let derived = call(&client, "get_thing_types", json!({ "type": 20_011_i64 })).await;
    let row = single_row(&derived);
    assert_eq!(row["sprite"], json!("LAMP"), "inherited from LampBase");
    assert_eq!(row["frame"], json!("B"), "and its frame letter");
    assert_eq!(
        row["radius"],
        json!(16_i64),
        "inherited radius, integer part"
    );
    assert_eq!(row["source"], json!("decorate"), "still a decorate thing");
    let mapped = call(&client, "get_thing_types", json!({ "type": 20_012_i64 })).await;
    let base = single_row(&mapped);
    assert_eq!(
        base["name"],
        json!("LampBase"),
        "DoomEdNums mapped the class"
    );
    assert_eq!(
        base["category"],
        json!("Custom (MAPINFO)"),
        "placed by MAPINFO"
    );
    assert_eq!(base["source"], json!("decorate"), "defined by DECORATE");
    let missing = call(&client, "get_thing_types", json!({ "type": 20_013_i64 })).await;
    let stub = single_row(&missing);
    assert_eq!(stub["name"], json!("NotAClass"), "name-only row");
    assert_eq!(stub["sprite"], json!(""), "nothing else known");
    assert!(
        warns(&missing["warnings"], "unknown class"),
        "the unknown class warns: {}",
        missing["warnings"]
    );
}

#[tokio::test]
async fn zscript_classes_come_in_through_includes_and_zmapinfo() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = custom_defs_client(dir.path()).await;
    let torch = call(&client, "get_thing_types", json!({ "type": 12_345_i64 })).await;
    assert_eq!(
        single_row(&torch),
        json!({
            "type": 12_345_i64,
            "name": "Torch",
            "sprite": "TORC",
            "frame": "D",
            "radius": 10_i64,
            "category": "Custom (MAPINFO)",
            "hanging": false,
            "source": "zscript",
        }),
        "the class was included and mapped"
    );
}

#[tokio::test]
async fn include_custom_false_hides_every_custom_row() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = custom_defs_client(dir.path()).await;
    let report = call(
        &client,
        "get_thing_types",
        json!({ "include_custom": false }),
    )
    .await;
    assert_eq!(report["count"].as_u64(), Some(123), "the plain DOOM II set");
    assert_eq!(report["warnings"], json!([]), "no parser warnings either");
    let kinds = types(&report);
    assert!(kinds.contains(&3001), "the Imp keeps its built-in number");
    assert!(!kinds.contains(&20_000), "no dehacked row");
    assert!(!kinds.contains(&20_010), "no decorate row");
    assert!(!kinds.contains(&12_345), "no zscript row");
}

#[tokio::test]
async fn custom_things_shift_the_filtered_counts() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = custom_defs_client(dir.path()).await;
    let report = call(&client, "get_thing_types", json!({})).await;
    assert_eq!(report["count"].as_u64(), Some(128), "123 - 1 + 6 rows");
    let lamps = call(&client, "get_thing_types", json!({ "search": "lamp" })).await;
    assert_eq!(
        types(&lamps),
        vec![85_u64, 86, 2028, 20_010, 20_011, 20_012],
        "the candelabra share the substring; the custom lamps join them"
    );
    let custom = call(
        &client,
        "get_thing_types",
        json!({ "category": "Custom (" }),
    )
    .await;
    assert_eq!(
        custom["count"].as_u64(),
        Some(5),
        "two lamps (DECORATE), the mapped base, NotAClass and the Torch (MAPINFO)"
    );
}
