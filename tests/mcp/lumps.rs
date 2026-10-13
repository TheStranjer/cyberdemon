//! The `lumps` tool group as seen by a real MCP client: directory listing
//! with filters and paging, and glob/content search over the merged view.

#[cfg(test)]
mod tests {
    use std::path::Path;

    use cyberdemon::wad::map::read_map;
    use cyberdemon::wad::{Wad, analyze};
    use rmcp::RoleClient;
    use rmcp::service::RunningService;
    use serde_json::{Value, json};

    use crate::fixtures::{iwad, pwad, two_room_map};
    use crate::support::{call, call_err, connect};

    /// A connected client for one test.
    type Client = RunningService<RoleClient, ()>;

    /// Bytes in one fixture flat.
    const FLAT_BYTES: u64 = 4_096;
    /// Bytes in the fixture `PLAYPAL` (14 palettes of 768).
    const PLAYPAL_BYTES: u64 = 10_752;
    /// Bytes in the fixture `COLORMAP` (34 tables of 256).
    const COLORMAP_BYTES: u64 = 8_704;
    /// Bytes in a `WAD` header.
    const HEADER_BYTES: u64 = 12;
    /// Bytes of member data in one fixture two-room map block: two things
    /// (20), seven linedefs (98), eight sidedefs (240), six vertices (24)
    /// and two sectors (52); the generated node lumps are empty.
    const MAP_BYTES: u64 = 434;

    /// Load the `IWAD` fixture as the base layer and stack the `PWAD`
    /// fixture as resource layer 1.
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

