//! The `lumps` tool group as seen by a real MCP client: directory listing
//! with filters and paging, glob/content search over the merged view, and
//! reading one lump through the shared selector.

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use cyberdemon::wad::map::read_map;
    use cyberdemon::wad::{Wad, analyze};
    use rmcp::RoleClient;
    use rmcp::service::RunningService;
    use serde_json::{Value, json};

    use crate::fixtures::{iwad, iwad_duplicated, pwad, two_room_map};
    use crate::support::{call, call_err, connect, http_get};

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

    /// Split an asset `URL` into the `http://localhost:<port>` origin and
    /// the request path for [`http_get`].
    fn split_asset_url(url: &str) -> (String, String) {
        let (base, rest) = url.split_once("/assets/").expect("an asset url");
        (base.to_owned(), format!("/assets/{rest}"))
    }

    /// Fetch a published asset with a raw `HTTP` `GET` and require success.
    async fn fetch_asset(url: &str) -> Vec<u8> {
        let (base, path) = split_asset_url(url);
        let (status, body) = http_get(&base, &path).await;
        assert_eq!(status, 200, "the asset serves: {url}");
        body
    }

    /// The `PLAYPAL` payload of the `IWAD` fixture, read from disk.
    fn playpal_bytes(dir: &Path) -> Vec<u8> {
        let path = iwad(dir);
        let wad = Wad::read_file(&path).expect("the fixture reads");
        wad.lumps
            .iter()
            .find(|lump| lump.name.as_bytes() == b"PLAYPAL")
            .expect("the IWAD holds PLAYPAL")
            .data
            .to_vec()
    }

    /// The space-separated uppercase hex of `data`'s first `count` bytes.
    fn hex_head(data: &[u8], count: usize) -> String {
        let head = &data[..count.min(data.len())];
        head.iter()
            .map(|byte| format!("{byte:02X}"))
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// The `{"path": ...}` argument object for `file`.
    fn path_arg(file: &Path) -> Value {
        json!({ "path": file.display().to_string() })
    }

    #[tokio::test]
    async fn get_lump_info_reports_the_row_and_nothing_more() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_plus_pwad(dir.path()).await;
        let report = call(
            &client,
            "get_lump",
            json!({ "name": "DEHACKED", "format": "info" }),
        )
        .await;
        assert_eq!(report["format"], json!("info"), "the applied format");
        assert_eq!(report["name"], json!("DEHACKED"), "row name");
        assert_eq!(report["size"], json!(16_u64), "the DeHackEd text");
        assert_eq!(report["kind"], json!("dehacked"), "row kind");
        assert_eq!(report["namespace"], Value::Null, "a global has no region");
        assert_eq!(
            report["source"],
            json!({ "layer": 1_u64 }),
            "the merged row shape carries the source",
        );
        assert!(report.get("index").is_some(), "and the scope index");
        assert!(report.get("text").is_none(), "info adds nothing");
        assert!(report.get("hex").is_none(), "info adds nothing");
        assert!(report.get("asset").is_none(), "info adds nothing");
    }

    #[tokio::test]
    async fn get_lump_text_decodes_truncates_and_flags_lossiness() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_plus_pwad(dir.path()).await;
        let report = call(
            &client,
            "get_lump",
            json!({ "name": "DEHACKED", "format": "text" }),
        )
        .await;
        assert_eq!(report["format"], json!("text"), "the applied format");
        assert_eq!(report["text"], json!("Thing 1 (Troop)\n"), "clean text");
        assert_eq!(report["lossy"], json!(false), "valid UTF-8");
        assert_eq!(report["truncated"], json!(false), "under the cap");
        let cut = call(
            &client,
            "get_lump",
            json!({ "name": "DEHACKED", "format": "text", "max_chars": 5_u64 }),
        )
        .await;
        assert_eq!(cut["text"], json!("Thing"), "cut at the character cap");
        assert_eq!(cut["truncated"], json!(true), "the cap was hit");
        let lossy = call(
            &client,
            "get_lump",
            json!({ "name": "PLAYPAL", "format": "text", "max_chars": 8_u64 }),
        )
        .await;
        assert_eq!(lossy["lossy"], json!(true), "the palette is not UTF-8");
        assert_eq!(lossy["truncated"], json!(true), "10752 bytes over 8 chars");
        let text = lossy["text"].as_str().expect("a text body");
        assert!(
            text.chars().any(|ch| ch == '\u{FFFD}'),
            "invalid sequences are replaced: {text:?}"
        );
    }

    #[tokio::test]
    async fn get_lump_hex_windows_the_payload() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_plus_pwad(dir.path()).await;
        let report = call(
            &client,
            "get_lump",
            json!({ "name": "DEHACKED", "format": "hex" }),
        )
        .await;
        assert_eq!(report["format"], json!("hex"), "the applied format");
        assert_eq!(
            report["hex"],
            json!("54 68 69 6E 67 20 31 20 28 54 72 6F 6F 70 29 0A"),
            "space-separated uppercase bytes",
        );
        assert_eq!(report["start"], json!(0_u64), "echoed window start");
        assert_eq!(report["length"], json!(16_u64), "the whole payload");
        let window = call(
            &client,
            "get_lump",
            json!({ "name": "DEHACKED", "format": "hex", "start": 6_u64, "length": 4_u64 }),
        )
        .await;
        assert_eq!(window["hex"], json!("31 20 28 54"), "\"1 (T\"");
        assert_eq!(window["start"], json!(6_u64), "echoed start");
        assert_eq!(window["length"], json!(4_u64), "echoed length");
        let clipped = call(
            &client,
            "get_lump",
            json!({ "name": "DEHACKED", "format": "hex", "start": 11_u64, "length": 4_096_u64 }),
        )
        .await;
        assert_eq!(
            clipped["hex"],
            json!("6F 6F 70 29 0A"),
            "clipped to the payload"
        );
        assert_eq!(clipped["length"], json!(5_u64), "the real window length");
        let (code, message) = call_err(
            &client,
            "get_lump",
            json!({ "name": "DEHACKED", "format": "hex", "start": 100_u64 }),
        )
        .await;
        assert_eq!(code, "invalid_argument", "start past the end");
        assert!(
            message.contains("past the end") && message.contains("16 bytes"),
            "the message shows the bounds: {message}"
        );
        let (cap_code, _) = call_err(
            &client,
            "get_lump",
            json!({ "name": "DEHACKED", "format": "hex", "length": 4_097_u64 }),
        )
        .await;
        assert_eq!(cap_code, "invalid_argument", "length over the cap");
    }

    #[tokio::test]
    async fn get_lump_url_publishes_the_raw_lump_bytes() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_plus_pwad(dir.path()).await;
        let expected = playpal_bytes(dir.path());
        let report = call(
            &client,
            "get_lump",
            json!({ "name": "PLAYPAL", "format": "url" }),
        )
        .await;
        assert_eq!(report["format"], json!("url"), "the applied format");
        assert_eq!(
            report["asset"]["filename"],
            json!("PLAYPAL.lmp"),
            "a palette has no dedicated extension",
        );
        assert_eq!(
            report["asset"]["mime"],
            json!("application/octet-stream"),
            "the media type follows the extension",
        );
        assert_eq!(report["asset"]["size"], json!(PLAYPAL_BYTES), "the size");
        assert!(
            report.get("preview_hex").is_none(),
            "an explicit url asks for no preview",
        );
        let url = report["asset"]["url"].as_str().expect("an asset url");
        assert!(
            url.starts_with("http://localhost:"),
            "a localhost origin: {url}"
        );
        let fetched = fetch_asset(url).await;
        assert_eq!(fetched, expected, "the served bytes are the lump verbatim");
    }

    #[tokio::test]
    async fn get_lump_auto_picks_text_info_and_url_by_kind() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_plus_pwad(dir.path()).await;
        let texty = call(&client, "get_lump", json!({ "name": "DEHACKED" })).await;
        assert_eq!(
            texty["format"],
            json!("text"),
            "a dehacked lump reads as text"
        );
        assert_eq!(
            texty["text"],
            json!("Thing 1 (Troop)\n"),
            "the decoded body"
        );
        let marker = call(&client, "get_lump", json!({ "name": "F_START" })).await;
        assert_eq!(marker["format"], json!("info"), "an empty lump is info");
        assert_eq!(marker["kind"], json!("marker"), "the row kind");
        let expected = playpal_bytes(dir.path());
        let binary = call(&client, "get_lump", json!({ "name": "PLAYPAL" })).await;
        assert_eq!(
            binary["format"],
            json!("url"),
            "binary goes to the asset server"
        );
        let preview = binary["preview_hex"].as_str().expect("a preview");
        assert_eq!(preview, hex_head(&expected, 64), "the first 64 bytes");
        let url = binary["asset"]["url"].as_str().expect("an asset url");
        let fetched = fetch_asset(url).await;
        assert_eq!(
            fetched, expected,
            "the full PLAYPAL bytes match the fixture"
        );
    }

    #[tokio::test]
    async fn get_lump_occurrence_numbers_the_matches_and_defaults_to_the_winner() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = connect().await;
        let base = iwad_duplicated(dir.path());
        call(&client, "open_iwad", path_arg(&base)).await;
        let last = call(
            &client,
            "get_lump",
            json!({ "name": "FLOOR1", "format": "hex", "length": 2_u64 }),
        )
        .await;
        assert_eq!(last["hex"], json!("03 03"), "the default is the last copy");
        assert_eq!(
            last["source"],
            json!({ "layer": 0_u64 }),
            "still the base layer's lump",
        );
        let first = call(
            &client,
            "get_lump",
            json!({ "name": "FLOOR1", "format": "hex", "length": 2_u64, "occurrence": 0_u64 }),
        )
        .await;
        assert_eq!(
            first["hex"],
            json!("01 01"),
            "occurrence 0 is the first copy"
        );
        let flats = call(
            &client,
            "get_lump",
            json!({ "name": "FLOOR1", "namespace": "flats", "format": "hex", "length": 2_u64 }),
        )
        .await;
        assert_eq!(flats["hex"], json!("03 03"), "the flats filter keeps both");
        assert_eq!(flats["namespace"], json!("flats"), "the row region");
        let (sparse_code, sparse_message) = call_err(
            &client,
            "get_lump",
            json!({ "name": "FLOOR1", "namespace": "sprites" }),
        )
        .await;
        assert_eq!(sparse_code, "not_found", "no floor in sprites");
        assert!(
            sparse_message.contains("not found in merged"),
            "the message names the scope: {sparse_message}"
        );
        let (code, message) = call_err(
            &client,
            "get_lump",
            json!({ "name": "FLOOR1", "occurrence": 2_u64 }),
        )
        .await;
        assert_eq!(code, "not_found", "only two copies exist");
        assert!(
            message.contains("only 2 lumps named FLOOR1") && message.contains("0 to 1"),
            "the message counts the matches: {message}"
        );
    }

    #[tokio::test]
    async fn get_lump_scopes_read_the_same_name_out_of_each_archive() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_plus_pwad(dir.path()).await;
        let layer = call(
            &client,
            "get_lump",
            json!({ "name": "FLOOR1", "scope": "layer:0", "format": "hex", "length": 2_u64 }),
        )
        .await;
        assert_eq!(layer["hex"], json!("01 01"), "the IWAD's flat");
        assert_eq!(layer["source"], json!({ "layer": 0_u64 }), "layer source");
        let merged = call(
            &client,
            "get_lump",
            json!({ "name": "FLOOR1", "format": "hex", "length": 2_u64 }),
        )
        .await;
        assert_eq!(
            merged["hex"],
            json!("02 02"),
            "the PWAD's override won the merge"
        );
        assert_eq!(merged["source"], json!({ "layer": 1_u64 }), "merged source");
        let (code, _) = call_err(
            &client,
            "get_lump",
            json!({ "name": "FLOOR1", "scope": "layer:9" }),
        )
        .await;
        assert_eq!(code, "not_found", "layer 9 is not loaded");
    }

    #[tokio::test]
    async fn get_lump_reports_missing_lumps_names_and_formats() {
        let client = connect().await;
        let (code, message) = call_err(&client, "get_lump", json!({ "name": "NOPE" })).await;
        assert_eq!(code, "not_found", "no such lump");
        assert_eq!(message, "lump NOPE not found in merged", "the message");
        let (name_code, _) =
            call_err(&client, "get_lump", json!({ "name": "TOO LONG NAME" })).await;
        assert_eq!(name_code, "invalid_argument", "not a lump name");
        let (format_code, format_message) = call_err(
            &client,
            "get_lump",
            json!({ "name": "NOPE", "format": "binary" }),
        )
        .await;
        assert_eq!(format_code, "invalid_argument", "not a format");
        assert!(
            format_message.contains("\"url\"") && format_message.contains("\"auto\""),
            "the hint lists the formats: {format_message}"
        );
    }

    /// The `{ "name": "DEHACKED", "path": ... }` arguments for `file`.
    fn extract_args(file: &Path) -> Value {
        json!({ "name": "DEHACKED", "path": file.display().to_string() })
    }

    #[tokio::test]
    async fn extract_lump_writes_the_bytes_and_replaces_only_when_told() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_plus_pwad(dir.path()).await;
        let target = dir.path().join("dehacked.txt");
        let report = call(&client, "extract_lump", extract_args(&target)).await;
        assert_eq!(
            report["path"],
            json!(target.display().to_string()),
            "echoed path"
        );
        assert_eq!(report["bytes"], json!(16_u64), "the payload size");
        assert_eq!(
            fs::read(&target).expect("the file landed"),
            b"Thing 1 (Troop)\n",
            "the raw bytes are on disk",
        );
        let (code, message) = call_err(&client, "extract_lump", extract_args(&target)).await;
        assert_eq!(code, "already_exists", "the target stands there");
        assert!(
            message.contains("overwrite true"),
            "the message names the fix: {message}"
        );
        let mut force = extract_args(&target);
        force["overwrite"] = json!(true);
        let again = call(&client, "extract_lump", force).await;
        assert_eq!(again["bytes"], json!(16_u64), "overwrite rewrote it");
        assert_eq!(
            fs::read(&target).expect("the file is still there"),
            b"Thing 1 (Troop)\n",
            "the rewritten file holds the lump bytes",
        );
    }

    #[tokio::test]
    async fn extract_lump_refuses_relative_paths_missing_parents_and_lumps() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_plus_pwad(dir.path()).await;
        let relative = call_err(
            &client,
            "extract_lump",
            json!({ "name": "DEHACKED", "path": "dehacked.txt" }),
        )
        .await;
        assert_eq!(relative.0, "invalid_argument", "a relative path is refused");
        assert!(
            relative.1.contains("not absolute"),
            "the message says what to pass: {}",
            relative.1
        );
        let nowhere = dir.path().join("missing-dir").join("x.bin");
        let (parent_code, parent_message) =
            call_err(&client, "extract_lump", extract_args(&nowhere)).await;
        assert_eq!(parent_code, "not_found", "the parent directory is missing");
        assert!(
            parent_message.contains("parent directory"),
            "the message names the problem: {parent_message}"
        );
        let missing_lump = dir.path().join("y.bin");
        let lump_args = json!({ "name": "NOPE", "path": missing_lump.display().to_string() });
        let (lump_code, _) = call_err(&client, "extract_lump", lump_args).await;
        assert_eq!(lump_code, "not_found", "the lump does not exist");
        assert!(!missing_lump.exists(), "a refused extract writes nothing");
    }
}
