//! `place_things` as seen by a real MCP client: the centred-outward spread
//! of four imps through the hall, cleared from one another and jamming
//! nothing in `check_playability`, the one-barrel-per-sector fill of both
//! rooms, a `near` circle that keeps its row inside it, the
//! `face_player_start` bearing snapped onto the fixture's start, the
//! shortfall warning of a tiny box, the dry run that computes positions and
//! writes nothing, the one-step undo, the seeded `random_seeded` angles that
//! replay exactly, the `Hexen` tid/special/args dressing, and the refusal
//! catalogue from ambiguous names to out-of-range counts, fields and skills.

use serde_json::{Value, json};

use crate::features::teleporter::reported_things;
use crate::features::{
    Client, angle_diff, dist_sq, features_loaded, formats_loaded, index_list, positions_of,
};
use crate::support::{call, call_err};

/// Checks the hall spread: every position stands inside sector 0 at
/// least 28 units off its one-sided walls and 48 from every other one.
fn expect_hall_spread(positions: &[[i64; 2]]) {
    for pair in positions {
        assert!(pair[0] >= 28_i64, "{pair:?} keeps the west wall");
        assert!(
            pair[1] >= 28_i64 && pair[1] <= 228_i64,
            "{pair:?} keeps the flat walls"
        );
        assert!(pair[0] <= 256_i64, "{pair:?} stays inside the hall");
    }
    for (left, _) in positions.iter().enumerate() {
        expect_clear_of_rest(positions, left);
    }
}

/// Checks one position clears every later one by 48 units.
fn expect_clear_of_rest(positions: &[[i64; 2]], from_index: usize) {
    let Some(from) = positions.get(from_index) else {
        return;
    };
    let floor = 48_i64.checked_mul(48).expect("test range");
    for to in positions
        .iter()
        .skip(from_index.checked_add(1).expect("small"))
    {
        assert!(
            dist_sq(*from, *to) >= floor,
            "imps stay 48 apart: {from:?} {to:?}"
        );
    }
}

/// Checks the placed rows: Imps standing in the hall, facing east by
/// default, unarmed ambush bits.
fn expect_imp_rows(rows: &[Value]) {
    for row in rows {
        assert_eq!(row["type"], json!(3001_i64), "every row is an Imp");
        assert_eq!(row["sector"], json!(0_i64), "the hall owns every spot");
        assert_eq!(row["angle"], json!(0_i64), "the default facing");
        let names = row["flag_names"].as_array().expect("flag names").clone();
        assert!(
            !names.contains(&json!("ambush")),
            "no ambush asked for: {row:?}"
        );
    }
}

/// Checks that every position lies within `radius` of `centre`.
fn expect_within(positions: &[[i64; 2]], centre: [i64; 2], radius: i64) {
    let squared = radius.checked_mul(radius).expect("test range");
    for pair in positions {
        assert!(
            dist_sq(*pair, centre) <= squared,
            "{pair:?} sits outside the circle"
        );
    }
}

/// Checks each placed row faces the fixture's start at `(64, 128)`
/// within one 45-degree snap.
fn expect_faces_the_start(rows: &[Value]) {
    for row in rows {
        let x = row["x"].as_i64().expect("an x");
        let y = row["y"].as_i64().expect("a y");
        let angle = row["angle"].as_i64().expect("an angle");
        let bearing = cyberdemon::fixed::atan2(
            128_i64.checked_sub(y).expect("test range"),
            64_i64.checked_sub(x).expect("test range"),
        )
        .to_degrees();
        assert!(
            angle_diff(angle, bearing) <= 45,
            "the Imp at ({x}, {y}) faces {angle}, the start sits at {bearing}"
        );
    }
}

