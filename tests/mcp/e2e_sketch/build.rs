//! The chain's building slice: `build_map_from_plan` with `finalize: true`
//! answers the three named rooms, the `door` and `stairs` connections in plan
//! order, the `door` and `exit` feature rows, all three things placed, no
//! judgment-gap warnings and no errors from the finalized validation; the
//! keycard and the imp stand exactly at their marks' reported `map_center`,
//! read back through `find_elements`; and `check_playability` answers `ok:
//! true` with every sector reached - counted against `describe_map`, the
//! plan's own room, door-band and stair-step sectors covering the whole index
//! range - the blue lock present in `keys`/`locks` and opened, no blocker and
//! nothing stuck. The two doomednums the pinned marks resolve to live here
//! with these checks.

use serde_json::{Value, json};

use crate::e2e_sketch::{Client, MAP, ROOM_NAMES, build_the_traced_plan, mark_of_colour};
use crate::support::call;

/// The doomednum a `Blue Keycard` name resolves to.
const BLUE_KEYCARD: i64 = 5_i64;

/// The doomednum an `imp` name resolves to.
const IMP: i64 = 3001_i64;

#[tokio::test]
async fn the_traced_plan_builds_a_fully_playable_map() {
    let dir = tempfile::tempdir().expect("temp dir");
    let chain = build_the_traced_plan(dir.path()).await;
    assert_build_report(&chain.built);
    assert_things_stand_on_their_marks(&chain.client, &chain.trace).await;
    assert_playable(&chain.client, &chain.built).await;
}

/// The build answers the three named rooms, the `door` and `stairs`
/// connections in plan order, the `door` and `exit` feature rows,
/// all three things placed, no judgment-gap warnings and no errors
/// from the finalized validation.
fn assert_build_report(built: &Value) {
    let rooms = built["rooms"].as_object().expect("a room bookkeeping");
    for name in ROOM_NAMES {
        assert!(
            rooms.contains_key(name),
            "room {name} owns sectors: {rooms:?}"
        );
    }
    assert_eq!(rooms.len(), ROOM_NAMES.len(), "exactly the three rooms");
    let connections = built["connections"].as_array().expect("a list");
    let connection_kinds: Vec<&str> = connections
        .iter()
        .map(|row| row["kind"].as_str().expect("a kind"))
        .collect();
    assert_eq!(
        connection_kinds,
        vec!["door", "stairs"],
        "the connections in plan order"
    );
    let features = built["features"].as_array().expect("a list");
    let feature_kinds: Vec<&str> = features
        .iter()
        .map(|row| row["kind"].as_str().expect("a kind"))
        .collect();
    assert_eq!(
        feature_kinds,
        vec!["door", "exit"],
        "doors first, exits last"
    );
    assert_eq!(
        features.get(1).expect("the exit row")["room"],
        json!("exit"),
        "the level ends in the raised room"
    );
    assert_eq!(
        built["things_placed"],
        json!(3_u64),
        "the key, the imp and the start"
    );
    assert_eq!(
        built["warnings"],
        json!([]),
        "key in hand, exit set: {:?}",
        built["warnings"]
    );
    assert_eq!(
        built["validation"]["errors"],
        json!([]),
        "the finalized map validates error-free: {}",
        built["validation"]
    );
}

/// The pinned `x`/`y` plan points travel the shared `units` exactly
/// like the trace's `map_center` fields: the keycard stands at the
/// X's map centre, the imp at the red dot's.
async fn assert_things_stand_on_their_marks(client: &Client, report: &Value) {
    let key = mark_of_colour(report, "#000000")["map_center"].clone();
    expect_thing_at(client, BLUE_KEYCARD, &key).await;
    let imp = mark_of_colour(report, "#FF0000")["map_center"].clone();
    expect_thing_at(client, IMP, &imp).await;
}

/// One `find_elements` thing of type `kind` standing at the report
/// point `map_center`.
async fn expect_thing_at(client: &Client, kind: i64, map_center: &Value) {
    let found = call(
        client,
        "find_elements",
        json!({
            "map": MAP,
            "kind": "things",
            "where": [{ "field": "type", "op": "eq", "value": kind }],
        }),
    )
    .await;
    assert_eq!(found["count"], json!(1_u64), "one thing of type {kind}");
    let row = found["rows"].get(0).expect("the thing row");
    let wanted = map_center.as_array().expect("a mapped centre");
    assert_eq!(
        row["x"],
        json!(wanted.first().and_then(Value::as_i64).expect("an x")),
        "the pinned x lands"
    );
    assert_eq!(
        row["y"],
        json!(wanted.get(1).and_then(Value::as_i64).expect("a y")),
        "and the pinned y"
    );
}

