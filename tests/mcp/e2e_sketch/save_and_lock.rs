//! The chain's shipping slices: `save_wad` then `new_wad` then `open_wad`
//! answers the same element counts and `ok` again, and the same plan rebuilt
//! without its `Blue Keycard` row answers `ok: false` with a `locked` blue
//! blocker on one of the door's own linedefs whose `hint`, repeated in
//! `next_steps`, names `add_thing`, the hall and the exit rooms held out of
//! reach behind it. The door's own rows come from
//! [`crate::e2e_sketch::views`]; the `{"path": ...}` argument object, the
//! `get_map_summary` element-count reader, the rebuilt plan's base request and
//! the card's plan name come from [`crate::e2e_sketch`].

use serde_json::{Value, json};

use crate::e2e_sketch::views::{door_feature, door_lines};
use crate::e2e_sketch::{KEY_NAME, MAP, build_the_traced_plan, map_counts, path_arg, plan_request};
use crate::support::call;

#[tokio::test]
async fn the_saved_wad_reopens_as_the_same_playable_map() {
    let dir = tempfile::tempdir().expect("temp dir");
    let chain = build_the_traced_plan(dir.path()).await;
    let before = map_counts(&chain.client).await;
    let file = dir.path().join("sketch-chain.wad");
    let saved = call(&chain.client, "save_wad", path_arg(&file)).await;
    assert_eq!(saved["path"], path_arg(&file)["path"], "the echoed target");
    assert_eq!(
        saved["dirty"],
        json!(false),
        "the save leaves nothing unsaved"
    );
    assert!(
        saved["bytes"].as_u64().expect("a byte count") > 0,
        "a file was written"
    );
    call(&chain.client, "new_wad", json!({})).await;
    let opened = call(&chain.client, "open_wad", path_arg(&file)).await;
    assert!(
        opened["lumps"].as_u64().expect("a lump count") > 0,
        "the saved archive opens again"
    );
    assert_eq!(
        map_counts(&chain.client).await,
        before,
        "the reloaded map holds the same elements"
    );
    let plays = call(&chain.client, "check_playability", json!({ "map": MAP })).await;
    assert!(
        plays["ok"].as_bool().expect("a verdict"),
        "the reloaded map plays: {plays}"
    );
    assert_eq!(plays["blockers"], json!([]), "and nothing is held out");
}

/// The traced plan with its `Blue Keycard` row gone and `replace` set:
/// the same map, with the blue door as a way nobody can open.
fn plan_without_the_key(trace: &Value) -> Value {
    let mut request = plan_request(trace);
    let things = request["things"].as_array_mut().expect("the plan's things");
    things.retain(|row| row["name"] != json!(KEY_NAME));
    request["replace"] = json!(true);
    request
}

/// The rebuild stood two things, not three, and the plan's own
/// judgment-gap warnings say what is missing.
fn assert_the_key_is_missing(built: &Value) {
    assert_eq!(
        built["things_placed"],
        json!(2_u64),
        "the imp and the player start: {}",
        built["things_placed"]
    );
    let warnings = built["warnings"].as_array().expect("a warning list");
    assert!(
        warnings.iter().any(|row| {
            let text = row.as_str().expect("a warning line");
            text.contains("blue") && text.contains("key")
        }),
        "the build reports the card the door wants: {warnings:?}"
    );
}