/// Checks each placed row is a hard-skill ambusher facing a compass
/// point.
fn expect_ambushed_hard(rows: &[Value]) {
    for row in rows {
        let names = row["flag_names"].as_array().expect("flag names").clone();
        assert!(
            names.contains(&json!("ambush")),
            "the ambush bit rode along: {row:?}"
        );
        assert!(
            names.contains(&json!("hard")),
            "the named skill rode along: {row:?}"
        );
        assert!(
            !names.contains(&json!("easy")),
            "the unnamed skills stayed off: {row:?}"
        );
        let angle = row["angle"].as_i64().expect("an angle");
        assert_eq!(
            angle.rem_euclid(45),
            0,
            "the seeded angle is a compass point: {angle}"
        );
    }
}

/// Asserts `place_things` refuses `args` with `code` for a message
/// containing `needle`.
async fn refuse_places(client: &Client, args: Value, code: &str, needle: &str) {
    let (found, message, _) = call_err(client, "place_things", args).await;
    assert_eq!(found, code, "refusal code for {needle:?}: {message}");
    assert!(message.contains(needle), "{needle:?}: {message}");
}

#[tokio::test]
async fn place_things_spreads_four_imps_through_the_hall() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    let result = call(
        &client,
        "place_things",
        json!({"thing": "imp", "sectors": [0_i64], "count": 4_u64}),
    )
    .await;
    assert_eq!(result["placed"], json!(4_u64), "four landed");
    assert_eq!(result["requested"], json!(4_u64), "four were asked");
    assert_eq!(result["warnings"], json!([]), "the hall held them all");
    assert_eq!(result["dry_run"], json!(false));
    assert_eq!(
        result["nodes_invalidated"],
        json!(false),
        "things are no geometry"
    );
    assert_eq!(result["copied_to_working"], json!(true));
    assert_eq!(result["map"], json!("MAP01"));
    assert!(
        result["next_steps"][0]
            .as_str()
            .expect("next step is text")
            .contains("check_playability"),
        "the report says what to check: {}",
        result["next_steps"]
    );
    assert_eq!(
        index_list(&result, "things"),
        vec![2_usize, 3, 4, 5],
        "after the fixture's two"
    );
    expect_hall_spread(&positions_of(&result));
    expect_imp_rows(&reported_things(&client, "MAP01", &result).await);
    let playable = call(&client, "check_playability", json!({})).await;
    assert_eq!(
        playable["stuck_things"],
        json!([]),
        "nothing is jammed into geometry: {}",
        playable["stuck_things"]
    );
}

#[tokio::test]
async fn place_things_per_sector_fills_both_rooms() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    let result = call(
        &client,
        "place_things",
        json!({"thing": 2035_u64, "sectors": [0_i64, 1_i64], "per_sector": 1_u64}),
    )
    .await;
    assert_eq!(result["placed"], json!(2_u64), "one per sector");
    assert_eq!(result["requested"], json!(2_u64));
    let positions = positions_of(&result);
    let hall = positions.first().expect("sector 0 first");
    let back = positions.get(1).expect("sector 1 second");
    assert!(hall[0] < 256_i64, "the hall barrel stands west of the step");
    assert!(back[0] > 256_i64, "the back-room barrel stands east of it");
    let rows = reported_things(&client, "MAP01", &result).await;
    assert_eq!(
        rows.iter()
            .map(|row| row["sector"].as_u64().expect("a sector"))
            .collect::<Vec<u64>>(),
        vec![0_u64, 1],
        "each row answers its own sector"
    );
}

#[tokio::test]
async fn place_things_near_a_circle_stays_inside_it() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    let result = call(
        &client,
        "place_things",
        json!({
            "thing": "Medikit",
            "near": {"x": 128_i64, "y": 128_i64, "radius": 100_i64},
            "count": 3_u64,
        }),
    )
    .await;
    assert_eq!(result["placed"], json!(3_u64), "the circle held three");
    expect_within(&positions_of(&result), [128_i64, 128_i64], 100_i64);
}

