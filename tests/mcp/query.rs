//! The `query` tool group as seen by a real MCP client: `find_elements`
//! over the two-room fixture, its ops and derived fields, the `within`
//! bounds beside an `add_thing`, the compact rows, the paging and the
//! `invalid_argument`/`not_found` refusals, the CD-108 bulk swaps —
//! `replace_texture` and `replace_flat` across the active map, every map
//! or a listed set, the doomednum-or-name thing swap
//! `replace_thing_type` with its name resolution, target check and
//! `allow_unknown` escape, all with their copy-on-write reports,
//! previews and one-step undo — and the `bulk_edit` tool: query-selected
//! set/add/flag edits on sectors, linedefs, things and vertices, the
//! never-clamped range refusal, the `max` guard, the geometry rule for
//! vertex moves, thing-type name resolution and the one-step undo.

#[cfg(test)]
mod tests {
    use std::path::Path;

    use rmcp::RoleClient;
    use rmcp::service::RunningService;
    use serde_json::{Value, json};

    use crate::fixtures::{iwad, pwad};
    use crate::support::{call, call_err, connect};

    /// A connected client for one test.
    type Client = RunningService<RoleClient, ()>;

    /// Load the two-room `IWAD` fixture as layer 0.
    async fn plain_iwad(dir: &Path) -> Client {
        let client = connect().await;
        let base = iwad(dir);
        call(
            &client,
            "open_iwad",
            json!({ "path": base.display().to_string() }),
        )
        .await;
        client
    }

    /// Run `find_elements` with these query parts over the fixture map.
    async fn find(client: &Client, args: Value) -> Value {
        let mut full = args;
        let Some(map) = full.as_object_mut() else {
            panic!("query arguments are an object");
        };
        map.entry("map").or_insert_with(|| json!("MAP01"));
        call(client, "find_elements", full).await
    }

    /// The `indices` list of a report, as plain integers.
    fn indices(report: &Value) -> Vec<u64> {
        report["indices"]
            .as_array()
            .expect("an index list")
            .iter()
            .map(|index| index.as_u64().expect("unsigned indices"))
            .collect()
    }

    /// Assert a query fails with `want_code` and a message carrying `want_text`.
    async fn refuse(client: &Client, args: Value, want_code: &str, want_text: &str) {
        let described = args.to_string();
        let (code, message) = call_err(client, "find_elements", args).await;
        assert_eq!(code, want_code, "error code for {described}: {message}");
        assert!(
            message.contains(want_text),
            "'{want_text}' missing from: {message}"
        );
    }

    /// One `{field, op, value}` filter as wire JSON.
    fn one(field: &str, op: &str, value: &Value) -> Value {
        json!([{ "field": field, "op": op, "value": value }])
    }

