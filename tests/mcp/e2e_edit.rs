//! The plain-English edit flow on the fixture map, end to end: the request
//! "make the second room darker, put three imps in it and lock it with a
//! red key door" answered the way `guides/editing-existing-maps.md` says to
//! answer it. The second room is picked out by the `describe_map` row that
//! carries its light 192, never by a literal index; one `run_batch` lands
//! the light `bulk_edit`, the three `place_things` imps, the `add_door`
//! `locked_red` and the `add_thing` keycard as one atomic step, with the
//! first edit copying the map block into the working layer and the later
//! ones joining the copy. The reads after prove each fact: `describe_map`
//! shows the room light 128 and the card standing in the first room,
//! `find_elements` counts the placed imps inside the room (the fixture's
//! own imp counted too) and the `DR` red-key special 28 on exactly the two
//! lines `add_door` reported, and `check_playability` has the red card
//! reachable, every red lock opened, the door band out of
//! `doors_never_opened` and no stuck things - while the room's unchanged
//! 32-unit step keeps it held by a `step_too_high` blocker and the
//! exit-less map stays `ok: false`, exactly as the report says. One `undo`
//! takes the whole request back: `get_map_summary` is the pre-batch report
//! again and the room's light reads 192.

#[cfg(test)]
mod tests {
    use std::path::Path;

    use rmcp::RoleClient;
    use rmcp::service::RunningService;
    use serde_json::{Value, json};

    use crate::fixtures::iwad;
    use crate::support::{call, connect};

    /// A connected client for one test.
    type Client = RunningService<RoleClient, ()>;

    /// The light of the fixture map's second room, as stored.
    const SECOND_ROOM_LIGHT: i64 = 192;
    /// The light the batch's `-64` delta leaves in the second room.
    const DARKENED_LIGHT: i64 = 128;
    /// The light of the fixture map's start room.
    const START_ROOM_LIGHT: i64 = 160;
    /// The doomednum of the imp the flow distributes.
    const IMP: i64 = 3001;
    /// The doomednum of the red keycard the flow places.
    const RED_KEYCARD: i64 = 13;
    /// The vanilla `DR` open-door-red special the locked door carries.
    const DR_RED: i64 = 28;
    /// How far the second room's floor stands above the door band's.
    const ROOM_STEP: i64 = 32;
    /// The undo label the four-step batch gets by default.
    const BATCH_LABEL: &str = "batch: bulk_edit, place_things, add_door, add_thing";

    /// Open the `DOOM II` fixture (one two-room binary `Doom` `MAP01`) as
    /// the base layer: map edits land in the working layer by copy-on-write.
    async fn doom_loaded(dir: &Path) -> Client {
        let client = connect().await;
        let base = iwad(dir);
        call(
            &client,
            "open_iwad",
            json!({"path": base.display().to_string()}),
        )
        .await;
        client
    }

    /// The whole-map read, without the overview image.
    async fn describe(client: &Client) -> Value {
        call(
            client,
            "describe_map",
            json!({"map": "MAP01", "render": false}),
        )
        .await
    }

    /// The `describe_map` row of sector `index`.
    fn sector_row(described: &Value, index: u64) -> Value {
        described["sectors"]
            .as_array()
            .expect("sector rows")
            .iter()
            .find(|row| row["sector"] == json!(index))
            .cloned()
            .expect("the row of that sector")
    }

    /// The sector index of the first `describe_map` row whose light is
    /// `light`: how the flow turns "the second room" into a number.
    fn sector_with_light(described: &Value, light: i64) -> u64 {
        described["sectors"]
            .as_array()
            .expect("sector rows")
            .iter()
            .find(|row| row["light"] == json!(light))
            .expect("a row at that light")["sector"]
            .as_u64()
            .expect("a sector index")
    }

    /// What the simulated request ran into: the pre-batch reads and the
    /// batch's own report, kept for the assertions to index off.
    struct Requested {
        /// The `get_map_summary` report from before the batch.
        pre: Value,
        /// The second room's index, read off its light-192 row.
        second: u64,
        /// The index of the start room, read off its light-160 row.
        start: u64,
        /// The `run_batch` report.
        batch: Value,
    }

    /// One batch step's structured result, by index.
    fn step_result(batch: &Value, step: usize) -> Value {
        batch["steps"]
            .as_array()
            .expect("step outcomes")
            .get(step)
            .expect("that step ran")["result"]
            .clone()
    }

