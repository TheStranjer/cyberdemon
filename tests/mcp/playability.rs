//! The `playability` tool group as seen by a real MCP client:
//! `check_playability` answers the player's question over plan-built maps
//! and the two-room fixture. A playable three-room map with a blue key
//! door, a keycard and a `walk_over` exit comes back `ok` with every
//! sector reached and its lock opened; dropping the keycard turns the door
//! into a `locked` blocker with its `add_thing` hint and empties the key
//! list. A forty-unit ledge reports `step_too_high` with its delta until
//! `add_stairs` climbs it; an imp shoved four units from a wall reports
//! `in_wall` with the line until `edit_thing` moves it out; a map with the
//! player start deleted answers `ok: false`, `start: null` and the
//! `add_thing type 1` hint, and a map with no exit at all names
//! `add_exit`. `render: true` publishes a highlighted PNG asset, a bad
//! skill refuses `invalid_argument` and an unknown map `not_found`.

#[cfg(test)]
mod tests {
    use std::path::Path;

    use rmcp::RoleClient;
    use rmcp::service::RunningService;
    use serde_json::{Value, json};

    use crate::fixtures::iwad_features;
    use crate::support::{call, call_err, connect, http_get};

    /// A connected client for one test.
    type Client = RunningService<RoleClient, ()>;

    /// Open the features `IWAD` (textured two-room `MAP01`) as layer 0.
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

    /// Three rooms `a | b | c` joined by a blue-key door and an opening,
    /// with a `walk_over` exit on room `c` and, when `with_key`, a blue
    /// card lying in the start room.
    fn three_room_plan(map: &str, with_key: bool) -> Value {
        let things: Vec<Value> = if with_key {
            vec![json!({"type": 5_i32, "room": "a"})]
        } else {
            Vec::new()
        };
        json!({
            "map": map,
            "rooms": [
                {"id": "a", "rect": {"x": 0_i64, "y": 0_i64, "w": 192_i64, "h": 192_i64}},
                {"id": "b", "rect": {"x": 192_i64, "y": 0_i64, "w": 192_i64, "h": 192_i64}},
                {"id": "c", "rect": {"x": 384_i64, "y": 0_i64, "w": 192_i64, "h": 192_i64}},
            ],
            "connections": [
                {
                    "id": "d1",
                    "from": "a",
                    "to": "b",
                    "kind": "door",
                    "style": "locked_blue",
                    "width": 64_i64,
                    "at": [192_i64, 96_i64],
                },
                {
                    "id": "c1",
                    "from": "b",
                    "to": "c",
                    "kind": "opening",
                    "width": 64_i64,
                    "at": [384_i64, 96_i64],
                },
            ],
            "features": [
                {"kind": "exit", "room": "c", "style": "walk_over"},
            ],
            "things": things,
            "player_start": {"room": "a"}
        })
    }

    /// Build `request` and check the finished map's playability.
    async fn checked(client: &Client, request: Value, map: &str) -> Value {
        call(client, "build_map_from_plan", request).await;
        call(client, "check_playability", json!({"map": map})).await
    }

