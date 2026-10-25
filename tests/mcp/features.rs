//! The `features` tool group as seen by a real MCP client: the room-scale
//! additions `add_door`, `add_window`, `add_switch`, `add_exit`,
//! `mark_secret`, `set_light_effect`, `add_stairs`, `add_lift` and
//! `add_teleporter`, and the `place_things` spread, each driven through a
//! live session and checked against the working `assemble_map` rows and
//! `validate_map` health they leave behind.
//!
//! The helpers every topic drives the server through live here: the three
//! fixture loaders, the working-row readers (`working_linedef(s)`,
//! `working_sector(s)`, `reported_lines`, `index_list`, `note_mentions`,
//! `flag_names`), the `expect_rooms` sector check, the shared `refuse_with`,
//! the `validate_map` gate `validate_clean`, the asset-server
//! `rendered_image` and `count_pixels` pair and the placement measures
//! `dist_sq`, `positions_of` and `angle_diff`. The tests themselves are
//! split by topic into the `door`, `window`, `switch`, `exit`, `secret`,
//! `light`, `stairs`, `lift`, `teleporter` and `place` modules.

use std::path::Path;

use cyberdemon::gfx::codec::decode_image;
use cyberdemon::gfx::image::RgbaImage;
use rmcp::RoleClient;
use rmcp::service::RunningService;
use serde_json::{Value, json};

use crate::fixtures::{iwad_features, iwad_l_room, pwad_map_formats};
use crate::support::{call, call_err, connect, http_get};

/// A connected client for one test.
type Client = RunningService<RoleClient, ()>;

/// Open the features `IWAD` (textured two-room `MAP01`, the door
/// wardrobe and `FLAT10`) as the base layer.
async fn features_loaded(dir: &Path) -> Client {
    let client = connect().await;
    let base = iwad_features(dir);
    call(
        &client,
        "open_iwad",
        json!({"path": base.display().to_string()}),
    )
    .await;
    client
}

/// Open the map-format fixture (`UDMF` `MAP03`, binary `Hexen`
/// `MAP04`, both the two-room map) as the base layer.
async fn formats_loaded(dir: &Path) -> Client {
    let client = connect().await;
    let extra = pwad_map_formats(dir);
    call(
        &client,
        "open_iwad",
        json!({"path": extra.display().to_string()}),
    )
    .await;
    client
}

/// Open the single-room L fixture (no room behind any wall).
async fn l_room_loaded(dir: &Path) -> Client {
    let client = connect().await;
    let base = iwad_l_room(dir);
    call(
        &client,
        "open_iwad",
        json!({"path": base.display().to_string()}),
    )
    .await;
    client
}

/// The working linedefs for `map`, in order.
async fn working_linedefs(client: &Client, map: &str) -> Vec<Value> {
    let report = call(
        client,
        "assemble_map",
        json!({"map": map, "scope": "working", "include": ["linedefs"]}),
    )
    .await;
    report["linedefs"]
        .as_array()
        .expect("linedefs is an array")
        .clone()
}

/// The working sector row at `index` for `map`.
async fn working_sector(client: &Client, map: &str, index: usize) -> Value {
    working_sectors(client, map)
        .await
        .get(index)
        .cloned()
        .expect("the sector row exists")
}

/// The working sector rows for `map`, in order.
async fn working_sectors(client: &Client, map: &str) -> Vec<Value> {
    let report = call(
        client,
        "assemble_map",
        json!({"map": map, "scope": "working", "include": ["sectors"]}),
    )
    .await;
    report["sectors"]
        .as_array()
        .expect("sectors is an array")
        .clone()
}

/// Checks that both rooms of the two-room fixture carry `light` and
/// `special`: one call reaches every sector the list named.
fn expect_rooms(rooms: &[Value], light: i32, special: i32) {
    assert_eq!(rooms.len(), 2_usize, "the fixture's two rooms");
    for (index, row) in rooms.iter().enumerate() {
        assert_eq!(
            row["light"],
            json!(light),
            "sector {index} is lit at {light}"
        );
        assert_eq!(
            row["special"],
            json!(special),
            "sector {index} carries special {special}"
        );
    }
}

/// The working linedef rows at the indices of an `add_door` result's
/// `field` list.
async fn reported_lines(client: &Client, map: &str, result: &Value, field: &str) -> Vec<Value> {
    let indices = index_list(result, field);
    let all = working_linedefs(client, map).await;
    let rows = indices
        .iter()
        .filter_map(|index| all.get(*index).cloned())
        .collect::<Vec<Value>>();
    assert_eq!(rows.len(), indices.len(), "every {field} index exists");
    rows
}

/// Requires `validate_map` to report no unexpected error- or
/// warning-severity findings for the working `map`. Every placement
/// rewrites the map block without node data (its result says so with
/// `nodes_invalidated`), so `nodes_missing` is the expected noise this
/// helper filters out, exactly as `edit_sectors` does.
async fn validate_clean(client: &Client, map: &str) {
    const EXPECTED: [&str; 1] = ["nodes_missing"];
    let report = call(
        client,
        "validate_map",
        json!({"map": map, "scope": "working"}),
    )
    .await;
    let unexpected: Vec<String> = report["issues"]
        .as_array()
        .expect("an issue list")
        .iter()
        .filter(|issue| {
            let severity = issue["severity"].as_str().expect("a severity");
            let code = issue["code"].as_str().expect("a code");
            (severity == "error" || severity == "warning") && !EXPECTED.contains(&code)
        })
        .map(|issue| issue["message"].as_str().expect("a message").to_owned())
        .collect();
    assert!(
        unexpected.is_empty(),
        "no findings beyond the invalidated node lumps: {unexpected:#?}"
    );
}