    /// Run the whole simulated request as one `run_batch`, after orienting
    /// on `describe_map` exactly like the guide's steps 1-3 ask.
    async fn run_the_request(client: &Client) -> Requested {
        let pre = call(client, "get_map_summary", json!({"map": "MAP01"})).await;
        let described = describe(client).await;
        let second = sector_with_light(&described, SECOND_ROOM_LIGHT);
        let start = sector_with_light(&described, START_ROOM_LIGHT);
        let steps = json!({"steps": [
            {"tool": "bulk_edit", "arguments": {
                "kind": "sectors",
                "within": {"sectors": [second]},
                "add": {"light": -64_i64},
            }},
            {"tool": "place_things", "arguments": {
                "thing": "imp",
                "sectors": [second],
                "count": 3_i64,
            }},
            {"tool": "add_door", "arguments": {
                "between": [start, second],
                "style": "locked_red",
            }},
            {"tool": "add_thing", "arguments": {
                "type": RED_KEYCARD,
                "x": 128_i64,
                "y": 64_i64,
            }},
        ]});
        let batch = call(client, "run_batch", steps).await;
        Requested {
            pre,
            second,
            start,
            batch,
        }
    }

    /// Checks the batch report: every step landed as one undo entry, the
    /// light delta previewed the chosen sector, the imps fitted without a
    /// shortfall, the first edit copied the block and the later ones joined
    /// the copy, and the door came back `locked_red` with its line pair.
    fn expect_batch(asked: &Requested) {
        let batch = &asked.batch;
        assert_eq!(batch["ok"], json!(true), "every step landed: {batch}");
        assert_eq!(batch["rolled_back"], json!(false), "nothing rolled back");
        assert_eq!(batch["undo_label"], json!(BATCH_LABEL), "one undo entry");
        let darken = step_result(batch, 0);
        assert_eq!(darken["matched"], json!(1_usize), "the one chosen sector");
        assert_eq!(darken["indices"], json!([asked.second]), "the right one");
        assert_eq!(
            darken["preview"][0]["after"]["light"],
            json!(DARKENED_LIGHT),
            "the 192 row minus the 64 the delta carries",
        );
        assert_eq!(
            darken["copied_to_working"],
            json!(true),
            "the batch's first edit copied the block",
        );
        let stock = step_result(batch, 1);
        assert_eq!(stock["placed"], json!(3_usize), "three imps stood in");
        assert_eq!(stock["requested"], json!(3_usize), "nothing short-fitted");
        assert_eq!(stock["warnings"], json!([]), "and the room took them all");
        assert_eq!(
            step_result(batch, 2)["copied_to_working"],
            json!(false),
            "the later steps joined the copy",
        );
        let door = step_result(batch, 2);
        assert_eq!(door["style"], json!("locked_red"), "the echoed style");
        assert_eq!(
            door["lines"].as_array().expect("the door line pair").len(),
            2_usize,
            "one use line per room",
        );
        assert_eq!(step_result(batch, 3)["thing"]["type"], json!(RED_KEYCARD));
    }

    /// Whether one `describe_map` `things` count row names `count` of `name`.
    fn counts_thing(row: &Value, name: &str, count: usize) -> bool {
        row["name"] == json!(name) && row["count"] == json!(count)
    }

    /// Checks the post-batch reads: the room darker and the card in the
    /// first room by `describe_map`, the placed imps inside the room and
    /// the `DR` red special on exactly the reported lines by `find_elements`.
    async fn expect_reads(client: &Client, asked: &Requested) {
        let described = describe(client).await;
        assert_eq!(
            sector_row(&described, asked.second)["light"],
            json!(DARKENED_LIGHT),
            "the second room reads darker",
        );
        assert!(
            sector_row(&described, asked.start)["things"]
                .as_array()
                .expect("thing counts")
                .iter()
                .any(|row| counts_thing(row, "Red Keycard", 1)),
            "the card stands in the first room: {}",
            sector_row(&described, asked.start)["things"],
        );
        let imps = call(
            client,
            "find_elements",
            json!({"kind": "things",
                   "where": [{"field": "type", "op": "eq", "value": IMP}],
                   "within": {"sectors": [asked.second]}}),
        )
        .await;
        let standing = imps["indices"].as_array().expect("imp indices");
        let placed = step_result(&asked.batch, 1)["things"]
            .as_array()
            .expect("placed indices")
            .clone();
        assert_eq!(
            placed
                .iter()
                .filter(|index| standing.contains(index))
                .count(),
            placed.len(),
            "every placed imp stands in the room",
        );
        assert_eq!(
            imps["count"],
            json!(4_usize),
            "the three placed plus the fixture's own imp make four",
        );
        let doors = call(
            client,
            "find_elements",
            json!({"kind": "linedefs",
                   "where": [{"field": "special", "op": "eq", "value": DR_RED}]}),
        )
        .await;
        assert_eq!(
            doors["count"],
            json!(2_usize),
            "the pair of room-facing lines",
        );
        assert_eq!(
            doors["indices"],
            step_result(&asked.batch, 2)["lines"],
            "exactly the lines add_door reported",
        );
    }