    /// The row named `wanted` from a report's `lumps` array.
    fn row_named<'rep>(report: &'rep Value, wanted: &str) -> &'rep Value {
        report["lumps"]
            .as_array()
            .expect("lumps is an array")
            .iter()
            .find(|row| row["name"] == json!(wanted))
            .unwrap_or_else(|| panic!("no row named {wanted} in {:?}", report["lumps"]))
    }

    /// The row names of a report, in result order.
    fn names(report: &Value) -> Vec<String> {
        report["lumps"]
            .as_array()
            .expect("lumps is an array")
            .iter()
            .map(|row| row["name"].as_str().expect("names are strings").to_owned())
            .collect()
    }

    #[test]
    fn the_fixture_map_block_round_trips_through_the_map_model() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let base = iwad(dir.path());
        let wad = Wad::read_file(&base).expect("the fixture is a valid WAD");
        let analysis = analyze(&wad.lumps);
        let block = analysis.maps.first().expect("the IWAD holds MAP01");
        let map = read_map(&wad.lumps, block).expect("the fixture block decodes");
        assert_eq!(map, two_room_map(), "the fixture is the two-room map");
        assert_eq!(
            map.bounds(),
            Some((0_i32, 0_i32, 512_i32, 256_i32)),
            "the hall spans 512 by 256"
        );
    }

    #[tokio::test]
    async fn list_lumps_merges_layers_and_reports_save_layout_offsets() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_plus_pwad(dir.path()).await;
        let report = call(&client, "list_lumps", json!({ "limit": 100_u64 })).await;
        assert_eq!(report["scope"], json!("merged"), "the default scope");
        assert_eq!(report["total"].as_u64(), Some(29), "iwad 17 + 12 additions");
        assert_eq!(report["offset"].as_u64(), Some(0), "echoed offset");
        assert_eq!(report["count"].as_u64(), Some(29), "one page");
        let floor = row_named(&report, "FLOOR1");
        assert_eq!(
            floor["index"].as_u64(),
            Some(14),
            "in place in the flats region, after the full MAP01 block"
        );
        assert_eq!(floor["size"].as_u64(), Some(FLAT_BYTES), "the PWAD's copy");
        assert_eq!(floor["kind"], json!("flat"), "classified in its region");
        assert_eq!(floor["namespace"], json!("flats"), "flats namespace");
        assert_eq!(floor["map"], Value::Null, "not map data");
        assert_eq!(
            floor["source"],
            json!({ "layer": 1_u64 }),
            "the PWAD's FLOOR1 won the region",
        );
        // The PWAD lays out: 12-byte header, empty FF_START, then FLOOR1.
        assert_eq!(
            floor["offset"],
            json!(HEADER_BYTES),
            "data right after the header"
        );
        let dehacked = row_named(&report, "DEHACKED");
        assert_eq!(
            dehacked["source"],
            json!({ "layer": 1_u64 }),
            "the PWAD's global"
        );
        assert_eq!(
            dehacked["offset"].as_u64(),
            Some(HEADER_BYTES.checked_add(FLAT_BYTES).expect("small sum")),
            "after the header and the 4096-byte flat the empty markers surround",
        );
        assert_eq!(
            row_named(&report, "PLAYPAL")["source"],
            json!({"layer": 0_u64}),
            "base copy"
        );
        assert_eq!(
            row_named(&report, "ENDOOM")["kind"],
            json!("endoom"),
            "size-matched kind"
        );
    }

    #[tokio::test]
    async fn list_lumps_layer_scope_reads_one_archive_with_its_own_offsets() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = connect().await;
        let base = iwad(dir.path());
        call(
            &client,
            "open_iwad",
            json!({ "path": base.display().to_string() }),
        )
        .await;
        let report = call(&client, "list_lumps", json!({ "scope": "layer:0" })).await;
        assert_eq!(report["scope"], json!({ "layer": 0_u64 }), "echoed scope");
        assert_eq!(report["total"].as_u64(), Some(17), "the IWAD fixture alone");
        assert_eq!(
            row_named(&report, "FLOOR1")["offset"].as_u64(),
            Some(
                HEADER_BYTES
                    .checked_add(PLAYPAL_BYTES)
                    .and_then(|sum| sum.checked_add(COLORMAP_BYTES))
                    .and_then(|sum| sum.checked_add(MAP_BYTES))
                    .expect("small sums")
            ),
            "header + PLAYPAL + COLORMAP + the MAP01 block's member data",
        );
        assert_eq!(
            row_named(&report, "FLOOR1")["source"],
            json!({ "layer": 0_u64 }),
            "layer rows carry the layer source",
        );
    }

    #[tokio::test]
    async fn list_lumps_working_scope_reads_the_editable_archive() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = connect().await;
        let extra = pwad(dir.path());
        call(
            &client,
            "open_wad",
            json!({ "path": extra.display().to_string() }),
        )
        .await;
        let report = call(&client, "list_lumps", json!({ "scope": "working" })).await;
        assert_eq!(report["scope"], json!("working"), "echoed scope");
        assert_eq!(report["total"].as_u64(), Some(15), "the PWAD fixture alone");
        assert_eq!(
            names(&report),
            vec![
                "FF_START", "FLOOR1", "FF_END", "DEHACKED", "MAP02", "THINGS", "LINEDEFS",
                "SIDEDEFS", "VERTEXES", "SEGS", "SSECTORS", "NODES", "SECTORS", "REJECT",
                "BLOCKMAP",
            ],
            "the working archive keeps its own markers and order",
        );
        assert_eq!(
            row_named(&report, "FLOOR1")["source"],
            json!("working"),
            "working source"
        );
    }

    #[tokio::test]
    async fn list_lumps_reports_an_out_of_range_layer_as_not_found() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_plus_pwad(dir.path()).await;
        let (code, message) = call_err(&client, "list_lumps", json!({ "scope": "layer:9" })).await;
        assert_eq!(code, "not_found", "no layer 9");
        assert!(
            message.contains("layer 9"),
            "the message names the missing layer: {message}"
        );
    }

    #[tokio::test]
    async fn list_lumps_rejects_a_bogus_scope_and_filters() {
        let client = connect().await;
        let (code, message) = call_err(&client, "list_lumps", json!({ "scope": "bogus" })).await;
        assert_eq!(code, "invalid_argument", "unknown scope");
        assert!(
            message.contains("merged"),
            "the hint lists the forms: {message}"
        );
        let (ns_code, ns_message) =
            call_err(&client, "list_lumps", json!({ "namespace": "nope" })).await;
        assert_eq!(ns_code, "invalid_argument", "unknown namespace");
        assert!(
            ns_message.contains("flats"),
            "the hint lists the namespaces: {ns_message}"
        );
        let (kind_code, kind_message) =
            call_err(&client, "list_lumps", json!({ "kind": "nope" })).await;
        assert_eq!(kind_code, "invalid_argument", "unknown kind");
        assert!(
            kind_message.contains("map_marker"),
            "the hint lists the kinds: {kind_message}"
        );
    }

    #[tokio::test]
    async fn list_lumps_filters_by_namespace_kind_and_map() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_plus_pwad(dir.path()).await;
        let flats = call(&client, "list_lumps", json!({ "namespace": "flats" })).await;
        assert_eq!(
            names(&flats),
            vec!["F_START", "FLOOR1", "F_END"],
            "the flats region of the merged view",
        );
        assert_eq!(
            row_named(&flats, "FLOOR1")["source"],
            json!({ "layer": 1_u64 }),
            "the filtered rows keep their sources",
        );
        let markers = call(&client, "list_lumps", json!({ "kind": "map_marker" })).await;
        assert_eq!(names(&markers), vec!["MAP01", "MAP02"], "both map headers");
        let map = call(&client, "list_lumps", json!({ "map": "MAP01" })).await;
        assert_eq!(
            names(&map),
            vec![
                "MAP01", "THINGS", "LINEDEFS", "SIDEDEFS", "VERTEXES", "SEGS", "SSECTORS", "NODES",
                "SECTORS", "REJECT", "BLOCKMAP",
            ],
            "the MAP01 block only",
        );
        assert!(
            map["lumps"]
                .as_array()
                .expect("array")
                .iter()
                .all(|row| row["map"] == json!("MAP01")),
            "every row names the filtered map",
        );
    }

    #[tokio::test]
    async fn list_lumps_pages_with_offset_limit_and_total() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_plus_pwad(dir.path()).await;
        let all = call(&client, "list_lumps", json!({ "limit": 100_u64 })).await;
        assert_eq!(all["total"].as_u64(), Some(29), "everything");
        let page = call(
            &client,
            "list_lumps",
            json!({ "offset": 5_u64, "limit": 4_u64 }),
        )
        .await;
        assert_eq!(page["total"].as_u64(), Some(29), "total counts all matches");
        assert_eq!(page["offset"].as_u64(), Some(5), "echoed offset");
        assert_eq!(page["count"].as_u64(), Some(4), "one page");
        assert_eq!(
            names(&page),
            names(&all).into_iter().skip(5).take(4).collect::<Vec<_>>(),
            "the page is the same slice of the full listing",
        );
        assert_eq!(
            page["lumps"][0]["index"].as_u64(),
            Some(5),
            "index counts positions in the scope, not the page",
        );
        let past = call(
            &client,
            "list_lumps",
            json!({ "offset": 100_u64, "limit": 4_u64 }),
        )
        .await;
        assert_eq!(past["count"].as_u64(), Some(0), "nothing past the end");
        assert_eq!(
            past["total"].as_u64(),
            Some(29),
            "total still reports the matches"
        );
    }

    #[tokio::test]
    async fn list_lumps_rejects_limits_outside_one_to_two_thousand() {
        let client = connect().await;
        let (zero_code, zero_message) =
            call_err(&client, "list_lumps", json!({ "limit": 0_u64 })).await;
        assert_eq!(zero_code, "invalid_argument", "limit 0 code");
        assert!(
            zero_message.contains("between 1 and 2000"),
            "the message states the range: {zero_message}"
        );
        let (big_code, big_message) =
            call_err(&client, "list_lumps", json!({ "limit": 2001_u64 })).await;
        assert_eq!(big_code, "invalid_argument", "limit 2001 code");
        assert!(
            big_message.contains("between 1 and 2000"),
            "the message states the range: {big_message}"
        );
    }

    #[tokio::test]
    async fn search_lumps_matches_name_globs() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_plus_pwad(dir.path()).await;
        let flats = call(&client, "search_lumps", json!({ "pattern": "F*" })).await;
        assert_eq!(flats["scope"], json!("merged"), "the default scope");
        assert_eq!(flats["count"].as_u64(), Some(3), "the flats region");
        assert_eq!(flats["truncated"], json!(false), "well below the limit");
        assert_eq!(
            names(&flats),
            vec!["F_START", "FLOOR1", "F_END"],
            "glob order"
        );
        assert_eq!(
            row_named(&flats, "FLOOR1")["match_offset"],
            Value::Null,
            "no content criterion means no offset",
        );
        let one = call(&client, "search_lumps", json!({ "pattern": "?LOOR1" })).await;
        assert_eq!(
            names(&one),
            vec!["FLOOR1"],
            "question matches one character"
        );
        assert_eq!(
            row_named(&one, "FLOOR1")["offset"].as_u64(),
            Some(HEADER_BYTES),
            "the row shape carries the save-layout offset",
        );
    }

    #[tokio::test]
    async fn search_lumps_filters_by_kind_and_namespace() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_plus_pwad(dir.path()).await;
        let palettes = call(&client, "search_lumps", json!({ "kind": "palette" })).await;
        assert_eq!(names(&palettes), vec!["PLAYPAL"], "the palette lump");
        let flats = call(&client, "search_lumps", json!({ "namespace": "flats" })).await;
        assert_eq!(
            names(&flats),
            vec!["F_START", "FLOOR1", "F_END"],
            "everything inside the flats region",
        );
    }

    #[tokio::test]
    async fn search_lumps_finds_text_and_reports_the_offset() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_plus_pwad(dir.path()).await;
        let hits = call(&client, "search_lumps", json!({ "contains_text": "troop" })).await;
        assert_eq!(
            names(&hits),
            vec!["DEHACKED"],
            "case-insensitive ASCII match"
        );
        assert_eq!(
            row_named(&hits, "DEHACKED")["match_offset"].as_u64(),
            Some(9),
            "\"Thing 1 (Troop)\" has Troop at byte 9",
        );
        assert_eq!(
            row_named(&hits, "DEHACKED")["source"],
            json!({ "layer": 1_u64 }),
            "found through the merged view",
        );
    }

    #[tokio::test]
    async fn search_lumps_finds_hex_bytes_with_and_without_spaces() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_plus_pwad(dir.path()).await;
        let spaced = call(&client, "search_lumps", json!({ "contains_hex": "28 54" })).await;
        assert_eq!(
            names(&spaced),
            vec!["DEHACKED"],
            "\"(T\" lives in the DeHackEd text"
        );
        assert_eq!(
            row_named(&spaced, "DEHACKED")["match_offset"].as_u64(),
            Some(8),
            "first occurrence offset",
        );
        let plain = call(
            &client,
            "search_lumps",
            json!({ "contains_hex": "5468696e67" }),
        )
        .await;
        assert_eq!(
            names(&plain),
            vec!["DEHACKED"],
            "lowercase digits, no spaces"
        );
        assert_eq!(
            row_named(&plain, "DEHACKED")["match_offset"].as_u64(),
            Some(0),
            "the payload starts with \"Thing\"",
        );
        let both = call(
            &client,
            "search_lumps",
            json!({ "contains_text": "troop", "contains_hex": "28 54" }),
        )
        .await;
        assert_eq!(
            row_named(&both, "DEHACKED")["match_offset"].as_u64(),
            Some(8),
            "the lowest content match wins when both needles hit",
        );
    }

    #[tokio::test]
    async fn search_lumps_needs_at_least_one_criterion() {
        let client = connect().await;
        let (code, message) = call_err(&client, "search_lumps", json!({})).await;
        assert_eq!(code, "invalid_argument", "no criteria code");
        assert!(
            message.contains("pattern") && message.contains("contains_hex"),
            "the message names the criteria: {message}"
        );
    }

    #[tokio::test]
    async fn search_lumps_rejects_bad_hex() {
        let client = connect().await;
        let (odd_code, odd_message) =
            call_err(&client, "search_lumps", json!({ "contains_hex": "4D5" })).await;
        assert_eq!(odd_code, "invalid_argument", "odd digit count");
        assert!(
            odd_message.contains("odd"),
            "the message says odd: {odd_message}"
        );
        let (digit_code, digit_message) =
            call_err(&client, "search_lumps", json!({ "contains_hex": "0G" })).await;
        assert_eq!(digit_code, "invalid_argument", "non-hex digit");
        assert!(
            digit_message.contains("'G'"),
            "the message names the digit: {digit_message}"
        );
    }

    #[tokio::test]
    async fn search_lumps_flags_truncation_at_the_limit() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_plus_pwad(dir.path()).await;
        let page = call(
            &client,
            "search_lumps",
            json!({ "pattern": "*", "limit": 2_u64 }),
        )
        .await;
        assert_eq!(page["count"].as_u64(), Some(2), "capped at the limit");
        assert_eq!(page["truncated"], json!(true), "more matched than returned");
        assert_eq!(names(&page).len(), 2, "two rows");
        let small = call(
            &client,
            "search_lumps",
            json!({ "pattern": "?LOOR1", "limit": 2_u64 }),
        )
        .await;
        assert_eq!(small["truncated"], json!(false), "one match, not truncated");
        assert_eq!(small["count"].as_u64(), Some(1), "one row");
    }

    #[tokio::test]
    async fn search_lumps_honours_the_scope_argument() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_plus_pwad(dir.path()).await;
        let base = call(
            &client,
            "search_lumps",
            json!({ "pattern": "FLOOR1", "scope": "layer:0" }),
        )
        .await;
        assert_eq!(
            base["scope"],
            json!({ "layer": 0_u64 }),
            "echoed layer scope"
        );
        assert_eq!(names(&base), vec!["FLOOR1"], "the IWAD's copy");
        assert_eq!(
            row_named(&base, "FLOOR1")["source"],
            json!({ "layer": 0_u64 }),
            "reported from the layer, not the winner",
        );
        assert_eq!(
            row_named(&base, "FLOOR1")["offset"].as_u64(),
            Some(
                HEADER_BYTES
                    .checked_add(PLAYPAL_BYTES)
                    .and_then(|sum| sum.checked_add(COLORMAP_BYTES))
                    .and_then(|sum| sum.checked_add(MAP_BYTES))
                    .expect("small sums")
            ),
            "the IWAD's own save layout",
        );
        let none = call(
            &client,
            "search_lumps",
            json!({ "pattern": "*", "scope": "working" }),
        )
        .await;
        assert_eq!(
            none["count"].as_u64(),
            Some(0),
            "the working archive is empty"
        );
        let (code, _) = call_err(
            &client,
            "search_lumps",
            json!({ "pattern": "FLOOR1", "scope": "layer:9" }),
        )
        .await;
        assert_eq!(code, "not_found", "out of range layer");
    }
}