/// The `locked` blocker row standing on one of the built door's own
/// linedefs: the reason the walk gives, the colour of card it wants
/// and the door band's own sector it holds out of reach.
fn locked_door_blocker<'rep>(report: &'rep Value, built: &Value) -> &'rep Value {
    let lines = door_lines(built);
    let band = door_feature(built)["sectors"]
        .as_array()
        .expect("a sector list");
    let row = report["blockers"]
        .as_array()
        .expect("a blocker list")
        .iter()
        .find(|row| {
            row["reason"] == json!("locked")
                && row["color"] == json!("blue")
                && lines.contains(&row["line"])
        })
        .unwrap_or_else(|| {
            panic!(
                "a locked blue door holds a sector out: {}",
                report["blockers"]
            )
        });
    assert_eq!(
        row["reason"],
        json!("locked"),
        "the walk names the rule that kept the sector out: {row}"
    );
    assert_eq!(
        row["color"],
        json!("blue"),
        "and the card the door wants: {row}"
    );
    assert!(
        band.contains(&row["sector"]),
        "the door band itself is the held-out sector: {row} against {band:?}"
    );
    row
}

/// The locked row explains itself with a real instruction: it names
/// the colour, the key and the tool that places one, and the report's
/// `next_steps` carries that same line.
fn assert_the_hint_names_the_key(report: &Value, blocker: &Value) {
    let hint = blocker["hint"].as_str().expect("a hint");
    assert!(!hint.is_empty(), "the blocker answers how to fix it");
    assert!(
        hint.contains("blue") && hint.contains("key"),
        "the hint names the card the door wants: {hint}"
    );
    assert!(
        hint.contains("add_thing"),
        "and the tool that places it: {hint}"
    );
    let steps = report["next_steps"].as_array().expect("a next-steps list");
    assert!(
        steps.contains(&json!(hint)),
        "the fix leads the next steps: {steps:?}"
    );
}

/// The walk's reading of the same map: `ok` is false, every blue lock
/// stayed shut, the hall behind the door is on the unreachable list
/// and no level end can be worked.
fn assert_the_player_is_held_out(report: &Value, built: &Value) {
    assert!(
        !report["ok"].as_bool().expect("a verdict"),
        "a locked door with no key does not play: {}",
        report["summary"]
    );
    assert!(
        report["keys"].as_array().expect("a key list").is_empty(),
        "the walk found no card to pick up"
    );
    for lock in report["locks"].as_array().expect("a lock list") {
        assert_eq!(
            lock["color"],
            json!("blue"),
            "the one lock is the blue gate"
        );
        assert!(
            !lock["opened"].as_bool().expect("a flag"),
            "and it stayed shut: {lock}"
        );
    }
    assert_held_out_of_the_hall(report, built);
}

/// The sectors behind the blue door — the plan's `hall` and `exit`
/// rooms — stayed out of reach, and so did the level end.
fn assert_held_out_of_the_hall(report: &Value, built: &Value) {
    let unreachable = report["sectors"]["unreachable"]
        .as_array()
        .expect("an unreachable list");
    let mut held_out: Vec<&Value> = Vec::new();
    for room in ["hall", "exit"] {
        held_out.extend(built["rooms"][room].as_array().expect("a sector list"));
    }
    assert!(
        held_out.len() >= 2,
        "the hall and the exit are both behind the door: {held_out:?}"
    );
    for sector in &held_out {
        assert!(
            unreachable.contains(sector),
            "the sector {sector} stayed behind the locked door: {unreachable:?}"
        );
    }
    assert!(
        report["exits"]
            .as_array()
            .expect("an exit list")
            .iter()
            .all(|exit| !exit["reachable"].as_bool().expect("a flag")),
        "no level end could be worked: {}",
        report["exits"]
    );
}

#[tokio::test]
async fn a_blue_door_without_its_key_holds_the_player_out() {
    let dir = tempfile::tempdir().expect("temp dir");
    let chain = build_the_traced_plan(dir.path()).await;
    let rebuilt = call(
        &chain.client,
        "build_map_from_plan",
        plan_without_the_key(&chain.trace),
    )
    .await;
    assert_the_key_is_missing(&rebuilt);
    let report = call(&chain.client, "check_playability", json!({ "map": MAP })).await;
    let blocker = locked_door_blocker(&report, &rebuilt);
    assert_the_hint_names_the_key(&report, blocker);
    assert_the_player_is_held_out(&report, &rebuilt);
}