    /// Whether one `blockers` row is the fixture's own step holding the
    /// second room out of reach: the door opens, the 32-unit ledge does not.
    fn held_by_the_step(row: &Value, sector: u64) -> bool {
        row["sector"] == json!(sector)
            && row["reason"] == json!("step_too_high")
            && row["delta"] == json!(ROOM_STEP)
    }

    /// Checks `check_playability`'s answer: the walked-onto card, both red
    /// locks opened, no `locked` blocker and nothing never-opened or stuck,
    /// and the room held by its step alone - the exit-less map stays not-ok.
    async fn expect_walk(client: &Client, asked: &Requested) {
        let report = call(client, "check_playability", json!({"map": "MAP01"})).await;
        let card = step_result(&asked.batch, 3)["index"].clone();
        assert_eq!(
            report["keys"],
            json!([{"thing": card, "color": "red", "reachable": true}]),
            "the walk walked onto the card: {}",
            report["keys"],
        );
        assert!(
            !report["locks"].as_array().expect("a lock list").is_empty()
                && report["locks"]
                    .as_array()
                    .expect("a lock list")
                    .iter()
                    .all(|lock| lock["color"] == json!("red") && lock["opened"] == json!(true)),
            "both door lines read opened: {}",
            report["locks"],
        );
        assert_eq!(
            report["doors_never_opened"],
            json!([]),
            "the band opened, so no door stands as never opened",
        );
        assert_eq!(
            report["stuck_things"],
            json!([]),
            "every thing can stand: {}",
            report["stuck_things"],
        );
        assert_eq!(
            report["blockers"]
                .as_array()
                .expect("a blocker list")
                .iter()
                .filter(|row| row["reason"] == json!("locked"))
                .count(),
            0_usize,
            "no locked blocker survives the card: {}",
            report["blockers"],
        );
        expect_held_by_the_step(&report, asked.second);
    }

    /// The exit-less fixture stays `ok: false`, keeps exactly the second
    /// room unreachable and explains it with its own `step_too_high` row.
    fn expect_held_by_the_step(report: &Value, sector: u64) {
        assert_eq!(
            report["ok"],
            json!(false),
            "the exit-less map stays not-ok, as the report says",
        );
        assert_eq!(
            report["sectors"]["unreachable"],
            json!([sector]),
            "only the room behind the step stayed out of reach",
        );
        assert!(
            report["blockers"]
                .as_array()
                .expect("a blocker list")
                .iter()
                .any(|row| held_by_the_step(row, sector)),
            "the step is named as the hold: {}",
            report["blockers"],
        );
    }

    #[tokio::test]
    async fn the_darken_stock_and_lock_request_lands_as_one_batch() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let asked = run_the_request(&client).await;
        expect_batch(&asked);
        expect_reads(&client, &asked).await;
        expect_walk(&client, &asked).await;
    }

    #[tokio::test]
    async fn one_undo_takes_the_whole_request_back() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = doom_loaded(dir.path()).await;
        let asked = run_the_request(&client).await;
        assert_eq!(asked.batch["ok"], json!(true), "the request landed");
        let undone = call(&client, "undo", json!({})).await;
        assert_eq!(
            undone["undone"],
            json!([BATCH_LABEL]),
            "one entry named the whole batch",
        );
        assert_eq!(
            undone["remaining_undo"],
            json!(0_usize),
            "and it was the only one: {}",
            undone["remaining_undo"],
        );
        assert_eq!(undone["dirty"], json!(false), "back to the saved state");
        let restored = call(&client, "get_map_summary", json!({"map": "MAP01"})).await;
        assert_eq!(
            restored["counts"], asked.pre["counts"],
            "every element count is the pre-batch one",
        );
        assert_eq!(restored, asked.pre, "the whole summary came back");
        let described = describe(&client).await;
        assert_eq!(
            sector_row(&described, asked.second)["light"],
            json!(SECOND_ROOM_LIGHT),
            "the second room is bright again",
        );
    }
}