#[tokio::test]
async fn place_things_faces_the_player_start() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    let result = call(
        &client,
        "place_things",
        json!({
            "thing": 3001_u64,
            "sectors": [0_i64],
            "count": 2_u64,
            "angle": "face_player_start",
        }),
    )
    .await;
    expect_faces_the_start(&reported_things(&client, "MAP01", &result).await);
}

#[tokio::test]
async fn place_things_reports_the_shortfall_of_a_tiny_box() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    let result = call(
        &client,
        "place_things",
        json!({
            "thing": 3001_u64,
            "bbox": [60_i64, 60_i64, 100_i64, 100_i64],
            "count": 200_u64,
        }),
    )
    .await;
    assert_eq!(result["placed"], json!(1_u64), "one imp clears the box");
    assert_eq!(result["requested"], json!(200_u64), "the ask is honest");
    assert_eq!(
        result["warnings"][0].as_str().expect("a warning"),
        "the bounding box fits only 1 of 200 things",
        "the documented shortfall note: {}",
        result["warnings"]
    );
    assert_eq!(
        result["positions"],
        json!([[80_i64, 80_i64]]),
        "the centred point"
    );
}

#[tokio::test]
async fn place_things_refuses_the_bad_wires() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    refuse_places(
        &client,
        json!({"thing": "skul", "sectors": [0_i64], "count": 1_u64}),
        "invalid_argument",
        "ambiguous",
    )
    .await;
    refuse_places(
        &client,
        json!({"thing": "skul", "sectors": [0_i64], "count": 1_u64}),
        "invalid_argument",
        "Skull on Pole",
    )
    .await;
    refuse_places(
        &client,
        json!({"thing": "platypus", "sectors": [0_i64], "count": 1_u64}),
        "invalid_argument",
        "no thing named 'platypus'",
    )
    .await;
    refuse_places(
        &client,
        json!({"thing": 3001_u64, "sectors": [0_i64], "count": 2_u64, "per_sector": 2_u64}),
        "invalid_argument",
        "not both",
    )
    .await;
    refuse_places(
        &client,
        json!({"thing": 3001_u64, "sectors": [0_i64], "bbox": [0_i64, 0_i64, 8_i64, 8_i64], "count": 1_u64}),
        "invalid_argument",
        "exactly one of sectors",
    )
    .await;
    refuse_places(
        &client,
        json!({"thing": 3001_u64, "count": 1_u64}),
        "invalid_argument",
        "exactly one of sectors",
    )
    .await;
}

#[tokio::test]
async fn place_things_refuses_the_bad_counts_and_fields() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    refuse_places(
        &client,
        json!({"thing": 3001_u64, "sectors": [0_i64], "count": 0_u64}),
        "invalid_argument",
        "count 0 is out of range",
    )
    .await;
    refuse_places(
        &client,
        json!({"thing": 3001_u64, "sectors": [0_i64], "count": 300_u64}),
        "invalid_argument",
        "count 300 is out of range",
    )
    .await;
    refuse_places(
        &client,
        json!({"thing": 3001_u64, "sectors": [99_i64], "count": 1_u64}),
        "not_found",
        "sector 99",
    )
    .await;
    refuse_places(
        &client,
        json!({"thing": 3001_u64, "sectors": [0_i64], "count": 1_u64, "tid": 7_i64}),
        "invalid_argument",
        "Doom-format",
    )
    .await;
    refuse_places(
        &client,
        json!({"thing": 3001_u64, "sectors": [0_i64], "count": 1_u64, "angle": "spin"}),
        "invalid_argument",
        "face_player_start, face_center, random_seeded",
    )
    .await;
    refuse_places(
        &client,
        json!({"thing": 3001_u64, "sectors": [0_i64], "count": 1_u64, "skills": ["nightmare"]}),
        "invalid_argument",
        "\"easy\", \"medium\", \"hard\"",
    )
    .await;
}