/// `check_playability` answers `ok`: every sector the built map owns
/// is reached, the blue lock stands in `locks` opened with its key
/// reached in `keys`, and nothing is blocked or stuck.
async fn assert_playable(client: &Client, built: &Value) {
    let report = call(client, "check_playability", json!({ "map": MAP })).await;
    assert!(
        report["ok"].as_bool().expect("a verdict"),
        "the traced map plays: {report}"
    );
    assert_eq!(report["blockers"], json!([]), "nothing holds a sector out");
    assert_eq!(report["stuck_things"], json!([]), "every thing stands");
    assert_all_sectors_reachable(client, built, &report).await;
    assert_blue_key_opened(&report);
    assert!(
        report["exits"]
            .as_array()
            .expect("an exit list")
            .iter()
            .any(|exit| exit["reachable"].as_bool().expect("a flag")),
        "a reachable level end: {}",
        report["exits"]
    );
}

/// The walk reaches every sector, its total is `describe_map`'s own
/// sector count, and the plan's room, door-band and stair-step
/// sectors are exactly the whole map.
async fn assert_all_sectors_reachable(client: &Client, built: &Value, report: &Value) {
    let described = call(
        client,
        "describe_map",
        json!({ "map": MAP, "render": false }),
    )
    .await;
    let total = report["sectors"]["total"].as_u64().expect("a total");
    assert_eq!(
        described["summary"]["counts"]["sectors"],
        json!(total),
        "the walk and the describe read the same map"
    );
    assert_eq!(
        report["sectors"]["unreachable"],
        json!([]),
        "no sector stays out of reach"
    );
    assert_eq!(
        report["sectors"]["reachable"],
        json!(total),
        "all of them reached"
    );
    assert_sector_cover(built, total);
}

/// The indices of every sector the build report names: each room's
/// and each connection's, deduplicated and ascending.
fn planned_sectors(built: &Value) -> Vec<u64> {
    let room_lists: Vec<&Value> = built["rooms"]
        .as_object()
        .expect("rooms")
        .values()
        .collect();
    let band_lists: Vec<&Value> = built["connections"]
        .as_array()
        .expect("connections")
        .iter()
        .map(|row| &row["sectors"])
        .collect();
    let mut rows: Vec<u64> = room_lists
        .iter()
        .chain(band_lists.iter())
        .flat_map(|list| list.as_array().expect("a sector list"))
        .map(|sector| sector.as_u64().expect("an index"))
        .collect();
    rows.sort_unstable();
    rows.dedup();
    rows
}

/// The planned sectors, room floors and connector bands alike, are
/// exactly the indices `0..total`, once each, and there are at
/// least the three rooms' worth.
fn assert_sector_cover(built: &Value, total: u64) {
    let planned = planned_sectors(built);
    assert!(
        planned.len() >= ROOM_NAMES.len(),
        "at least one sector per planned room"
    );
    let whole: Vec<u64> = (0_u64..total).collect();
    assert_eq!(planned, whole, "the plan's sectors are the whole map");
}

/// The walk saw the one blue key and reached it, saw the blue lock
/// and opened it, and no door stayed shut behind the player.
fn assert_blue_key_opened(report: &Value) {
    let keys = report["keys"].as_array().expect("a key list");
    assert_eq!(keys.len(), 1, "the one keycard: {keys:?}");
    assert_eq!(
        keys.first().expect("the key")["color"],
        json!("blue"),
        "the card answers the blue lock"
    );
    assert!(
        keys.first().expect("the key")["reachable"]
            .as_bool()
            .expect("a flag"),
        "the key stands where the player reaches it before the door"
    );
    let locks = report["locks"].as_array().expect("a lock list");
    assert!(!locks.is_empty(), "the blue door is a lock the walk saw");
    for lock in locks {
        assert_eq!(lock["color"], json!("blue"), "the one locked gate");
        assert!(
            lock["opened"].as_bool().expect("a flag"),
            "the key opens it: {lock}"
        );
    }
    assert_eq!(
        report["doors_never_opened"],
        json!([]),
        "no door was left shut"
    );
    assert_eq!(
        report["unreachable_things"],
        json!([]),
        "every thing stands in a reached sector"
    );
}
