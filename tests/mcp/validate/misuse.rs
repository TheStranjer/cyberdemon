//! The picture-kind half of the `validate_map` wire tests: a flat worn as
//! a wall texture and a wall texture worn as a flat report exactly once,
//! as `misused_resource` rows whose message names the element, the role
//! and the fix tool - and the editing calls that write those names warn
//! with the same sentence as they go. Suppression is tested from both
//! sides: the misuse row replaces the `missing_texture` / `missing_flat`
//! row, and an undefined name of neither kind keeps its old row.

use serde_json::{Value, json};

use super::tests::{issue_codes, plain_iwad};
use crate::support::call;

/// The misuse rows of a report, by code.
fn misuse_rows(report: &Value) -> Vec<Value> {
    report["issues"]
        .as_array()
        .expect("an issue list")
        .iter()
        .filter(|row| row["code"] == json!("misused_resource"))
        .cloned()
        .collect()
}

/// The single misuse row of a report, panicking when there is not exactly
/// one.
#[track_caller]
fn only_row(report: &Value) -> Value {
    let rows = misuse_rows(report);
    assert_eq!(rows.len(), 1, "exactly one misuse row: {report}");
    rows.into_iter().next().expect("the one row")
}

#[tokio::test]
async fn a_flat_worn_as_a_wall_texture_warns_and_reports_misuse() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = plain_iwad(dir.path()).await;
    let edited = call(
        &client,
        "edit_sidedef",
        json!({ "map": "MAP01", "index": 0_i32, "middle": "FLOOR1" }),
    )
    .await;
    let warnings = edited["warnings"].as_array().expect("a warning list");
    assert_eq!(warnings.len(), 1, "the flat face warns once: {edited}");
    assert!(
        warnings[0]
            .as_str()
            .expect("text")
            .contains("the middle texture \"FLOOR1\" on sidedef 0 is carried only as a flat"),
        "the warning names the face, the name and the kind: {warnings:?}"
    );
    let report = call(&client, "validate_map", json!({ "map": "MAP01" })).await;
    let row = only_row(&report);
    assert_eq!(
        row["severity"],
        json!("error"),
        "a misused picture is broken"
    );
    assert_eq!(row["sidedefs"], json!([0_u64]), "the sidedef named");
    assert!(
        row["message"]
            .as_str()
            .expect("text")
            .contains("create_texture_from_image"),
        "the message names the fix: {row}"
    );
    assert_eq!(
        report["counts"]["error"],
        json!(1_u64),
        "the misuse row replaces the undefined-face row, never doubles it: {report}"
    );
    assert!(
        !issue_codes(&report).contains(&"missing_texture".to_owned()),
        "missing_texture holds back on the misused face: {report}"
    );
}

#[tokio::test]
async fn a_wall_texture_worn_as_a_flat_warns_and_reports_misuse() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = plain_iwad(dir.path()).await;
    let edited = call(
        &client,
        "edit_sector",
        json!({ "map": "MAP01", "index": [0_i32, 1_i32], "floor_flat": "WALL1" }),
    )
    .await;
    let warnings = edited["warnings"].as_array().expect("a warning list");
    assert_eq!(warnings.len(), 1, "one plane role, one sentence: {edited}");
    assert!(
        warnings[0]
            .as_str()
            .expect("text")
            .contains("the floor flat \"WALL1\" on sectors 0, 1 is defined only as a wall texture"),
        "the warning lists both edited sectors: {warnings:?}"
    );
    let report = call(&client, "validate_map", json!({ "map": "MAP01" })).await;
    let rows = misuse_rows(&report);
    assert_eq!(rows.len(), 2, "sector 0 and sector 1: {report}");
    assert!(
        rows.iter().all(|row| row["message"]
            .as_str()
            .unwrap_or_default()
            .contains("edit_sector")),
        "every row names the fix tool: {rows:?}"
    );
    assert_eq!(
        report["counts"]["error"],
        json!(2_u64),
        "the misuse rows replace the undefined-flat rows, never double them: {report}"
    );
    assert!(
        !issue_codes(&report).contains(&"missing_flat".to_owned()),
        "missing_flat holds back on misused planes: {report}"
    );
    let undo = call(&client, "undo", json!({})).await;
    assert_eq!(undo["undone"][0], json!("edit 2 sectors"), "one step back");
    let clean = call(&client, "validate_map", json!({ "map": "MAP01" })).await;
    assert_eq!(
        clean["counts"]["error"],
        json!(0_u64),
        "the fixture is clean again: {clean}"
    );
}

#[tokio::test]
async fn a_name_the_view_lacks_still_reports_missing_texture_not_misuse() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = plain_iwad(dir.path()).await;
    call(
        &client,
        "edit_sidedef",
        json!({ "map": "MAP01", "index": 0_i32, "middle": "STARTAN3" }),
    )
    .await;
    let report = call(&client, "validate_map", json!({ "map": "MAP01" })).await;
    let codes = issue_codes(&report);
    assert!(
        codes.contains(&"missing_texture".to_owned())
            && !codes.contains(&"misused_resource".to_owned()),
        "STARTAN3 is neither a carried flat nor a defined texture: {codes:?}"
    );
}

#[tokio::test]
async fn codes_selects_misused_resource_by_name() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let client = plain_iwad(dir.path()).await;
    call(
        &client,
        "edit_sidedef",
        json!({ "map": "MAP01", "index": 0_i32, "middle": "FLOOR1" }),
    )
    .await;
    let report = call(
        &client,
        "validate_map",
        json!({ "map": "MAP01", "codes": ["misused_resource"] }),
    )
    .await;
    assert_eq!(
        report["checks_run"],
        json!(["misused_resource"]),
        "one check ran"
    );
    assert_eq!(misuse_rows(&report).len(), 1, "the row the code selects");
    assert_eq!(report["counts"]["error"], json!(1_u64), "exactly it");
}