#[tokio::test]
async fn place_things_dry_run_writes_nothing() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    let result = call(
        &client,
        "place_things",
        json!({"thing": "imp", "sectors": [0_i64], "count": 4_u64, "dry_run": true}),
    )
    .await;
    assert_eq!(result["placed"], json!(4_u64), "the dry run computes four");
    assert_eq!(result["dry_run"], json!(true));
    assert_eq!(result["positions"].as_array().expect("positions").len(), 4);
    let (code, message, _) = call_err(
        &client,
        "assemble_map",
        json!({"map": "MAP01", "scope": "working"}),
    )
    .await;
    assert_eq!(
        code, "not_found",
        "the working archive stayed empty: {message}"
    );
}

#[tokio::test]
async fn place_things_undoes_as_one_step() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    call(
        &client,
        "place_things",
        json!({"thing": "imp", "sectors": [0_i64, 1_i64], "per_sector": 2_u64}),
    )
    .await;
    let rows = call(
        &client,
        "assemble_map",
        json!({"map": "MAP01", "scope": "working", "include": ["things"]}),
    )
    .await;
    assert_eq!(
        rows["things"].as_array().expect("things").len(),
        6_usize,
        "four imps joined the fixture's two"
    );
    let undone = call(&client, "undo", json!({})).await;
    assert_eq!(
        undone["undone"],
        json!(["place things"]),
        "the whole call rolls back together"
    );
    let merged = call(
        &client,
        "assemble_map",
        json!({"map": "MAP01", "scope": "merged", "include": ["things"]}),
    )
    .await;
    assert_eq!(
        merged["things"].as_array().expect("things").len(),
        2_usize,
        "the layer was never written"
    );
}

#[tokio::test]
async fn place_things_sets_skills_ambush_and_the_seeded_facing() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = features_loaded(dir.path()).await;
    let request = json!({
        "thing": 3001_u64,
        "sectors": [0_i64],
        "count": 4_u64,
        "skills": ["hard"],
        "ambush": true,
        "angle": "random_seeded",
        "seed": 7_u64,
    });
    let result = call(&client, "place_things", request.clone()).await;
    assert_eq!(result["placed"], json!(4_u64));
    let rows = reported_things(&client, "MAP01", &result).await;
    expect_ambushed_hard(&rows);
    let undone = call(&client, "undo", json!({"steps": 1_usize})).await;
    assert_eq!(undone["undone"], json!(["place things"]));
    let again = call(&client, "place_things", request).await;
    let second = reported_things(&client, "MAP01", &again).await;
    let first_angles: Vec<i64> = rows
        .iter()
        .map(|row| row["angle"].as_i64().expect("an angle"))
        .collect();
    let second_angles: Vec<i64> = second
        .iter()
        .map(|row| row["angle"].as_i64().expect("an angle"))
        .collect();
    assert_eq!(
        first_angles, second_angles,
        "the same seed replays the same angles"
    );
}

#[tokio::test]
async fn place_things_dresses_hexen_script_fields() {
    let dir = tempfile::tempdir().expect("temp dir");
    let client = formats_loaded(dir.path()).await;
    let result = call(
        &client,
        "place_things",
        json!({
            "map": "MAP04",
            "thing": 3001_u64,
            "sectors": [0_i64],
            "count": 1_u64,
            "tid": 5_i64,
            "special": {"number": 10_i64, "args": [1_i64, 2_i64, 3_i64, 4_i64, 5_i64]},
        }),
    )
    .await;
    assert_eq!(
        result["placed"],
        json!(1_u64),
        "the Hexen map takes the call"
    );
    let rows = reported_things(&client, "MAP04", &result).await;
    let row = rows.first().expect("one placed thing");
    assert_eq!(row["tid"], json!(5_i64), "the tid copied");
    assert_eq!(row["special"], json!(10_i64), "the special copied");
    assert_eq!(
        row["args"],
        json!([1_i64, 2_i64, 3_i64, 4_i64, 5_i64]),
        "the args copied"
    );
}
