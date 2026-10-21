//! The `lumps` tool group as seen by a real MCP client: directory listing
//! with filters and paging, glob/content search over the merged view, reading
//! one lump through the shared selector, and the editing tools — `add_lump`,
//! the import tools, `replace_lump`, `remove_lump`, `rename_lump` and
//! `reorder_lumps` — whose every change lands in the working archive as one
//! undoable edit, copying a read-only lump out of its layer when there is
//! nothing else to write.

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
    use crate::support::{call, call_err, connect, http_get, http_put};

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
        assert_eq!(report["total"].as_u64(), Some(45), "iwad 33 + 12 additions");
        assert_eq!(report["offset"].as_u64(), Some(0), "echoed offset");
        assert_eq!(report["count"].as_u64(), Some(45), "one page");
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
        assert_eq!(report["total"].as_u64(), Some(33), "the IWAD fixture alone");
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
        let (code, message, _) =
            call_err(&client, "list_lumps", json!({ "scope": "layer:9" })).await;
        assert_eq!(code, "not_found", "no layer 9");
        assert!(
            message.contains("layer 9"),
            "the message names the missing layer: {message}"
        );
    }

    #[tokio::test]
    async fn list_lumps_rejects_a_bogus_scope_and_filters() {
        let client = connect().await;
        let (code, message, _) = call_err(&client, "list_lumps", json!({ "scope": "bogus" })).await;
        assert_eq!(code, "invalid_argument", "unknown scope");
        assert!(
            message.contains("merged"),
            "the hint lists the forms: {message}"
        );
        let (ns_code, ns_message, _) =
            call_err(&client, "list_lumps", json!({ "namespace": "nope" })).await;
        assert_eq!(ns_code, "invalid_argument", "unknown namespace");
        assert!(
            ns_message.contains("flats"),
            "the hint lists the namespaces: {ns_message}"
        );
        let (kind_code, kind_message, _) =
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
            vec!["F_START", "FLOOR1", "PNGFLAT", "F_END"],
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
        assert_eq!(all["total"].as_u64(), Some(45), "everything");
        let page = call(
            &client,
            "list_lumps",
            json!({ "offset": 5_u64, "limit": 4_u64 }),
        )
        .await;
        assert_eq!(page["total"].as_u64(), Some(45), "total counts all matches");
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
            Some(45),
            "total still reports the matches"
        );
    }

    #[tokio::test]
    async fn list_lumps_rejects_limits_outside_one_to_two_thousand() {
        let client = connect().await;
        let (zero_code, zero_message, _) =
            call_err(&client, "list_lumps", json!({ "limit": 0_u64 })).await;
        assert_eq!(zero_code, "invalid_argument", "limit 0 code");
        assert!(
            zero_message.contains("between 1 and 2000"),
            "the message states the range: {zero_message}"
        );
        let (big_code, big_message, _) =
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
            vec!["F_START", "FLOOR1", "PNGFLAT", "F_END"],
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
        let (code, message, _) = call_err(&client, "search_lumps", json!({})).await;
        assert_eq!(code, "invalid_argument", "no criteria code");
        assert!(
            message.contains("pattern") && message.contains("contains_hex"),
            "the message names the criteria: {message}"
        );
    }

    #[tokio::test]
    async fn search_lumps_rejects_bad_hex() {
        let client = connect().await;
        let (odd_code, odd_message, _) =
            call_err(&client, "search_lumps", json!({ "contains_hex": "4D5" })).await;
        assert_eq!(odd_code, "invalid_argument", "odd digit count");
        assert!(
            odd_message.contains("odd"),
            "the message says odd: {odd_message}"
        );
        let (digit_code, digit_message, _) =
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
        let (code, _, _) = call_err(
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
        let (code, message, _) = call_err(
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
        let (cap_code, _, _) = call_err(
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
        let (sparse_code, sparse_message, _) = call_err(
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
        let (code, message, _) = call_err(
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
        let (code, _, _) = call_err(
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
        let (code, message, _) = call_err(&client, "get_lump", json!({ "name": "NOPE" })).await;
        assert_eq!(code, "not_found", "no such lump");
        assert_eq!(message, "lump NOPE not found in merged", "the message");
        let (name_code, _, _) =
            call_err(&client, "get_lump", json!({ "name": "TOO LONG NAME" })).await;
        assert_eq!(name_code, "invalid_argument", "not a lump name");
        let (format_code, format_message, _) = call_err(
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
        let (code, message, _) = call_err(&client, "extract_lump", extract_args(&target)).await;
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
        let (parent_code, parent_message, _) =
            call_err(&client, "extract_lump", extract_args(&nowhere)).await;
        assert_eq!(parent_code, "not_found", "the parent directory is missing");
        assert!(
            parent_message.contains("parent directory"),
            "the message names the problem: {parent_message}"
        );
        let missing_lump = dir.path().join("y.bin");
        let lump_args = json!({ "name": "NOPE", "path": missing_lump.display().to_string() });
        let (lump_code, _, _) = call_err(&client, "extract_lump", lump_args).await;
        assert_eq!(lump_code, "not_found", "the lump does not exist");
        assert!(!missing_lump.exists(), "a refused extract writes nothing");
    }

    /// The `{"source": ...}` argument object for the absolute `file`.
    fn source_arg(file: &Path) -> Value {
        json!({ "source": file.display().to_string() })
    }

    /// The working archive's lump names, in directory order.
    async fn working_names(client: &Client) -> Vec<String> {
        let report = call(client, "list_lumps", json!({ "scope": "working" })).await;
        names(&report)
    }

    /// `get_session_info`'s undo-label list.
    async fn undo_labels(client: &Client) -> Vec<String> {
        let info = call(client, "get_session_info", Value::Null).await;
        info["undo"]
            .as_array()
            .expect("undo is an array")
            .iter()
            .map(|label| label.as_str().expect("labels are strings").to_owned())
            .collect()
    }

    /// Append ONE, TWO and THREE, in that order, as global lumps.
    async fn seed_three(client: &Client) {
        call(client, "add_lump", json!({ "name": "ONE", "text": "a" })).await;
        call(client, "add_lump", json!({ "name": "TWO", "text": "b" })).await;
        call(client, "add_lump", json!({ "name": "THREE", "text": "c" })).await;
    }

    /// A session where every refused edit left nothing behind: an empty
    /// working directory, no history and a clean flag.
    async fn assert_untouched(client: &Client) {
        let listing = call(client, "list_lumps", json!({ "scope": "working" })).await;
        assert_eq!(
            listing["total"],
            json!(0_u64),
            "nothing landed in the working archive"
        );
        assert!(
            undo_labels(client).await.is_empty(),
            "and nothing was recorded"
        );
        let info = call(client, "get_session_info", Value::Null).await;
        assert_eq!(info["working"]["dirty"], json!(false), "still clean");
    }

    #[tokio::test]
    async fn add_lump_text_appends_and_reports_the_new_row() {
        let client = connect().await;
        let report = call(
            &client,
            "add_lump",
            json!({ "name": "MYTEXT", "text": "hello world" }),
        )
        .await;
        assert_eq!(report["name"], json!("MYTEXT"), "echoed name");
        assert_eq!(report["index"], json!(0_u64), "appended at 0");
        assert_eq!(report["size"], json!(11_u64), "the text's bytes");
        assert_eq!(
            report["namespace"],
            Value::Null,
            "a loose lump has no region"
        );
        assert!(
            report.get("source").is_none(),
            "add_lump echoes no source: {report}"
        );
        let listing = call(&client, "list_lumps", json!({ "scope": "working" })).await;
        assert_eq!(names(&listing), vec!["MYTEXT"], "the working archive grew");
        assert_eq!(row_named(&listing, "MYTEXT")["kind"], json!("text"));
        let read = call(
            &client,
            "get_lump",
            json!({ "name": "mytext", "scope": "working", "format": "text" }),
        )
        .await;
        assert_eq!(read["text"], json!("hello world"), "written as-is");
        let info = call(&client, "get_session_info", Value::Null).await;
        assert_eq!(
            info["working"]["dirty"],
            json!(true),
            "the session is dirty"
        );
        assert_eq!(
            info["undo"],
            json!(["add_lump MYTEXT"]),
            "the undo label names the tool and the lump"
        );
    }

    #[tokio::test]
    async fn add_lump_hex_source_and_empty_form_the_payloads() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = connect().await;
        let hexed = call(
            &client,
            "add_lump",
            json!({ "name": "MUSHEAD", "hex": "4D 55 53 1A" }),
        )
        .await;
        assert_eq!(hexed["size"], json!(4_u64), "four hex pairs");
        let read = call(
            &client,
            "get_lump",
            json!({ "name": "MUSHEAD", "scope": "working", "format": "hex" }),
        )
        .await;
        assert_eq!(read["hex"], json!("4D 55 53 1A"), "the decoded bytes");
        let payload = dir.path().join("payload.bin");
        fs::write(&payload, [0xDE_u8, 0xAD, 0xBE, 0xEF]).expect("write the source file");
        let mut args = source_arg(&payload);
        args["name"] = json!("PAYLOAD");
        let fetched = call(&client, "add_lump", args).await;
        assert_eq!(fetched["index"], json!(1_u64), "appended second");
        assert_eq!(fetched["size"], json!(4_u64), "the file's bytes");
        let marker = call(&client, "add_lump", json!({ "name": "MYMARKER" })).await;
        assert_eq!(marker["index"], json!(2_u64), "appended third");
        assert_eq!(marker["size"], json!(0_u64), "no data field is a marker");
        assert_eq!(
            working_names(&client).await,
            vec!["MUSHEAD", "PAYLOAD", "MYMARKER"],
            "directory order after plain appends"
        );
    }

    #[tokio::test]
    async fn add_lump_places_by_index_before_and_after() {
        let client = connect().await;
        seed_three(&client).await;
        let spliced = call(
            &client,
            "add_lump",
            json!({ "name": "X", "text": "x", "index": 1_u64 }),
        )
        .await;
        assert_eq!(spliced["index"], json!(1_u64), "reported position");
        let before = call(
            &client,
            "add_lump",
            json!({ "name": "Y", "text": "y", "before": "TWO" }),
        )
        .await;
        assert_eq!(before["index"], json!(2_u64), "just before TWO");
        let after = call(
            &client,
            "add_lump",
            json!({ "name": "Z", "text": "z", "after": "ONE" }),
        )
        .await;
        assert_eq!(after["index"], json!(1_u64), "just after ONE");
        assert_eq!(
            working_names(&client).await,
            vec!["ONE", "Z", "X", "Y", "TWO", "THREE"],
            "every anchor honoured in order"
        );
    }

    #[tokio::test]
    async fn add_lump_namespace_placement_creates_and_reuses_a_region() {
        let client = connect().await;
        let flats = call(
            &client,
            "add_lump",
            json!({ "name": "FLATA", "text": "f", "namespace": "flats" }),
        )
        .await;
        assert_eq!(flats["index"], json!(1_u64), "between the created markers");
        assert_eq!(flats["namespace"], json!("flats"), "the new region");
        let second = call(
            &client,
            "add_lump",
            json!({ "name": "FLATB", "text": "g", "namespace": "flats" }),
        )
        .await;
        assert_eq!(second["index"], json!(2_u64), "before the existing F_END");
        let listing = call(&client, "list_lumps", json!({ "scope": "working" })).await;
        assert_eq!(
            names(&listing),
            vec!["F_START", "FLATA", "FLATB", "F_END"],
            "the region was created once and reused"
        );
        assert_eq!(
            row_named(&listing, "FLATB")["kind"],
            json!("flat"),
            "classified by its region"
        );
        assert_eq!(
            row_named(&listing, "F_START")["namespace"],
            json!("flats"),
            "the markers carry the region too"
        );
    }

    #[tokio::test]
    async fn add_lump_refuses_twins_but_namespaces_and_allow_duplicate_let_them_through() {
        let client = connect().await;
        call(
            &client,
            "add_lump",
            json!({ "name": "TWIN", "text": "one" }),
        )
        .await;
        let (code, message, _) = call_err(
            &client,
            "add_lump",
            json!({ "name": "TWIN", "text": "two" }),
        )
        .await;
        assert_eq!(code, "already_exists", "same name, same (global) area");
        assert!(
            message.contains("allow_duplicate") && message.contains("replace_lump"),
            "the message names both exits: {message}"
        );
        assert_eq!(
            working_names(&client).await,
            vec!["TWIN"],
            "the refused add stored nothing"
        );
        assert_eq!(
            undo_labels(&client).await,
            vec!["add_lump TWIN"],
            "and left no history"
        );
        let forced = call(
            &client,
            "add_lump",
            json!({ "name": "TWIN", "text": "two", "allow_duplicate": true }),
        )
        .await;
        assert_eq!(forced["index"], json!(1_u64), "the twin is appended");
        let region = call(
            &client,
            "add_lump",
            json!({ "name": "TWIN", "text": "three", "namespace": "sprites" }),
        )
        .await;
        assert_eq!(
            region["namespace"],
            json!("sprites"),
            "a region twin is no global clash"
        );
        let (again_code, again_message, _) = call_err(
            &client,
            "add_lump",
            json!({ "name": "TWIN", "text": "four", "namespace": "sprites" }),
        )
        .await;
        assert_eq!(
            again_code, "already_exists",
            "now the sprites region has a twin"
        );
        assert!(
            again_message.contains("the sprites region"),
            "the message names the region: {again_message}"
        );
    }

    #[tokio::test]
    async fn add_lump_rejects_conflicting_fields_without_editing() {
        let client = connect().await;
        let (payload_code, payload_message, _) = call_err(
            &client,
            "add_lump",
            json!({ "name": "A", "text": "t", "hex": "4D55" }),
        )
        .await;
        assert_eq!(payload_code, "invalid_argument", "two payload fields");
        assert!(
            payload_message.contains("at most one"),
            "the message states the rule: {payload_message}"
        );
        let (text_source_code, _, _) = call_err(
            &client,
            "add_lump",
            json!({ "name": "A", "text": "t", "source": "/x/y.bin" }),
        )
        .await;
        assert_eq!(text_source_code, "invalid_argument", "text plus source");
        let (place_code, place_message, _) = call_err(
            &client,
            "add_lump",
            json!({ "name": "A", "index": 1_u64, "before": "B" }),
        )
        .await;
        assert_eq!(place_code, "invalid_argument", "two placement fields");
        assert!(
            place_message.contains("at most one"),
            "the message states the rule: {place_message}"
        );
        assert_untouched(&client).await;
    }

    #[tokio::test]
    async fn add_lump_rejects_malformed_arguments_without_editing() {
        let client = connect().await;
        let (hex_code, hex_message, _) =
            call_err(&client, "add_lump", json!({ "name": "A", "hex": "4Z" })).await;
        assert_eq!(hex_code, "invalid_argument", "not a hex pair");
        assert!(
            hex_message.starts_with("hex "),
            "the error names the hex field: {hex_message}"
        );
        let (odd_code, _, _) =
            call_err(&client, "add_lump", json!({ "name": "A", "hex": "4D5" })).await;
        assert_eq!(odd_code, "invalid_argument", "an odd digit count");
        let (index_code, index_message, _) =
            call_err(&client, "add_lump", json!({ "name": "A", "index": 99_u64 })).await;
        assert_eq!(
            index_code, "invalid_argument",
            "index past the empty directory"
        );
        assert!(
            index_message.contains("past the end"),
            "the message says where the index may go: {index_message}"
        );
        let (anchor_code, _, _) = call_err(
            &client,
            "add_lump",
            json!({ "name": "A", "before": "GHOST" }),
        )
        .await;
        assert_eq!(anchor_code, "not_found", "no anchor lump to place by");
        let (name_code, _, _) = call_err(
            &client,
            "add_lump",
            json!({ "name": "TOO LONG FOR WAD", "text": "t" }),
        )
        .await;
        assert_eq!(
            name_code, "invalid_argument",
            "the name is over eight bytes"
        );
        let (region_code, _, _) = call_err(
            &client,
            "add_lump",
            json!({ "name": "A", "text": "t", "namespace": "nope" }),
        )
        .await;
        assert_eq!(region_code, "invalid_argument", "not a namespace");
        assert_untouched(&client).await;
    }

    #[tokio::test]
    async fn add_lump_passes_a_missing_source_file_through_as_not_found() {
        let client = connect().await;
        let (code, message, _) = call_err(
            &client,
            "add_lump",
            json!({ "name": "GONE", "source": "/definitely-not-here/cyberdemon.bin" }),
        )
        .await;
        assert_eq!(code, "not_found", "the fetch reports the missing file");
        assert!(
            message.contains("/definitely-not-here"),
            "the message names the path: {message}"
        );
        assert_untouched(&client).await;
    }

    #[tokio::test]
    async fn namespace_placement_joins_the_markers_of_an_opened_working_archive() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = connect().await;
        let extra = pwad(dir.path());
        call(&client, "open_wad", path_arg(&extra)).await;
        let report = call(
            &client,
            "add_lump",
            json!({ "name": "FLOOR2", "hex": "00", "namespace": "flats" }),
        )
        .await;
        assert_eq!(
            report["index"],
            json!(2_u64),
            "just before the FF_END alias"
        );
        assert_eq!(
            report["namespace"],
            json!("flats"),
            "the alias region counts"
        );
        assert_eq!(
            working_names(&client).await,
            vec![
                "FF_START", "FLOOR1", "FLOOR2", "FF_END", "DEHACKED", "MAP02", "THINGS",
                "LINEDEFS", "SIDEDEFS", "VERTEXES", "SEGS", "SSECTORS", "NODES", "SECTORS",
                "REJECT", "BLOCKMAP",
            ],
            "the flat joined the existing region without creating a new one"
        );
        let (code, message, _) = call_err(
            &client,
            "add_lump",
            json!({ "name": "FLOOR1", "hex": "01", "namespace": "flats" }),
        )
        .await;
        assert_eq!(code, "already_exists", "the region already holds that name");
        assert!(
            message.contains("the flats region"),
            "the message names the region: {message}"
        );
    }

    #[tokio::test]
    async fn import_asset_derives_names_and_import_file_as_lump_shares_the_behaviour() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = connect().await;
        let png = dir.path().join("TITLEPIC.png");
        fs::write(&png, b"pretend-png-bytes").expect("write the source file");
        let report = call(&client, "import_asset", source_arg(&png)).await;
        assert_eq!(report["name"], json!("TITLEPIC"), "the stem, uppercased");
        assert_eq!(report["index"], json!(0_u64), "appended");
        assert_eq!(report["size"], json!(17_u64), "bytes as-is");
        assert_eq!(report["namespace"], Value::Null, "loose by default");
        assert_eq!(
            report["source"],
            png.display().to_string(),
            "the import tools echo the source"
        );
        let mut named = source_arg(&png);
        named["name"] = json!("MYICON");
        named["after"] = json!("TITLEPIC");
        let explicit = call(&client, "import_file_as_lump", named).await;
        assert_eq!(explicit["name"], json!("MYICON"), "an explicit name wins");
        assert_eq!(
            explicit["index"],
            json!(1_u64),
            "the placement fields apply too"
        );
        let hidden = dir.path().join(".png");
        fs::write(&hidden, b"x").expect("write the underivable file");
        let (code, message, _) = call_err(&client, "import_asset", source_arg(&hidden)).await;
        assert_eq!(code, "invalid_argument", "nothing usable in the file name");
        assert!(
            message.contains("pass name"),
            "the message asks for the name: {message}"
        );
        let mut named_hidden = source_arg(&hidden);
        named_hidden["name"] = json!("STILL_OK");
        named_hidden["namespace"] = json!("patches");
        let saved = call(&client, "import_file_as_lump", named_hidden).await;
        assert_eq!(saved["name"], json!("STILL_OK"), "with a name it imports");
        assert_eq!(saved["namespace"], json!("patches"), "placement applies");
        assert_eq!(
            working_names(&client).await,
            vec!["TITLEPIC", "MYICON", "P_START", "STILL_OK", "P_END"],
            "one region created, nothing else moved"
        );
        let (twin_code, _, _) = call_err(&client, "import_asset", source_arg(&png)).await;
        assert_eq!(twin_code, "already_exists", "the derived twin is refused");
        assert_eq!(
            undo_labels(&client).await,
            vec!["import TITLEPIC", "import MYICON", "import STILL_OK"],
            "every import labels the history"
        );
    }

    #[tokio::test]
    async fn import_asset_reads_an_uploaded_asset_url() {
        let client = connect().await;
        let info = call(&client, "get_asset_server_info", Value::Null).await;
        let base = info["base_url"].as_str().expect("an origin").to_owned();
        let body = b"uploaded-flat-bytes";
        let (status, receipt) = http_put(&base, "/upload/UPFLAT.lmp", body).await;
        assert_eq!(status, 201, "the upload stored");
        let receipt_json: Value = serde_json::from_slice(&receipt).expect("a JSON receipt");
        let url = receipt_json["url"]
            .as_str()
            .expect("the asset url")
            .to_owned();
        let report = call(&client, "import_asset", json!({ "source": url })).await;
        assert_eq!(
            report["name"],
            json!("UPFLAT"),
            "derived from the stored file name"
        );
        let size = u64::try_from(body.len()).expect("a small count");
        assert_eq!(report["size"], json!(size), "the uploaded bytes");
        assert_eq!(report["source"], json!(url), "the url echoes back");
        let length = u64::try_from(body.len()).expect("a small count");
        let read = call(
            &client,
            "get_lump",
            json!({ "name": "UPFLAT", "scope": "working", "format": "hex", "length": length }),
        )
        .await;
        assert_eq!(
            read["hex"],
            json!(hex_head(body, body.len())),
            "the bytes came back unconverted"
        );
        assert_eq!(
            undo_labels(&client).await,
            vec!["import UPFLAT"],
            "the shared undo label"
        );
    }

    /// A session with the `IWAD` fixture as its only layer: the working
    /// archive is empty, so every edit there is a copy-on-write.
    async fn iwad_only(dir: &Path) -> Client {
        let client = connect().await;
        let base = iwad(dir);
        call(&client, "open_iwad", path_arg(&base)).await;
        client
    }

    /// A session with the duplicated-flat `IWAD`: two `FLOOR1` copies in the
    /// flats region, for the selection tests.
    async fn iwad_with_twins(dir: &Path) -> Client {
        let client = connect().await;
        let base = iwad_duplicated(dir);
        call(&client, "open_iwad", path_arg(&base)).await;
        client
    }

    /// Appends `text` (or nothing for a marker) as a new working lump.
    async fn add(client: &Client, lump: &str, body: &str) {
        call(client, "add_lump", json!({ "name": lump, "text": body })).await;
    }

    /// Appends two global `TWIN` copies, the second forced past the duplicate
    /// rule, so occurrence selection has something to choose between.
    async fn add_twins(client: &Client) {
        add(client, "TWIN", "first").await;
        call(
            client,
            "add_lump",
            json!({ "name": "TWIN", "text": "second", "allow_duplicate": true }),
        )
        .await;
    }

    /// The merged view's `get_lump` hex window for `name`.
    async fn merged_hex(client: &Client, name: &str, length: u64) -> String {
        let read = call(
            client,
            "get_lump",
            json!({ "name": name, "format": "hex", "length": length }),
        )
        .await;
        read["hex"].as_str().expect("a hex body").to_owned()
    }

    /// The `{ "name": ..., "hex": ... }` arguments for `replace_lump`.
    fn replace_hex(name: &str, digits: &str) -> Value {
        json!({ "name": name, "hex": digits })
    }

    /// The bytes of the `IWAD` fixture's `FLOOR1`, read straight from disk.
    fn iwad_flat_bytes(dir: &Path) -> Vec<u8> {
        let wad = Wad::read_file(&iwad(dir)).expect("the fixture reads");
        wad.lumps
            .iter()
            .find(|lump| lump.name.as_bytes() == b"FLOOR1")
            .expect("the IWAD holds FLOOR1")
            .data
            .to_vec()
    }

    #[tokio::test]
    async fn replace_lump_writes_new_text_over_a_working_lump_in_place() {
        let client = connect().await;
        add(&client, "NOTE", "hello").await;
        add(&client, "OTHER", "x").await;
        let report = call(
            &client,
            "replace_lump",
            json!({ "name": "note", "text": "hi there" }),
        )
        .await;
        assert_eq!(
            report,
            json!({ "name": "NOTE", "index": 0_u64, "size": 8_u64, "created_override": false }),
            "the documented result, written in place"
        );
        assert_eq!(
            working_names(&client).await,
            vec!["NOTE", "OTHER"],
            "the directory did not grow"
        );
        let read = call(
            &client,
            "get_lump",
            json!({ "name": "NOTE", "scope": "working", "format": "text" }),
        )
        .await;
        assert_eq!(read["text"], json!("hi there"), "the new bytes stand");
        assert_eq!(read["index"].as_u64(), Some(0), "same position");
        assert_eq!(
            undo_labels(&client).await,
            vec!["add_lump NOTE", "add_lump OTHER", "replace_lump NOTE"],
            "the replace is its own undoable step"
        );
    }

    #[tokio::test]
    async fn replace_lump_takes_its_payload_from_hex_or_a_fetched_source() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = connect().await;
        add(&client, "NOTE", "hello").await;
        let hexed = call(&client, "replace_lump", replace_hex("NOTE", "4D 55 53 1A")).await;
        assert_eq!(hexed["size"], json!(4_u64), "four hex pairs");
        assert_eq!(hexed["index"], json!(0_u64), "in place again");
        assert_eq!(
            merged_hex(&client, "NOTE", 4).await,
            "4D 55 53 1A",
            "the decoded bytes are what the view shows"
        );
        let payload = dir.path().join("note.txt");
        fs::write(&payload, b"from a file").expect("write the source file");
        let fetched = call(
            &client,
            "replace_lump",
            json!({ "name": "NOTE", "source": payload.display().to_string() }),
        )
        .await;
        assert_eq!(fetched["size"], json!(11_u64), "the file's bytes");
        assert_eq!(
            merged_hex(&client, "NOTE", 4).await,
            hex_of(b"from"),
            "the fetched bytes landed"
        );
    }

    /// The space-separated uppercase hex of `data`.
    fn hex_of(data: &[u8]) -> String {
        data.iter()
            .map(|byte| format!("{byte:02X}"))
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[tokio::test]
    async fn replace_lump_picks_a_working_copy_by_occurrence() {
        let client = connect().await;
        add_twins(&client).await;
        let first = call(
            &client,
            "replace_lump",
            json!({ "name": "TWIN", "text": "picked", "occurrence": 0_u64 }),
        )
        .await;
        assert_eq!(first["index"], json!(0_u64), "the first copy");
        assert_eq!(
            first["created_override"],
            json!(false),
            "both copies are working lumps"
        );
        let read = call(
            &client,
            "get_lump",
            json!({ "name": "TWIN", "scope": "working", "occurrence": 0_u64, "format": "text" }),
        )
        .await;
        assert_eq!(read["text"], json!("picked"), "the first copy changed");
        assert_eq!(
            merged_hex(&client, "TWIN", 6).await,
            hex_of(b"second"),
            "the winning (last) copy kept its bytes"
        );
    }

    #[tokio::test]
    async fn replace_lump_refuses_an_occurrence_past_the_working_copies() {
        let client = connect().await;
        add_twins(&client).await;
        let (code, message, _) = call_err(
            &client,
            "replace_lump",
            json!({ "name": "TWIN", "text": "t", "occurrence": 9_u64 }),
        )
        .await;
        assert_eq!(code, "not_found", "no ninth working copy");
        assert!(
            message.contains("only 2 lumps named TWIN"),
            "the message counts the working copies: {message}"
        );
        assert_eq!(
            merged_hex(&client, "TWIN", 6).await,
            hex_of(b"second"),
            "the refused call wrote nothing"
        );
    }

    #[tokio::test]
    async fn replace_lump_needs_the_namespace_to_match_the_copy() {
        let client = connect().await;
        add_twins(&client).await;
        let (code, message, _) = call_err(
            &client,
            "replace_lump",
            json!({ "name": "TWIN", "text": "t", "namespace": "flats" }),
        )
        .await;
        assert_eq!(code, "not_found", "no TWIN sits in a flats region");
        assert!(
            message.contains("flats region"),
            "the message names the region it looked in: {message}"
        );
        assert_eq!(
            working_names(&client).await,
            vec!["TWIN", "TWIN"],
            "no region was created for the refusal"
        );
    }

    #[tokio::test]
    async fn replace_lump_overrides_an_iwad_flat_inside_its_region() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_only(dir.path()).await;
        let report = call(&client, "replace_lump", replace_hex("FLOOR1", "01 02 03")).await;
        assert_eq!(
            report,
            json!({ "name": "FLOOR1", "index": 1_u64, "size": 3_u64, "created_override": true }),
            "the override was created between new markers"
        );
        let listing = call(&client, "list_lumps", json!({ "scope": "working" })).await;
        assert_eq!(
            names(&listing),
            vec!["F_START", "FLOOR1", "F_END"],
            "a flats region appeared in the working archive"
        );
        assert_eq!(
            row_named(&listing, "FLOOR1")["namespace"],
            json!("flats"),
            "the override sits in the region it came from"
        );
        assert_eq!(merged_hex(&client, "FLOOR1", 3).await, "01 02 03");
    }

    #[tokio::test]
    async fn an_override_leaves_the_iwad_layer_and_its_file_untouched() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_only(dir.path()).await;
        call(&client, "replace_lump", replace_hex("FLOOR1", "01 02 03")).await;
        let layer = call(
            &client,
            "list_lumps",
            json!({ "scope": "layer:0", "namespace": "flats" }),
        )
        .await;
        assert_eq!(
            row_named(&layer, "FLOOR1")["size"].as_u64(),
            Some(FLAT_BYTES),
            "the IWAD layer still holds its own flat"
        );
        assert_eq!(
            iwad_flat_bytes(dir.path()).len(),
            usize::try_from(FLAT_BYTES).expect("a small count"),
            "and the file on disk is unchanged"
        );
        assert_eq!(
            undo_labels(&client).await,
            vec!["replace_lump FLOOR1"],
            "one undoable edit for the whole override"
        );
    }

    #[tokio::test]
    async fn replace_lump_copies_the_whole_map_block_before_the_lump() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_only(dir.path()).await;
        let report = call(&client, "replace_lump", replace_hex("THINGS", "AA BB")).await;
        assert!(
            report["created_override"]
                .as_bool()
                .expect("a created_override flag"),
            "map data from a layer is copied first"
        );
        assert_eq!(report["index"].as_u64(), Some(1), "the copied THINGS");
        assert_eq!(report["size"].as_u64(), Some(2), "the new payload");
        assert_eq!(
            working_names(&client).await,
            vec![
                "MAP01", "THINGS", "LINEDEFS", "SIDEDEFS", "VERTEXES", "SEGS", "SSECTORS", "NODES",
                "SECTORS", "REJECT", "BLOCKMAP",
            ],
            "the entire MAP01 block arrived, header first"
        );
    }

    #[tokio::test]
    async fn a_copied_block_wins_the_merged_view_and_needs_copying_once() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_only(dir.path()).await;
        call(&client, "replace_lump", replace_hex("THINGS", "AA BB")).await;
        let merged = call(&client, "list_lumps", json!({ "map": "MAP01" })).await;
        assert!(
            merged["lumps"]
                .as_array()
                .expect("an array")
                .iter()
                .all(|row| row["source"] == json!("working")),
            "the copied block is what an engine would see: {merged}"
        );
        let again = call(&client, "replace_lump", replace_hex("LINEDEFS", "CC")).await;
        assert_eq!(
            again["created_override"],
            json!(false),
            "the block is already in the working archive, so this is a plain overwrite"
        );
        assert_eq!(again["index"].as_u64(), Some(2), "the copied LINEDEFS");
        assert_eq!(
            undo_labels(&client).await,
            vec!["replace_lump THINGS", "replace_lump LINEDEFS"],
            "copy plus write is one step, then a plain overwrite"
        );
    }

    #[tokio::test]
    async fn replace_lump_reports_a_lump_no_loaded_archive_holds() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_only(dir.path()).await;
        let (code, message, _) = call_err(&client, "replace_lump", replace_hex("NOPE", "00")).await;
        assert_eq!(code, "not_found", "no archive holds NOPE");
        assert!(
            message.contains("add_lump"),
            "the message names the tool that creates one: {message}"
        );
        let (region_code, region_message, _) = call_err(
            &client,
            "replace_lump",
            json!({ "name": "FLOOR1", "hex": "00", "namespace": "sprites" }),
        )
        .await;
        assert_eq!(region_code, "not_found", "FLOOR1 is no sprite");
        assert!(
            region_message.contains("sprites region"),
            "the message names the region: {region_message}"
        );
        assert_untouched(&client).await;
    }

    #[tokio::test]
    async fn replace_lump_needs_exactly_one_payload_field() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_only(dir.path()).await;
        let (none_code, none_message, _) =
            call_err(&client, "replace_lump", json!({ "name": "PLAYPAL" })).await;
        assert_eq!(none_code, "invalid_argument", "a replace needs a payload");
        assert!(
            none_message.contains("exactly one"),
            "the message states the rule: {none_message}"
        );
        let (two_code, two_message, _) = call_err(
            &client,
            "replace_lump",
            json!({ "name": "PLAYPAL", "text": "t", "hex": "00" }),
        )
        .await;
        assert_eq!(two_code, "invalid_argument", "one payload field only");
        assert!(
            two_message.contains("exactly one"),
            "the message states the rule: {two_message}"
        );
        assert_untouched(&client).await;
    }

    #[tokio::test]
    async fn replace_lump_rejects_malformed_arguments_without_editing() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_only(dir.path()).await;
        let (hex_code, hex_message, _) =
            call_err(&client, "replace_lump", replace_hex("PLAYPAL", "4Z")).await;
        assert_eq!(hex_code, "invalid_argument", "not a hex pair");
        assert!(
            hex_message.starts_with("hex "),
            "the error names the hex field: {hex_message}"
        );
        let (name_code, _, _) = call_err(
            &client,
            "replace_lump",
            json!({ "name": "TOO LONG FOR WAD", "text": "t" }),
        )
        .await;
        assert_eq!(
            name_code, "invalid_argument",
            "the name is over eight bytes"
        );
        let (region_code, _, _) = call_err(
            &client,
            "replace_lump",
            json!({ "name": "PLAYPAL", "namespace": "nope", "text": "t" }),
        )
        .await;
        assert_eq!(region_code, "invalid_argument", "not a namespace");
        assert_untouched(&client).await;
    }

    #[tokio::test]
    async fn replace_lump_passes_a_missing_source_file_through_as_not_found() {
        let client = connect().await;
        add(&client, "NOTE", "keep").await;
        let (code, message, _) = call_err(
            &client,
            "replace_lump",
            json!({ "name": "NOTE", "source": "/definitely-not-here/cyberdemon.bin" }),
        )
        .await;
        assert_eq!(code, "not_found", "the fetch reports the missing file");
        assert!(
            message.contains("/definitely-not-here"),
            "the message names the path: {message}"
        );
        let read = call(
            &client,
            "get_lump",
            json!({ "name": "NOTE", "scope": "working", "format": "text" }),
        )
        .await;
        assert_eq!(read["text"], json!("keep"), "the write never started");
        assert_eq!(
            undo_labels(&client).await,
            vec!["add_lump NOTE"],
            "and no history was recorded"
        );
    }

    #[tokio::test]
    async fn remove_lump_takes_the_selected_copy_and_reports_its_old_index() {
        let client = connect().await;
        seed_three(&client).await;
        let report = call(&client, "remove_lump", json!({ "names": ["TWO"] })).await;
        assert_eq!(
            report,
            json!({ "removed": [{ "name": "TWO", "index": 1_u64 }] }),
            "the documented result shape"
        );
        assert_eq!(
            working_names(&client).await,
            vec!["ONE", "THREE"],
            "the named lump is gone"
        );
        assert_eq!(
            undo_labels(&client).await,
            vec![
                "add_lump ONE",
                "add_lump TWO",
                "add_lump THREE",
                "remove_lump TWO"
            ],
            "the removal is the newest history entry"
        );
    }

    #[tokio::test]
    async fn remove_lump_takes_several_names_in_one_edit() {
        let client = connect().await;
        seed_three(&client).await;
        let report = call(&client, "remove_lump", json!({ "names": ["THREE", "ONE"] })).await;
        assert_eq!(
            report["removed"],
            json!([
                { "name": "ONE", "index": 0_u64 },
                { "name": "THREE", "index": 2_u64 }
            ]),
            "reported in directory order with the pre-removal indices"
        );
        assert_eq!(
            working_names(&client).await,
            vec!["TWO"],
            "one edit, two gone"
        );
        assert_eq!(
            undo_labels(&client).await,
            vec![
                "add_lump ONE",
                "add_lump TWO",
                "add_lump THREE",
                "remove_lump THREE, ONE"
            ],
            "the label lists every name in the order given"
        );
    }

    #[tokio::test]
    async fn remove_lump_takes_the_last_copy_or_the_numbered_one() {
        let client = connect().await;
        add_twins(&client).await;
        call(
            &client,
            "add_lump",
            json!({ "name": "TWIN", "text": "third", "allow_duplicate": true }),
        )
        .await;
        let last = call(&client, "remove_lump", json!({ "names": ["TWIN"] })).await;
        assert_eq!(
            last["removed"],
            json!([{ "name": "TWIN", "index": 2_u64 }]),
            "the default occurrence is the last copy"
        );
        let first = call(
            &client,
            "remove_lump",
            json!({ "names": ["TWIN"], "occurrence": 0_u64 }),
        )
        .await;
        assert_eq!(
            first["removed"],
            json!([{ "name": "TWIN", "index": 0_u64 }]),
            "occurrence 0 is the first of what is left"
        );
        assert_eq!(
            working_names(&client).await,
            vec!["TWIN"],
            "one twin remains, the middle one"
        );
    }

    #[tokio::test]
    async fn remove_lump_all_occurrences_clears_every_twin_at_once() {
        let client = connect().await;
        add_twins(&client).await;
        let swept = call(
            &client,
            "remove_lump",
            json!({ "names": ["TWIN"], "all_occurrences": true }),
        )
        .await;
        assert_eq!(
            swept["removed"],
            json!([
                { "name": "TWIN", "index": 0_u64 },
                { "name": "TWIN", "index": 1_u64 }
            ]),
            "every copy went, each with the index it held"
        );
        assert_eq!(
            working_names(&client).await,
            Vec::<String>::new(),
            "the working archive is empty again"
        );
        assert_eq!(
            undo_labels(&client).await,
            vec!["add_lump TWIN", "add_lump TWIN", "remove_lump TWIN"],
            "one removal for both copies"
        );
    }

    #[tokio::test]
    async fn remove_lump_aborts_the_whole_edit_when_any_name_is_missing() {
        let client = connect().await;
        add(&client, "MINE", "m").await;
        let (code, message, _) = call_err(
            &client,
            "remove_lump",
            json!({ "names": ["MINE", "GHOST"] }),
        )
        .await;
        assert_eq!(code, "not_found", "GHOST is not in the working archive");
        assert!(
            message.contains("read-only"),
            "the message explains the layers: {message}"
        );
        assert!(
            message.contains("replace_lump"),
            "the message names the override tool: {message}"
        );
        assert_eq!(
            working_names(&client).await,
            vec!["MINE"],
            "MINE stayed: the edit rolled back"
        );
        assert_eq!(
            undo_labels(&client).await,
            vec!["add_lump MINE"],
            "and no history was recorded"
        );
    }

    #[tokio::test]
    async fn remove_lump_cannot_touch_an_iwad_lump() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_only(dir.path()).await;
        let (code, message, _) =
            call_err(&client, "remove_lump", json!({ "names": ["PLAYPAL"] })).await;
        assert_eq!(code, "not_found", "an IWAD lump cannot be removed");
        assert!(
            message.contains("read-only"),
            "the message explains the layers: {message}"
        );
        assert_untouched(&client).await;
    }

    #[tokio::test]
    async fn remove_lump_rejects_an_empty_list_and_a_shared_occurrence() {
        let client = connect().await;
        seed_three(&client).await;
        let (empty_code, empty_message, _) =
            call_err(&client, "remove_lump", json!({ "names": [] })).await;
        assert_eq!(empty_code, "invalid_argument", "at least one name");
        assert!(
            empty_message.contains("at least one"),
            "the message states the rule: {empty_message}"
        );
        let (shared_code, shared_message, _) = call_err(
            &client,
            "remove_lump",
            json!({ "names": ["ONE", "TWO"], "occurrence": 0_u64 }),
        )
        .await;
        assert_eq!(shared_code, "invalid_argument", "occurrence needs one name");
        assert!(
            shared_message.contains("occurrence"),
            "the message names the field: {shared_message}"
        );
        assert_eq!(
            working_names(&client).await,
            vec!["ONE", "TWO", "THREE"],
            "both refusals removed nothing"
        );
    }

    #[tokio::test]
    async fn remove_lump_rejects_bad_names_regions_and_occurrences() {
        let client = connect().await;
        seed_three(&client).await;
        let (name_code, _, _) =
            call_err(&client, "remove_lump", json!({ "names": ["TOO LONG"] })).await;
        assert_eq!(name_code, "invalid_argument", "an unparsable name");
        let (region_code, _, _) = call_err(
            &client,
            "remove_lump",
            json!({ "names": ["ONE"], "namespace": "nope" }),
        )
        .await;
        assert_eq!(region_code, "invalid_argument", "not a namespace");
        let (past_code, past_message, _) = call_err(
            &client,
            "remove_lump",
            json!({ "names": ["ONE"], "occurrence": 5_u64 }),
        )
        .await;
        assert_eq!(past_code, "not_found", "no fifth copy of ONE");
        assert!(
            past_message.contains("0 to 0"),
            "the message names the range: {past_message}"
        );
        assert_eq!(
            working_names(&client).await,
            vec!["ONE", "TWO", "THREE"],
            "every refusal removed nothing"
        );
    }

    #[tokio::test]
    async fn remove_lump_honours_the_namespace_filter() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = connect().await;
        let extra = pwad(dir.path());
        call(&client, "open_wad", path_arg(&extra)).await;
        let report = call(
            &client,
            "remove_lump",
            json!({ "names": ["FLOOR1"], "namespace": "flats" }),
        )
        .await;
        assert_eq!(
            report["removed"],
            json!([{ "name": "FLOOR1", "index": 1_u64 }]),
            "the region copy went"
        );
        assert_eq!(
            working_names(&client).await,
            vec![
                "FF_START", "FF_END", "DEHACKED", "MAP02", "THINGS", "LINEDEFS", "SIDEDEFS",
                "VERTEXES", "SEGS", "SSECTORS", "NODES", "SECTORS", "REJECT", "BLOCKMAP",
            ],
            "the region's markers stayed"
        );
    }

    #[tokio::test]
    async fn rename_lump_moves_a_working_lump_and_keeps_its_bytes() {
        let client = connect().await;
        add(&client, "OLD", "payload").await;
        let report = call(
            &client,
            "rename_lump",
            json!({ "name": "old", "new_name": "new" }),
        )
        .await;
        assert_eq!(
            report,
            json!({ "old_name": "OLD", "new_name": "NEW", "index": 0_u64 }),
            "the documented result shape, both names uppercased"
        );
        assert_eq!(
            working_names(&client).await,
            vec!["NEW"],
            "renamed in place"
        );
        let read = call(
            &client,
            "get_lump",
            json!({ "name": "NEW", "scope": "working", "format": "text" }),
        )
        .await;
        assert_eq!(read["text"], json!("payload"), "the payload survived");
        assert_eq!(
            undo_labels(&client).await,
            vec!["add_lump OLD", "rename_lump OLD -> NEW"],
            "the label shows both names"
        );
    }

    #[tokio::test]
    async fn rename_lump_refuses_a_taken_name_unless_told_otherwise() {
        let client = connect().await;
        add(&client, "A", "1").await;
        add(&client, "B", "2").await;
        let (code, message, _) = call_err(
            &client,
            "rename_lump",
            json!({ "name": "A", "new_name": "B" }),
        )
        .await;
        assert_eq!(code, "already_exists", "B is taken in the same region");
        assert!(
            message.contains("allow_duplicate"),
            "the message names the fix: {message}"
        );
        assert_eq!(
            working_names(&client).await,
            vec!["A", "B"],
            "the refusal renamed nothing"
        );
    }

    #[tokio::test]
    async fn rename_lump_allows_a_twin_when_asked_and_labels_the_edit() {
        let client = connect().await;
        add(&client, "A", "1").await;
        add(&client, "B", "2").await;
        let forced = call(
            &client,
            "rename_lump",
            json!({ "name": "A", "new_name": "B", "allow_duplicate": true }),
        )
        .await;
        assert_eq!(forced["new_name"], json!("B"), "the twin was allowed");
        assert_eq!(forced["index"], json!(0_u64), "still in place");
        assert_eq!(
            working_names(&client).await,
            vec!["B", "B"],
            "both copies now carry the name"
        );
        assert_eq!(
            undo_labels(&client).await,
            vec!["add_lump A", "add_lump B", "rename_lump A -> B"],
            "the accepted rename is the newest step"
        );
    }

    #[tokio::test]
    async fn rename_lump_rejects_a_bad_new_name_and_a_missing_lump() {
        let client = connect().await;
        add(&client, "MINE", "m").await;
        let (long_code, long_message, _) = call_err(
            &client,
            "rename_lump",
            json!({ "name": "MINE", "new_name": "TOO LONG FOR WAD" }),
        )
        .await;
        assert_eq!(
            long_code, "invalid_argument",
            "the new name is over eight bytes"
        );
        assert!(
            long_message.contains("limit is 8"),
            "the message states the limit: {long_message}"
        );
        let (bad_code, _, _) = call_err(
            &client,
            "rename_lump",
            json!({ "name": "GHOST", "new_name": "OK" }),
        )
        .await;
        assert_eq!(
            bad_code, "not_found",
            "the working archive has no such lump"
        );
        let (region_code, _, _) = call_err(
            &client,
            "rename_lump",
            json!({ "name": "MINE", "new_name": "OK", "namespace": "nope" }),
        )
        .await;
        assert_eq!(region_code, "invalid_argument", "not a namespace");
        assert_eq!(
            working_names(&client).await,
            vec!["MINE"],
            "no refusal changed anything"
        );
    }

    #[tokio::test]
    async fn rename_lump_cannot_touch_an_iwad_lump() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_only(dir.path()).await;
        let (code, message, _) = call_err(
            &client,
            "rename_lump",
            json!({ "name": "FLOOR1", "new_name": "FLOORX" }),
        )
        .await;
        assert_eq!(code, "not_found", "an IWAD lump cannot be renamed");
        assert!(
            message.contains("replace_lump"),
            "the message names the override tool: {message}"
        );
        assert_untouched(&client).await;
    }

    #[tokio::test]
    async fn rename_lump_reaches_the_copy_on_write_block_it_was_given() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_only(dir.path()).await;
        call(&client, "replace_lump", replace_hex("THINGS", "AA")).await;
        let report = call(
            &client,
            "rename_lump",
            json!({ "name": "THINGS", "new_name": "STUFF" }),
        )
        .await;
        assert_eq!(report["index"], json!(1_u64), "the copied member");
        assert_eq!(
            working_names(&client).await.get(1),
            Some(&"STUFF".to_owned()),
            "the working copy was renamed, the IWAD's was not"
        );
        let layer = call(
            &client,
            "list_lumps",
            json!({ "scope": "layer:0", "map": "MAP01" }),
        )
        .await;
        assert_eq!(
            names(&layer).get(1),
            Some(&"THINGS".to_owned()),
            "the read-only block still spells THINGS"
        );
        assert_eq!(
            undo_labels(&client).await,
            vec!["replace_lump THINGS", "rename_lump THINGS -> STUFF"],
            "both edits are undoable steps"
        );
    }

    #[tokio::test]
    async fn replace_lump_overrides_the_winning_copy_of_two_layer_flats() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_with_twins(dir.path()).await;
        let found = call(
            &client,
            "search_lumps",
            json!({ "pattern": "FLOOR1", "namespace": "flats" }),
        )
        .await;
        assert_eq!(
            found["count"].as_u64(),
            Some(2),
            "the IWAD fixture holds two FLOOR1 copies"
        );
        let report = call(&client, "replace_lump", replace_hex("FLOOR1", "EE")).await;
        assert!(
            report["created_override"]
                .as_bool()
                .expect("a created_override flag"),
            "the override was created"
        );
        let flats = call(&client, "list_lumps", json!({ "namespace": "flats" })).await;
        let copies: Vec<&Value> = flats["lumps"]
            .as_array()
            .expect("an array")
            .iter()
            .filter(|row| row["name"] == json!("FLOOR1"))
            .collect();
        assert_eq!(copies.len(), 2, "the region still shows two copies");
        assert_eq!(
            copies.last().expect("the winning copy")["source"],
            json!("working"),
            "the override replaced the copy that wins, in place"
        );
        assert_eq!(
            copies.first().expect("the losing copy")["size"].as_u64(),
            Some(FLAT_BYTES),
            "the losing IWAD copy is untouched in the view"
        );
        assert_eq!(merged_hex(&client, "FLOOR1", 1).await, "EE");
    }

    /// Appends the global lumps `A` to `E`, in that order.
    async fn letters(client: &Client) {
        for lump in ["A", "B", "C", "D", "E"] {
            add(client, lump, lump).await;
        }
    }

    #[tokio::test]
    async fn reorder_lumps_moves_one_named_lump_and_anchors_another() {
        let client = connect().await;
        letters(&client).await;
        let front = call(
            &client,
            "reorder_lumps",
            json!({ "name": "C", "index": 0_u64 }),
        )
        .await;
        assert_eq!(
            front,
            json!({ "moved": ["C"], "from_index": 2_u64, "to_index": 0_u64 }),
            "the documented result shape"
        );
        assert_eq!(
            working_names(&client).await,
            vec!["C", "A", "B", "D", "E"],
            "C leads the directory now"
        );
        let anchored = call(
            &client,
            "reorder_lumps",
            json!({ "name": "B", "after": "E" }),
        )
        .await;
        assert_eq!(
            anchored["to_index"],
            json!(4_u64),
            "the directory without B has four lumps, so after E is index 4"
        );
        assert_eq!(
            working_names(&client).await,
            vec!["C", "A", "D", "E", "B"],
            "B follows E, the destination counted after the removal"
        );
        assert_eq!(
            undo_labels(&client).await,
            vec![
                "add_lump A",
                "add_lump B",
                "add_lump C",
                "add_lump D",
                "add_lump E",
                "reorder_lumps C",
                "reorder_lumps B"
            ],
            "each move is its own undoable step (the unit tests walk the stack back)"
        );
    }

    #[tokio::test]
    async fn reorder_lumps_moves_a_run_of_two_before_a_named_lump() {
        let client = connect().await;
        letters(&client).await;
        let report = call(
            &client,
            "reorder_lumps",
            json!({ "name": "B", "count": 2_u64, "before": "A" }),
        )
        .await;
        assert_eq!(
            report,
            json!({ "moved": ["B", "C"], "from_index": 1_u64, "to_index": 0_u64 }),
            "the whole run is reported in directory order"
        );
        assert_eq!(
            working_names(&client).await,
            vec!["B", "C", "A", "D", "E"],
            "B and C travelled together, still side by side"
        );
        let read = call(
            &client,
            "get_lump",
            json!({ "name": "C", "scope": "working", "format": "text" }),
        )
        .await;
        assert_eq!(read["index"], json!(1_u64), "C kept its run position");
        assert_eq!(read["text"], json!("C"), "and its payload");
    }

    #[tokio::test]
    async fn reorder_lumps_takes_a_run_from_from_index() {
        let client = connect().await;
        letters(&client).await;
        let report = call(
            &client,
            "reorder_lumps",
            json!({ "from_index": 0_u64, "count": 2_u64, "index": 3_u64 }),
        )
        .await;
        assert_eq!(
            report,
            json!({ "moved": ["A", "B"], "from_index": 0_u64, "to_index": 3_u64 }),
            "index 3 is the end of the three lumps left"
        );
        assert_eq!(
            working_names(&client).await,
            vec!["C", "D", "E", "A", "B"],
            "the run was appended"
        );
    }

    #[tokio::test]
    async fn reorder_lumps_into_a_namespace_creates_the_markers() {
        let client = connect().await;
        letters(&client).await;
        let report = call(
            &client,
            "reorder_lumps",
            json!({ "name": "C", "count": 2_u64, "namespace": "flats" }),
        )
        .await;
        assert_eq!(
            report,
            json!({ "moved": ["C", "D"], "from_index": 2_u64, "to_index": 4_u64 }),
            "the run landed between the new markers"
        );
        assert_eq!(
            working_names(&client).await,
            vec!["A", "B", "E", "F_START", "C", "D", "F_END"],
            "a flats region was created at the end and the run sits inside it"
        );
        let flats = call(
            &client,
            "list_lumps",
            json!({ "scope": "working", "namespace": "flats" }),
        )
        .await;
        assert_eq!(
            names(&flats),
            vec!["F_START", "C", "D", "F_END"],
            "the classifier sees the moved run as region members"
        );
    }

    #[tokio::test]
    async fn reorder_lumps_moves_a_whole_map_block_by_count() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_only(dir.path()).await;
        call(&client, "replace_lump", replace_hex("THINGS", "AA BB")).await;
        add(&client, "NOTE", "n").await;
        let report = call(
            &client,
            "reorder_lumps",
            json!({ "name": "MAP01", "count": 11_u64, "index": 1_u64 }),
        )
        .await;
        assert_eq!(
            report["moved"],
            json!([
                "MAP01", "THINGS", "LINEDEFS", "SIDEDEFS", "VERTEXES", "SEGS", "SSECTORS", "NODES",
                "SECTORS", "REJECT", "BLOCKMAP"
            ]),
            "the header and every member travelled as one run"
        );
        assert_eq!(
            report["to_index"],
            json!(1_u64),
            "index 1 of the single lump the block left behind"
        );
        let listing = call(&client, "list_lumps", json!({ "scope": "working" })).await;
        assert_eq!(
            names(&listing).first(),
            Some(&"NOTE".to_owned()),
            "the note is first and the block follows it whole: {listing}"
        );
        let block = call(
            &client,
            "list_lumps",
            json!({ "scope": "working", "map": "MAP01" }),
        )
        .await;
        assert_eq!(
            block["total"].as_u64(),
            Some(11),
            "the block is still one block"
        );
        assert_eq!(
            row_named(&block, "MAP01")["index"].as_u64(),
            Some(1),
            "its header starts where the run landed"
        );
        assert_eq!(
            undo_labels(&client).await.last(),
            Some(&"reorder_lumps MAP01".to_owned()),
            "one undoable step for eleven lumps"
        );
    }

    #[tokio::test]
    async fn reorder_lumps_refuses_a_run_without_a_destination() {
        let client = connect().await;
        letters(&client).await;
        let (both_code, both_message, _) = call_err(
            &client,
            "reorder_lumps",
            json!({ "name": "C", "from_index": 2_u64, "index": 0_u64 }),
        )
        .await;
        assert_eq!(both_code, "invalid_argument", "name and from_index");
        assert!(
            both_message.contains("exactly one"),
            "the message states the rule: {both_message}"
        );
        let (refined_code, refined_message, _) = call_err(
            &client,
            "reorder_lumps",
            json!({ "from_index": 2_u64, "occurrence": 0_u64, "index": 0_u64 }),
        )
        .await;
        assert_eq!(refined_code, "invalid_argument", "occurrence needs name");
        assert!(
            refined_message.contains("from_index"),
            "the message names the field: {refined_message}"
        );
        let (zero_code, zero_message, _) = call_err(
            &client,
            "reorder_lumps",
            json!({ "name": "C", "count": 0_u64, "index": 0_u64 }),
        )
        .await;
        assert_eq!(zero_code, "invalid_argument", "an empty run");
        assert!(
            zero_message.contains("at least 1"),
            "the message states the floor: {zero_message}"
        );
        let (none_code, none_message, _) = call_err(
            &client,
            "reorder_lumps",
            json!({ "name": "C", "occurrence": 1_u64 }),
        )
        .await;
        assert_eq!(none_code, "invalid_argument", "a destination is required");
        assert!(
            none_message.contains("no append default"),
            "the message says so: {none_message}"
        );
        assert_eq!(
            working_names(&client).await,
            vec!["A", "B", "C", "D", "E"],
            "the plan refusals touched nothing"
        );
        assert_eq!(undo_labels(&client).await.len(), 5, "and recorded nothing");
    }

    #[tokio::test]
    async fn reorder_lumps_refuses_a_run_past_the_end_a_self_anchor_and_missing_names() {
        let client = connect().await;
        letters(&client).await;
        let (overflow_code, overflow_message, _) = call_err(
            &client,
            "reorder_lumps",
            json!({ "name": "D", "count": 4_u64, "index": 0_u64 }),
        )
        .await;
        assert_eq!(
            overflow_code, "invalid_argument",
            "the run leaves the archive"
        );
        assert!(
            overflow_message.contains("can hold at most 2"),
            "the message names the room: {overflow_message}"
        );
        let (index_code, index_message, _) = call_err(
            &client,
            "reorder_lumps",
            json!({ "name": "A", "count": 2_u64, "index": 4_u64 }),
        )
        .await;
        assert_eq!(
            index_code, "invalid_argument",
            "index 4 of three lumps left"
        );
        assert!(
            index_message.contains("between 0 and 3"),
            "the range excludes the moved run: {index_message}"
        );
        let (inside_code, inside_message, _) = call_err(
            &client,
            "reorder_lumps",
            json!({ "name": "B", "count": 3_u64, "after": "D" }),
        )
        .await;
        assert_eq!(inside_code, "invalid_argument", "the anchor is in the run");
        assert!(
            inside_message.contains("one of the moved lumps"),
            "the message says why: {inside_message}"
        );
        assert_eq!(
            working_names(&client).await,
            vec!["A", "B", "C", "D", "E"],
            "every refusal left the order alone"
        );
        assert_eq!(
            undo_labels(&client).await.len(),
            5,
            "and none of them was recorded"
        );
    }

    #[tokio::test]
    async fn reorder_lumps_reports_a_missing_run_or_anchor_as_not_found() {
        let client = connect().await;
        letters(&client).await;
        let (missing_code, missing_message, _) = call_err(
            &client,
            "reorder_lumps",
            json!({ "name": "GHOST", "index": 0_u64 }),
        )
        .await;
        assert_eq!(missing_code, "not_found", "no such working lump");
        assert!(
            missing_message.contains("cannot be moved"),
            "the message explains the layers: {missing_message}"
        );
        let (past_code, past_message, _) = call_err(
            &client,
            "reorder_lumps",
            json!({ "name": "C", "occurrence": 1_u64, "index": 0_u64 }),
        )
        .await;
        assert_eq!(
            past_code, "not_found",
            "one C, so occurrence 1 is out of range"
        );
        assert!(
            past_message.contains("0 to 0"),
            "the message names the range: {past_message}"
        );
        let (anchor_code, anchor_message, _) = call_err(
            &client,
            "reorder_lumps",
            json!({ "name": "A", "after": "GHOST" }),
        )
        .await;
        assert_eq!(anchor_code, "not_found", "no such anchor");
        assert!(
            anchor_message.contains("leaves in place"),
            "the message names the run rule: {anchor_message}"
        );
        assert_eq!(
            working_names(&client).await,
            vec!["A", "B", "C", "D", "E"],
            "every refusal left the order alone"
        );
        assert_eq!(
            undo_labels(&client).await.len(),
            5,
            "and none of them was recorded"
        );
    }

    #[tokio::test]
    async fn reorder_lumps_cannot_move_an_iwad_lump() {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let client = iwad_only(dir.path()).await;
        let (code, message, _) = call_err(
            &client,
            "reorder_lumps",
            json!({ "name": "PLAYPAL", "index": 0_u64 }),
        )
        .await;
        assert_eq!(code, "not_found", "an IWAD lump cannot be moved");
        assert!(
            message.contains("read-only"),
            "the message explains the layers: {message}"
        );
        assert!(
            message.contains("add_lump"),
            "the message names the way in: {message}"
        );
        assert_untouched(&client).await;
    }

    /// A working archive holding a loose `FLATA` and a flats-region `FLATA`,
    /// so copy selection has something to choose between.
    async fn twin_flats(client: &Client) {
        add(client, "FLATA", "loose").await;
        call(
            client,
            "add_lump",
            json!({ "name": "FLATA", "text": "region", "namespace": "flats" }),
        )
        .await;
    }

    #[tokio::test]
    async fn reorder_lumps_picks_the_copy_with_from_namespace_and_occurrence() {
        let client = connect().await;
        twin_flats(&client).await;
        assert_eq!(
            working_names(&client).await,
            vec!["FLATA", "F_START", "FLATA", "F_END"],
            "a loose copy and a region copy"
        );
        let (code, message, _) = call_err(
            &client,
            "reorder_lumps",
            json!({ "name": "FLATA", "from_namespace": "sprites", "index": 0_u64 }),
        )
        .await;
        assert_eq!(code, "not_found", "no sprite FLATA");
        assert!(
            message.contains("the sprites region"),
            "the message names the region it looked in: {message}"
        );
        let region_copy = call(
            &client,
            "reorder_lumps",
            json!({ "name": "FLATA", "from_namespace": "flats", "index": 0_u64 }),
        )
        .await;
        assert_eq!(
            region_copy["from_index"],
            json!(2_u64),
            "from_namespace picked the region copy"
        );
        let first = call(
            &client,
            "get_lump",
            json!({
                "name": "FLATA",
                "scope": "working",
                "occurrence": 0_u64,
                "format": "text"
            }),
        )
        .await;
        assert_eq!(first["text"], json!("region"), "the moved copy leads");
        let loose_copy = call(
            &client,
            "reorder_lumps",
            json!({ "name": "FLATA", "occurrence": 1_u64, "after": "F_END" }),
        )
        .await;
        assert_eq!(
            loose_copy["from_index"],
            json!(1_u64),
            "occurrence 1 is the second of the two matches"
        );
        assert_eq!(
            working_names(&client).await,
            vec!["FLATA", "F_START", "F_END", "FLATA"],
            "the second copy is loose again, behind the markers"
        );
    }
}