    #[tokio::test]
    async fn sectors_filter_by_light_and_the_report_carries_rows() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = plain_iwad(dir.path()).await;
        let report = find(
            &client,
            json!({ "kind": "sectors", "where": one("light", "gt", &json!(170_i64)) }),
        )
        .await;
        assert_eq!(report["kind"], json!("sectors"), "the kind travels");
        assert_eq!(report["count"], json!(1_i64), "one bright room");
        assert_eq!(indices(&report), vec![1_u64], "sector 1 at light 192");
        assert_eq!(report["rows"][0]["light"], json!(192_i64), "the row light");
        assert_eq!(
            report["rows"][0]["floor"],
            json!(32_i64),
            "and the raised floor"
        );
        assert_eq!(
            report["rows"][0]["ceiling_flat"],
            json!("FLOOR1"),
            "and the flat"
        );
        assert_eq!(report["next_offset"], Value::Null, "one page took all");
    }

    #[tokio::test]
    async fn an_unfiltered_query_keeps_every_sector() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = plain_iwad(dir.path()).await;
        let all = find(&client, json!({ "kind": "sectors" })).await;
        assert_eq!(all["count"], json!(2_i64), "no filters keeps every sector");
        assert_eq!(indices(&all), vec![0_u64, 1], "ascending");
    }

    #[tokio::test]
    async fn derived_linedef_fields_find_the_shared_step_line() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = plain_iwad(dir.path()).await;
        let two_sided = find(
            &client,
            json!({ "kind": "linedefs", "where": one("two_sided", "eq", &json!(true)) }),
        )
        .await;
        assert_eq!(indices(&two_sided), vec![2_u64], "the step line is shared");
        assert_eq!(
            two_sided["rows"][0]["v1"],
            json!(4_i64),
            "from the north vertex"
        );
        assert_eq!(
            two_sided["rows"][0]["v2"],
            json!(1_i64),
            "to the south vertex"
        );
        assert_eq!(
            two_sided["rows"][0]["front_sector"],
            json!(0_i64),
            "fronting the hall"
        );
        assert_eq!(
            two_sided["rows"][0]["back_sector"],
            json!(1_i64),
            "over the step"
        );
        let step = find(
            &client,
            json!({ "kind": "linedefs", "where": one("front_lower", "eq", &json!("STEP1")) }),
        )
        .await;
        assert_eq!(
            indices(&step),
            vec![2_u64],
            "the same line through its face"
        );
    }

    #[tokio::test]
    async fn linedef_flags_and_length_measure_the_walls() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = plain_iwad(dir.path()).await;
        let blocking = find(
            &client,
            json!({ "kind": "linedefs", "where": one("flags", "has", &json!("blocking")) }),
        )
        .await;
        assert_eq!(
            indices(&blocking),
            vec![0_u64, 1, 3, 4, 5, 6],
            "every wall but the step line blocks"
        );
        let long = find(
            &client,
            json!({ "kind": "linedefs", "where": one("length", "eq", &json!(256_i64)) }),
        )
        .await;
        assert_eq!(long["count"], json!(7_i64), "the box walls are all 256");
    }

    #[tokio::test]
    async fn things_filter_by_type_and_name_glob() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = plain_iwad(dir.path()).await;
        let start = find(
            &client,
            json!({ "kind": "things", "where": one("type", "eq", &json!(1_i64)) }),
        )
        .await;
        assert_eq!(indices(&start), vec![0_u64], "the player start");
        assert_eq!(start["rows"][0]["name"], json!("Player 1 Start"), "named");
        assert_eq!(start["rows"][0]["x"], json!(64_i64), "at 64, 128");
        let imps = find(
            &client,
            json!({ "kind": "things", "where": one("name", "glob", &json!("imp*")) }),
        )
        .await;
        assert_eq!(
            indices(&imps),
            vec![1_u64],
            "the fixture imp, lowercase pattern"
        );
    }

    /// Add a second imp inside sector 1 and report the client's row index.
    async fn add_imp_in_room(client: &Client) {
        call(
            client,
            "add_thing",
            json!({ "map": "MAP01", "type": 3001_i64, "x": 384_i64, "y": 64_i64 }),
        )
        .await;
    }

    #[tokio::test]
    async fn a_newly_placed_imp_joins_the_glob() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = plain_iwad(dir.path()).await;
        add_imp_in_room(&client).await;
        let imps = find(
            &client,
            json!({ "kind": "things", "where": one("name", "glob", &json!("imp*")) }),
        )
        .await;
        assert_eq!(indices(&imps), vec![1_u64, 2], "the added imp joins");
        let ambush = find(
            &client,
            json!({ "kind": "things", "where": one("flags", "has", &json!("easy")) }),
        )
        .await;
        assert_eq!(
            ambush["count"],
            json!(3_i64),
            "every spawn carries a skill bit"
        );
    }

    /// A `name glob "imp*"` query bounded by these sectors.
    fn imp_query(sectors: &Value) -> Value {
        json!({
            "kind": "things",
            "where": one("name", "glob", &json!("imp*")),
            "within": { "sectors": sectors },
        })
    }

    #[tokio::test]
    async fn imps_are_bounded_by_their_sectors() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = plain_iwad(dir.path()).await;
        add_imp_in_room(&client).await;
        let in_room = find(&client, imp_query(&json!([1_i64]))).await;
        assert_eq!(
            indices(&in_room),
            vec![1_u64, 2],
            "both imps stand in sector 1"
        );
        let in_hall = find(&client, imp_query(&json!([0_i64]))).await;
        assert_eq!(in_hall["count"], json!(0_i64), "none in the hall");
        assert!(
            in_hall["indices"].as_array().expect("a list").is_empty(),
            "an empty page, not an error"
        );
    }

    #[tokio::test]
    async fn sidedefs_filter_by_their_textures() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = plain_iwad(dir.path()).await;
        let middle = find(
            &client,
            json!({ "kind": "sidedefs", "where": one("middle", "eq", &json!("WALL1")) }),
        )
        .await;
        assert_eq!(
            indices(&middle),
            vec![0_u64, 1, 4, 5, 6, 7],
            "the WALL1-dressed faces"
        );
        assert_eq!(
            middle["rows"][0]["sector"],
            json!(0_i64),
            "the sidedef shape"
        );
        let lower = find(
            &client,
            json!({ "kind": "sidedefs", "where": one("lower", "eq", &json!("STEP1")) }),
        )
        .await;
        assert_eq!(indices(&lower), vec![2_u64], "the step's lower face");
    }

    #[tokio::test]
    async fn vertices_box_in_and_page_in_orders() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = plain_iwad(dir.path()).await;
        let boxed = find(
            &client,
            json!({ "kind": "vertices", "within": { "bbox": [0_i64, 0_i64, 256_i64, 256_i64] } }),
        )
        .await;
        assert_eq!(
            indices(&boxed),
            vec![0_u64, 1, 4, 5],
            "the hall box corners"
        );
        assert_eq!(
            boxed["rows"][1],
            json!({"index": 1_i64, "x": 256_i64, "y": 0_i64}),
            "the vertex shape"
        );
        let page1 = find(&client, json!({ "kind": "vertices", "limit": 2_i64 })).await;
        assert_eq!(indices(&page1), vec![0_u64, 1], "the first page of two");
        assert_eq!(page1["count"], json!(6_i64), "six vertices in total");
        assert_eq!(
            page1["next_offset"],
            json!(2_i64),
            "the rest continues at 2"
        );
    }

    #[tokio::test]
    async fn paging_walks_to_the_end_and_past_it() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = plain_iwad(dir.path()).await;
        let last = find(
            &client,
            json!({ "kind": "vertices", "limit": 2_i64, "offset": 4_i64 }),
        )
        .await;
        assert_eq!(indices(&last), vec![4_u64, 5], "the final page");
        assert_eq!(last["next_offset"], Value::Null, "nothing left behind");
        let past = find(&client, json!({ "kind": "vertices", "offset": 99_i64 })).await;
        assert_eq!(past["count"], json!(6_i64), "the total stays the truth");
        assert!(
            past["indices"].as_array().expect("a list").is_empty(),
            "an offset past the end is empty, not an error"
        );
    }

    #[tokio::test]
    async fn an_unknown_map_is_not_found() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = plain_iwad(dir.path()).await;
        refuse(
            &client,
            json!({ "map": "MAP99", "kind": "sectors" }),
            "not_found",
            "MAP99",
        )
        .await;
    }

    #[tokio::test]
    async fn bad_fields_ops_and_names_list_the_valid_ones() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = plain_iwad(dir.path()).await;
        refuse(
            &client,
            json!({ "kind": "sectors", "where": one("aro", "eq", &json!(1_i64)) }),
            "invalid_argument",
            "valid fields are index, floor",
        )
        .await;
        refuse(
            &client,
            json!({ "kind": "sectors", "where": one("aro", "eq", &json!(1_i64)) }),
            "invalid_argument",
            "floor_flat",
        )
        .await;
        refuse(
            &client,
            json!({ "kind": "sectors", "where": one("floor_flat", "gt", &json!("A")) }),
            "invalid_argument",
            "does not apply",
        )
        .await;
        refuse(
            &client,
            json!({ "kind": "linedefs", "where": one("flags", "has", &json!("impassable")) }),
            "invalid_argument",
            "blocking",
        )
        .await;
        refuse(
            &client,
            json!({ "kind": "sectors", "where": one("light", "eq", &json!("x")) }),
            "invalid_argument",
            "needs a number",
        )
        .await;
    }

    #[tokio::test]
    async fn limit_bounds_are_refused_before_anything_is_read() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = plain_iwad(dir.path()).await;
        refuse(
            &client,
            json!({ "kind": "things", "limit": 0_i64 }),
            "invalid_argument",
            "between 1 and 1000",
        )
        .await;
        refuse(
            &client,
            json!({ "kind": "things", "limit": 1001_i64 }),
            "invalid_argument",
            "between 1 and 1000",
        )
        .await;
    }

    /// Load the `IWAD` fixture and merge the `PWAD` fixture (a layer-only
    /// `MAP02` block, an overridden `FLOOR1` and a `DEHACKED` global) in.
    async fn iwad_plus_pwad(dir: &Path) -> Client {
        let client = connect().await;
        let base = iwad(dir);
        let extra = pwad(dir);
        call(
            &client,
            "open_iwad",
            json!({ "path": base.display().to_string() }),
        )
        .await;
        call(
            &client,
            "merge_wad",
            json!({ "path": extra.display().to_string() }),
        )
        .await;
        client
    }

    /// The six `WALL1`-dressed middles of the two-room fixture.
    const FIXTURE_WALL1_MIDDLES: [u64; 6] = [0_u64, 1, 4, 5, 6, 7];

    /// A `{kind: "sidedefs", middle: NAME}` query over one map.
    fn middles_wearing(map: &str, name: &str) -> Value {
        json!({
            "map": map,
            "kind": "sidedefs",
            "where": one("middle", "eq", &json!(name)),
        })
    }

    #[tokio::test]
    async fn replace_texture_swaps_the_fixture_wall_across_one_undoable_step() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = plain_iwad(dir.path()).await;
        let report = call(
            &client,
            "replace_texture",
            json!({ "from": "WALL1", "to": "STEP1" }),
        )
        .await;
        assert_eq!(
            report["maps"],
            json!({ "MAP01": 6_i64 }),
            "the fixture WALL1 sits on six middles: {report}"
        );
        assert_eq!(report["total"], json!(6_i64), "the sum over the maps");
        assert_eq!(
            report["copied_maps"],
            json!(["MAP01"]),
            "the map came from the IWAD layer"
        );
        assert_eq!(report["dry_run"], json!(false), "the swap wrote");
        assert!(
            report["next_steps"]
                .as_array()
                .is_some_and(|steps| !steps.is_empty()),
            "the report names what to do next: {report}"
        );
        let after = find(&client, middles_wearing("MAP01", "STEP1")).await;
        assert_eq!(
            indices(&after),
            FIXTURE_WALL1_MIDDLES,
            "the six middles wear STEP1 now"
        );
    }

    #[tokio::test]
    async fn replace_texture_lists_faces_and_refuses_stray_names_and_targets() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = plain_iwad(dir.path()).await;
        let report = call(
            &client,
            "replace_texture",
            json!({ "from": "WALL1", "to": "STEP1", "parts": ["upper"] }),
        )
        .await;
        assert_eq!(report["maps"], json!({ "MAP01": 0_i64 }), "no WALL1 upper");
        assert_eq!(report["total"], json!(0_i64), "nothing changed");
        let standing = find(&client, middles_wearing("MAP01", "WALL1")).await;
        assert_eq!(
            indices(&standing),
            FIXTURE_WALL1_MIDDLES,
            "a face-listed miss writes nothing to the other faces"
        );
        let (face_code, face_message) = call_err(
            &client,
            "replace_texture",
            json!({ "from": "WALL1", "to": "STEP1", "parts": ["roof"] }),
        )
        .await;
        assert_eq!(face_code, "invalid_argument", "stray face code");
        assert!(
            face_message.contains("upper")
                && face_message.contains("middle")
                && face_message.contains("lower"),
            "the message lists the three faces: {face_message}"
        );
        let (gone_code, gone_message) = call_err(
            &client,
            "replace_texture",
            json!({ "from": "WALL1", "to": "NOSUCH" }),
        )
        .await;
        assert_eq!(gone_code, "not_found", "unknown target code");
        assert!(
            gone_message.contains("list_textures"),
            "the fix is named: {gone_message}"
        );
        let untouched = find(&client, middles_wearing("MAP01", "WALL1")).await;
        assert_eq!(
            indices(&untouched),
            FIXTURE_WALL1_MIDDLES,
            "the refusals wrote nothing"
        );
    }

    #[tokio::test]
    async fn replace_texture_over_all_maps_copies_both_blocks_and_undoes_as_one() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_plus_pwad(dir.path()).await;
        let report = call(
            &client,
            "replace_texture",
            json!({ "from": "WALL1", "to": "STEP1", "maps": "all" }),
        )
        .await;
        assert_eq!(
            report["maps"],
            json!({ "MAP01": 6_i64, "MAP02": 6_i64 }),
            "every merged block answered"
        );
        assert_eq!(report["total"], json!(12_i64), "the sum over both maps");
        assert_eq!(
            report["copied_maps"],
            json!(["MAP01", "MAP02"]),
            "both maps came from read-only layers"
        );
        let undone = call(&client, "undo", json!({})).await;
        assert_eq!(
            undone["undone"],
            json!(["replace_texture WALL1 -> STEP1"]),
            "one labelled step carried both maps"
        );
        assert_eq!(
            undone["remaining_undo"],
            json!(0_i64),
            "nothing else was recorded"
        );
        let back01 = find(&client, middles_wearing("MAP01", "WALL1")).await;
        assert_eq!(
            indices(&back01),
            FIXTURE_WALL1_MIDDLES,
            "MAP01 reads its layer block again after the undo"
        );
        let back02 = find(&client, middles_wearing("MAP02", "WALL1")).await;
        assert_eq!(
            indices(&back02),
            FIXTURE_WALL1_MIDDLES,
            "and so does MAP02: both working blocks came off with the one step"
        );
    }

    #[tokio::test]
    async fn replace_flat_moves_only_the_listed_surfaces() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = plain_iwad(dir.path()).await;
        let report = call(
            &client,
            "replace_flat",
            json!({ "from": "FLOOR1", "to": "PNGFLAT", "surfaces": ["floor"] }),
        )
        .await;
        assert_eq!(report["maps"], json!({ "MAP01": 2_i64 }), "the two floors");
        assert_eq!(report["total"], json!(2_i64), "only the floors were listed");
        assert_eq!(
            report["copied_maps"],
            json!(["MAP01"]),
            "the map was copied in"
        );
        let sectors = find(&client, json!({ "kind": "sectors" })).await;
        let rows = sectors["rows"].as_array().expect("two sector rows");
        assert_eq!(rows.len(), 2_usize, "both fixture sectors");
        assert_eq!(
            rows[0]["floor_flat"],
            json!("PNGFLAT"),
            "the hall floor moved"
        );
        assert_eq!(
            rows[0]["ceiling_flat"],
            json!("FLOOR1"),
            "and only the floor: the hall ceiling stands"
        );
        assert_eq!(
            rows[1]["floor_flat"],
            json!("PNGFLAT"),
            "the room floor moved"
        );
        assert_eq!(
            rows[1]["ceiling_flat"],
            json!("FLOOR1"),
            "and only the floor: the room ceiling stands"
        );
        let (stray_code, stray_message) = call_err(
            &client,
            "replace_flat",
            json!({ "from": "FLOOR1", "to": "PNGFLAT", "surfaces": ["roof"] }),
        )
        .await;
        assert_eq!(stray_code, "invalid_argument", "stray surface code");
        assert!(
            stray_message.contains("floor") && stray_message.contains("ceiling"),
            "the message lists the two surfaces: {stray_message}"
        );
        let (gone_code, gone_message) = call_err(
            &client,
            "replace_flat",
            json!({ "from": "FLOOR1", "to": "NOSUCH" }),
        )
        .await;
        assert_eq!(gone_code, "not_found", "unknown flat code");
        assert!(
            gone_message.contains("list_flats"),
            "the fix is named: {gone_message}"
        );
    }

    #[tokio::test]
    async fn a_dry_run_previews_counts_without_writing_or_recording() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = plain_iwad(dir.path()).await;
        add_imp_in_room(&client).await;
        let texture = call(
            &client,
            "replace_texture",
            json!({ "from": "WALL1", "to": "STEP1", "dry_run": true }),
        )
        .await;
        assert_eq!(texture["total"], json!(6_i64), "the swap is previewed");
        assert_eq!(texture["dry_run"], json!(true), "flagged as a preview");
        assert_eq!(
            texture["copied_maps"],
            json!([]),
            "a preview copies nothing"
        );
        let still = find(&client, middles_wearing("MAP01", "WALL1")).await;
        assert_eq!(
            indices(&still),
            FIXTURE_WALL1_MIDDLES,
            "nothing was written"
        );
        let flats = call(
            &client,
            "replace_flat",
            json!({ "from": "FLOOR1", "to": "PNGFLAT", "dry_run": true }),
        )
        .await;
        assert_eq!(flats["total"], json!(4_i64), "two sectors, both surfaces");
        let described = call(&client, "describe_map", json!({})).await;
        assert_eq!(
            described["sectors"][0]["floor_flat"],
            json!("FLOOR1"),
            "describe_map still reads the layer's flats: {described}"
        );
        assert_eq!(
            described["sectors"][0]["ceiling_flat"],
            json!("FLOOR1"),
            "floor and ceiling alike"
        );
        let undone = call(&client, "undo", json!({})).await;
        assert_eq!(
            undone["undone"],
            json!(["add thing 3001"]),
            "undo walks the previous stack, no new entry above it"
        );
        assert_eq!(
            undone["remaining_undo"],
            json!(0_i64),
            "only the imp was there"
        );
    }

    /// A `{kind: "things", type: n}` query over the fixture map.
    fn things_of_type(kind: i64) -> Value {
        json!({ "kind": "things", "where": one("type", "eq", &json!(kind)) })
    }

    #[tokio::test]
    async fn replace_thing_type_resolves_names_and_moves_every_imp() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = plain_iwad(dir.path()).await;
        add_imp_in_room(&client).await;
        let report = call(
            &client,
            "replace_thing_type",
            json!({ "from": "imp", "to": "demon" }),
        )
        .await;
        assert_eq!(
            report["from"],
            json!(3001_i64),
            "the exact name Imp resolves"
        );
        assert_eq!(
            report["to"],
            json!(3002_i64),
            "the exact name Demon wins over CyberDemon (16) and the other substrings"
        );
        assert_eq!(report["maps"], json!({"MAP01": 2_i64}), "both imps moved");
        assert_eq!(report["total"], json!(2_i64), "the total is the imp count");
        assert_eq!(report["dry_run"], json!(false), "the swap wrote");
        let demons = find(&client, things_of_type(3002)).await;
        assert_eq!(
            indices(&demons),
            vec![1_u64, 2],
            "every imp reads back as a demon"
        );
        assert_eq!(
            demons["rows"][0]["x"],
            json!(384_i64),
            "the moved thing keeps its x"
        );
        assert_eq!(demons["rows"][0]["angle"], json!(180_i64), "and its facing");
        let start = find(&client, things_of_type(1)).await;
        assert_eq!(
            start["rows"][0],
            json!({
                "index": 0_i64, "type": 1_i64, "name": "Player 1 Start",
                "x": 64_i64, "y": 128_i64, "angle": 0_i64,
            }),
            "the player start's type, position and facing stand unchanged"
        );
        let flagged = find(
            &client,
            json!({
                "kind": "things",
                "where": [
                    { "field": "type", "op": "eq", "value": 1_i64 },
                    { "field": "flags", "op": "has", "value": "hard" },
                ],
            }),
        )
        .await;
        assert_eq!(
            flagged["count"],
            json!(1_i64),
            "and so do its presence flags"
        );
    }

    #[tokio::test]
    async fn replace_thing_type_refuses_undefined_targets_unless_allowed() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = plain_iwad(dir.path()).await;
        let (from_code, from_message) = call_err(
            &client,
            "replace_thing_type",
            json!({ "from": "not-a-thing", "to": "demon" }),
        )
        .await;
        assert_eq!(from_code, "invalid_argument", "unknown from name code");
        assert!(
            from_message.contains("from:"),
            "the failing parameter is named: {from_message}"
        );
        let (to_code, to_message) = call_err(
            &client,
            "replace_thing_type",
            json!({ "from": "imp", "to": "not-a-thing" }),
        )
        .await;
        assert_eq!(to_code, "invalid_argument", "unknown to name code");
        assert!(
            to_message.contains("to:"),
            "the failing parameter is named: {to_message}"
        );
        let (number_code, number_message) = call_err(
            &client,
            "replace_thing_type",
            json!({ "from": "imp", "to": 99999_i64 }),
        )
        .await;
        assert_eq!(number_code, "invalid_argument", "undefined target code");
        assert!(
            number_message.contains("get_thing_types") && number_message.contains("allow_unknown"),
            "the reference and the escape hatch are named: {number_message}"
        );
        let standing = find(&client, things_of_type(3001)).await;
        assert_eq!(
            indices(&standing),
            vec![1_u64],
            "the refusals wrote nothing"
        );
        let report = call(
            &client,
            "replace_thing_type",
            json!({ "from": "imp", "to": 9999_i64, "allow_unknown": true }),
        )
        .await;
        assert_eq!(
            report["to"],
            json!(9999_i64),
            "the written number reports as-is"
        );
        assert_eq!(
            report["total"],
            json!(1_i64),
            "the fixture imp was rewritten"
        );
        let odd = find(&client, things_of_type(9999)).await;
        assert_eq!(indices(&odd), vec![1_u64], "the thing reads back as 9999");
    }

    #[tokio::test]
    async fn replace_thing_type_refuses_a_number_the_map_format_cannot_store() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = plain_iwad(dir.path()).await;
        let (code, message) = call_err(
            &client,
            "replace_thing_type",
            json!({ "from": "imp", "to": 99999_i64, "allow_unknown": true }),
        )
        .await;
        assert_eq!(code, "limit_exceeded", "the binary Doom u16 limit");
        assert!(
            message.contains("99999") && message.contains("u16"),
            "the message names the number and the field: {message}"
        );
        let standing = find(&client, things_of_type(3001)).await;
        assert_eq!(indices(&standing), vec![1_u64], "the refusal wrote nothing");
    }

    #[tokio::test]
    async fn replace_thing_type_previews_without_recording_history() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = plain_iwad(dir.path()).await;
        add_imp_in_room(&client).await;
        let preview = call(
            &client,
            "replace_thing_type",
            json!({ "from": 3001_i64, "to": 3002_i64, "dry_run": true }),
        )
        .await;
        assert_eq!(preview["maps"], json!({"MAP01": 2_i64}), "counts reported");
        assert_eq!(preview["dry_run"], json!(true), "flagged as a preview");
        assert_eq!(
            preview["copied_maps"],
            json!([]),
            "a preview copies nothing"
        );
        let standing = find(&client, things_of_type(3001)).await;
        assert_eq!(indices(&standing), vec![1_u64, 2], "nothing was written");
        let undone = call(&client, "undo", json!({})).await;
        assert_eq!(
            undone["undone"],
            json!(["add thing 3001"]),
            "the preview recorded no step of its own"
        );
        assert_eq!(
            undone["remaining_undo"],
            json!(0_i64),
            "the add was the only entry"
        );
    }

    #[tokio::test]
    async fn one_undo_restores_every_thing_type_as_a_labeled_step() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = plain_iwad(dir.path()).await;
        add_imp_in_room(&client).await;
        let report = call(
            &client,
            "replace_thing_type",
            json!({ "from": 3001_i64, "to": 3002_i64 }),
        )
        .await;
        assert_eq!(report["total"], json!(2_i64), "both imps moved");
        assert_eq!(
            report["copied_maps"],
            json!([]),
            "add_thing had copied the map in"
        );
        let demons = find(&client, things_of_type(3002)).await;
        assert_eq!(
            indices(&demons),
            vec![1_u64, 2],
            "and they read back as demons"
        );
        let undone = call(&client, "undo", json!({})).await;
        assert_eq!(
            undone["undone"],
            json!(["replace_thing_type 3001 -> 3002"]),
            "one labeled step carried both things"
        );
        assert_eq!(
            undone["remaining_undo"],
            json!(1_i64),
            "the add step stands below"
        );
        let back = find(&client, things_of_type(3001)).await;
        assert_eq!(
            indices(&back),
            vec![1_u64, 2],
            "one undo restored every type"
        );
        let gone = find(&client, things_of_type(3002)).await;
        assert_eq!(gone["count"], json!(0_i64), "no demons are left");
    }

    #[tokio::test]
    async fn replace_thing_type_sweeps_all_maps_and_refuses_unknown_ones() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_plus_pwad(dir.path()).await;
        let (code, message) = call_err(
            &client,
            "replace_thing_type",
            json!({ "from": 3001_i64, "to": 3002_i64, "maps": ["MAP99"] }),
        )
        .await;
        assert_eq!(code, "not_found", "unknown map code");
        assert!(
            message.contains("MAP99") && message.contains("list_maps"),
            "the message names the map and list_maps: {message}"
        );
        let report = call(
            &client,
            "replace_thing_type",
            json!({ "from": 3001_i64, "to": 3002_i64, "maps": "all" }),
        )
        .await;
        assert_eq!(
            report["maps"],
            json!({"MAP01": 1_i64, "MAP02": 1_i64}),
            "every merged block answered"
        );
        assert_eq!(report["total"], json!(2_i64), "the sum over both maps");
        assert_eq!(
            report["copied_maps"],
            json!(["MAP01", "MAP02"]),
            "both maps came from read-only layers"
        );
        let undone = call(&client, "undo", json!({})).await;
        assert_eq!(
            undone["undone"],
            json!(["replace_thing_type 3001 -> 3002"]),
            "one labelled step carried both maps"
        );
        assert_eq!(
            undone["remaining_undo"],
            json!(0_i64),
            "nothing else was recorded"
        );
        let back01 = find(&client, things_of_type(3001)).await;
        assert_eq!(
            indices(&back01),
            vec![1_u64],
            "MAP01's imp is back after the undo"
        );
    }

    /// Run `bulk_edit` with these arguments over the fixture map.
    async fn edit(client: &Client, args: Value) -> Value {
        let mut full = args;
        let Some(map) = full.as_object_mut() else {
            panic!("bulk arguments are an object");
        };
        map.entry("map").or_insert_with(|| json!("MAP01"));
        call(client, "bulk_edit", full).await
    }

    #[tokio::test]
    async fn bulk_edit_shifts_matching_sector_lights_in_one_step() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = plain_iwad(dir.path()).await;
        let report = edit(
            &client,
            json!({
                "kind": "sectors",
                "where": one("light", "gt", &json!(170_i64)),
                "add": {"light": -32_i64},
            }),
        )
        .await;
        assert_eq!(report["matched"], json!(1_i64), "only the bright room");
        assert_eq!(report["changed"], json!(1_i64), "its light moved");
        assert_eq!(indices(&report), vec![1_u64], "sector 1 changed");
        assert_eq!(
            report["nodes_invalidated"],
            json!(false),
            "light edits keep the nodes"
        );
        assert_eq!(
            report["copied_to_working"],
            json!(true),
            "the layer map was copied"
        );
        let rows = find(&client, json!({"kind": "sectors"})).await;
        assert_eq!(
            rows["rows"][0]["light"],
            json!(160_i64),
            "sector 0 kept its light"
        );
        assert_eq!(
            rows["rows"][1]["light"],
            json!(160_i64),
            "sector 1 came down to 160"
        );
    }

    #[tokio::test]
    async fn bulk_edit_dry_run_previews_without_writing_or_recording() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = plain_iwad(dir.path()).await;
        let preview = edit(
            &client,
            json!({
                "kind": "sectors",
                "where": one("light", "gt", &json!(170_i64)),
                "add": {"light": -32_i64},
                "dry_run": true,
            }),
        )
        .await;
        assert_eq!(preview["dry_run"], json!(true), "flagged as a preview");
        assert_eq!(preview["changed"], json!(1_i64), "the change is counted");
        assert_eq!(
            preview["preview"][0],
            json!({
                "index": 1_i64,
                "before": {"light": 192_i64},
                "after": {"light": 160_i64},
            }),
            "the preview row: {preview}"
        );
        assert_eq!(
            preview["copied_to_working"],
            json!(false),
            "a preview copies nothing"
        );
        let rows = find(&client, json!({"kind": "sectors"})).await;
        assert_eq!(
            rows["rows"][1]["light"],
            json!(192_i64),
            "the map is unchanged"
        );
        let (code, message) = call_err(&client, "undo", json!({})).await;
        assert_eq!(code, "not_found", "the preview recorded no step");
        assert_eq!(message, "nothing to undo", "the stack stayed empty");
    }

    #[tokio::test]
    async fn bulk_edit_never_clamps_and_writes_nothing_on_a_bad_value() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = plain_iwad(dir.path()).await;
        let (code, message) = call_err(
            &client,
            "bulk_edit",
            json!({"map": "MAP01", "kind": "sectors", "add": {"light": -200_i64}}),
        )
        .await;
        assert_eq!(code, "invalid_argument", "out of range refuses");
        assert!(
            message.contains("sector 0") && message.contains("-40"),
            "the refusal names the row and the would-be value: {message}"
        );
        let rows = find(&client, json!({"kind": "sectors"})).await;
        assert_eq!(rows["rows"][0]["light"], json!(160_i64), "sector 0 stands");
        assert_eq!(
            rows["rows"][1]["light"],
            json!(192_i64),
            "and so does sector 1"
        );
    }

    #[tokio::test]
    async fn bulk_edit_max_guard_refuses_before_writing() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = plain_iwad(dir.path()).await;
        let (code, message) = call_err(
            &client,
            "bulk_edit",
            json!({"kind": "sectors", "max": 0_i64, "add": {"light": 1_i64}}),
        )
        .await;
        assert_eq!(code, "limit_exceeded", "two matches over max 0 refuse");
        assert!(
            message.contains("max") && message.contains("dry_run") && message.contains("where"),
            "the refusal names max, dry_run and the narrowing: {message}"
        );
        let rows = find(&client, json!({"kind": "sectors"})).await;
        assert_eq!(
            rows["rows"][0]["light"],
            json!(160_i64),
            "the guard wrote nothing"
        );
    }

    #[tokio::test]
    async fn bulk_edit_moves_vertices_only_by_add_and_invalidates_nodes() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = plain_iwad(dir.path()).await;
        let (code, message) = call_err(
            &client,
            "bulk_edit",
            json!({"kind": "vertices", "set": {"x": 0_i64}}),
        )
        .await;
        assert_eq!(code, "invalid_argument", "vertices take no set");
        assert!(
            message.contains("add"),
            "the refusal points at add: {message}"
        );
        let report = edit(&client, json!({"kind": "vertices", "add": {"x": 16_i64}})).await;
        assert_eq!(report["matched"], json!(6_i64), "all six fixture vertices");
        assert_eq!(report["changed"], json!(6_i64), "all six slid");
        assert_eq!(
            report["nodes_invalidated"],
            json!(true),
            "moving referenced vertices is geometry"
        );
        let moved = find(
            &client,
            json!({"kind": "vertices", "where": one("x", "eq", &json!(16_i64))}),
        )
        .await;
        assert_eq!(indices(&moved), vec![0_u64, 5], "the west wall slid east");
    }

    #[tokio::test]
    async fn bulk_edit_sets_linedef_flags_by_name() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = plain_iwad(dir.path()).await;
        let report = edit(
            &client,
            json!({"kind": "linedefs", "flags_set": ["blockmonsters"]}),
        )
        .await;
        assert_eq!(
            report["matched"],
            json!(7_i64),
            "every fixture line matched"
        );
        assert_eq!(report["changed"], json!(7_i64), "every line gained the bit");
        let gated = find(
            &client,
            json!({"kind": "linedefs", "where": one("flags", "has", &json!("blockmonsters"))}),
        )
        .await;
        assert_eq!(
            indices(&gated),
            (0..7).collect::<Vec<u64>>(),
            "the bits read back through the query"
        );
        let (code, message) = call_err(
            &client,
            "bulk_edit",
            json!({
                "kind": "linedefs",
                "where": one("flags", "has", &json!("playeruse")),
                "flags_set": ["mapped"],
            }),
        )
        .await;
        assert_eq!(
            code, "invalid_argument",
            "a Hexen flag name is not for this map"
        );
        assert!(
            message.contains("playeruse") && message.contains("doom family"),
            "the refusal names the flag and the map's family: {message}"
        );
    }

    #[tokio::test]
    async fn bulk_edit_resolves_thing_type_names_and_refuses_the_stray() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = plain_iwad(dir.path()).await;
        let swapped = edit(
            &client,
            json!({
                "kind": "things",
                "where": one("name", "glob", &json!("imp*")),
                "set": {"type": "demon"},
            }),
        )
        .await;
        assert_eq!(swapped["matched"], json!(1_i64), "the fixture imp matched");
        assert_eq!(swapped["changed"], json!(1_i64), "its type moved");
        assert_eq!(
            swapped["nodes_invalidated"],
            json!(false),
            "thing edits keep the nodes"
        );
        let demons = find(&client, things_of_type(3002)).await;
        assert_eq!(
            indices(&demons),
            vec![1_u64],
            "the imp reads back as a demon"
        );
        let (stray_code, stray_message) = call_err(
            &client,
            "bulk_edit",
            json!({"kind": "things", "set": {"type": "platypus"}}),
        )
        .await;
        assert_eq!(stray_code, "invalid_argument", "no thing is named platypus");
        assert!(
            stray_message.contains("get_thing_types"),
            "the reference is named: {stray_message}"
        );
    }

    #[tokio::test]
    async fn one_undo_restores_every_bulk_edit_field_on_every_element() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = plain_iwad(dir.path()).await;
        let report = edit(
            &client,
            json!({
                "kind": "sectors",
                "set": {"light": 96_i64},
                "add": {"tag": 7_i64, "floor": 8_i64},
            }),
        )
        .await;
        assert_eq!(report["matched"], json!(2_i64), "both sectors matched");
        assert_eq!(report["changed"], json!(2_i64), "both moved on every field");
        let rows = find(&client, json!({"kind": "sectors"})).await;
        assert_eq!(rows["rows"][0]["light"], json!(96_i64), "sector 0 darkened");
        assert_eq!(
            rows["rows"][1]["tag"],
            json!(8_i64),
            "sector 1's tag rode the add"
        );
        let undone = call(&client, "undo", json!({})).await;
        assert_eq!(
            undone["undone"],
            json!(["bulk_edit sectors (2)"]),
            "one labeled step carried every field on both sectors"
        );
        let back = find(&client, json!({"kind": "sectors"})).await;
        let restored = back["rows"].as_array().expect("two sector rows");
        let hall = restored.first().expect("sector 0's row");
        assert_eq!(hall["light"], json!(160_i64), "sector 0's light is back");
        assert_eq!(hall["tag"], json!(0_i64), "and its tag");
        assert_eq!(hall["floor"], json!(0_i64), "and its floor");
        let room = restored.get(1).expect("sector 1's row");
        assert_eq!(room["light"], json!(192_i64), "sector 1's light is back");
        assert_eq!(room["tag"], json!(1_i64), "and its tag");
        assert_eq!(room["floor"], json!(32_i64), "and its floor");
    }
}
