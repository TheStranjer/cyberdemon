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
//!
//! The same file's `describe_map` reads both maps as sector rows: the
//! fixture answers the default call with its two rows in index order, the
//! step's `via` and `floor_delta`, things counted by definition name and
//! the labelled overview PNG published and fetchable; the blueprint with
//! every feature kind wears its `door` bands (`via: "door"` neighbours),
//! `lift`, `teleport_destination`, `secret` and `exit_room` roles. The
//! `sectors` filter dedupes, sorts and refuses a past-the-end index
//! `not_found` with the valid `0..N` range; `limit`/`offset` page the rows
//! with `next_offset`, `limit: 0` and `501` refuse `invalid_argument`, and
//! `render: false` answers `image` null.

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

        let (skill_code, skill_message, _) = call_err(
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
        let (map_code, _map_message, _) = call_err(
            &client,
            "check_playability",
            json!({"map": "MAP99", "skill": "hard"}),
        )
        .await;
        assert_eq!(map_code, "not_found", "an unknown map refuses");
    }

    /// The `skill/reference/plan.md` worked map: six rooms, two wired
    /// doors, an opening gap, a remote gate with its switch, a lift, a
    /// teleporter, a walk-over exit, a strobing and a secret room - every
    /// `describe_map` role the plan can install, with its card placed so
    /// the build warns nothing.
    fn blueprint_with_every_feature() -> Value {
        json!({
            "map": "MAP12",
            "wall_thickness": 0_i64,
            "rooms": [
                {"id": "start", "rect": {"x": 0_i64, "y": 0_i64, "w": 128_i64, "h": 128_i64}},
                {"id": "corridor", "rect": {"x": 128_i64, "y": 0_i64, "w": 128_i64, "h": 128_i64}},
                {"id": "cell", "rect": {"x": 256_i64, "y": 0_i64, "w": 128_i64, "h": 128_i64},
                 "secret": true},
                {"id": "ledge", "rect": {"x": 128_i64, "y": 128_i64, "w": 128_i64, "h": 128_i64},
                 "floor": 24_i64},
                {"id": "end", "rect": {"x": 128_i64, "y": 256_i64, "w": 128_i64, "h": 128_i64},
                 "floor": 24_i64},
                {"id": "foyer", "rect": {"x": -128_i64, "y": 0_i64, "w": 128_i64, "h": 128_i64}}
            ],
            "connections": [
                {"id": "blue_door", "from": "start", "to": "corridor", "kind": "door",
                 "style": "locked_blue"},
                {"id": "cell_door", "from": "corridor", "to": "cell", "kind": "door"},
                {"id": "gap", "from": "corridor", "to": "ledge", "kind": "opening"},
                {"id": "gate", "from": "ledge", "to": "end", "kind": "door", "style": "remote"},
                {"id": "home", "from": "start", "to": "foyer", "kind": "opening"}
            ],
            "features": [
                {"kind": "lift", "room": "ledge", "style": "use_wall", "low_floor": -40_i64},
                {"kind": "teleporter", "from_room": "corridor", "to_room": "ledge"},
                {"kind": "switch", "room": "ledge", "opens": "gate", "wall": "east"},
                {"kind": "exit", "room": "end", "style": "walk_over"}
            ],
            "things": [{"name": "Blue Keycard", "room": "start"}],
            "player_start": {"room": "start"}
        })
    }

    /// The `describe_map` row describing sector `sector` (this page must
    /// hold it).
    fn sector_row(report: &Value, sector: u64) -> &Value {
        report["sectors"]
            .as_array()
            .expect("a sector list")
            .iter()
            .find(|row| row["sector"] == json!(sector))
            .unwrap_or_else(|| panic!("a row for sector {sector}: {}", report["sectors"]))
    }

    /// Whether a `describe_map` row wears `role`.
    fn wears(row: &Value, role: &str) -> bool {
        row["roles"]
            .as_array()
            .expect("a role list")
            .iter()
            .any(|found| *found == json!(role))
    }

    /// The `describe_map` row of `report`'s sector `sector`, reaching its
    /// neighbour across `across`.
    fn neighbour_row(report: &Value, sector: u64, across: u64) -> &Value {
        sector_row(report, sector)["neighbours"]
            .as_array()
            .expect("a neighbour list")
            .iter()
            .find(|row| row["sector"] == json!(across))
            .unwrap_or_else(|| {
                panic!(
                    "sector {sector} neighbours {across}: {}",
                    sector_row(report, sector)
                )
            })
    }

    /// The one sector `build_map_from_plan` reported for room `room`.
    fn built_room(built: &Value, room: &str) -> u64 {
        built["rooms"][room]
            .as_array()
            .expect("a room sector list")
            .first()
            .expect("a plan room owns one sector")
            .as_u64()
            .expect("an index")
    }

    /// The band sector `build_map_from_plan` reported for connection
    /// `index` (zero-based plan order).
    fn built_band(built: &Value, index: usize) -> u64 {
        built["connections"]
            .as_array()
            .expect("a connection list")
            .get(index)
            .expect("a connection")["sectors"]
            .as_array()
            .expect("a sector list")
            .first()
            .expect("one band")
            .as_u64()
            .expect("an index")
    }

    /// The teleporter row's destination square, its second sector.
    fn teleport_landing(built: &Value) -> u64 {
        let row = built["features"]
            .as_array()
            .expect("a feature list")
            .iter()
            .find(|entry| entry["kind"] == json!("teleporter"))
            .expect("the teleporter row");
        row["sectors"]
            .as_array()
            .expect("a sector list")
            .last()
            .expect("the destination square")
            .as_u64()
            .expect("an index")
    }

    /// The blueprint built and the whole-map read over it, without an
    /// image: the shared setup of the filtered and paged tests.
    async fn built_and_described(client: &Client) -> (Value, Value) {
        let built = call(
            client,
            "build_map_from_plan",
            blueprint_with_every_feature(),
        )
        .await;
        let report = call(
            client,
            "describe_map",
            json!({"map": "MAP12", "render": false}),
        )
        .await;
        (built, report)
    }

    /// The `sector` indices of a `describe_map` report's rows, in order.
    fn reported_sectors(report: &Value) -> Vec<u64> {
        report["sectors"]
            .as_array()
            .expect("a sector list")
            .iter()
            .map(|row| row["sector"].as_u64().expect("a sector index"))
            .collect()
    }

    /// The headline of the fixture read: the map name, both rows in index
    /// order on one page, and the element counts beside the block.
    fn expect_fixture_headline(report: &Value) {
        assert_eq!(
            report["map"],
            json!("MAP01"),
            "the merged view's only map answers by default"
        );
        assert_eq!(
            reported_sectors(report),
            vec![0_u64, 1],
            "the hall then the raised room"
        );
        assert_eq!(report["next_offset"], Value::Null, "two rows fit one page");
        assert_eq!(
            report["summary"]["counts"],
            json!({
                "things": 2_u64, "vertices": 6_u64, "linedefs": 7_u64,
                "sidedefs": 8_u64, "sectors": 2_u64,
            }),
            "the summary counts match the fixture block"
        );
    }

    /// The step the two rooms share, read from either side.
    fn expect_fixture_neighbours(report: &Value) {
        assert_eq!(
            neighbour_row(report, 0, 1)["via"],
            json!("step_up"),
            "the raised room stands above the hall"
        );
        assert_eq!(
            neighbour_row(report, 0, 1)["floor_delta"],
            json!(32_i64),
            "the fixture's own 32-unit step"
        );
        assert_eq!(
            neighbour_row(report, 1, 0)["via"],
            json!("step_down"),
            "and the same line reads down from the ledge"
        );
        assert_eq!(
            neighbour_row(report, 1, 0)["floor_delta"],
            json!(-32_i64),
            "mirrored"
        );
    }

    /// What the fixture's two rooms hold: the start and the imp, each
    /// counted by definition name.
    fn expect_fixture_contents(report: &Value) {
        assert_eq!(
            sector_row(report, 0)["things"],
            json!([{"name": "Player 1 Start", "count": 1_u64}]),
            "the start counted by definition name"
        );
        assert_eq!(
            sector_row(report, 1)["things"],
            json!([{"name": "Imp", "count": 1_u64}]),
            "and the imp"
        );
        assert_eq!(
            sector_row(report, 1)["tag"],
            json!(1_i64),
            "the raised room's stored tag"
        );
    }

    /// The default `render` published a labelled overview that serves as
    /// a PNG under the map's name.
    async fn expect_served_overview(report: &Value) {
        assert_eq!(
            report["image"]["mime"],
            json!("image/png"),
            "the labelled overview rides along by default"
        );
        assert!(
            report["image"]["filename"]
                .as_str()
                .expect("a filename")
                .starts_with("MAP01"),
            "carrying the map's name: {}",
            report["image"]
        );
        let url = report["image"]["url"].as_str().expect("an asset url");
        let (base, rest) = url.split_once("/assets/").expect("an asset url");
        let (status, body) = http_get(base, &format!("/assets/{rest}")).await;
        assert_eq!(status, 200, "the overview serves: {url}");
        let png_head: [u8; 4] = body[..4].try_into().expect("a four-byte head");
        assert_eq!(png_head, [0x89_u8, b'P', b'N', b'G'], "a PNG on the wire");
    }

    #[tokio::test]
    async fn describe_map_reads_the_two_room_fixture_as_it_stands() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = features_loaded(dir.path()).await;
        let report = call(&client, "describe_map", json!({})).await;
        expect_fixture_headline(&report);
        expect_fixture_neighbours(&report);
        expect_fixture_contents(&report);
        expect_served_overview(&report).await;
    }

    /// The two wired door bands read as doors, and their rooms reach
    /// them through a `via: "door"` neighbour.
    fn expect_door_bands(built: &Value, report: &Value) {
        let blue_band = built_band(built, 0);
        let cell_band = built_band(built, 1);
        let start = built_room(built, "start");
        let corridor = built_room(built, "corridor");
        assert!(
            wears(sector_row(report, blue_band), "door")
                && wears(sector_row(report, cell_band), "door"),
            "the wired bands are doors: {}",
            report["sectors"]
        );
        assert_eq!(
            neighbour_row(report, start, blue_band)["via"],
            json!("door"),
            "the start reaches its band through a door"
        );
        assert_eq!(
            neighbour_row(report, corridor, blue_band)["via"],
            json!("door"),
            "and so does the corridor"
        );
        assert_eq!(
            neighbour_row(report, corridor, cell_band)["via"],
            json!("door"),
            "the cell's band door opens on the corridor"
        );
    }

    /// The lift, teleport destination, secret and exit roles, one per
    /// room the blueprint wired them into.
    fn expect_feature_roles(built: &Value, report: &Value) {
        let ledge = built_room(built, "ledge");
        let end = built_room(built, "end");
        let cell = built_room(built, "cell");
        let landing = teleport_landing(built);
        assert!(
            wears(sector_row(report, ledge), "lift"),
            "the ledge rides as a lift: {}",
            sector_row(report, ledge)["roles"]
        );
        assert!(
            wears(sector_row(report, landing), "teleport_destination"),
            "the teleporter lands on its square: {}",
            sector_row(report, landing)["roles"]
        );
        assert!(
            wears(sector_row(report, cell), "secret"),
            "the hidden cell is a secret: {}",
            sector_row(report, cell)["roles"]
        );
        assert!(
            wears(sector_row(report, end), "exit_room"),
            "walking the end room's boundaries exits: {}",
            sector_row(report, end)["roles"]
        );
        assert!(
            report["summary"]["secret_sectors"]
                .as_array()
                .expect("a secret list")
                .contains(&json!(cell)),
            "and the summary lists it: {}",
            report["summary"]["secret_sectors"]
        );
    }

    #[tokio::test]
    async fn describe_map_reads_the_roles_a_blueprint_wired() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = features_loaded(dir.path()).await;
        let (built, report) = built_and_described(&client).await;
        assert_eq!(
            built["warnings"],
            json!([]),
            "the plan is complete: {built}"
        );
        expect_door_bands(&built, &report);
        expect_feature_roles(&built, &report);
        assert_eq!(
            report["image"],
            Value::Null,
            "render: false publishes nothing"
        );
    }

    /// The `sectors` filter: deduped and sorted rows, and the summary
    /// staying whole behind them.
    async fn expect_filtered(client: &Client, whole: &Value) {
        let filtered = call(
            client,
            "describe_map",
            json!({"map": "MAP12", "sectors": [2_u64, 0_u64, 2_u64], "render": false}),
        )
        .await;
        assert_eq!(
            reported_sectors(&filtered),
            vec![0_u64, 2],
            "deduplicated, ascending, and only the asked rows"
        );
        assert_eq!(
            filtered["summary"], whole["summary"],
            "the summary stays whole behind the filter"
        );
    }

    /// An index at the sector count refuses `not_found` with the range,
    /// the parameter and the map named.
    async fn expect_index_refused(client: &Client, count: u64) {
        let (code, message, hint) = call_err(
            client,
            "describe_map",
            json!({"map": "MAP12", "sectors": [count]}),
        )
        .await;
        assert_eq!(code, "not_found", "one past the last sector refuses");
        let last = count.checked_sub(1).expect("a sector count above one");
        assert!(
            message.contains(&format!("valid indices are 0..={last}"))
                && message.contains("`sectors`")
                && message.contains("MAP12"),
            "the message names the parameter, the map and the range: {message}"
        );
        assert_eq!(
            hint, "describe_map or find_elements returns indices",
            "the shared index hint names the readers"
        );
    }

    /// `limit` pages the rows with `next_offset`, and an offset past the
    /// end simply answers nothing.
    async fn expect_paging(client: &Client, count: u64) {
        let first = call(
            client,
            "describe_map",
            json!({"map": "MAP12", "limit": 1_u64, "render": false}),
        )
        .await;
        assert_eq!(reported_sectors(&first), vec![0_u64], "one row on the page");
        assert_eq!(first["next_offset"], json!(1_u64), "naming where to go on");
        let tail = count.checked_sub(1).expect("more sectors than one");
        let last = call(
            client,
            "describe_map",
            json!({"map": "MAP12", "offset": tail, "render": false}),
        )
        .await;
        assert_eq!(
            reported_sectors(&last),
            vec![tail],
            "the tail page holds the last row"
        );
        assert_eq!(last["next_offset"], Value::Null, "with nothing left behind");
        let past = call(
            client,
            "describe_map",
            json!({"map": "MAP12", "offset": count, "render": false}),
        )
        .await;
        assert_eq!(
            past["sectors"],
            json!([]),
            "an offset past the end is no error"
        );
        assert_eq!(past["next_offset"], Value::Null, "and it has no next page");
    }

    /// The two ends of the `limit` range, each refused by name.
    async fn expect_limits_refused(client: &Client) {
        let (zero_code, zero_message, _) = call_err(
            client,
            "describe_map",
            json!({"map": "MAP12", "limit": 0_u64}),
        )
        .await;
        assert_eq!(zero_code, "invalid_argument", "a zero page refuses");
        assert!(
            zero_message.contains("between 1 and 500"),
            "the message names the range: {zero_message}"
        );
        let (over_code, over_message, _) = call_err(
            client,
            "describe_map",
            json!({"map": "MAP12", "limit": 501_u64}),
        )
        .await;
        assert_eq!(over_code, "invalid_argument", "and one row over the cap");
        assert!(
            over_message.contains("between 1 and 500"),
            "again naming the range: {over_message}"
        );
    }

    #[tokio::test]
    async fn describe_map_filters_pages_and_refuses_as_documented() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = features_loaded(dir.path()).await;
        let (_, whole) = built_and_described(&client).await;
        let count = whole["summary"]["counts"]["sectors"]
            .as_u64()
            .expect("a sector count");
        assert!(count > 3, "the blueprint fills the map: {count} sectors");
        expect_filtered(&client, &whole).await;
        expect_index_refused(&client, count).await;
        expect_paging(&client, count).await;
        expect_limits_refused(&client).await;
    }

    #[tokio::test]
    async fn describe_map_refuses_the_way_the_other_map_tools_do() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = features_loaded(dir.path()).await;
        let (map_code, _map_message, _) =
            call_err(&client, "describe_map", json!({"map": "MAP99"})).await;
        assert_eq!(map_code, "not_found", "an unknown map refuses");

        let bare = connect().await;
        let (empty_code, empty_message, _) = call_err(&bare, "describe_map", json!({})).await;
        assert_eq!(empty_code, "invalid_argument", "the empty session refuses");
        assert!(
            empty_message.contains("no map given and no active map"),
            "the shared resolver's own words: {empty_message}"
        );
    }
}