/// Requires `add_door` to reject `args` with `invalid_argument`
/// naming `needle`.
async fn refuse_with(client: &Client, args: Value, needle: &str) {
    let note = args.to_string();
    let (code, message, _) = call_err(client, "add_door", args).await;
    assert_eq!(code, "invalid_argument", "refusal code for {note}");
    assert!(
        message.contains(needle),
        "the refusal for {note} must mention {needle:?}: {message}"
    );
}

/// Requires `add_door` to reject `args` naming an element the map does
/// not hold with the shared `not_found` index refusal, mentioning
/// `needle`.
async fn refuse_missing_with(client: &Client, args: Value, needle: &str) {
    let note = args.to_string();
    let (code, message, hint) = call_err(client, "add_door", args).await;
    assert_eq!(code, "not_found", "index-refusal code for {note}");
    assert!(
        message.contains(needle) && message.contains("does not exist"),
        "the refusal for {note} must mention {needle:?}: {message}"
    );
    assert_eq!(
        hint, "describe_map or find_elements returns indices",
        "the shared index hint names the readers"
    );
}

/// The indices of an `add_door` result list field.
fn index_list(result: &Value, field: &str) -> Vec<usize> {
    result[field]
        .as_array()
        .expect("the index list is an array")
        .iter()
        .map(|value| usize::try_from(value.as_u64().expect("indices are unsigned")).expect("fits"))
        .collect()
}

/// Whether one of the report's `notes` mentions `needle`.
fn note_mentions(result: &Value, needle: &str) -> bool {
    result["notes"]
        .as_array()
        .expect("notes is an array")
        .iter()
        .filter_map(Value::as_str)
        .any(|note| note.contains(needle))
}

/// The working linedef row at `index` for `map`.
async fn working_linedef(client: &Client, map: &str, index: usize) -> Value {
    working_linedefs(client, map)
        .await
        .get(index)
        .cloned()
        .expect("the linedef row exists")
}

/// The `flag_names` of a linedef row.
fn flag_names(row: &Value) -> Vec<&str> {
    row["flag_names"]
        .as_array()
        .expect("flag names is an array")
        .iter()
        .filter_map(Value::as_str)
        .collect()
}

/// Fetch and decode a render report's PNG asset.
async fn rendered_image(report: &Value) -> RgbaImage {
    let url = report["asset"]["url"].as_str().expect("an asset url");
    let (base, rest) = url.split_once("/assets/").expect("an asset url");
    let (status, body) = http_get(base, &format!("/assets/{rest}")).await;
    assert_eq!(status, 200, "the asset serves: {url}");
    decode_image(&body).expect("the view is a readable PNG")
}

/// The number of pixels of `image` that are exactly `color`.
fn count_pixels(image: &RgbaImage, color: [u8; 4]) -> usize {
    (0..image.height)
        .flat_map(|row| (0..image.width).map(move |col| (col, row)))
        .filter(|&(col, row)| image.get(col, row) == Some(color))
        .count()
}

/// The squared distance between two `[x, y]` positions.
fn dist_sq(a: [i64; 2], b: [i64; 2]) -> i64 {
    let dx = a[0].checked_sub(b[0]).expect("test coordinates fit");
    let dy = a[1].checked_sub(b[1]).expect("test coordinates fit");
    dx.checked_mul(dx)
        .expect("test range")
        .checked_add(dy.checked_mul(dy).expect("test range"))
        .expect("test range")
}

/// The `place_things` result's positions as `[x, y]` pairs.
fn positions_of(result: &Value) -> Vec<[i64; 2]> {
    result["positions"]
        .as_array()
        .expect("positions is an array")
        .iter()
        .map(|pair| {
            let edges = pair.as_array().expect("a position pair");
            [
                edges.first().expect("x").as_i64().expect("x is an integer"),
                edges.get(1).expect("y").as_i64().expect("y is an integer"),
            ]
        })
        .collect()
}

/// The smaller circular difference between two angles in degrees.
fn angle_diff(a: i64, b: i64) -> i64 {
    let raw = a.checked_sub(b).expect("small angles").rem_euclid(360);
    raw.min(360_i64.checked_sub(raw).expect("small angles"))
}

#[cfg(test)]
#[path = "features/door.rs"]
mod door;
#[cfg(test)]
#[path = "features/exit.rs"]
mod exit;
#[cfg(test)]
#[path = "features/lift.rs"]
mod lift;
#[cfg(test)]
#[path = "features/light.rs"]
mod light;
#[cfg(test)]
#[path = "features/place.rs"]
mod place;
#[cfg(test)]
#[path = "features/secret.rs"]
mod secret;
#[cfg(test)]
#[path = "features/stairs.rs"]
mod stairs;
#[cfg(test)]
#[path = "features/switch.rs"]
mod switch;
#[cfg(test)]
#[path = "features/teleporter.rs"]
mod teleporter;
#[cfg(test)]
#[path = "features/window.rs"]
mod window;