    /// The row naming `reason`, `None` when the blockers carry none.
    fn blocker<'report>(report: &'report Value, reason: &str) -> Option<&'report Value> {
        let rows = report["blockers"].as_array()?;
        rows.iter().find(|row| row["reason"] == json!(reason))
    }

    #[tokio::test]
    async fn a_played_out_plan_with_its_key_is_ok() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = features_loaded(dir.path()).await;
        let report = checked(&client, three_room_plan("MAP05", true), "MAP05").await;
        assert_eq!(report["ok"], json!(true), "the map plays: {report}");
        assert_eq!(
            report["sectors"]["unreachable"],
            json!([]),
            "every sector stands on the walked path"
        );
        assert_eq!(
            report["sectors"]["reachable"], report["sectors"]["total"],
            "counts agree"
        );
        assert!(
            report["exits"]
                .as_array()
                .expect("an exit list")
                .iter()
                .any(|exit| exit["reachable"] == json!(true)),
            "the walk_over exit works: {}",
            report["exits"]
        );
        assert!(
            !report["locks"].as_array().expect("a lock list").is_empty()
                && report["locks"]
                    .as_array()
                    .expect("a lock list")
                    .iter()
                    .all(|lock| lock["opened"] == json!(true)),
            "the blue door opened: {}",
            report["locks"]
        );
        assert_eq!(
            report["keys"]
                .as_array()
                .expect("a key list")
                .first()
                .and_then(|key| key["reachable"].as_bool()),
            Some(true),
            "the card was walked onto: {}",
            report["keys"]
        );
        assert_eq!(
            report["next_steps"],
            json!(["render_view from the player start to check the look"]),
            "the only step left is looking"
        );
        assert!(
            report["summary"]
                .as_str()
                .expect("a summary")
                .starts_with("Exit reachable."),
            "the summary leads with the answer: {}",
            report["summary"]
        );
    }

    #[tokio::test]
    async fn the_same_map_without_its_key_holds_at_the_door() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = features_loaded(dir.path()).await;
        let report = checked(&client, three_room_plan("MAP05", false), "MAP05").await;
        assert_eq!(report["ok"], json!(false), "the exit is behind the lock");
        assert_eq!(report["keys"], json!([]), "no key spawns anywhere");
        let locked = blocker(&report, "locked").expect("a locked blocker");
        assert_eq!(locked["color"], json!("blue"), "the door wants blue");
        assert_eq!(
            locked["hint"],
            json!("place a blue key in a reachable area (add_thing)"),
            "and the hint names the fix"
        );
        assert!(
            report["doors_never_opened"]
                .as_array()
                .expect("a door list")
                .iter()
                .any(|sector| sector == &locked["sector"]),
            "the band is also reported as never opened: {}",
            report["doors_never_opened"]
        );
        assert_eq!(
            report["summary"]
                .as_str()
                .expect("a summary")
                .split_whitespace()
                .next()
                .unwrap_or_default(),
            "No",
            "the exit sentence turns negative: {}",
            report["summary"]
        );
        assert!(
            report["next_steps"]
                .as_array()
                .expect("a step list")
                .iter()
                .any(|step| step
                    .as_str()
                    .is_some_and(|text| text.contains("place a blue key"))),
            "the first steps carry the lock's hint: {}",
            report["next_steps"]
        );
    }

    /// The plan behind [`a_forty_unit_ledge_blocks_until_the_stairs_arrive`]:
    /// a low room and a forty-unit-high one joined by one opening.
    fn ledge_plan() -> Value {
        json!({
            "map": "MAP05",
            "rooms": [
                {"id": "low", "rect": {"x": 0_i64, "y": 0_i64, "w": 256_i64, "h": 256_i64}},
                {
                    "id": "high",
                    "rect": {"x": 256_i64, "y": 0_i64, "w": 256_i64, "h": 256_i64},
                    "floor": 40_i64,
                    "ceiling": 168_i64,
                },
            ],
            "connections": [
                {"id": "c1", "from": "low", "to": "high", "kind": "opening", "width": 64_i64},
            ],
            "player_start": {"room": "low"}
        })
    }

    /// The unreachable sector indices of a report.
    fn unreachable(report: &Value) -> Vec<u64> {
        report["sectors"]["unreachable"]
            .as_array()
            .expect("an unreachable list")
            .iter()
            .map(|sector| sector.as_u64().expect("a sector index"))
            .collect()
    }

    #[tokio::test]
    async fn a_forty_unit_ledge_blocks_until_the_stairs_arrive() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = features_loaded(dir.path()).await;
        let built = call(&client, "build_map_from_plan", ledge_plan()).await;
        let high: Vec<u64> = built["rooms"]["high"]
            .as_array()
            .expect("the high room's sectors")
            .iter()
            .map(|sector| sector.as_u64().expect("a sector index"))
            .collect();
        let report = call(&client, "check_playability", json!({"map": "MAP05"})).await;
        let step = blocker(&report, "step_too_high").expect("a step blocker");
        assert_eq!(step["delta"], json!(40_i64), "the ledge is forty up");
        assert!(
            !unreachable(&report).is_empty(),
            "the high side stays out of reach: {report}"
        );
        assert!(
            step["hint"]
                .as_str()
                .expect("a hint")
                .contains("add_stairs"),
            "the hint names the stair tool: {step}"
        );
        call(
            &client,
            "add_stairs",
            json!({
                "map": "MAP05",
                "from": {"x": 128_i64, "y": 128_i64},
                "to": {"x": 384_i64, "y": 128_i64},
            }),
        )
        .await;
        let after = call(&client, "check_playability", json!({"map": "MAP05"})).await;
        let left = unreachable(&after);
        assert!(
            high.iter().all(|sector| !left.contains(sector)),
            "the climb reaches the high room: unreachable {left:?} of {high:?}"
        );
        assert!(
            blocker(&after, "step_too_high").is_none(),
            "and names no step left: {after}"
        );
        assert!(
            after["sectors"]["reachable"].as_u64() > report["sectors"]["reachable"].as_u64(),
            "the map widened: {} then {} now",
            report["sectors"]["reachable"],
            after["sectors"]["reachable"]
        );
    }

    #[tokio::test]
    async fn an_imp_in_a_wall_reports_the_line_until_it_moves() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = features_loaded(dir.path()).await;
        call(
            &client,
            "add_thing",
            json!({"map": "MAP01", "type": 3001_i32, "x": 4_i32, "y": 128_i32}),
        )
        .await;
        let report = call(&client, "check_playability", json!({"map": "MAP01"})).await;
        let jammed = report["stuck_things"]
            .as_array()
            .expect("a stuck list")
            .iter()
            .find(|row| row["reason"] == json!("in_wall"))
            .expect("the wall-shoved imp");
        assert_eq!(jammed["name"], json!("Imp"), "named by the definitions");
        assert_eq!(jammed["line"], json!(0_u64), "the west wall is line 0");
        assert_eq!(
            jammed["hint"],
            json!("move it at least 20 units away from line 0 (edit_thing)"),
            "the hint quotes the body radius"
        );
        let imp = jammed["thing"].as_u64().expect("a thing index");
        let lost = report["unreachable_things"]
            .as_array()
            .expect("an unreachable list")
            .iter()
            .find(|row| row["name"] == json!("Imp"))
            .expect("the raised-room imp stands out of reach");
        assert_eq!(lost["category"], json!("Monsters"), "category travels");
        assert_eq!(
            blocker(&report, "step_too_high").map(|row| row["delta"].clone()),
            Some(json!(32_i64)),
            "the fixture's own 32-unit step is the blocker: {report}"
        );
        call(
            &client,
            "edit_thing",
            json!({"map": "MAP01", "index": imp, "x": 20_i32}),
        )
        .await;
        let after = call(&client, "check_playability", json!({"map": "MAP01"})).await;
        assert!(
            !after["stuck_things"]
                .as_array()
                .expect("a stuck list")
                .iter()
                .any(|row| row["reason"] == json!("in_wall")),
            "the moved imp is nobody's complaint again: {}",
            after["stuck_things"]
        );
    }

    #[tokio::test]
    async fn missing_starts_and_missing_exits_answer_without_error() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = features_loaded(dir.path()).await;
        let build = json!({
            "map": "MAP05",
            "rooms": [
                {"id": "a", "rect": {"x": 0_i64, "y": 0_i64, "w": 256_i64, "h": 128_i64}},
            ],
            "connections": [],
            "player_start": {"room": "a"}
        });
        let report = checked(&client, build, "MAP05").await;
        assert_eq!(report["ok"], json!(false), "a room is not a level");
        assert!(
            report["start"].is_object(),
            "the start stands: {}",
            report["start"]
        );
        assert!(
            report["summary"]
                .as_str()
                .expect("a summary")
                .contains("The map has no exit."),
            "one room with no way out says so: {}",
            report["summary"]
        );
        assert_eq!(
            report["sectors"]["unreachable"],
            json!([]),
            "the single room is fully reached: {report}"
        );
        assert!(
            report["next_steps"]
                .as_array()
                .expect("a step list")
                .iter()
                .any(|step| step.as_str().is_some_and(|text| text.contains("add_exit"))),
            "the exit hint leads: {}",
            report["next_steps"]
        );
        let start = report["start"]["thing"].as_u64().expect("a thing index");
        call(
            &client,
            "remove_thing",
            json!({"map": "MAP05", "index": start}),
        )
        .await;
        let alone = call(&client, "check_playability", json!({"map": "MAP05"})).await;
        assert_eq!(alone["ok"], json!(false), "and now nobody spawns");
        assert_eq!(alone["start"], json!(null), "start reads null");
        assert_eq!(alone["sectors"]["reachable"], json!(0_u64));
        assert!(
            alone["next_steps"]
                .as_array()
                .expect("a step list")
                .first()
                .and_then(Value::as_str)
                .is_some_and(|text| text.contains("add_thing type 1")),
            "the first step asks for a start: {}",
            alone["next_steps"]
        );
    }

    #[tokio::test]
    async fn a_highlighted_overview_publishes_and_the_wired_errors_refuse() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = features_loaded(dir.path()).await;
        call(
            &client,
            "add_thing",
            json!({"map": "MAP01", "type": 3001_i32, "x": 4_i32, "y": 128_i32}),
        )
        .await;
        let report = call(
            &client,
            "check_playability",
            json!({"map": "MAP01", "render": true}),
        )
        .await;
        assert_eq!(report["render"]["mime"], json!("image/png"));
        assert!(
            report["render"]["filename"]
                .as_str()
                .expect("a filename")
                .starts_with("MAP01"),
            "the overview carries the map's name: {}",
            report["render"]
        );
        let url = report["render"]["url"].as_str().expect("an asset url");
        let (base, rest) = url.split_once("/assets/").expect("an asset url");
        let (status, body) = http_get(base, &format!("/assets/{rest}")).await;
        assert_eq!(status, 200, "the overview serves: {url}");
        let png_head: [u8; 4] = body[..4].try_into().expect("a four-byte head");
        assert_eq!(png_head, [0x89_u8, b'P', b'N', b'G'], "a PNG on the wire");
        let plain = call(&client, "check_playability", json!({"map": "MAP01"})).await;
        assert_eq!(
            plain["render"],
            Value::Null,
            "without the flag the field reads null"
        );

        let (skill_code, skill_message) = call_err(
            &client,
            "check_playability",
            json!({"map": "MAP01", "skill": "nightmare"}),
        )
        .await;
        assert_eq!(skill_code, "invalid_argument", "a bad skill refuses");
        assert!(
            skill_message.contains("\"medium\""),
            "and names the choices: {skill_message}"
        );
        let (map_code, _map_message) = call_err(
            &client,
            "check_playability",
            json!({"map": "MAP99", "skill": "hard"}),
        )
        .await;
        assert_eq!(map_code, "not_found", "an unknown map refuses");
    }
}
